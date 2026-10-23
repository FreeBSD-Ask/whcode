#!/usr/bin/env python3
"""GUI 自动化证据（FR-715／T151）：三套主题截图、按键验证、空闲资源采样。

判据（见 `specs/001-whcode-harness/spec.md` FR-712／FR-713／FR-714）：
- 三套主题的客户端区域只出现该主题的精确色值（另计中间灰比例）；
- 按键 `3` 切视图（窗口标题随之变化）、`t` 循环三套主题（不重启）、`q` 退出码 0；
- 空闲 10 s 的 RSS／私有内存／线程／句柄／CPU 落盘，供 Check 252 判定；
- 最小化 10 s 的 CPU 增量（停绘）与还原后的标题／画面（FR-714）。

用法：
    python ci/gui-probe.py [--exe target/release/whcode.exe] [--out artifacts/gui-run-2026-10-23]

工作区建在仓库外的临时目录，避免污染仓库。

坐标与尺寸口径：脚本先声明 per-monitor v2 DPI 感知（全部按物理像素），再把窗口
调整到能完整落在主屏内的尺寸后截图；报告记录 `dpi_scale`、默认客户端尺寸与
调整后尺寸，避免把“窗口超出屏幕”误当成渲染问题。
"""

from __future__ import annotations

import argparse
import ctypes
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
from ctypes import wintypes

from PIL import ImageGrab

REPO = pathlib.Path(__file__).resolve().parents[1]
THEMES = ("eink-day", "eink-night", "amoled-night")
THEME_PALETTES = {
    "eink-day": ("#FFFFFF", "#000000"),
    "eink-night": ("#000000", "#FFFFFF"),
    "amoled-night": ("#000000", "#E6E6E6"),
}

# 坐标口径统一为物理像素：否则 GetClientRect 返回的是 DPI 虚拟化后的逻辑像素，
# 与 ImageGrab 的物理像素不一致，会在非 100% 缩放下截错区域。
try:
    if not ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4)):
        raise OSError("SetProcessDpiAwarenessContext 失败")
except (AttributeError, OSError):
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except (AttributeError, OSError):
        ctypes.windll.user32.SetProcessDPIAware()

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32.EnumWindows.argtypes = [
    ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM),
    wintypes.LPARAM,
]
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.GetWindowThreadProcessId.restype = wintypes.DWORD
user32.GetForegroundWindow.restype = wintypes.HWND
user32.AttachThreadInput.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.BOOL]
user32.BringWindowToTop.argtypes = [wintypes.HWND]
user32.SetFocus.argtypes = [wintypes.HWND]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
user32.SetForegroundWindow.argtypes = [wintypes.HWND]
user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
user32.IsIconic.argtypes = [wintypes.HWND]
user32.SetWindowPos.argtypes = [
    wintypes.HWND,
    wintypes.HWND,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_uint,
]
user32.GetDpiForWindow.argtypes = [wintypes.HWND]
user32.GetDpiForWindow.restype = ctypes.c_uint
user32.keybd_event.argtypes = [wintypes.BYTE, wintypes.BYTE, wintypes.DWORD, ctypes.c_size_t]

VK_3 = 0x33
VK_6 = 0x36
VK_Q = 0x51
VK_R = 0x52
VK_T = 0x54
SW_MINIMIZE = 6
SW_RESTORE = 9
SWP_NOZORDER = 0x0004
SWP_NOACTIVATE = 0x0010


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


def fit_window(hwnd: int, client_w: int = 1100, client_h: int = 900) -> list[int]:
    """把窗口移到左上角并调整到完整可见的物理尺寸，返回调整后的客户端尺寸。

    应用默认窗口是 1200×800 **逻辑**像素；在 200% 缩放的屏幕上等于 2400×1600
    物理像素，超出屏幕后截图会被裁切。这里按物理像素给出可完整容纳的客户端区域，
    并把实际尺寸记入证据（资源预算仍以帧缓冲像素数为准）。
    """
    window_rect = wintypes.RECT()
    client_rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(window_rect))
    user32.GetClientRect(hwnd, ctypes.byref(client_rect))
    frame_w = (window_rect.right - window_rect.left) - client_rect.right
    frame_h = (window_rect.bottom - window_rect.top) - client_rect.bottom
    screen_w = user32.GetSystemMetrics(0)
    screen_h = user32.GetSystemMetrics(1)
    width = min(client_w, screen_w - 20) + frame_w
    height = min(client_h, screen_h - 80) + frame_h
    user32.SetWindowPos(hwnd, 0, 10, 10, width, height, SWP_NOZORDER | SWP_NOACTIVATE)
    time.sleep(0.5)
    user32.GetClientRect(hwnd, ctypes.byref(client_rect))
    return [client_rect.right, client_rect.bottom]


def on_screen(box: list[int]) -> bool:
    """客户端区域是否完整落在主屏内（截图有效性的前置条件）。"""
    return (
        box[0] >= 0
        and box[1] >= 0
        and box[2] <= user32.GetSystemMetrics(0)
        and box[3] <= user32.GetSystemMetrics(1)
    )


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


def client_size(hwnd: int) -> tuple[int, int]:
    rect = wintypes.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(rect))
    return rect.right, rect.bottom


def grab_client(hwnd: int, path: pathlib.Path, timeout: float = 10.0):  # noqa: ANN201
    """等待窗口可见且客户端区域非空后截图；超时返回 `None` 并保留失败原因。

    最小化后窗口可能尚未完成还原（`IsIconic` 仍为真或客户端区为 0×0），
    此时 `ImageGrab.grab` 会得到空图并触发 Pillow 的 `cannot write empty image`。
    """
    deadline = time.time() + timeout
    box = (0, 0, 0, 0)
    while time.time() < deadline:
        if not user32.IsIconic(hwnd):
            box = client_box(hwnd)
            if box[2] - box[0] > 0 and box[3] - box[1] > 0:
                break
        time.sleep(0.2)
    if box[2] - box[0] <= 0 or box[3] - box[1] <= 0:
        return None, list(box)
    image = ImageGrab.grab(bbox=box)
    if image.width <= 0 or image.height <= 0:
        return None, list(box)
    image.save(path)
    return image, list(box)


def focus(hwnd: int) -> bool:
    """把目标窗口切到前台；返回是否成功。

    `SetForegroundWindow` 对后台进程有限制，需先附加到当前前台线程的输入队列，
    否则按键会发到别的窗口、截图也会截到别的窗口（表现为“q 不退出”）。
    """
    if user32.GetForegroundWindow() == hwnd:
        return True
    foreground = user32.GetForegroundWindow()
    foreground_thread = user32.GetWindowThreadProcessId(foreground, None)
    current_thread = kernel32.GetCurrentThreadId()
    attached = bool(user32.AttachThreadInput(foreground_thread, current_thread, True))
    user32.BringWindowToTop(hwnd)
    user32.SetForegroundWindow(hwnd)
    user32.SetFocus(hwnd)
    if attached:
        user32.AttachThreadInput(foreground_thread, current_thread, False)
    time.sleep(0.3)
    return user32.GetForegroundWindow() == hwnd


def press(vk: int, hwnd: int) -> bool:
    """向目标窗口注入一次按键；窗口不在前台时先尝试切前台。"""
    if not focus(hwnd):
        return False
    user32.keybd_event(vk, 0, 0, 0)
    time.sleep(0.05)
    user32.keybd_event(vk, 0, 0x0002, 0)
    time.sleep(0.2)
    return True


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

        user32.ShowWindow(hwnd, SW_RESTORE)
        result["focus_initial"] = focus(hwnd)
        time.sleep(1.0)
        result["dpi_scale"] = round(user32.GetDpiForWindow(hwnd) / 96.0, 2)
        result["default_client_size"] = list(client_size(hwnd))
        result["fitted_client_size"] = fit_window(hwnd)
        time.sleep(1.0)

        result["title_initial"] = window_title(hwnd)
        shot = out_dir / f"gui-{theme}.png"
        image, box = grab_client(hwnd, shot)
        result["client_box"] = box
        result["client_on_screen"] = on_screen(box)
        if image is None:
            result["error"] = "初始截图失败：客户端区域为空"
            return result
        result["screenshot"] = str(shot)
        result["client_size"] = [image.width, image.height]
        result["pixels"] = pixel_stats(image)

        # FR-712：空闲基准必须在注入任何按键之前采样（交互后的句柄数不代表空闲态）
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

        press(VK_3, hwnd)
        press(VK_R, hwnd)
        press(VK_R, hwnd)
        time.sleep(1.0)
        result["title_after_keys"] = window_title(hwnd)
        grab_client(hwnd, out_dir / f"gui-{theme}-view3.png")

        # 等宽字体证据：diff 预览使用 JetBrains Mono（拉丁）+ Sarasa Mono SC（CJK）
        press(VK_6, hwnd)
        time.sleep(0.8)
        result["title_after_diff"] = window_title(hwnd)
        grab_client(hwnd, out_dir / f"gui-{theme}-diff.png")
        press(VK_3, hwnd)
        time.sleep(0.5)

        # FR-713：`t` 键循环三套主题，切换不重启（三次后回到起始主题）
        start = THEMES.index(theme)
        expected = [
            sorted(THEME_PALETTES[THEMES[(start + step) % len(THEMES)]])
            for step in (1, 2, 3)
        ]
        actual: list[list[str] | None] = []
        for step in range(3):
            if not press(VK_T, hwnd):
                actual.append(None)
                continue
            time.sleep(0.4)
            image, _ = grab_client(hwnd, out_dir / f"gui-{theme}-theme{step + 1}.png")
            if image is None:
                actual.append(None)
                continue
            actual.append(sorted(item["rgb"] for item in pixel_stats(image)["top_colors"][:2]))
        result["theme_cycle"] = {"expected": expected, "actual": actual, "ok": expected == actual}

        if measure_resources:
            # FR-714：最小化期间必须停止绘制（实测 CPU 增量 + 还原后一次重绘）
            user32.ShowWindow(hwnd, SW_MINIMIZE)
            time.sleep(1.5)
            before = sample(process.pid)
            result["minimized_iconic"] = bool(user32.IsIconic(hwnd))
            time.sleep(10)
            after = sample(process.pid)
            result["minimized"] = {
                "cpu_single_core_pct": round((after["cpu"] - before["cpu"]) / 10.0 * 100, 3),
                "cpu_seconds_delta": round(after["cpu"] - before["cpu"], 3),
            }
            user32.ShowWindow(hwnd, SW_RESTORE)
            result["focus_after_restore"] = focus(hwnd)
            time.sleep(1.0)
            result["title_after_restore"] = window_title(hwnd)
            restored = out_dir / f"gui-{theme}-restored.png"
            image, box = grab_client(hwnd, restored)
            result["restored_client_box"] = box
            result["restored_on_screen"] = on_screen(box)
            if image is None:
                result["restored_error"] = "还原后截图失败：客户端区域为空"
            else:
                result["restored_screenshot"] = str(restored)
                result["restored_pixels"] = pixel_stats(image)

        result["quit_key_delivered"] = press(VK_Q, hwnd)
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
    parser.add_argument(
        "--workspace",
        default="",
        help="留空则每次新建临时工作区（跑完删除），保证截图内容可复现",
    )
    parser.add_argument("--out", default=str(REPO / "artifacts" / "gui-run-2026-10-23"))
    args = parser.parse_args()

    exe = pathlib.Path(args.exe)
    out_dir = pathlib.Path(args.out)
    if not exe.is_file():
        print(f"缺少可执行文件：{exe}（先 cargo build --release -p whcode-bin）", file=sys.stderr)
        return 2
    out_dir.mkdir(parents=True, exist_ok=True)
    if args.workspace:
        workspace = pathlib.Path(args.workspace)
        workspace.mkdir(parents=True, exist_ok=True)
        ephemeral = False
    else:
        workspace = pathlib.Path(tempfile.mkdtemp(prefix="whcode-gui-run-"))
        ephemeral = True
    seed_sessions(exe, workspace)

    report: dict[str, object] = {"schema_version": 1, "exe": str(exe), "workspace": str(workspace)}
    results = []
    try:
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
        failures = [
            item
            for item in results
            if "error" in item
            or item.get("exit_code") != 0
            or not item.get("theme_cycle", {}).get("ok", False)
            or item.get("focus_initial") is False
            or item.get("focus_after_restore") is False
        ]
        if failures:
            names = "、".join(str(item.get("theme")) for item in failures)
            print(f"未达标主题：{names}（详见 summary.json）", file=sys.stderr)
        return 1 if failures else 0
    finally:
        if ephemeral:
            shutil.rmtree(workspace, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
