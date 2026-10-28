#!/usr/bin/env python3
"""Windows 发布包打包与安装验证（tasks.md T118；Check 118；ADR-013／ADR-043）。

纪律（与 AGENTS.md §5/§9、docs/guides/signal-chain-completeness.md 同源）：
  - 只用 Python 标准库；不调用 PowerShell 之外的 shell，不使用任何 GNU 工具；
  - 打包内容是**白名单**：不在清单里的文件一律不进包（防止把本地状态、密钥、
    工作区数据库随包发布）；
  - 安装验证必须在**仓库外**的临时目录解包运行，且 cwd 也切到解包目录——
    否则“能跑”可能只是因为仓库里的 `vendor/duckdb` 与 `target/` 仍在 DLL 搜索路径上；
  - 只记录可归因信号：每条命令记录退出码与 stdout/stderr 摘要，缺字段写
    `未取得`，不以 0 条输出冒充通过；
  - 产物为 UTF-8 无 BOM、LF。

架构口径（ADR-013／ADR-043）：
  - `--target x86_64-pc-windows-msvc`（默认）：amd64 是唯一验收目标，可打包并安装验证；
  - `--target aarch64-pc-windows-msvc`：ARM64 是构建交付目标（非验收目标）。exe 取自
    `target/<三元组>/release/`（交叉构建）或 `target/release/`（原生 ARM64 宿主构建）；
  - `--verify` 只在**宿主架构与目标架构一致**时允许：amd64 宿主无法运行 arm64 二进制，
    反之亦然。交叉产物的安装验证由原生 ARM64 runner / 设备取得，本机只打包。

用法：
    python .github/scripts/package-windows.py                      # 只打包（主机架构）
    python .github/scripts/package-windows.py --verify             # 打包 + 仓库外解包运行验证
    python .github/scripts/package-windows.py --verify --record artifacts/install-verify-windows-amd64.txt
    python .github/scripts/package-windows.py --out target/dist --verify
    python .github/scripts/package-windows.py --target aarch64-pc-windows-msvc   # ARM64 包
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import platform
import shutil
import subprocess
import sys
import tempfile
import time
import zipfile

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]

#: 目标三元组 -> 包名中的架构标识与人类可读描述（ADR-043）。
SUPPORTED_TARGETS: dict[str, str] = {
    "x86_64-pc-windows-msvc": "amd64",
    "aarch64-pc-windows-msvc": "arm64",
}

#: 随包发布的文档（仓库相对路径 -> 包内文件名）。兼容矩阵必须随包发布（Check 118）。
DOC_FILES: dict[str, str] = {
    "README.md": "README.md",
    "LICENSE": "LICENSE",
    "CHANGELOG.md": "CHANGELOG.md",
    "docs/matrices/compatibility.md": "compatibility.md",
    "docs/matrices/platform-matrix.md": "platform-matrix.md",
}

#: 内置字体的来源与许可（仓库相对路径 -> 包内路径）。
#: 可执行文件内嵌了 OFL 字体子集，按许可要求必须随包附版权声明与许可原文。
FONT_FILES: dict[str, str] = {
    "assets/fonts/SOURCES.md": "assets/fonts/SOURCES.md",
    "assets/fonts/source/inter/LICENSE.txt": "assets/fonts/Inter-LICENSE.txt",
    "assets/fonts/source/noto-sans-sc/LICENSE": "assets/fonts/NotoSansCJKsc-LICENSE.txt",
    "assets/fonts/source/jetbrains-mono/OFL.txt": "assets/fonts/JetBrainsMono-OFL.txt",
    "assets/fonts/source/sarasa-mono-sc/LICENSE": "assets/fonts/SarasaMonoSC-LICENSE.txt",
}


def fail(message: str) -> int:
    print(f"打包失败：{message}", file=sys.stderr)
    return 1


def sha256(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def host_triple() -> str | None:
    """宿主 Windows 架构 -> Rust 三元组；非 Windows 或未知架构返回 None。"""
    if platform.system() != "Windows":
        return None
    machine = platform.machine().upper()
    if machine == "AMD64":
        return "x86_64-pc-windows-msvc"
    if machine == "ARM64":
        return "aarch64-pc-windows-msvc"
    return None


def read_version() -> str:
    """读发布版本号：优先 crate 显式 `version`，`version.workspace = true` 时回落根 Cargo.toml。

    不猜版本：两处都取不到就报错，让打包失败而不是发布一个 `whcode-true-*.zip`。
    """
    crate_text = (REPO_ROOT / "crates" / "whcode-bin" / "Cargo.toml").read_text(encoding="utf-8")
    in_package = False
    for line in crate_text.splitlines():
        stripped = line.strip()
        if stripped == "[package]":
            in_package = True
        elif stripped.startswith("["):
            in_package = False
        elif in_package and "=" in stripped:
            key, raw = (part.strip() for part in stripped.split("=", 1))
            if key == "version":
                if raw != "workspace":
                    return raw.strip('"')
                break
            if key.startswith("version."):
                # `version.workspace = true`：版本号在根 Cargo.toml 的 [workspace.package]。
                break

    root_text = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    in_workspace_package = False
    for line in root_text.splitlines():
        stripped = line.strip()
        if stripped == "[workspace.package]":
            in_workspace_package = True
        elif stripped.startswith("["):
            in_workspace_package = False
        elif in_workspace_package and stripped.startswith("version"):
            return stripped.split("=", 1)[1].strip().strip('"')
    raise RuntimeError("未能解析版本号（crate 与 workspace 均无显式 version）")


def find_duckdb_dll(target: str) -> pathlib.Path:
    """定位 fetch-duckdb.py 校验过的预编译 DLL（按目标三元组；多版本缓存时取最高版本目录）。"""
    roots = sorted((REPO_ROOT / "vendor" / "duckdb").glob(f"*/{target}/duckdb.dll"))
    if not roots:
        raise RuntimeError(
            f"未找到 vendor/duckdb/<版本>/{target}/duckdb.dll，"
            "请先运行 python .github/scripts/fetch-duckdb.py "
            + ("" if target == (host_triple() or "") else f"--target {target}")
        )
    return roots[-1]


def binary_path(target: str) -> pathlib.Path:
    """发布 exe 路径：宿主构建在 target/release/，交叉构建在 target/<三元组>/release/。"""
    if target == host_triple():
        return REPO_ROOT / "target" / "release" / "whcode.exe"
    return REPO_ROOT / "target" / target / "release" / "whcode.exe"


def find_rustc() -> str | None:
    """定位 `rustc`：PATH 优先，其次 `CARGO_HOME`／`~/.cargo/bin`（CI 与本机 PATH 不同）。

    取不到时返回 None——记录写 `未取得`，不猜版本、也不因此让打包失败。
    """
    found = shutil.which("rustc") or shutil.which("rustc.exe")
    if found:
        return found
    home = os.environ.get("CARGO_HOME")
    candidates = [pathlib.Path(home) / "bin" / "rustc.exe"] if home else []
    candidates.append(pathlib.Path.home() / ".cargo" / "bin" / "rustc.exe")
    for candidate in candidates:
        if candidate.is_file():
            return str(candidate)
    return None


def collect_binaries(target: str) -> dict[str, pathlib.Path]:
    exe = binary_path(target)
    if not exe.is_file():
        raise RuntimeError(
            f"未找到 {exe.relative_to(REPO_ROOT)}，请先 cargo build --release --locked -p whcode-bin"
            + ("" if target == (host_triple() or "") else f" --target {target}")
        )
    return {"whcode.exe": exe, "duckdb.dll": find_duckdb_dll(target)}


def can_execute_locally(target: str) -> bool:
    """宿主能否直接运行该目标的二进制（决定 --version 戳记与 --verify 是否可用）。"""
    return target == host_triple()


def build_package(out_dir: pathlib.Path, target: str) -> tuple[pathlib.Path, list[dict[str, object]]]:
    """组装包目录并压缩；返回 (zip 路径, 清单条目)。"""
    version = read_version()
    arch = SUPPORTED_TARGETS[target]
    package_name = f"whcode-{version}-windows-{arch}"
    staging = out_dir / package_name
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir(parents=True)

    entries: list[dict[str, object]] = []
    for name, source in collect_binaries(target).items():
        shutil.copy2(source, staging / name)
        entries.append(
            {
                "file": name,
                "bytes": source.stat().st_size,
                "sha256": sha256(source),
                "origin": str(source.relative_to(REPO_ROOT)).replace("\\", "/"),
            }
        )
    for relative, packed in DOC_FILES.items():
        source = REPO_ROOT / relative
        if not source.is_file():
            raise RuntimeError(f"缺失随包文件：{relative}")
        shutil.copy2(source, staging / packed)
        entries.append(
            {
                "file": packed,
                "bytes": source.stat().st_size,
                "sha256": sha256(source),
                "origin": relative,
            }
        )
    for relative, packed in FONT_FILES.items():
        source = REPO_ROOT / relative
        if not source.is_file():
            raise RuntimeError(f"缺失字体许可文件：{relative}")
        target_path = staging / packed
        target_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target_path)
        entries.append(
            {
                "file": packed,
                "bytes": source.stat().st_size,
                "sha256": sha256(source),
                "origin": relative,
            }
        )

    executable_here = can_execute_locally(target)
    if executable_here:
        stamp = subprocess.run(
            [str(staging / "whcode.exe"), "--version"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        binary_version = stamp.stdout.strip() or "未取得（--version 无输出）"
    else:
        # 交叉构建产物不能在当前宿主运行：显式标注，不伪造“可执行”信号（信号链完备原则）。
        binary_version = f"未取得（{target} 产物不在 {platform.machine()} 宿主上执行）"
    rustc_path = find_rustc()
    rustc = (
        subprocess.run(
            [rustc_path, "-vV"],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
        if rustc_path
        else None
    )
    rustc_line = "未取得（rustc 不在 PATH，也未在 CARGO_HOME/~/.cargo/bin 找到）"
    if rustc is not None:
        rustc_line = (rustc.stdout.splitlines() or ["未取得（rustc 无输出）"])[0] or "未取得（rustc 无输出）"
    scope_line = (
        "Windows 11 amd64 唯一验收目标（ADR-013）；"
        "FreeBSD/NetBSD/OpenBSD/DragonFly 延后，本包不作任何 BSD 运行结论"
        if target == "x86_64-pc-windows-msvc"
        else "Windows 11 amd64 为唯一验收目标（ADR-013）；ARM64 为构建交付目标（ADR-043），"
        "本包不作验收结论，运行证据以原生 ARM64 采集为准"
    )
    lines = [
        f"WHCode Windows {arch} 发布包",
        f"package = {package_name}",
        f"version = {version}",
        f"target_triple = {target}",
        f"binary_version = {binary_version}",
        f"rustc = {rustc_line}",
        f"python = {platform.python_version()}",
        f"os = {platform.platform()}",
        f"generated_utc = {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",
        f"platform_scope = {scope_line}",
        "",
        "清单（文件 / 字节 / SHA256 / 来源）：",
    ]
    for entry in entries:
        lines.append(f"  {entry['file']}  {entry['bytes']}  {entry['sha256']}  <- {entry['origin']}")
    lines += [
        "",
        "安装：解压到任意目录，运行该目录内的 whcode.exe（duckdb.dll 必须与其同目录）。",
        "自检：whcode doctor        （人类可读）",
        "      whcode doctor --json （脚本可读，含 duckdb_engine_version 与三档运行时等级）",
        "数据：默认在当前目录创建 whcode.duckdb；用 --workspace DIR 指定工作区。",
        "字体：可执行文件内嵌 SIL OFL 1.1 字体子集；来源与许可见 assets/fonts/SOURCES.md 与 *-LICENSE.txt。",
        "退出权：数据经 `whcode db archive --export <PATH>` 带走；删除该目录即完全卸载（无系统级注册）。",
    ]
    (staging / "PACKAGE-INFO.txt").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    info = staging / "PACKAGE-INFO.txt"
    entries.append(
        {
            "file": "PACKAGE-INFO.txt",
            "bytes": info.stat().st_size,
            "sha256": sha256(info),
            "origin": "本脚本生成",
        }
    )

    zip_path = out_dir / f"{package_name}.zip"
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for child in sorted(staging.rglob("*")):
            if child.is_file():
                archive.write(child, arcname=f"{package_name}/{child.name}")
    shutil.rmtree(staging)
    return zip_path, entries


def run_command(argv: list[str], cwd: pathlib.Path) -> dict[str, object]:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            argv,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
            check=False,
        )
        code, out, err = completed.returncode, completed.stdout, completed.stderr
    except (OSError, subprocess.TimeoutExpired) as exc:
        code, out, err = -1, "", f"{type(exc).__name__}: {exc}"
    return {
        "argv": argv,
        "exit_code": code,
        "elapsed_ms": round((time.monotonic() - started) * 1000),
        "stdout": out,
        "stderr": err,
    }


def verify_install(zip_path: pathlib.Path) -> tuple[int, list[dict[str, object]]]:
    """仓库外临时目录解包并运行三条命令；返回 (失败数, 结果列表)。"""
    workspace_root = pathlib.Path(tempfile.mkdtemp(prefix="whcode-install-check-"))
    results: list[dict[str, object]] = []
    try:
        with zipfile.ZipFile(zip_path) as archive:
            archive.extractall(workspace_root)
        package_dir = workspace_root / zip_path.name[: -len(".zip")]
        exe = package_dir / "whcode.exe"
        if not exe.is_file():
            return 1, [{"argv": ["<解包>"], "exit_code": -1, "stdout": "", "stderr": "解包后未找到 whcode.exe"}]

        results.append(run_command([str(exe), "--version"], package_dir))
        results.append(run_command([str(exe), "doctor", "--json"], package_dir))
        data_dir = workspace_root / "workspace"
        data_dir.mkdir()
        results.append(run_command([str(exe), "--workspace", str(data_dir), "db", "stats", "--json"], package_dir))

        failures = 0
        # 判据：退出码 + 可归因信号（版本串、schema_version、引擎版本、数据库文件实际生成）。
        version = results[0]
        if version["exit_code"] != 0 or not str(version["stdout"]).strip().startswith("whcode "):
            failures += 1
            version["assertion"] = "失败：--version 未返回 whcode 版本串"
        else:
            version["assertion"] = "通过"

        doctor = results[1]
        doctor_payload: dict[str, object] = {}
        try:
            doctor_payload = json.loads(str(doctor["stdout"]))
        except json.JSONDecodeError as exc:
            doctor["assertion"] = f"失败：doctor --json 不可解析（{exc}）"
            failures += 1
        else:
            missing = [key for key in ("schema_version", "whcode_version", "duckdb_engine_version") if key not in doctor_payload]
            if doctor["exit_code"] != 0 or missing or doctor_payload.get("duckdb_engine_version") is None:
                failures += 1
                doctor["assertion"] = (
                    f"失败：退出码 {doctor['exit_code']}；缺字段 {missing or '无'}；"
                    f"引擎版本 {doctor_payload.get('duckdb_engine_version')!r}"
                )
            else:
                doctor["assertion"] = "通过（DuckDB 引擎版本来自运行期实测）"

        stats = results[2]
        db_file = data_dir / "whcode.duckdb"
        try:
            stats_payload = json.loads(str(stats["stdout"]))
        except json.JSONDecodeError as exc:
            stats_payload = {}
            stats["assertion"] = f"失败：db stats --json 不可解析（{exc}）"
            failures += 1
        if stats["exit_code"] != 0 or not db_file.is_file():
            failures += 1
            stats["assertion"] = (
                f"失败：退出码 {stats['exit_code']}；whcode.duckdb 存在={db_file.is_file()}"
            )
        elif stats_payload:
            stats["assertion"] = f"通过（库文件 {db_file.stat().st_size} 字节）"
        return failures, results
    finally:
        shutil.rmtree(workspace_root, ignore_errors=True)


def write_record(path: pathlib.Path, zip_path: pathlib.Path, entries: list[dict[str, object]], results: list[dict[str, object]], target: str) -> None:
    lines = [
        f"WHCode Windows {SUPPORTED_TARGETS[target]} 发布包安装验证记录（tasks.md T118／Check 118；ADR-043 口径见 platform_scope）",
        f"utc = {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",
        f"package = {zip_path.name}（{zip_path.stat().st_size} 字节，sha256={sha256(zip_path)}）",
        f"target_triple = {target}",
        f"os = {platform.platform()}",
        f"machine = {platform.machine()}",
        "",
        "验证方式：解包到仓库外临时目录，并把 cwd 切到解包目录后运行；",
        "临时目录在记录生成后删除（不留本地状态）。",
        "",
        "包内清单：",
    ]
    for entry in entries:
        lines.append(f"  {entry['file']}  {entry['bytes']}  {entry['sha256']}")
    lines += ["", "命令与结果："]
    for result in results:
        lines.append(f"  argv = {' '.join(str(a) for a in result['argv'])}")
        lines.append(
            f"  exit_code = {result['exit_code']}"
            + (f"  elapsed_ms = {result['elapsed_ms']}" if "elapsed_ms" in result else "")
            + f"  assertion = {result.get('assertion', '未取得')}"
        )
        stdout = str(result["stdout"]).strip()
        stderr = str(result["stderr"]).strip()
        lines.append(f"  stdout（前 600 字符）= {stdout[:600]}" if stdout else "  stdout = 空")
        lines.append(f"  stderr（前 400 字符）= {stderr[:400]}" if stderr else "  stderr = 空")
        lines.append("")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def main() -> int:
    parser = argparse.ArgumentParser(description="Windows 发布包打包与安装验证")
    parser.add_argument(
        "--target",
        default="x86_64-pc-windows-msvc",
        choices=sorted(SUPPORTED_TARGETS),
        help="目标三元组（默认 amd64 验收目标；ARM64 见 ADR-043）",
    )
    parser.add_argument("--out", default=str(REPO_ROOT / "target" / "dist"), help="输出目录（默认 target/dist）")
    parser.add_argument("--verify", action="store_true", help="仓库外临时目录解包并运行验证")
    parser.add_argument("--record", default=None, help="验证记录输出路径（需与 --verify 同用）")
    args = parser.parse_args()
    target = args.target

    host = host_triple()
    if host is None:
        return fail(f"本脚本只在 Windows 上产出交付物，当前为 {platform.platform()}（ADR-013）")
    if args.verify and not can_execute_locally(target):
        return fail(
            f"宿主 {platform.machine()} 无法运行 {target} 产物，拒绝 --verify（交叉编译不冒充平台结论，"
            "ADR-013／G-01）；ARM64 包的安装验证须在原生 ARM64 runner 或设备上取得（ADR-043）。"
            "如只要打包，请去掉 --verify。"
        )

    out_dir = pathlib.Path(args.out)
    out_dir.mkdir(parents=True, exist_ok=True)
    try:
        zip_path, entries = build_package(out_dir, target)
    except RuntimeError as exc:
        return fail(str(exc))

    print(f"发布包：{zip_path}（{zip_path.stat().st_size} 字节）")
    for entry in entries:
        print(f"  {entry['file']}: {entry['bytes']} 字节")

    if not args.verify:
        print("未做安装验证（加 --verify 在仓库外解包运行；仅宿主架构与目标一致时可用）")
        return 0

    failures, results = verify_install(zip_path)
    for result in results:
        argv = [str(item) for item in result["argv"]]
        label = " ".join([pathlib.Path(argv[0]).name] + argv[1:]) if argv else "（空）"
        print(f"  {label}: 退出码 {result['exit_code']} — {result.get('assertion', '未取得')}")
    if args.record:
        record = pathlib.Path(args.record)
        if not record.is_absolute():
            record = REPO_ROOT / record
        write_record(record, zip_path, entries, results, target)
        print(f"验证记录：{record}")
    if failures:
        return fail(f"安装验证未通过（{failures} 项失败）")
    print("安装验证通过：三条命令均有可归因信号，库文件在临时工作区实际生成")
    return 0


if __name__ == "__main__":
    sys.exit(main())
