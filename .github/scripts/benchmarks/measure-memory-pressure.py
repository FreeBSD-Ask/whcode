#!/usr/bin/env python3
"""内存压力通道（P7.3／FR-792／ADR-025 修订）：真实产品进程在**压力场景**下的峰值 RSS。

与既有两条口径互补、不重叠：

- `measure-nfr-budgets.py`：NFR-201/202 冻结预算（serve 空载 36 MB / 10 万事件读路径 80 MB），
  **判据取中位数**，是门禁；
- `measure-resources.py`：测试进程（`cargo test`）峰值 RSS ≤ 512 MB，是 M1 门禁；
- 本脚本：**只记录不判定**。场景是「并发读者」与「写入压力（bench baseline）」，
  没有冻结预算——记录峰值让"压力下会涨到多少"可查，预算需由证据另行立 ADR（ADR-025 口径：
  未冻结即不得把本通道当门禁，也不得以单次记录冻结预算）。

信号链纪律：零样本＝未取得信号（`measured=false`、退出码 1），不得以 0 MB 冒充达标；
请求失败逐条计数，失败率非零时场景仍记录但 `errors>0` 显式可见。

用法：
    python .github/scripts/benchmarks/measure-memory-pressure.py [--events 20000] [--repeat 3]
        [--readers 4] [--rounds 30] [--out artifacts/memory-pressure-report.json] [--skip-build]

平台：本脚本的采样用 Win32 `GetProcessMemoryInfo`（与 M1/C4 同源）；非 Windows 平台
返回 `measured=false` 并说明原因（不假装测过）。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wintypes
import datetime
import hashlib
import json
import pathlib
import platform
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[3]
EXE = ROOT / "target" / "release" / "whcode.exe"
REPORT = ROOT / "artifacts" / "memory-pressure-report.json"
POLL_SECONDS = 0.05

PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010

IS_WINDOWS = sys.platform == "win32"

if IS_WINDOWS:
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)  # type: ignore[attr-defined]
    psapi = ctypes.WinDLL("psapi", use_last_error=True)  # type: ignore[attr-defined]

    class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
        """`PROCESS_MEMORY_COUNTERS`（psapi.h）。"""

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


def open_process(pid: int) -> int:
    return kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)


def rss_mb(handle: int, counters: "PROCESS_MEMORY_COUNTERS") -> float:
    if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
        return counters.WorkingSetSize / (1024 * 1024)
    return -1.0


def peak_mb(handle: int, counters: "PROCESS_MEMORY_COUNTERS") -> float:
    """`GetProcessMemoryInfo` 一并返回 PeakWorkingSetSize（psapi 无独立的 peak 入口）。"""
    if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
        return counters.PeakWorkingSetSize / (1024 * 1024)
    return -1.0


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_port(port: int, deadline_s: float = 20.0) -> bool:
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.settimeout(0.5)
            if sock.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.1)
    return False


def get_json(url: str, timeout: float = 5.0) -> dict:
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


def run_captured(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    """子进程不带控制台句柄（CI 上 pwsh 会把控制台句柄传进来，Popen 句柄会失效）。"""
    kwargs.setdefault("capture_output", True)
    kwargs.setdefault("text", True)
    return subprocess.run(cmd, **kwargs)


def prepare_workspace(dir_path: pathlib.Path) -> None:
    """经产品自身入口迁移（不绕开 schema_migrations 记账）。"""
    dir_path.mkdir(parents=True, exist_ok=True)
    run_captured(
        [str(EXE), "db", "stats", "--workspace", str(dir_path)],
        cwd=ROOT,
        check=True,
    )


def seed_workspace(dir_path: pathlib.Path, session_id: str, events: int, tasks: int) -> None:
    """压力夹具：`events` 条事件 + `tasks` 条任务（与 NFR-202 同表同列，规模可调）。"""
    prepare_workspace(dir_path)
    import duckdb

    conn = duckdb.connect(str(dir_path / "whcode.duckdb"))
    conn.execute(
        "INSERT INTO sessions (id,title,created_at,last_active_at,status,stage) "
        "VALUES (?, 'memory-pressure 夹具', epoch_ms(1), epoch_ms(1), 'active', 'plan')",
        [session_id],
    )
    conn.executemany(
        "INSERT INTO events (seq,id,session_id,kind,payload_json,ts,persistence_level) "
        "VALUES (?,?,?,?,?,current_timestamp,'redacted')",
        [
            (
                seq,
                f"E-{seq}",
                session_id,
                "StreamingChunk",
                '{"pad":"' + "x" * 64 + '","seq":%d}' % seq,
            )
            for seq in range(1, events + 1)
        ],
    )
    conn.executemany(
        "INSERT INTO tasks (id,session_id,title,stage,status,acceptance_json,retry_count,updated_at) "
        "VALUES (?,?,?,?,?,?,0,current_timestamp)",
        [
            (f"T-{index}", session_id, f"任务 {index}", "plan", "pending", "[]")
            for index in range(1, tasks + 1)
        ],
    )
    conn.close()


def exe_fingerprint() -> dict:
    if not EXE.exists():
        return {"path": str(EXE), "exists": False}
    digest = hashlib.sha256(EXE.read_bytes()).hexdigest()
    version = run_captured([str(EXE), "--version"], cwd=ROOT)
    return {
        "path": str(EXE),
        "exists": True,
        "sha256": digest,
        "bytes": EXE.stat().st_size,
        "version": (version.stdout or "").strip(),
    }


def summarize(samples: list[float]) -> dict:
    if not samples:
        return {"samples": 0, "measured": False, "reason": "零样本（未取得信号）"}
    ordered = sorted(samples)
    return {
        "samples": len(samples),
        "measured": True,
        "median_mb": round(ordered[len(ordered) // 2], 2),
        "p95_mb": round(ordered[int(len(ordered) * 0.95)], 2),
        "max_mb": round(ordered[-1], 2),
    }


def measure_serve_concurrent_readers(events: int, readers: int, rounds: int) -> dict:
    """并发读者：4 线程 × 30 轮 × 3 端点（sessions/events/tasks），采样 serve 进程 RSS。"""
    ws = pathlib.Path(tempfile.mkdtemp(prefix="whcode-pressure-serve-"))
    seed_workspace(ws, "S-PRESSURE", events, max(events // 10, 100))
    port = free_port()
    process = subprocess.Popen(
        [
            str(EXE),
            "serve",
            "--web",
            "--tls",
            "off",
            "--bind",
            f"127.0.0.1:{port}",
            "--workspace",
            str(ws),
        ],
        cwd=ROOT,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    samples: list[float] = []
    errors = [0]
    requests = [0]
    lock = threading.Lock()

    def reader() -> None:
        for _ in range(rounds):
            for path in ("/api/sessions", "/api/events?session=S-PRESSURE&after=0", "/api/tasks"):
                try:
                    get_json(f"http://127.0.0.1:{port}{path}")
                    with lock:
                        requests[0] += 1
                except Exception:  # noqa: BLE001 - 失败计数即可，详情由 errors 反映
                    with lock:
                        errors[0] += 1

    try:
        if not wait_port(port):
            return {"scenario": "serve_concurrent_readers", "measured": False,
                    "reason": "serve 未在 20 s 内监听端口（未取得信号）"}
        handle = open_process(process.pid)
        counters = PROCESS_MEMORY_COUNTERS()
        counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
        threads = [threading.Thread(target=reader, daemon=True) for _ in range(readers)]
        for thread in threads:
            thread.start()
        while any(thread.is_alive() for thread in threads):
            value = rss_mb(handle, counters)
            if value >= 0:
                samples.append(value)
            time.sleep(POLL_SECONDS)
        for thread in threads:
            thread.join()
        peak = peak_mb(handle, counters)
        kernel32.CloseHandle(handle)
    finally:
        process.kill()
        process.wait(timeout=10)

    summary = summarize(samples)
    summary.update({
        "scenario": "serve_concurrent_readers",
        "fixture": {"events": events, "tasks": max(events // 10, 100)},
        "readers": readers,
        "rounds": rounds,
        "requests": requests[0],
        "errors": errors[0],
        "peak_working_set_mb": round(peak, 2) if peak >= 0 else None,
        "budget_mb": None,
        "verdict": "recorded",
        "method": "serve --web --tls off；并发读 sessions/events/tasks；"
                  "GetProcessMemoryInfo 50 ms 轮询采样；无冻结预算，只记录",
    })
    return summary


def measure_bench_write(events: int, repeat: int) -> dict:
    """写入压力：`bench baseline`（固定夹具，release 构建、192MB 缓冲上限）。"""
    out = ROOT / "local" / "memory-pressure-baseline.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    process = subprocess.Popen(
        [
            str(EXE),
            "bench",
            "baseline",
            "--out",
            str(out),
            "--events",
            str(events),
            "--repeat",
            str(repeat),
        ],
        cwd=ROOT,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    samples: list[float] = []
    try:
        handle = open_process(process.pid)
        counters = PROCESS_MEMORY_COUNTERS()
        counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
        while process.poll() is None:
            value = rss_mb(handle, counters)
            if value >= 0:
                samples.append(value)
            time.sleep(POLL_SECONDS)
        peak = peak_mb(handle, counters)
        kernel32.CloseHandle(handle)
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=10)

    summary = summarize(samples)
    summary.update({
        "scenario": "bench_write",
        "fixture": {"events": events, "repeat": repeat},
        "exit_code": process.returncode,
        "report": str(out) if out.exists() else None,
        "peak_working_set_mb": round(peak, 2) if peak >= 0 else None,
        "budget_mb": None,
        "verdict": "recorded",
        "method": "whcode bench baseline（固定夹具、192MB 缓冲上限、release）；"
                  "GetProcessMemoryInfo 50 ms 轮询采样；无冻结预算，只记录",
    })
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description="内存压力通道（只记录不判定；P7.3／FR-792）")
    parser.add_argument("--events", type=int, default=20_000, help="读压力夹具事件数（默认 20000）")
    parser.add_argument("--repeat", type=int, default=3, help="bench 重复次数（默认 3）")
    parser.add_argument("--readers", type=int, default=4, help="并发读线程数（默认 4）")
    parser.add_argument("--rounds", type=int, default=30, help="每个读者轮数（默认 30）")
    parser.add_argument("--out", type=pathlib.Path, default=REPORT, help="报告输出路径")
    parser.add_argument("--skip-build", action="store_true", help="跳过 release 构建")
    args = parser.parse_args()

    if args.events <= 0 or args.repeat <= 0 or args.readers <= 0 or args.rounds <= 0:
        print("FAIL: 所有规模参数必须大于 0（没有样本无法记录）", flush=True)
        return 1
    if not IS_WINDOWS:
        print("FAIL: 本通道的采样依赖 Win32 GetProcessMemoryInfo；当前平台未测得信号", flush=True)
        return 1

    if not args.skip_build:
        print("构建 release（whcode-bin）…", flush=True)
        run_captured(
            ["cargo", "build", "--locked", "--release", "-p", "whcode-bin"],
            cwd=ROOT,
            check=True,
        )

    report = {
        "channel": "memory-pressure",
        "requirement": "FR-792",
        "adr": "ADR-025",
        "verdict": "recorded",
        "budget_mb": None,
        "utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "os": sys.platform,
        "python": sys.version.split()[0],
        "exe": exe_fingerprint(),
        "poll_seconds": POLL_SECONDS,
        "scenarios": [],
    }
    report["machine"] = platform.machine()
    report["platform"] = platform.platform()

    print("场景 1/2：并发读者（serve）…", flush=True)
    report["scenarios"].append(
        measure_serve_concurrent_readers(args.events, args.readers, args.rounds)
    )
    print("场景 2/2：写入压力（bench baseline）…", flush=True)
    report["scenarios"].append(measure_bench_write(args.events, args.repeat))

    measured = [s for s in report["scenarios"] if s.get("measured")]
    report["measured_scenarios"] = len(measured)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )

    print(f"\n报告：{args.out}")
    for scenario in report["scenarios"]:
        if not scenario.get("measured"):
            print(f"  {scenario['scenario']}: 未取得信号（{scenario.get('reason')}）")
            continue
        print(
            f"  {scenario['scenario']}: 中位 {scenario['median_mb']} MB／"
            f"p95 {scenario['p95_mb']} MB／峰值 {scenario['max_mb']} MB"
            f"（{scenario['samples']} 样本；只记录不判定）"
        )
    if len(measured) != len(report["scenarios"]):
        print("FAIL: 有场景未取得信号（不得记为通过）")
        return 1
    print("RECORDED: 压力通道记录完成（无冻结预算，不作达标结论）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
