"""TUI 启动分支冒烟脚本（tasks.md T024 / V-003 辅助证据）。

用途：验证 `whcode-bin` 无参数启动时的分支选择，以及 `--version` / `doctor` 的既有行为。

背景（Windows 实测事实）：crossterm 在 Windows 上经 `CONOUT$` 直接打开控制台设备
（`crossterm_winapi::Handle::current_out_handle`），因此**只要进程附加了控制台**，即使
stdout 被重定向为管道，`terminal_size()` 仍返回 `Some`。真正的“无控制台”环境需用
`DETACHED_PROCESS` 启动才能复现；此时才走骨架分支并立即退出（不挂起）。

默认只跑**非交互**用例（headless 分支、`--version`、`doctor`）。加 `--with-console` 会额外
运行一次“附加控制台”用例——它会进入交互 TUI 并等待按键，脚本将以超时终止该子进程，并在
`finally` 中把控制台输入模式恢复为“熟”模式（补齐 crossterm raw mode 清除的 3 个位）。

用法：`python script/tui_non_tty_smoke.py [--with-console]`（需先 `cargo build -p whcode-bin`）。
"""

import ctypes
import subprocess
import sys
from ctypes import wintypes
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")
sys.stderr.reconfigure(encoding="utf-8")

EXE = Path(__file__).resolve().parent.parent / "target" / "debug" / "whcode-bin.exe"

DETACHED_PROCESS = 0x00000008
# ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT
NOT_RAW_MODE_MASK = 0x7
GENERIC_READ_WRITE = 0xC0000000
FILE_SHARE_READ_WRITE = 0x3
OPEN_EXISTING = 3

KERNEL32 = ctypes.windll.kernel32
KERNEL32.CreateFileW.restype = wintypes.HANDLE
KERNEL32.CreateFileW.argtypes = [
    wintypes.LPCWSTR,
    wintypes.DWORD,
    wintypes.DWORD,
    ctypes.c_void_p,
    wintypes.DWORD,
    wintypes.DWORD,
    wintypes.HANDLE,
]
KERNEL32.CloseHandle.argtypes = [wintypes.HANDLE]

SKELETON_MARK = "骨架阶段"


def ensure_cooked_console() -> None:
    """把当前控制台输入模式恢复为“熟”模式（幂等；无控制台时跳过）。"""
    handle = KERNEL32.CreateFileW(
        "CONIN$",
        GENERIC_READ_WRITE,
        FILE_SHARE_READ_WRITE,
        None,
        OPEN_EXISTING,
        0,
        None,
    )
    if handle == wintypes.HANDLE(-1).value:
        print("[console] 无可用控制台，跳过输入模式恢复")
        return
    mode = wintypes.DWORD()
    if KERNEL32.GetConsoleMode(handle, ctypes.byref(mode)):
        restored = mode.value | NOT_RAW_MODE_MASK
        KERNEL32.SetConsoleMode(handle, restored)
        print(f"[console] 输入模式恢复：{hex(mode.value)} -> {hex(restored)}")
    KERNEL32.CloseHandle(handle)


def run(label: str, args: list[str], *, creationflags: int = 0, timeout: float = 10.0) -> None:
    try:
        proc = subprocess.run(
            [str(EXE), *args],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            stdin=subprocess.DEVNULL,
            timeout=timeout,
            creationflags=creationflags,
        )
    except subprocess.TimeoutExpired as exc:
        partial = exc.stdout or ""
        if isinstance(partial, str):
            partial = partial.encode("utf-8", "replace")
        entered_alt_screen = b"\x1b[?1049h" in partial
        print(f"[{label}] 超时（{timeout}s 内未退出），已终止子进程")
        print(f"    捕获到备用屏幕序列(alternate screen)：{entered_alt_screen}")
        print("    → 有可用控制台时进入交互 TUI 并等待按键，属预期交互行为")
        print("-" * 60)
        return

    body = proc.stdout
    print(f"[{label}] args={args!r} exit={proc.returncode} skeleton_branch={SKELETON_MARK in body}")
    for line in body.splitlines():
        print(f"    stdout| {line}")
    for line in proc.stderr.splitlines():
        print(f"    stderr| {line}")
    print("-" * 60)


if __name__ == "__main__":
    if not EXE.exists():
        sys.exit(f"未找到可执行文件：{EXE}（先运行 cargo build -p whcode-bin）")
    with_console = "--with-console" in sys.argv[1:]
    try:
        # 无控制台：DETACHED_PROCESS，terminal_size() 应为 None → 骨架分支，立即退出。
        run("headless/detached", [], creationflags=DETACHED_PROCESS)
        # 既有入口不受影响。
        run("version", ["--version"])
        run("doctor", ["doctor"])
        if with_console:
            # 附加控制台但 stdio 被重定向：terminal_size() 仍为 Some → 进入交互 TUI。
            run("console-attached", [], timeout=3.0)
    finally:
        ensure_cooked_console()
