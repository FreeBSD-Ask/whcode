#!/usr/bin/env python3
"""临时文件、中间产物与垃圾文件清理机制（T158）。

## 理论依据（软件工程）

1. **构建可再现性**：`target/` 可由 源码 + `Cargo.lock` + `vendor/` 完全再生
   （`cargo build --locked` 不依赖残留增量）。删除它零信息损失；相反，残留的
   陈旧增量可能掩盖配置漂移——"清构建"是排障的标准第一步。验证工作树的
   `target/` 同理：它只是同一套源码的另一份增量缓存，不承载任何证据。
2. **源码—产物分离**：仓库只保留源码与声明式配置；一切可再生产物不入库、可回收
   （repository-layout 五类分层中"本地产物永不入库"的回收侧）。
3. **白名单优于黑名单**：只删除"能证明是垃圾"的对象——固定路径（`target/`）、
   已知前缀（系统 TEMP 下的 `whcode-*` 测试残留）、垃圾扩展名（`*.tmp` 等）；
   不做递归通配猜测。`local/` 只按显式模式回收，长期工具、凭据、参考源码与
   `artifacts/`（运行证据，治理放行文件入库）**永久豁免**。
4. **不可逆操作门禁**：默认 dry-run 只报告；`--apply` 显式执行。执行前复核
   活跃构建进程（cargo/rustc）与最近 30 分钟内活跃的临时目录，避免误删正在
   使用的工作区。
5. **证据留痕**：`--record` 将清理清单与释放量落 `artifacts/`，可审计、可复审。

## 范围

| scope | 对象 | 再生成方式 |
|---|---|---|
| `target` | 仓库 `target/` 与 `git worktree list` 登记的每个工作树的 `target/` | `cargo build --locked` |
| `temp` | 系统 TEMP 下 `whcode-*`／`wt-probe-*` 残留（目录与文件） | 测试重跑自然重建 |
| `stray` | 仓库内 `*.tmp/*.bak/*.orig/*.rej/*~/*.log` 与根 `whcode.duckdb*` | 不应存在（存在即误操作产物） |
| `script` | `local/` 下匹配 [`SCRIPT_DELETE_PATTERNS`] 的临时脚本与运行日志 | 均为一次性工作产物，不再生（长期工具与凭据在 [`SCRIPT_KEEP_ALWAYS`] 显式保留） |
| `cache` | 仓库内 `__pycache__/` 与 `*.pyc`（Python 解释器缓存） | 下次运行脚本自动重建 |
| `download` | `local/` 下匹配 [`DOWNLOAD_DELETE_PATTERNS`] 的下载缓存与旧发布目录 | 需要时重新下载或重打包（参考源码克隆与凭据在 [`DOWNLOAD_KEEP_ALWAYS`] 保留） |

## 用法

    python .github/scripts/clean.py --scope all          # dry-run：只报告清单与释放量
    python .github/scripts/clean.py --scope all --apply  # 执行清理
    python .github/scripts/clean.py --scope all --apply --record  # 执行并留证据到 artifacts/
    python .github/scripts/clean.py --scope all --verbose        # 列出全部对象（默认每类前 10 大）

`--scope` 取 `target` / `temp` / `stray` / `script` / `cache` / `download` / `all`（默认 `all`）。
退出码：0 成功（含 dry-run；0 字节对象被权限拒绝时登记阻塞）；1 有对象删除失败（占用且仍有内容）；2 参数错误。
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

REPO = pathlib.Path(__file__).resolve().parents[2]

#: 系统 TEMP 下本项目的测试残留前缀（与 crates/tests 里的临时目录命名对应）。
TEMP_PREFIXES = ("whcode-", "wt-probe-")

#: 仓库内垃圾文件模式（stray）。git 跟踪文件恒不清理。
STRAY_PATTERNS = ["*.tmp", "*.bak", "*.orig", "*.rej", "*~", "*.log"]

#: 永久豁免目录（stray 扫描跳过）：.git 与可再生产物根、运行证据目录。
EXEMPT_DIRS = {".git", "target", "vendor", "script", "artifacts", "node_modules"}

#: local/ 的删除模式：一次性工作脚本与运行日志（2026-10-24 用户规则更新：
#: local/ 不再“保留不清理”，改为按本机制回收）。
SCRIPT_DELETE_PATTERNS = [
    "t[0-9]*.py",      # 任务一次性脚本（t033_close_docs、t111_worker_bootstrap_probe…）
    "add_*.py",        # 文档/规格批量补丁
    "apply_*.py",
    "append_*.py",
    "update_*.py",
    "sync_*.py",
    "fix_*.py",
    "patch_*.py",
    "merge_*.py",
    "dedupe_*.py",
    "insert_*.py",
    "rename_*.py",
    "reorder_*.py",
    "export_*.py",
    "inspect_*.py",
    "list_*.py",
    "query_*.py",
    "verify_*.py",
    "accept_*.py",
    "allow_*.py",
    "close_*.py",
    "test_*.py",
    "*probe*.py",      # 探测脚本（probe_*、i18n_probe、width_probe、probe_wt_tui…）
    "tui_*.py",        # TUI 一次性取证（smoke/manual/views/v003）
    "*_baseline.py",   # 基线 spike
    "partial-*.md",    # 临时文档草稿
    "*.log",           # 运行日志
    "*.zip",           # 下载缓存（vendor/ 已解包校验）
]

#: local/ 永久保留清单（优先于删除模式）：
#: - `sensenova.env` 是凭据（AGENTS：不入库、不进日志），任何清理不得触碰；
#: - `dml_cte_ab.py`、`baseline_v7.py` 是 ADR-036 的测量工具（数据修改 CTE A/B 与改造后基线），
#:   证据引用它们以保证可复现，故保留；
#: - EOL、NFR 与测试进程资源测量脚本已迁入 `.github/scripts/`（CI 唯一执行入口），
#:   `local/` 不再保留同名副本，避免双源漂移；
#: - `check_workflows.py` 不在此列：入库版 `.github/scripts/check-workflows.py` 是唯一事实源，
#:   `local/` 同名副本由本机制回收（避免“本地绿、CI 红”）；
#: - `sacred_*` 是源文本考据工作数据，配套的导出/检查脚本一并保留
#:   （删脚本则考据数据不可复现）。
SCRIPT_KEEP_ALWAYS = [
    "register_change.py",
    "precommit-verify.sh",
    "sensenova.env",
    "dml_cte_ab.py",
    "baseline_v7.py",
    "sacred_sources.duckdb",
    "sacred_exports",
    "export_sacred_sources.py",
    "inspect_sacred_sources.py",
    "list_sacred_files.py",
    "query_yike_quotes.py",
]

#: local/ 下载缓存与旧发布目录（2026-11-10 用户指令：加强缓存清理）。
#: 仅匹配"可重新取得"的对象：CI 日志与 artifact 下载、旧发布包、DuckDB 命令行包、
#: 审计中间产物与压缩包。参考源码克隆、凭据与考据数据不在删除模式内。
DOWNLOAD_DELETE_PATTERNS = [
    "duckdb-cli-*",
    "release-*",
    "ci-*",
    "audit*",
    "*.tar.gz",
    "*.tgz",
    "*.zip",
    "*.7z",
]

#: download 永久豁免：外部参考源码（研究证据以路径＋commit 指向，删掉即失引用）、
#: 凭据与考据数据（与 SCRIPT_KEEP_ALWAYS 同口径，显式重复以避免两套规则漂移）。
DOWNLOAD_KEEP_ALWAYS = [
    "dsh-src",
    "zcode-src",
    "better-harness-src",
    "sacred_sources.duckdb",
    "sacred_exports",
    "sensenova.env",
]

#: cache 扫描跳过的目录：版本库元数据、可再生产物根与构建依赖。
CACHE_EXEMPT_DIRS = {".git", "target", "vendor", "node_modules"}

#: 活跃保护：最近 N 秒修改过的对象视为“可能正在使用”，跳过。
ACTIVE_WINDOW_SECONDS = 30 * 60

#: target 清理前检查的构建进程名（存在即拒绝执行，避免炸掉活跃构建）。
BUILD_PROCESS_NAMES = ("cargo.exe", "rustc.exe", "rustdoc.exe", "cargo-clippy.exe")


def dir_size(path: pathlib.Path) -> int:
    total = 0
    for root, _dirs, files in os.walk(path, onerror=lambda _e: None):
        for name in files:
            try:
                total += (pathlib.Path(root) / name).stat().st_size
            except OSError:
                continue
    return total


def git_tracked(rel: str) -> bool:
    result = subprocess.run(
        ["git", "ls-files", "--error-unmatch", rel],
        cwd=REPO, capture_output=True, check=False,
    )
    return result.returncode == 0


def worktree_roots() -> list[pathlib.Path]:
    """仓库根与 `git worktree list` 登记的其他工作树（目录存在才返回）。

    工作树是并行验证的常用手段，其 `target/` 与主仓库同源同性质，却因为不在
    固定路径上而被旧版清理遗漏——本机实测单棵工作树可达 15 GiB。
    """
    roots = [REPO]
    result = subprocess.run(["git", "worktree", "list", "--porcelain"],
                            cwd=REPO, capture_output=True, text=True, check=False)
    if result.returncode != 0:
        return roots
    for line in result.stdout.splitlines():
        if not line.startswith("worktree "):
            continue
        path = pathlib.Path(line[len("worktree "):].strip())
        if path == REPO or not path.is_dir():
            continue
        roots.append(path)
    return roots


def scan_target() -> list[dict[str, object]]:
    items: list[dict[str, object]] = []
    for root in worktree_roots():
        target = root / "target"
        if not target.is_dir():
            continue
        items.append({"scope": "target", "path": str(target), "bytes": dir_size(target),
                      "regenerate": "cargo build --locked"})
    return items


def scan_temp(now: float) -> list[dict[str, object]]:
    items: list[dict[str, object]] = []
    temp = pathlib.Path(os.environ.get("TEMP") or os.environ.get("TMP") or "/tmp")
    for entry in sorted(temp.glob("whcode-*")) + sorted(temp.glob("wt-probe-*")):
        if not entry.is_dir() and not entry.is_file():
            continue
        try:
            age = now - entry.stat().st_mtime
        except OSError:
            continue
        if age < ACTIVE_WINDOW_SECONDS:
            continue  # 活跃保护：可能正被并行测试使用
        size = dir_size(entry) if entry.is_dir() else entry.stat().st_size
        items.append({"scope": "temp", "path": str(entry), "bytes": size,
                      "regenerate": "测试重跑自然重建"})
    return items


def scan_stray() -> list[dict[str, object]]:
    items: list[dict[str, object]] = []
    for root, dirs, files in os.walk(REPO):
        rel_root = pathlib.Path(root).relative_to(REPO)
        if rel_root.parts and rel_root.parts[0] in EXEMPT_DIRS:
            dirs[:] = []
            continue
        dirs[:] = [d for d in dirs if d not in EXEMPT_DIRS]
        for name in files:
            if not any(pathlib.PurePath(name).match(pattern) for pattern in STRAY_PATTERNS):
                continue
            path = pathlib.Path(root) / name
            rel = str(path.relative_to(REPO))
            if git_tracked(rel):
                continue  # 跟踪文件永不清理（哪怕名字像垃圾）
            items.append({"scope": "stray", "path": str(path),
                          "bytes": path.stat().st_size,
                          "regenerate": "不应存在；存在即误操作产物"})
    for duck in sorted(REPO.glob("whcode.duckdb*")):
        items.append({"scope": "stray", "path": str(duck),
                      "bytes": duck.stat().st_size,
                      "regenerate": "whcode 运行时自动创建"})
    return items


def scan_script(now: float) -> tuple[list[dict[str, object]], int]:
    """local/ 的临时脚本与运行日志（删除模式 + 永久保留清单 + 活跃保护）。"""
    items: list[dict[str, object]] = []
    skipped_active = 0
    root = REPO / "local"
    if not root.is_dir():
        return items, 0
    for path in sorted(root.iterdir()):
        if path.name in SCRIPT_KEEP_ALWAYS:
            continue
        if not any(pathlib.PurePath(path.name).match(p) for p in SCRIPT_DELETE_PATTERNS):
            continue
        try:
            if now - path.stat().st_mtime < ACTIVE_WINDOW_SECONDS:
                skipped_active += 1
                continue
        except OSError:
            continue
        size = dir_size(path) if path.is_dir() else path.stat().st_size
        items.append({"scope": "script", "path": str(path), "bytes": size,
                      "regenerate": "一次性工作产物，不再生（内容已交付即无保留价值）"})
    return items, skipped_active


def scan_cache(now: float) -> tuple[list[dict[str, object]], int]:
    """仓库内 Python 解释器缓存：`__pycache__/` 目录与散落的 `*.pyc`。

    这类缓存在 `.gitignore` 中已排除，但会在每次运行门禁脚本时重建；堆积本身不
    占大空间，却会污染目录树、干扰"工作区是否干净"的判断，故单列 scope 定期回收。
    """
    items: list[dict[str, object]] = []
    skipped_active = 0
    for root, dirs, files in os.walk(REPO):
        dirs[:] = [d for d in dirs if d not in CACHE_EXEMPT_DIRS]
        if "__pycache__" in dirs:
            dirs.remove("__pycache__")
            path = pathlib.Path(root) / "__pycache__"
            try:
                if now - path.stat().st_mtime < ACTIVE_WINDOW_SECONDS:
                    skipped_active += 1
                    continue
                size = dir_size(path)
            except OSError:
                continue
            items.append({"scope": "cache", "path": str(path), "bytes": size,
                          "regenerate": "下次运行 Python 脚本自动重建"})
        for name in files:
            if not name.endswith(".pyc"):
                continue
            path = pathlib.Path(root) / name
            try:
                if now - path.stat().st_mtime < ACTIVE_WINDOW_SECONDS:
                    skipped_active += 1
                    continue
                size = path.stat().st_size
            except OSError:
                continue
            items.append({"scope": "cache", "path": str(path), "bytes": size,
                          "regenerate": "下次运行 Python 脚本自动重建"})
    return items, skipped_active


def scan_download(now: float) -> tuple[list[dict[str, object]], int]:
    """local/ 的下载缓存与旧发布目录（删除模式 + 永久保留清单 + 活跃保护）。"""
    items: list[dict[str, object]] = []
    skipped_active = 0
    root = REPO / "local"
    if not root.is_dir():
        return items, 0
    for path in sorted(root.iterdir()):
        if path.name in DOWNLOAD_KEEP_ALWAYS:
            continue
        if not any(pathlib.PurePath(path.name).match(p) for p in DOWNLOAD_DELETE_PATTERNS):
            continue
        try:
            if now - path.stat().st_mtime < ACTIVE_WINDOW_SECONDS:
                skipped_active += 1
                continue
        except OSError:
            continue
        size = dir_size(path) if path.is_dir() else path.stat().st_size
        items.append({"scope": "download", "path": str(path), "bytes": size,
                      "regenerate": "需要时重新下载或重打包"})
    return items, skipped_active


def build_processes_running() -> list[str]:
    """列出正在运行的构建进程名（Windows tasklist；其他平台返回空）。"""
    if os.name != "nt":
        return []
    result = subprocess.run(["tasklist", "/FO", "CSV", "/NH"],
                            capture_output=True, text=True, check=False)
    running = []
    for line in result.stdout.splitlines():
        name = line.strip().strip('"').split('","')[0].lower()
        if name in BUILD_PROCESS_NAMES:
            running.append(name)
    return sorted(set(running))


def remove(item: dict[str, object]) -> str | None:
    path = pathlib.Path(str(item["path"]))
    try:
        if path.is_dir():
            # ignore_errors：占用/权限错误不中断整批；删除后仍存在即记失败。
            # Python 3.14 已弃用 onerror 回调（旧签名会 TypeError），不用回调。
            shutil.rmtree(path, ignore_errors=True)
        elif path.is_file():
            path.unlink()
    except Exception as err:  # noqa: BLE001——删除失败逐项记录，不中断其余对象
        return str(err)
    return None if not path.exists() else "删除后仍存在（部分文件被占用）"


def main() -> int:
    parser = argparse.ArgumentParser(description="临时/中间/垃圾文件清理（T158）")
    parser.add_argument("--scope",
                        choices=["target", "temp", "stray", "script", "local",
                                 "cache", "download", "all"],
                        default="all")
    parser.add_argument("--apply", action="store_true", help="执行清理（默认 dry-run）")
    parser.add_argument("--record", action="store_true", help="证据落 artifacts/clean-run-*.json")
    parser.add_argument("--verbose", action="store_true", help="列出全部对象（默认每个 scope 前 10 大）")
    args = parser.parse_args()

    now = time.time()
    items: list[dict[str, object]] = []
    skipped_active = 0
    if args.scope in ("target", "all"):
        items += scan_target()
    if args.scope in ("temp", "all"):
        items += scan_temp(now)
    if args.scope in ("stray", "all"):
        items += scan_stray()
    if args.scope in ("script", "local", "all"):
        script_items, script_skipped = scan_script(now)
        items += script_items
        skipped_active += script_skipped
    if args.scope in ("cache", "all"):
        cache_items, cache_skipped = scan_cache(now)
        items += cache_items
        skipped_active += cache_skipped
    if args.scope in ("download", "all"):
        download_items, download_skipped = scan_download(now)
        items += download_items
        skipped_active += download_skipped

    if args.apply and args.scope in ("target", "all"):
        running = build_processes_running()
        if running:
            print(f"拒绝执行：检测到活跃构建进程 {running}，请等待其结束后重试", file=sys.stderr)
            return 1

    total_bytes = sum(int(item["bytes"]) for item in items)
    mode = "APPLY" if args.apply else "DRY-RUN"
    print(f"[{mode}] scope={args.scope} 对象 {len(items)} 个，合计 {total_bytes / 2**30:.2f} GiB"
          + (f"（活跃保护跳过 {skipped_active} 个）" if skipped_active else ""))
    by_scope: dict[str, list[dict[str, object]]] = {}
    for item in items:
        by_scope.setdefault(str(item["scope"]), []).append(item)
    for scope in ("target", "temp", "stray", "script", "cache", "download"):
        entries = by_scope.get(scope)
        if not entries:
            continue
        scope_bytes = sum(int(entry["bytes"]) for entry in entries)
        print(f"  [{scope}] {len(entries)} 个，{scope_bytes / 2**30:.2f} GiB")
        shown = entries if args.verbose else sorted(
            entries, key=lambda entry: int(entry["bytes"]), reverse=True)[:10]
        for item in shown:
            print(f"    {int(item['bytes']) / 2**20:>10.1f} MB  {item['path']}")
        if len(shown) < len(entries):
            print(f"    …另有 {len(entries) - len(shown)} 个较小对象（--verbose 查看）")

    failures: list[dict[str, object]] = []
    blocked: list[dict[str, object]] = []
    if args.apply:
        for item in items:
            error = remove(item)
            if not error:
                continue
            entry = {"path": str(item["path"]), "error": error,
                     "bytes": int(item["bytes"])}
            # 0 字节对象（多为 ACL 拒绝枚举的空目录）登记为阻塞，不计入删除失败：
            # 它不占空间，反复重试也不会成功，需要管理员或重启后才能回收。
            if int(item["bytes"]) == 0:
                entry["reason"] = "0 字节对象被权限或占用拒绝，登记阻塞"
                blocked.append(entry)
            else:
                failures.append(entry)
        print(f"已删除 {len(items) - len(failures) - len(blocked)}/{len(items)} 个对象；"
              f"失败 {len(failures)}；阻塞 {len(blocked)}")
        for failure in failures:
            print(f"  FAIL {failure['path']}：{failure['error']}")
        for entry in blocked:
            print(f"  BLOCKED {entry['path']}：{entry['error']}")

    if args.record:
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        report = {
            "schema_version": 1,
            "date": datetime.datetime.now().isoformat(timespec="seconds"),
            "mode": mode,
            "scope": args.scope,
            "items": len(items),
            "skipped_active": skipped_active,
            "bytes_total": total_bytes,
            "bytes_freed": sum(int(i["bytes"]) for i in items) - sum(
                int(f["bytes"]) for f in failures
            ),
            "failures": failures,
            "blocked": blocked,
        }
        out = REPO / "artifacts" / f"clean-run-{stamp}.json"
        out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"证据：{out}")

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
