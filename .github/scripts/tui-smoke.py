#!/usr/bin/env python3
"""TUI 可自动化冒烟（T024／V-003；CI 与本地同源）。

只覆盖**无需交互终端**的三项：
  1. `--version` 退出码 0 且输出含程序名；
  2. `doctor --json` 退出码 0，信封含 `duckdb_engine_version`；
  3. 无参数启动且 stdin 不可交互时优雅退出（退出码 0，不挂起、不进入 TUI）。

真实键盘输入、resize 重绘、Ctrl-C 回显恢复与强杀残留四项依赖真实控制台，
无法在托管 runner 上自动化，始终保持人工证据缺口（V-003），本脚本不冒充通过。

用法：
    python .github/scripts/tui-smoke.py [--exe target/release/whcode.exe] [--json artifacts/tui-smoke.json]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile

REPO = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_EXE = REPO / "target" / "release" / "whcode.exe"


def run(args: list[str], timeout: float = 60.0) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", default=str(DEFAULT_EXE))
    parser.add_argument("--json", default=str(REPO / "artifacts" / "tui-smoke.json"))
    args = parser.parse_args()

    exe = pathlib.Path(args.exe)
    if not exe.is_file():
        sys.exit(f"缺少可执行文件：{exe}（先 cargo build --release -p whcode-bin）")

    workspace = tempfile.mkdtemp(prefix="whcode-tui-smoke-")
    checks: list[dict[str, object]] = []

    version = run([str(exe), "--version"])
    checks.append(
        {
            "check": "version_exit_zero",
            "ok": version.returncode == 0 and "whcode" in version.stdout.lower(),
            "detail": f"exit={version.returncode} out={version.stdout.strip()[:80]}",
        }
    )

    doctor = run([str(exe), "doctor", "--json"])
    ok_doctor = False
    detail = f"exit={doctor.returncode}"
    if doctor.returncode == 0:
        try:
            payload = json.loads(doctor.stdout)
            ok_doctor = (
                payload.get("schema_version") == 1
                and payload.get("duckdb_engine_version") is not None
            )
            detail += f" engine={payload.get('duckdb_engine_version')}"
        except json.JSONDecodeError as err:
            detail += f" json_error={err}"
    checks.append({"check": "doctor_json_envelope", "ok": ok_doctor, "detail": detail})

    # stdin 关闭 + 非交互 stdout：应打印提示并优雅退出，而不是进入 TUI 等待按键。
    try:
        non_tty = subprocess.run(
            [str(exe), "--workspace", workspace],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=20,
        )
        checks.append(
            {
                "check": "graceful_exit_on_closed_stdin",
                "ok": non_tty.returncode == 0,
                "detail": f"exit={non_tty.returncode} out={non_tty.stdout.strip()[:80]}",
            }
        )
    except subprocess.TimeoutExpired:
        checks.append(
            {
                "check": "graceful_exit_on_closed_stdin",
                "ok": False,
                "detail": "20 秒超时：无交互 stdin 时不得进入 TUI 等待按键",
            }
        )

    failed = [item for item in checks if not item["ok"]]
    report = {
        "schema_version": 1,
        "exe": str(exe),
        "workspace": workspace,
        "checks": checks,
        "failed": len(failed),
        "manual_gaps": [
            "物理键盘输入计数",
            "真实 resize 重绘",
            "Ctrl-C 退出与终端回显恢复",
            "强杀进程后 raw mode 残留",
        ],
        "note": "本报告只覆盖可自动化三项；manual_gaps 需真实控制台人工留证（V-003）。",
    }
    out = pathlib.Path(args.json)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    for item in checks:
        print(f"{'PASS' if item['ok'] else 'FAIL'} {item['check']}: {item['detail']}")
    print(f"报告：{out}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
