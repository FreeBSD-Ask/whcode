#!/usr/bin/env python3
"""GUI 自动化证据（FR-715／T151）：三套主题截图、按键验证、空闲资源采样。

判据（见 `specs/001-whcode-harness/spec.md` FR-712／FR-713／FR-714）：
- 三套主题的客户端区域只出现该主题的精确色值（另计中间灰比例）；
- 按键 `3` 切视图（窗口标题随之变化）、`q` 退出码 0；
- 空闲 10 s 的 RSS／私有内存／线程／句柄／CPU 落盘，供 Check 252 判定。

用法：
    python ci/gui-probe.py [--exe target/release/whcode.exe] [--out artifacts/gui-run-2026-10-23]

工作区建在仓库外的临时目录，避免污染仓库。
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import pathlib
import subprocess
import sys
import time
from ctypes import wintypes

from PIL import ImageGrab

REPO = pathlib.Path(__file__).resolve().parents[1]
THEMES = ("eink-day", "eink-night", "amoled-night")

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.EnumWindows.argtypes = [
    ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM),
    wintypes.LPARAM,
]
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
user32.keybd_event.argtypes = [wintypes.BYTE, wintypes.BYTE, wintypes.DWORD, ctypes.c_size_t]

VK_3 = 0x33
VK_Q = 0x51
VK_R = 0x52


def find_window(pid: int) -> int:
    found: list[int] = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def callback(hwnd, _):  # noqa: ANN001
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(callback, 0)
    return found[0] if found else 0


def window_title(hwnd: int) -> str:
    length = user32.GetWindowTextLengthW(hwnd)
    buffer = ctypes.create_unicode_buffer(length + 1)
    user32.GetWindowTextW(hwnd, buffer, length + 1)
    return buffer.value


def client_box(hwnd: int) -> tuple[int, int, int, int]:
    rect = wintypes.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(rect))
    origin = wintypes.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(origin))
    return (origin.x, origin.y, origin.x + rect.right, origin.y + rect.bottom)


def press(vk: int) -> None:
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(vk, 0, 0x0002, 0)
    time.sleep(0.2)


def sample(pid: int) -> dict[str, float]:
    script = (
        "$p = Get-Process -Id {pid}; "
        "[pscustomobject]@{{"
        "ws=[math]::Round($p.WorkingSet64/1MB,1);"
        "pm=[math]::Round($p.PrivateMemorySize64/1MB,1);"
        "threads=$p.Threads.Count;"
        "handles=$p.HandleCount;"
        "cpu=$p.TotalProcessorTime.TotalSeconds"
        "}} | ConvertTo-Json -Compress"
    ).format(pid=pid)
    out = subprocess.run(
        ["powershell", "-NoProfile", "-Command", script], capture_output=True, text=True, check=True
    )
    return json.loads(out.stdout.strip())


def pixel_stats(image) -> dict[str, float]:  # noqa: ANN001
    """统计最多三种出现最多的颜色与中间灰比例（墨水屏关键判据）。"""
    rgb = image.convert("RGB")
    total = rgb.width * rgb.height
    counts: dict[tuple[int, int, int], int] = {}
    gray = 0
    for red, green, blue in rgb.getdata():
        counts[(red, green, blue)] = counts.get((red, green, blue), 0) + 1
        luminance = (red * 299 + green * 587 + blue * 114) // 1000
        if 32 <= luminance <= 223:
            gray += 1
    top = sorted(counts.items(), key=lambda item: item[1], reverse=True)[:3]
    return {
        "pixels": total,
        "top_colors": [
            {"rgb": f"#{r:02X}{g:02X}{b:02X}", "ratio": round(count / total, 4)} for (r, g, b), count in top
        ],
        "mid_gray_ratio": round(gray / total, 4),
    }


def seed_sessions(exe: pathlib.Path, workspace: pathlib.Path) -> None:
    stamp = time.strftime("%H%M%S")
    for title in (
        f"整理需求基线与验收场景 {stamp}",
        f"验证事件游标重放 {stamp}",
        f"核对三套主题色值 {stamp}",
    ):
        subprocess.run(
            [str(exe), "--workspace", str(workspace), "session", "new", "--title", title],
            capture_output=True,
            check=False,
        )


def run_theme(
    exe: pathlib.Path,
    workspace: pathlib.Path,
    theme: str,
    out_dir: pathlib.Path,
    measure_resources: bool,
) -> dict[str, object]:
    env = dict(os.environ, WHCODE_UI_THEME=theme)
    process = subprocess.Popen(
        [str(exe), "gui", "--workspace", str(workspace)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    result: dict[str, object] = {"theme": theme}
    try:
        hwnd = 0
        for _ in range(150):
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                result["error"] = f"进程提前退出 code={process.returncode} out={stdout!r} err={stderr!r}"
                return result
            hwnd = find_window(process.pid)
            if hwnd:
                break
            time.sleep(0.2)
        if not hwnd:
            result["error"] = "未找到窗口"
            return result

        user32.ShowWindow(hwnd, 9)
        user32.SetForegroundWindow(hwnd)
        time.sleep(2.0)

        result["title_initial"] = window_title(hwnd)
        shot = out_dir / f"gui-{theme}.png"
        image = ImageGrab.grab(bbox=client_box(hwnd))
        image.save(shot)
        result["screenshot"] = str(shot)
        result["client_size"] = [image.width, image.height]
        result["pixels"] = pixel_stats(image)

        press(VK_3)
        press(VK_R)
        press(VK_R)
        time.sleep(1.0)
        result["title_after_keys"] = window_title(hwnd)
        image = ImageGrab.grab(bbox=client_box(hwnd))
        image.save(out_dir / f"gui-{theme}-view3.png")

        if measure_resources:
            first = sample(process.pid)
            time.sleep(10)
            second = sample(process.pid)
            result["resources"] = {
                "working_set_mb": second["ws"],
                "private_mb": second["pm"],
                "threads": second["threads"],
                "handles": second["handles"],
                "idle_cpu_single_core_pct": round((second["cpu"] - first["cpu"]) / 10.0 * 100, 3),
            }

        press(VK_Q)
        try:
            result["exit_code"] = process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            result["exit_code"] = "未退出（超时）"
        return result
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()


def main() -> int:
    parser = argparse.ArgumentParser(description="GUI 自动化证据（FR-715）")
    parser.add_argument("--exe", default=str(REPO / "target" / "release" / "whcode.exe"))
    parser.add_argument("--workspace", default=str(pathlib.Path(os.environ["TEMP"]) / "whcode-gui-run"))
    parser.add_argument("--out", default=str(REPO / "artifacts" / "gui-run-2026-10-23"))
    args = parser.parse_args()

    exe = pathlib.Path(args.exe)
    workspace = pathlib.Path(args.workspace)
    out_dir = pathlib.Path(args.out)
    if not exe.is_file():
        print(f"缺少可执行文件：{exe}（先 cargo build --release -p whcode-bin）", file=sys.stderr)
        return 2
    out_dir.mkdir(parents=True, exist_ok=True)
    workspace.mkdir(parents=True, exist_ok=True)
    seed_sessions(exe, workspace)

    report: dict[str, object] = {"schema_version": 1, "exe": str(exe), "workspace": str(workspace)}
    results = []
    for theme in THEMES:
        outcome = run_theme(exe, workspace, theme, out_dir, measure_resources=theme == "eink-day")
        results.append(outcome)
        print(json.dumps(outcome, ensure_ascii=False))
    report["themes"] = results

    summary = out_dir / "summary.json"
    summary.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    print(f"证据写入 {summary}")
    errors = [item for item in results if "error" in item]
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
