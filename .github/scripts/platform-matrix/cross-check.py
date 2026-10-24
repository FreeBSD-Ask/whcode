#!/usr/bin/env python3
"""跨目标代码级检查（tasks.md T021 补充证据；ADR-010、G-01）。

目的：在本机对 BSD 目标做 `cargo check`，记录“代码级兼容”证据与完整工作区的**预期失败**
原因。两个都必须如实落盘：

  1. 排除含原生 C 依赖的 crate（whcode-store → libduckdb-sys → ureq → aws-lc-sys；
     whcode-ssh → russh → aws-lc-rs；whcode-bin 依赖 store）后，类型检查应通过；
  2. 完整工作区检查**预期失败**——把第一条错误摘要记录下来，证明阻塞是“缺目标 C 工具链”
     这一具体原因，而非猜测。

纪律：`cargo check` 只做类型检查、不链接、不运行，**不构成** V-001/V-002/V-004 的平台通过
结论（ADR-010、G-01）。产物 JSON 每条结果都带 `is_runtime_evidence: false`。

用法：python ci/platform-matrix/cross-check.py [输出目录，默认 artifacts]
"""

from __future__ import annotations

import datetime
import json
import os
import pathlib
import re
import subprocess
import sys
import time

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
NO_EVIDENCE = "<未取得证据"

#: 排除名单：依赖图里有目标平台 C 代码的 crate（含传递依赖它们的 bin）。
EXCLUDED = ["whcode-store", "whcode-bin", "whcode-ssh"]

#: 被检查的 BSD 目标（均须先用 `rustup target add` 安装 std）。
TARGETS = ["x86_64-unknown-freebsd", "x86_64-unknown-netbsd"]

#: 完整工作区失败时用于摘录原因的模式（按优先级）。
FAILURE_PATTERNS = [
    re.compile(r'failed to find tool "[^"]+": program not found.*'),
    re.compile(r"error occurred in cc-rs:.*"),
    re.compile(r"^error: could not compile .*", re.MULTILINE),
    re.compile(r"^error(\[\w+\])?: .*", re.MULTILINE),
]


def cargo_path() -> str | None:
    hit = os.environ.get("CARGO")
    if hit and pathlib.Path(hit).is_file():
        return hit
    home = pathlib.Path.home() / ".cargo" / "bin"
    for name in ("cargo.exe", "cargo"):
        candidate = home / name
        if candidate.is_file():
            return str(candidate)
    import shutil

    return shutil.which("cargo")


def installed_targets() -> set[str]:
    rustup = pathlib.Path.home() / ".cargo" / "bin"
    exe = None
    for name in ("rustup.exe", "rustup"):
        candidate = rustup / name
        if candidate.is_file():
            exe = str(candidate)
            break
    if exe is None:
        import shutil

        exe = shutil.which("rustup")
    if exe is None:
        return set()
    result = subprocess.run(
        [exe, "target", "list", "--installed"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=120,
    )
    return {line.strip() for line in (result.stdout or "").splitlines() if line.strip()}


def run_cargo(cargo: str, args: list[str]) -> dict:
    command = [cargo, *args]
    started = time.monotonic()
    try:
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=3600,
        )
        exit_code = completed.returncode
        output = (completed.stdout or "") + (completed.stderr or "")
    except subprocess.TimeoutExpired:
        exit_code = -1
        output = f"{NO_EVIDENCE}：命令超时>"
    elapsed = round(time.monotonic() - started, 1)

    first_reason = f"{NO_EVIDENCE}：退出码 {exit_code}，未匹配已知失败模式>"
    for pattern in FAILURE_PATTERNS:
        match = pattern.search(output)
        if match:
            first_reason = match.group(0).strip()[:300]
            break

    return {
        "command": " ".join(command),
        "exit_code": exit_code,
        "elapsed_seconds": elapsed,
        "passed": exit_code == 0,
        "failure_reason": None if exit_code == 0 else first_reason,
        "is_runtime_evidence": False,
        "note": "cargo check 只做类型检查，不构成运行验证（ADR-010、G-01）",
    }


def main() -> int:
    cargo = cargo_path()
    out_dir = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path("artifacts")
    out_dir = out_dir if out_dir.is_absolute() else REPO_ROOT / out_dir
    out_dir.mkdir(parents=True, exist_ok=True)

    if cargo is None:
        print("cargo 不存在：无法执行跨目标检查", file=sys.stderr)
        return 1

    available = installed_targets()
    results: dict = {
        "schema_version": 1,
        "generated_at_utc": datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
        "host": "见 deps-windows.txt 的 host_triple（本脚本不重复探测）",
        "excluded_crates": EXCLUDED,
        "exclusion_reason": "依赖图含目标平台 C 代码：store→libduckdb-sys→ureq→aws-lc-sys；ssh→russh→aws-lc-rs",
        "discipline": "cargo check 只做类型检查（不链接、不运行），不构成平台运行验证（ADR-010、G-01）",
        "targets": {},
    }

    for target in TARGETS:
        if target not in available:
            results["targets"][target] = {
                "skipped": True,
                "reason": f"目标未安装：先执行 rustup target add {target}",
                "is_runtime_evidence": False,
            }
            continue

        subset_args = [
            "check",
            "--workspace",
            *[f"--exclude={name}" for name in EXCLUDED],
            f"--target={target}",
        ]
        full_args = ["check", "--workspace", f"--target={target}"]

        print(f"检查 {target}（排除 {', '.join(EXCLUDED)}）…")
        subset = run_cargo(cargo, subset_args)
        print(f"  子集：{'通过' if subset['passed'] else '失败'}（{subset['elapsed_seconds']}s）")

        print(f"检查 {target}（完整工作区，预期失败）…")
        full = run_cargo(cargo, full_args)
        print(
            f"  完整：{'意外通过' if full['passed'] else '按预期失败'}（{full['elapsed_seconds']}s）"
        )
        if full["passed"]:
            # 阻塞可能已被解除（例如装了交叉 C 工具链）；如实记录，提示更新 ADR-DB-BUILD。
            full["note"] = "完整工作区交叉检查通过：与记录的阻塞不符，须复核 ADR-DB-BUILD 与依赖图"

        results["targets"][target] = {
            "subset_check": subset,
            "full_workspace_check": full,
            "is_runtime_evidence": False,
        }

    out_file = out_dir / "cross-check-windows.json"
    out_file.write_text(
        json.dumps(results, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    print(f"已写出 {out_file}")
    # “完整工作区预期失败”不是脚本失败：脚本自身的成功标准是两条命令都执行并落盘。
    return 0


if __name__ == "__main__":
    sys.exit(main())
