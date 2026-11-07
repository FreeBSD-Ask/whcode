#!/usr/bin/env python3
"""品牌图标派生：从 `assets/brand/logo.svg` 生成 GUI 窗口图标与 Web favicon。

性质：`assets/brand/logo.svg` 是**唯一母版**（作者创作，BSD-2-Clause，见
`assets/brand/SOURCES.md`）；本脚本从母版派生两种二进制形态并**入库**：

| 产物 | 消费方 | 形态 |
|---|---|---|
| `crates/whcode-gui/assets/brand/icon-64.rgba` | `whcode-gui`（窗口图标） | 64×64 原始 RGBA，`include_bytes!` 直接可用，**不引入 PNG 解码依赖** |
| `crates/whcode-web/frontend/favicon.ico` | `whcode-web`（`/favicon.ico`） | 16/32/48 三帧 ICO |

为什么派生而不在运行期解码母版：GUI 与 Web 都禁止为一张图标引入图像解码依赖
（NFR-215 依赖最小化；ADR-014 前端零构建链）。派生在构建期之前一次完成，产物入库，
运行期只 `include_bytes!`。

母版结构（如实记录）：`logo.svg` 是**位图封装**——`<defs>` 里的两个 `<image>` 内嵌
base64 PNG（整页 `img1`、叠加层 `img2`），由两个 `<use x=… y=…>` 叠合。本脚本按
`<use>` 的偏移合成，**不是**只取第一层。

用法：
    python .github/scripts/make-brand-assets.py            # 重新生成全部产物
    python .github/scripts/make-brand-assets.py --check    # 只校验产物与母版一致（CI 可用）

依赖：pillow（已在 requirements-dev.txt；仅本地/CI 工具，产品运行期零第三方依赖）。

复审触发（ADR-046 口径，绑定命令＋判定）：
  1. `python .github/scripts/make-brand-assets.py --check` 非零退出 → 产物与母版漂移；
  2. 母版缺失 `<image>`/`<use>` 或层数变化 → 脚本显式报错退出（不回退到“取第一层”，
     因为那会静默生成错误图标）；
  3. 新增消费方（如安装包图标、托盘图标）→ 本脚本与 ADR-061 同批复审。
"""

from __future__ import annotations

import argparse
import base64
import io
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MASTER = ROOT / "assets" / "brand" / "logo.svg"

GUI_ICON = ROOT / "crates" / "whcode-gui" / "assets" / "brand" / "icon-64.rgba"
GUI_ICON_SIZE = 64
WEB_FAVICON = ROOT / "crates" / "whcode-web" / "frontend" / "favicon.ico"
FAVICON_SIZES = (16, 32, 48)

# 母版画布尺寸（viewBox 与 <image> 尺寸均为 1080×1080）；变化即视为母版换了版面。
CANVAS = 1080

IMAGE_RE = re.compile(
    r'<image[^>]*?id="(?P<id>img\d+)"[^>]*?href="data:image/png;base64,(?P<data>[^"]+)"',
    re.S,
)
USE_RE = re.compile(r'<use[^>]*?href="#(?P<id>img\d+)"[^>]*?x="(?P<x>\d+)"[^>]*?y="(?P<y>\d+)"')


def _pil():  # 延迟导入：让 `--help` 在未装 pillow 时仍可用
    try:
        from PIL import Image
    except ImportError:  # pragma: no cover - 环境缺失路径
        sys.exit(
            "缺少 pillow：python -m pip install -r requirements-dev.txt（仅生成期需要）"
        )
    return Image


def load_master():
    """解码母版并按其 `<use>` 偏移合成，返回合成后的 RGBA 图。"""
    Image = _pil()
    if not MASTER.is_file():
        sys.exit(f"母版缺失：{MASTER.relative_to(ROOT)}")
    svg = MASTER.read_text(encoding="utf-8")

    layers: dict[str, bytes] = {m["id"]: m["data"] for m in IMAGE_RE.finditer(svg)}
    uses = [(m["id"], int(m["x"]), int(m["y"])) for m in USE_RE.finditer(svg)]
    if not layers or not uses:
        sys.exit(
            "母版结构不符合预期：需要 <image id=\"imgN\" href=\"data:image/png;base64,…\"> "
            "与 <use href=\"#imgN\" x=… y=…>；不得回退到只取第一层"
        )
    missing = [name for name, _, _ in uses if name not in layers]
    if missing:
        sys.exit(f"母版 <use> 引用了不存在的层：{missing}")

    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    for name, x, y in uses:  # 文档顺序即叠加顺序
        image = Image.open(io.BytesIO(base64.b64decode(layers[name]))).convert("RGBA")
        canvas.alpha_composite(image, (x, y))
    return canvas


def gui_icon_bytes(master) -> bytes:
    """64×64 原始 RGBA（逐像素、无压缩、无元数据 → 字节级可复现）。"""
    return master.resize((GUI_ICON_SIZE, GUI_ICON_SIZE)).tobytes()


def favicon_bytes(master) -> bytes:
    """16/32/48 三帧 ICO（PIL 写 BMP 帧，不含时间戳 → 字节级可复现）。"""
    buffer = io.BytesIO()
    master.save(buffer, format="ICO", sizes=[(s, s) for s in FAVICON_SIZES])
    return buffer.getvalue()


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="从 assets/brand/logo.svg 派生图标产物")
    parser.add_argument(
        "--check",
        action="store_true",
        help="只校验已入库产物与母版一致（不一致即非零退出）",
    )
    args = parser.parse_args(argv)

    master = load_master()
    outputs = {GUI_ICON: gui_icon_bytes(master), WEB_FAVICON: favicon_bytes(master)}

    if args.check:
        drifted: list[str] = []
        for path, expected in outputs.items():
            rel = path.relative_to(ROOT)
            if not path.is_file():
                drifted.append(f"{rel}：缺失")
            elif path.read_bytes() != expected:
                drifted.append(f"{rel}：与母版派生结果不一致")
        if drifted:
            print("品牌图标产物与母版漂移：")
            for line in drifted:
                print(f"  - {line}")
            print("修复：python .github/scripts/make-brand-assets.py")
            return 1
        print(f"品牌图标产物一致（{len(outputs)} 个）")
        return 0

    for path, data in outputs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        print(f"写出 {path.relative_to(ROOT)}（{len(data)} 字节）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
