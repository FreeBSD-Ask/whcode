#!/usr/bin/env python3
"""NFR-201/202/205/206 冻结预算测量（T115、ADR-025、V-006 测量纪律；CI 与本地同源）。

测量对象是**真实产品进程**（`whcode serve`／真实控制台 TUI），与
`.github/scripts/benchmarks/measure-resources.py`（测试进程口径）互补；NFR-203/207/209 由
`tests/stability/test_nfr_budget.rs` 进程内测量，NFR-204（72 h 斜率）属 T116。

用法：
    python .github/scripts/benchmarks/measure-nfr-budgets.py [--only nfr201,nfr202] [--skip-build]

输出：artifacts/nfr-budget-report.json + 控制台逐项 PASS/FAIL。
纪律：采样方法写明、零样本判“未取得信号”（不计达标）、分位数输出、
预算取 ADR-025 冻结值，实测劣于预算不得以未采样搪塞。
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wintypes
import datetime
import json
import pathlib
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[3]
EXE = ROOT / "target" / "release" / "whcode.exe"
REPORT = ROOT / "artifacts" / "nfr-budget-report.json"

BUDGETS_MB = {"nfr201": 36.0, "nfr202": 80.0}  # NFR-201 经 ADR-035 由 35 MB 修订为 36 MB
BUDGET_CPU_PCT = {"nfr205": 0.5, "nfr206": 8.0}
POLL_SECONDS = 0.05

PROCESS_QUERY_INFORMATION = 0x0400
PROCESS_VM_READ = 0x0010
CREATE_NEW_CONSOLE = 0x00000010
GENERIC_READ = 0x80000000
GENERIC_WRITE = 0x40000000
FILE_SHARE_READ = 0x1
FILE_SHARE_WRITE = 0x2
OPEN_EXISTING = 3
INVALID_HANDLE_VALUE = -1
KEY_EVENT = 0x0001

# ---------------------------------------------------------------- Win32 采样


class PROCESS_MEMORY_COUNTERS(ctypes.Structure):
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
    _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]


class CHAR_UNION(ctypes.Union):
    _fields_ = [("UnicodeChar", wintypes.WCHAR), ("AsciiChar", ctypes.c_char)]


class KEY_EVENT_RECORD(ctypes.Structure):
    _fields_ = [
        ("bKeyDown", wintypes.BOOL),
        ("wRepeatCount", wintypes.WORD),
        ("wVirtualKeyCode", wintypes.WORD),
        ("wVirtualScanCode", wintypes.WORD),
        ("uChar", CHAR_UNION),
        ("dwControlKeyState", wintypes.DWORD),
    ]


class EVENT_UNION(ctypes.Union):
    _fields_ = [("KeyEvent", KEY_EVENT_RECORD)]


class INPUT_RECORD(ctypes.Structure):
    _fields_ = [("EventType", wintypes.WORD), ("Event", EVENT_UNION)]


kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)  # type: ignore[attr-defined]
psapi = ctypes.WinDLL("psapi", use_last_error=True)  # type: ignore[attr-defined]
kernel32.CreateFileW.restype = wintypes.HANDLE
kernel32.CreateFileW.argtypes = [
    wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, wintypes.LPVOID,
    wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE,
]


def open_process(pid: int) -> int:
    return kernel32.OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, False, pid)


def rss_mb(handle: int, counters: PROCESS_MEMORY_COUNTERS) -> float:
    if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
        return counters.WorkingSetSize / (1024 * 1024)
    return -1.0


def cpu_seconds(handle: int, kernel: FILETIME, user: FILETIME) -> float:
    if kernel32.GetProcessTimes(handle,
                                ctypes.byref(FILETIME()), ctypes.byref(FILETIME()),
                                ctypes.byref(kernel), ctypes.byref(user)):
        def ft(v: FILETIME) -> float:
            return ((v.dwHighDateTime << 32) | v.dwLowDateTime) / 10_000_000
        return ft(kernel) + ft(user)
    return -1.0


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_port(port: int, deadline_s: float = 20.0) -> bool:
    deadline = time.monotonic() + deadline_s
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return True
        except OSError:
            time.sleep(0.2)
    return False


def get_json(url: str, timeout: float = 5.0) -> dict:
    with urllib.request.urlopen(url, timeout=timeout) as response:
        return json.loads(response.read().decode("utf-8"))


# ---------------------------------------------------------------- 夹具


def build_release() -> None:
    subprocess.run(
        ["cargo", "build", "--locked", "--release", "-p", "whcode-bin"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )


def prepare_workspace(dir_path: pathlib.Path) -> None:
    """经产品自身入口迁移（不绕开 schema_migrations 记账）。"""
    dir_path.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [str(EXE), "db", "stats", "--workspace", str(dir_path)],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )


def seed_workspace(dir_path: pathlib.Path, session_id: str, events: int, tasks: int) -> None:
    """以 10 万事件 + 1 万任务对齐 test_session_switch 同夹具口径。"""
    prepare_workspace(dir_path)
    import duckdb

    conn = duckdb.connect(str(dir_path / "whcode.duckdb"))
    conn.execute(
        "INSERT INTO sessions (id,title,created_at,last_active_at,status,stage) "
        "VALUES (?, 'nfr202 夹具', epoch_ms(1), epoch_ms(1), 'active', 'plan')",
        [session_id],
    )
    rows = [
        (
            seq, f"E-{seq}", session_id, "StreamingChunk",
            '{"pad":"' + "x" * 64 + '","seq":%d}' % seq,
        )
        for seq in range(1, events + 1)
    ]
    conn.executemany(
        "INSERT INTO events (seq,id,session_id,kind,payload_json,ts,persistence_level) "
        "VALUES (?,?,?,?,?,current_timestamp,'redacted')",
        rows,
    )
    task_rows = [
        (f"T-{index}", session_id, f"任务 {index}", "plan", "pending", "[]")
        for index in range(1, tasks + 1)
    ]
    conn.executemany(
        "INSERT INTO tasks (id,session_id,title,stage,status,acceptance_json,retry_count,updated_at) "
        "VALUES (?,?,?,?,?,?,0,current_timestamp)",
        task_rows,
    )
    conn.close()


# ---------------------------------------------------------------- 测量项


def measure_nfr201() -> dict:
    """NFR-201 主进程空载 RSS ≤ 35 MB：serve 打开空工作区后空闲 10 s。"""
    ws = pathlib.Path(tempfile.mkdtemp(prefix="whcode-nfr201-"))
    prepare_workspace(ws)
    port = free_port()
    process = subprocess.Popen(
        [str(EXE), "serve", "--web", "--bind", f"127.0.0.1:{port}", "--workspace", str(ws)],
        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_port(port):
            return {"nfr": "NFR-201", "measured": False, "passed": False,
                    "reason": "serve 未在 20 s 内监听端口（未取得信号）"}
        time.sleep(1.0)  # 首请求后的页缓存沉降
        handle = open_process(process.pid)
        counters = PROCESS_MEMORY_COUNTERS()
        counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
        samples = []
        deadline = time.monotonic() + 10.0
        while time.monotonic() < deadline:
            value = rss_mb(handle, counters)
            if value >= 0:
                samples.append(value)
            time.sleep(POLL_SECONDS)
        kernel32.CloseHandle(handle)
    finally:
        process.kill()
    if not samples:
        return {"nfr": "NFR-201", "measured": False, "passed": False,
                "reason": "零样本（未取得信号）"}
    samples.sort()
    median = samples[len(samples) // 2]
    p95 = samples[int(len(samples) * 0.95)]
    peak = samples[-1]
    return {
        "nfr": "NFR-201", "measured": True,
        "budget_mb": BUDGETS_MB["nfr201"], "samples": len(samples),
        "median_mb": round(median, 2), "p95_mb": round(p95, 2), "max_mb": round(peak, 2),
        "passed": median <= BUDGETS_MB["nfr201"],
        "method": "serve --web 空工作区空闲 10 s，GetProcessMemoryInfo 50 ms 轮询，判据取中位数",
    }


def measure_nfr202() -> dict:
    """NFR-202 单会话活跃 RSS ≤ 80 MB：10 万事件工作区 + 持续读请求。"""
    ws = pathlib.Path(tempfile.mkdtemp(prefix="whcode-nfr202-"))
    seed_workspace(ws, "S-BIG", 100_000, 10_000)
    port = free_port()
    process = subprocess.Popen(
        [str(EXE), "serve", "--web", "--bind", f"127.0.0.1:{port}", "--workspace", str(ws)],
        cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    try:
        if not wait_port(port):
            return {"nfr": "NFR-202", "measured": False, "passed": False,
                    "reason": "serve 未在 20 s 内监听端口（未取得信号）"}
        handle = open_process(process.pid)
        counters = PROCESS_MEMORY_COUNTERS()
        counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
        samples = []
        cursor = 99_500
        for _ in range(20):
            get_json(f"http://127.0.0.1:{port}/api/sessions")
            get_json(f"http://127.0.0.1:{port}/api/events?session=S-BIG&after={cursor}")
            get_json(f"http://127.0.0.1:{port}/api/tasks")
            value = rss_mb(handle, counters)
            if value >= 0:
                samples.append(value)
            time.sleep(0.1)
        kernel32.CloseHandle(handle)
    finally:
        process.kill()
    if not samples:
        return {"nfr": "NFR-202", "measured": False, "passed": False,
                "reason": "零样本（未取得信号）"}
    samples.sort()
    median = samples[len(samples) // 2]
    peak = samples[-1]
    return {
        "nfr": "NFR-202", "measured": True,
        "budget_mb": BUDGETS_MB["nfr202"], "samples": len(samples),
        "median_mb": round(median, 2), "max_mb": round(peak, 2),
        "passed": median <= BUDGETS_MB["nfr202"],
        "method": "10 万事件+1 万任务工作区，20 轮 sessions/events/tasks 读请求，"
                  "GetProcessMemoryInfo 采样，判据取中位数",
    }


def measure_nfr205() -> dict:
    """NFR-205 TUI 空闲 CPU ≤ 0.5% 单核：真实控制台 TUI 空闲 10 s。"""
    try:
        import win32console
        import win32event  # noqa: F401
        import win32process  # noqa: F401
    except ImportError:
        return {"nfr": "NFR-205", "measured": False, "passed": False,
                "reason": "pywin32 不可用，真实控制台 TUI 无法驱动（未取得信号）"}
    import win32api

    ws = pathlib.Path(tempfile.mkdtemp(prefix="whcode-nfr205-"))
    prepare_workspace(ws)
    startup = win32process.STARTUPINFO()
    process_handle, _thread, pid, _tid = win32process.CreateProcess(
        None, f'"{EXE}" --workspace "{ws}"', None, None, False,
        CREATE_NEW_CONSOLE, None, str(ROOT), startup,
    )
    handle = open_process(pid)
    counters = PROCESS_MEMORY_COUNTERS()
    counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
    kernel_t, user_t = FILETIME(), FILETIME()
    attached = False
    try:
        time.sleep(2.0)  # 启动首帧与迁移完成
        cpu_start = cpu_seconds(handle, kernel_t, user_t)
        if cpu_start < 0:
            return {"nfr": "NFR-205", "measured": False, "passed": False,
                    "reason": "GetProcessTimes 取数失败（未取得信号）"}
        time.sleep(10.0)  # 空闲窗口：不注入任何输入
        cpu_end = cpu_seconds(handle, kernel_t, user_t)
        rss = rss_mb(handle, counters)
        # 注入 q 必须落到**子进程**的新控制台：先脱离本进程控制台再附着子进程
        # （tui_manual_smoke 同款；CONIN$ 打开的是“当前附着控制台”的输入缓冲）。
        win32console.FreeConsole()
        attached = True
        deadline = time.monotonic() + 20.0
        while True:
            try:
                win32console.AttachConsole(pid)
                break
            except Exception:  # noqa: BLE001  子控制台未就绪则重试
                if time.monotonic() > deadline:
                    raise
                time.sleep(0.1)
        records = []
        for down in (True, False):
            record = INPUT_RECORD()
            record.EventType = KEY_EVENT
            key = KEY_EVENT_RECORD()
            key.bKeyDown = 1 if down else 0
            key.wRepeatCount = 1
            key.uChar.UnicodeChar = "q"
            record.Event.KeyEvent = key
            records.append(record)
        conin = kernel32.CreateFileW(
            "CONIN$", GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, 0, None,
        )
        array = (INPUT_RECORD * len(records))(*records)
        written = wintypes.DWORD(0)
        ok = kernel32.WriteConsoleInputW(conin, array, len(records), ctypes.byref(written))
        kernel32.CloseHandle(conin)
        if not ok:
            return {"nfr": "NFR-205", "measured": False, "passed": False,
                    "reason": "WriteConsoleInputW 失败（未取得信号）"}
        win32console.FreeConsole()
        code = win32event.WaitForSingleObject(process_handle, 10_000)
    finally:
        kernel32.CloseHandle(handle)
        if attached:
            try:
                win32console.FreeConsole()
            except Exception:  # noqa: BLE001  控制台已随子进程消亡
                pass
    if code != 0:
        win32api.TerminateProcess(process_handle, 1)
        return {"nfr": "NFR-205", "measured": False, "passed": False,
                "reason": "TUI 未在 10 s 内响应 q 退出（未取得信号）"}
    wall = 10.0
    cpu_pct = (cpu_end - cpu_start) / wall * 100.0
    return {
        "nfr": "NFR-205", "measured": True,
        "budget_cpu_pct": BUDGET_CPU_PCT["nfr205"],
        "cpu_pct": round(cpu_pct, 3), "rss_mb_at_exit": round(rss, 2) if rss >= 0 else None,
        "passed": cpu_pct <= BUDGET_CPU_PCT["nfr205"],
        "method": "真实控制台 TUI（CREATE_NEW_CONSOLE + AttachConsole）空闲 10 s（无输入），"
                  "GetProcessTimes 前后差 / 墙钟 = 单核百分比；q 注入退出",
    }


def measure_nfr206() -> dict:
    """NFR-206 LLM 流式期间 CPU ≤ 8% 单核：外部采样运行中的测试可执行文件。

    两个负载：限速（25 chunks/s，代表性速率，作判据）与满喂（容量佐证）。
    注意：必须采样**测试可执行文件**本身，不能采 `cargo test` 包装进程
    （cargo 自身在测试期间几乎零 CPU——首跑实测采到 0.0% 的歧义信号，已修正）。
    """
    build = subprocess.run(
        ["cargo", "test", "--locked", "--no-run", "-p", "whcode-tests", "--test", "nfr_budget"],
        cwd=ROOT, capture_output=True, text=True,
    )
    if build.returncode != 0:
        return {"nfr": "NFR-206", "measured": False, "passed": False,
                "reason": "测试目标构建失败（未取得信号）"}
    candidates = sorted(
        (ROOT / "target" / "debug" / "deps").glob("nfr_budget-*.exe"),
        key=lambda path: path.stat().st_mtime,
    )
    if not candidates:
        return {"nfr": "NFR-206", "measured": False, "passed": False,
                "reason": "未找到测试可执行文件（未取得信号）"}
    test_exe = str(candidates[-1])

    def run_case(filter_name: str) -> tuple[subprocess.Popen, str]:
        process = subprocess.Popen(
            [test_exe, filter_name, "--ignored", "--exact", "--nocapture"],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
        )
        handle = open_process(process.pid)
        counters = PROCESS_MEMORY_COUNTERS()
        counters.cb = ctypes.sizeof(PROCESS_MEMORY_COUNTERS)
        kernel_t, user_t = FILETIME(), FILETIME()
        started = time.monotonic()
        cpu_start = cpu_seconds(handle, kernel_t, user_t)
        peak_rss = 0.0
        samples = 0
        while process.poll() is None:
            value = rss_mb(handle, counters)
            if value >= 0:
                peak_rss = max(peak_rss, value)
                samples += 1
            time.sleep(POLL_SECONDS)
        cpu_end = cpu_seconds(handle, kernel_t, user_t)
        kernel32.CloseHandle(handle)
        output = process.stdout.read() if process.stdout else ""
        wall = time.monotonic() - started
        stats = {
            "exit": process.returncode,
            "cpu_start": cpu_start, "cpu_end": cpu_end,
            "wall_s": round(wall, 2), "peak_rss_mb": round(peak_rss, 2),
            "samples": samples, "output": output,
        }
        return stats, output

    paced, paced_out = run_case("nfr206_llm_stream_paced_realistic_rate")
    if paced["exit"] != 0 or paced["cpu_start"] < 0 or paced["cpu_end"] < 0 or paced["samples"] == 0:
        return {"nfr": "NFR-206", "measured": False, "passed": False,
                "reason": f"限速负载未成功或未取得信号（exit={paced['exit']}）",
                "stdout_tail": paced_out[-2000:]}
    cpu_pct = (paced["cpu_end"] - paced["cpu_start"]) / paced["wall_s"] * 100.0
    capacity = None
    full, full_out = run_case("nfr206_llm_stream_full_feed_capacity")
    if full["exit"] == 0:
        for line in full_out.splitlines():
            if "chunks/s/核" in line:
                try:
                    capacity = float(line.split("=")[1].split("chunks/s")[0].strip())
                except (IndexError, ValueError):
                    capacity = None
    return {
        "nfr": "NFR-206", "measured": True,
        "budget_cpu_pct": BUDGET_CPU_PCT["nfr206"],
        "cpu_pct_overall": round(cpu_pct, 3), "wall_s": paced["wall_s"],
        "peak_rss_mb": paced["peak_rss_mb"], "samples": paced["samples"],
        "full_feed_capacity_chunks_per_s_per_core": capacity,
        "passed": cpu_pct <= BUDGET_CPU_PCT["nfr206"],
        "method": "限速流式（25 chunks/s，代表性 token 速率）测试可执行文件整生命周期 "
                  "GetProcessTimes 采样，整体 CPU% 为含夹具开销的保守上界（判据）；"
                  "满喂容量为余量佐证（不作判据）",
    }


# ---------------------------------------------------------------- 编排


def discipline() -> dict:
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True).stdout.strip()
    uname = subprocess.run(["cmd", "/C", "ver"], capture_output=True, text=True).stdout.strip()
    return {
        "generated_at": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "os": uname,
        "toolchain": rustc,
        "build_type": "release",
        "exe": str(EXE),
        "sampling": "GetProcessMemoryInfo（RSS）/ GetProcessTimes（CPU），50 ms 轮询；"
                    "Rust 侧 tests/stability/test_nfr_budget.rs 进程内 Instant+current_rss_bytes",
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", default="nfr201,nfr202,nfr205,nfr206")
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    selected = [item.strip() for item in args.only.split(",") if item.strip()]

    if not args.skip_build:
        build_release()
    results = []
    runners = {
        "nfr201": measure_nfr201,
        "nfr202": measure_nfr202,
        "nfr205": measure_nfr205,
        "nfr206": measure_nfr206,
    }
    for name in selected:
        if name not in runners:
            continue
        print(f"测量 {name} …", flush=True)
        results.append(runners[name]())

    report = {"discipline": discipline(), "results": results}
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")

    failures = 0
    for result in results:
        if not result.get("measured"):
            failures += 1
            print(f"FAIL {result['nfr']}: 未取得信号——{result.get('reason')}")
            continue
        status = "PASS" if result["passed"] else "FAIL"
        failures += 0 if result["passed"] else 1
        if "cpu_pct" in result:
            print(f"{status} {result['nfr']}: CPU {result['cpu_pct']}%"
                  f"（预算 ≤ {result['budget_cpu_pct']}% 单核）")
        elif "cpu_pct_overall" in result:
            print(f"{status} {result['nfr']}: CPU（整体保守上界）{result['cpu_pct_overall']}%"
                  f"（预算 ≤ {result['budget_cpu_pct']}% 单核，wall {result['wall_s']}s）")
        else:
            print(f"{status} {result['nfr']}: RSS 中位 {result['median_mb']} MB / "
                  f"p95 {result.get('p95_mb')} MB / 峰值 {result['max_mb']} MB"
                  f"（预算 ≤ {result['budget_mb']} MB，样本 {result['samples']}）")
    print(f"报告：{REPORT}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
