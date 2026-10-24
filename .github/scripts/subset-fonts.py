#!/usr/bin/env python3
"""生成 GUI 运行时字体子集（FR-716）：完整字体留在 assets/fonts/source，子集进 crate。

字符集 = ASCII/Latin + 通用标点/符号 + GB2312 全量字符 + 仓库内出现的所有非 ASCII 字符
（i18n、源码字符串、文档），保证界面与常见内容不缺字；子集外的生僻字由系统字体回退。

依赖：fonttools（`python -m pip install fonttools`）；生成物入库，日常构建不需要本脚本。

用法：
    python .github/scripts/subset-fonts.py [--out crates/whcode-gui/assets/fonts]
"""

from __future__ import annotations

import argparse
import pathlib
import sys

from fontTools import subset

ROOT = pathlib.Path(__file__).resolve().parents[2]

# 完整字体 → 子集文件名（GUI 运行时只加载这些）
FONTS = {
    "source/inter/Inter-Regular.ttf": "Inter-Regular.subset.ttf",
    "source/noto-sans-sc/NotoSansCJKsc-Regular.otf": "NotoSansCJKsc-Regular.subset.otf",
    "source/jetbrains-mono/JetBrainsMono-Regular.ttf": "JetBrainsMono-Regular.subset.ttf",
    "source/sarasa-mono-sc/SarasaMonoSC-Regular.ttf": "SarasaMonoSC-Regular.subset.ttf",
}

RANGES = [
    (0x0020, 0x007E),  # ASCII
    (0x00A0, 0x00FF),  # Latin-1
    (0x2000, 0x206F),  # 通用标点
    (0x20A0, 0x20CF),  # 货币符号
    (0x2100, 0x214F),  # 字母式符号
    (0x2190, 0x21FF),  # 箭头
    (0x2200, 0x22FF),  # 数学运算符
    (0x2500, 0x257F),  # 制表符
    (0x25A0, 0x25FF),  # 几何图形
    (0x2600, 0x27BF),  # 杂项/装饰符号
    (0x3000, 0x303F),  # CJK 标点
    (0xFF00, 0xFFEF),  # 全角字符
]


def gb2312_chars() -> set[str]:
    """GB2312 全量（符号区 + 6763 汉字）。"""
    chars: set[str] = set()
    for high in range(0xA1, 0xF8):
        for low in range(0xA1, 0xFF):
            try:
                chars.add(bytes([high, low]).decode("gb2312"))
            except UnicodeDecodeError:
                continue
    return chars


def repo_chars() -> set[str]:
    """仓库文本里实际出现的非 ASCII 字符（界面词条、源码字符串、文档）。"""
    chars: set[str] = set()
    patterns = ("crates/**/*.rs", "crates/**/*.properties", "docs/**/*.md", "specs/**/*.md")
    for pattern in patterns:
        for path in ROOT.glob(pattern):
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            chars.update(ch for ch in text if ord(ch) > 0x7F)
    return chars


def codepoints() -> set[int]:
    points = set()
    for start, end in RANGES:
        points.update(range(start, end + 1))
    points.update(ord(ch) for ch in gb2312_chars())
    points.update(ord(ch) for ch in repo_chars())
    return points


def build(source: pathlib.Path, target: pathlib.Path, points: set[int]) -> None:
    options = subset.Options()
    options.layout_features = ["*"]
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.notdef_outline = True
    options.recalc_bounds = True
    font = subset.load_font(str(source), options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=points)
    subsetter.subset(font)
    target.parent.mkdir(parents=True, exist_ok=True)
    font.save(str(target))


def main() -> int:
    parser = argparse.ArgumentParser(description="生成 GUI 字体子集（FR-716）")
    parser.add_argument("--out", default=str(ROOT / "crates" / "whcode-gui" / "assets" / "fonts"))
    args = parser.parse_args()

    points = codepoints()
    print(f"字符集：{len(points)} 个码位（含 GB2312 与仓库文本）", file=sys.stderr)
    out_dir = pathlib.Path(args.out)
    missing = []
    total = 0
    for source_rel, target_name in FONTS.items():
        source = ROOT / "assets" / "fonts" / source_rel
        if not source.is_file():
            missing.append(str(source))
            continue
        target = out_dir / target_name
        build(source, target, points)
        size = target.stat().st_size
        total += size
        print(f"{target_name}: {size / 1024 / 1024:.2f} MiB", file=sys.stderr)
    if missing:
        for path in missing:
            print(f"缺少完整字体：{path}", file=sys.stderr)
        return 2
    print(f"子集合计：{total / 1024 / 1024:.2f} MiB", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
