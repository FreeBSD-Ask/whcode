#!/usr/bin/env python3
"""标准基线门禁（ADR-072／FR-779）：`docs/guides/standards-baseline.md` 结构检查。

默认模式（离线，接入 platform-verification.yml）：
1. 基线文件存在；
2. 「基线登记」表存在，表头含六列，且每条数据行六字段齐全；
3. 「本仓判定」列取值合法（已覆盖／部分覆盖／缺口／有意偏离／不适用／停用引用锚点）；
4. 「已停用与负向事实登记」节在场且至少 3 条登记。

--strict 模式（默认不接入 CI；接入时点见 ADR-072 复审触发）：
另查已停用对象在 `docs/guides/*.md`、`docs/matrices/*.md` 与 `AGENTS.md` 中的引用，
同一行没有停用／状态注记关键字即失败。

只做行级结构检查（不依赖第三方包，与 check-workflows.py 同约束）；不验证版本事实本身，
版本真伪以官方页复核为准（每年至少一次，ADR-072）。
"""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BASELINE = ROOT / "docs" / "guides" / "standards-baseline.md"

REQUIRED_COLUMNS = ("编号／名称", "现行版本", "状态", "本仓判定", "本地映射锚点", "复审触发")
LEGAL_VERDICTS = (
    "已覆盖",
    "部分覆盖",
    "缺口",
    "有意偏离",
    "不适用",
    "停用引用锚点",
)
RETIRED_SECTION = "## 已停用与负向事实登记"
MIN_RETIRED_ROWS = 3

# --strict 用：已停用对象与可接受的注记关键字（同一行）。
STRICT_TOKENS = (
    "IEEE 1016",
    "IEEE 730",
    "IEEE 828",
    "12207:2017",
    "MISRA Rust",
    "25000.51-2019",
)
STRICT_ANNOTATIONS = (
    "停用",
    "Inactive-Reserved",
    "已撤销",
    "已并入",
    "不存在",
    "误记",
)


def table_rows(lines: list[str], section: str) -> list[str]:
    """返回指定二级标题下以 | 开头、非表头非分隔线的数据行。"""
    rows: list[str] = []
    inside = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("## "):
            inside = stripped.startswith(section)
            continue
        if not inside or not stripped.startswith("|"):
            continue
        cells = [cell.strip() for cell in stripped.strip("|").split("|")]
        if all(set(cell) <= set("-: ") for cell in cells):
            continue
        if any(required in cell for cell in cells for required in REQUIRED_COLUMNS):
            continue
        rows.append(stripped)
    return rows


def cell_values(row: str) -> list[str]:
    return [cell.strip() for cell in row.strip("|").split("|")]


def check_default(problems: list[str]) -> None:
    if not BASELINE.exists():
        problems.append(f"{BASELINE.relative_to(ROOT)} 不存在")
        return
    text = BASELINE.read_text(encoding="utf-8")
    lines = text.splitlines()
    if "## 基线登记" not in text:
        problems.append("缺少「## 基线登记」节")
    if RETIRED_SECTION not in text:
        problems.append(f"缺少「{RETIRED_SECTION}」节")
    rows = table_rows(lines, "## 基线登记")
    if not rows:
        problems.append("「基线登记」表没有数据行")
    for row in rows:
        cells = cell_values(row)
        if len(cells) < 6 or any(cell == "" for cell in cells[:6]):
            problems.append(f"登记行六字段不齐全：{row[:60]}……")
            continue
        verdict = cells[3]
        if not verdict.startswith(LEGAL_VERDICTS):
            problems.append(f"判定取值不合法（{verdict[:20]}……）：须以 {LEGAL_VERDICTS} 之一开头")
    retired = table_rows(lines, RETIRED_SECTION)
    if len(retired) < MIN_RETIRED_ROWS:
        problems.append(f"「已停用与负向事实登记」少于 {MIN_RETIRED_ROWS} 条（当前 {len(retired)}）")


def check_strict(problems: list[str]) -> None:
    targets = sorted((ROOT / "docs" / "guides").glob("*.md"))
    targets += sorted((ROOT / "docs" / "matrices").glob("*.md"))
    agents = ROOT / "AGENTS.md"
    if agents.exists():
        targets.append(agents)
    for path in targets:
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for token in STRICT_TOKENS:
                if token not in line:
                    continue
                if not any(annotation in line for annotation in STRICT_ANNOTATIONS):
                    problems.append(
                        f"{path.relative_to(ROOT)}:{number}: 已停用对象“{token}”裸引用，"
                        "同一行须带停用／状态注记（见 standards-baseline.md 规则节）"
                    )


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    problems: list[str] = []
    check_default(problems)
    if "--strict" in sys.argv[1:]:
        check_strict(problems)
    if problems:
        for problem in problems:
            print(f"FAIL: {problem}")
        return 1
    mode = "默认" if "--strict" not in sys.argv[1:] else "默认＋strict"
    print(f"标准基线门禁通过（{mode}）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
