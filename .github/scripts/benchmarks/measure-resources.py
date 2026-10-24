#!/usr/bin/env python3
"""测试进程资源测量（资源铁律 M1/C4：内存与 CPU 达标检查；CI 与本地同源）。

运行指定的测试目标并采样测试进程的 WorkingSet（RSS）峰值与 CPU 累计时间，
输出 artifacts/resource-report.json（含测量纪律字段）。

用法：
    python .github/scripts/benchmarks/measure-resources.py --test extreme_scale --test session_switch

标准（docs/guides/resource-budget.md）：
    - 测试进程峰值 RSS ≤ 512 MB/进程（M1）；
    - 会话切换响应 ≤ 200 ms（C4，由测试断言覆盖，本脚本记录进程级 CPU）。

信号链纪律：**零样本不等于达标**。若未观测到测试进程（样本数为 0），
判为“未取得信号”，报告 `measured: false` 并计入违规——不得以 0 MB 冒充 PASS。
采样改用 Win32 `GetProcessMemoryInfo`（直接持有进程句柄、50 ms 轮询），
避免每次采样都启动 PowerShell（旧实现单样本耗时 > 20 s，实测只能采到 1 次）。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wintypes
import json
import pathlib
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[3]
BUDGET_RSS_MB = 512
POLL_SECONDS = 0.05

PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
    """`PROCESS_MEMORY_COUNTERS`（Win32，`psapi.h`）。"""

    _fields_ = [
        ("cb", wintypes.DWORD),
        ("PageFaultCount", wintypes.DWORD),
        ("PeakWorkingSetSize", ctypes.c_size_t),
        ("WorkingSetSize", ctypes.c_size_t),
        ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPagedPoolUsage", ctypes.c_size_t),
        ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
        ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
        ("PagefileUsage", ctypes.c_size_t),
        ("PeakPagefileUsage", ctypes.c_size_t),
    ]


class FILETIME(ctypes.Structure):
    """`FILETIME`（Win32）。"""

    _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]


def _filetime_seconds(value: FILETIME) -> float:
    ticks = (value.dwHighDateTime << 32) | value.dwLowDateTime
    return ticks / 10_000_000


def compute_executable(target: str) -> pathlib.Path | None:
    """构建并返回测试目标的可执行文件（不运行）。"""
    build = subprocess.run(
        ["cargo", "test", "--locked", "--no-run", "--test", target],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if build.returncode != 0:
        return None
    candidates = sorted(
        (ROOT / "target" / "debug" / "deps").glob(f"{target}-*.exe"),
        key=lambda path: path.stat().st_mtime,
    )
    return candidates[-1] if candidates else None


def measure(target: str) -> dict:
    """运行测试目标并采样 RSS 峰值与 CPU 累计时间（零样本判为未取得信号）。"""
    executable = compute_executable(target)
    if executable is None:
        return {
            "target": target,
            "measured": False,
            "passed": False,
            "wall_s": 0.0,
            "samples": 0,
            "peak_rss_mb": 0,
            "cpu_s": 0.0,
            "within_budget": False,
            "reason": "构建失败：未产出测试可执行文件",
            "stdout_tail": "",
        }

    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    psapi = ctypes.windll.psapi  # type: ignore[attr-defined]
    started = time.monotonic()
    process = subprocess.Popen(
        [str(executable)],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    handle = kernel32.OpenProcess(
        PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, process.pid
    )
    peak_rss = 0.0
    peak_cpu = 0.0
    samples = 0
    counters = PROCESS_MEMORY_COUNTERS()
    counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
    creation, exit_time = FILETIME(), FILETIME()
    kernel, user = FILETIME(), FILETIME()

    def sample_once() -> None:
        """采一次样本（句柄仍有效时）。"""
        nonlocal peak_rss, peak_cpu, samples
        if not handle:
            return
        if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            peak_rss = max(peak_rss, counters.WorkingSetSize / (1024 * 1024))
            samples += 1
        if kernel32.GetProcessTimes(
            handle,
            ctypes.byref(creation),
            ctypes.byref(exit_time),
            ctypes.byref(kernel),
            ctypes.byref(user),
        ):
            peak_cpu = max(
                peak_cpu, _filetime_seconds(kernel) + _filetime_seconds(user)
            )

    try:
        # 先采一次：极短的测试目标也不能因“退出得快”被判为零样本。
        sample_once()
        while process.poll() is None:
            time.sleep(POLL_SECONDS)
            sample_once()
    finally:
        output = process.communicate()[0] or ""
        if handle:
            kernel32.CloseHandle(handle)
    wall = round(time.monotonic() - started, 1)

    measured = samples > 0
    passed = process.returncode == 0
    return {
        "target": target,
        "measured": measured,
        "passed": passed,
        "wall_s": wall,
        "samples": samples,
        "peak_rss_mb": round(peak_rss, 1),
        "cpu_s": round(peak_cpu, 2),
        "within_budget": measured and peak_rss <= BUDGET_RSS_MB,
        "reason": "" if measured else "未观测到测试进程（零样本，不得判为达标）",
        "stdout_tail": "" if passed else output[-500:],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--test", action="append", default=[], help="测试目标名（可重复）")
    args = parser.parse_args()
    targets = args.test or ["session_switch"]
    if not targets:
        print("无测试目标")
        return 1

    results = []
    for target in targets:
        print(f"测量 {target} …")
        results.append(measure(target))

    violations = [
        item
        for item in results
        if not item["measured"] or not item["within_budget"] or not item["passed"]
    ]
    report = {
        "task": "资源铁律 M1/C4 测量",
        "date": time.strftime("%Y-%m-%d"),
        "platform": "windows-amd64",
        "budget": {
            "peak_rss_mb_per_process": BUDGET_RSS_MB,
            "source": "docs/guides/resource-budget.md M1",
        },
        "measurement": {
            "method": "Win32 GetProcessMemoryInfo / GetProcessTimes，50 ms 轮询（持有进程句柄）",
            "note": "仅 Windows 验收平台；跨平台数据未取得（ADR-013）；零样本判为未取得信号",
        },
        "results": results,
        "violations": [item["target"] for item in violations],
    }
    out = ROOT / "artifacts" / "resource-report.json"
    out.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    for item in results:
        ok = item["measured"] and item["within_budget"] and item["passed"]
        status = "PASS" if ok else "FAIL"
        detail = (
            f"峰值 RSS {item['peak_rss_mb']} MB（预算 {BUDGET_RSS_MB} MB），"
            f"CPU {item['cpu_s']} s，样本 {item['samples']}，wall {item['wall_s']}s"
            if item["measured"]
            else item["reason"]
        )
        print(f"{status}: {item['target']} {detail}")
    if violations:
        print(f"FAIL: {len(violations)} 个目标不达标，禁止落地（铁律）")
        return 1
    print(f"全部达标；报告已写入 {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
