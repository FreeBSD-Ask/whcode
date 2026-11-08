#!/usr/bin/env python3
"""ADR 已知限制门禁：新 ADR 必须含“已知限制／失效条件”承载节（FR-763／ADR-060 项 J）。

性质：结构检查，不做语义判定——只回答“决策文档有没有把自身的失效条件写下来”，
不回答“写下的失效条件是否正确、是否覆盖真实风险”。通过本门禁不得被当作
“该 ADR 的风险已被评估”的证据。

判定口径（可复现、无例外）：
  1. 存量豁免：`ADR-<n>.md` 且 `n <= 62`（2026-11-08 快照）视为存量，白名单豁免，
     理由见 `LEGACY_MAX_ADR`；`ADR-DB-BUILD.md` 为非编号专用文档，同属存量。
  2. 其余 ADR 必须命中下列任一承载形态，且节内有正文（不能只有标题）：
     - 标题（`##`—`######`）含“已知限制”或“失效条件”；
     - 列表项粗体引导（如 `- **将来失效条件与复审触发**：…`）。
  3. 豁免不是永久权利：存量 ADR 被实质性修订时，应在该次修订中补节并从白名单移除
     （同 ADR-046“存量不回填、以可复现事实为准”的口径）。

局限（必须承认）：
  - 只检测“有没有”，不检测“够不够”；空话式失效条件（如“情况变化时复审”）同样通过。
  - 编号规则依赖 ADR 编号单调递增；若未来出现回填低编号，需改白名单并把判定依据写进
    本脚本的复审触发，不得口头豁免。

复审触发：本脚本失败即先判定“新 ADR 漏节”还是“编号/白名单口径需修订”；
`LEGACY_MAX_ADR` 与实际 ADR 编号范围不再吻合（例如存量全部补节完成）时复审并缩小豁免。

用法：
    python .github/scripts/check-adr-limits.py [--verbose]
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ADR_DIR = ROOT / "docs" / "adr"

# 2026-11-08 快照：ADR-001—ADR-062 为存量（FR-763 明确“存量白名单豁免”）。
LEGACY_MAX_ADR = 62
# 非编号专用文档：构建说明，不含决策承诺，不参与本门禁。
LEGACY_NAMES = frozenset({"ADR-DB-BUILD.md"})

NUMBERED = re.compile(r"^ADR-(\d+)-.+\.md$")
HEADING = re.compile(r"^#{2,6}\s*.*(已知限制|失效条件)")
BOLD_LEAD = re.compile(r"^\s*[-*]\s*\*\*.*(已知限制|失效条件).*\*\*")
SECTION_BODY_WINDOW = 20


def has_section(lines: list[str]) -> bool:
    """命中承载形态且其后 20 行内存在非标题正文。"""
    for index, line in enumerate(lines):
        if not (HEADING.match(line) or BOLD_LEAD.match(line)):
            continue
        if HEADING.match(line) and line.lstrip().startswith("#"):
            # 标题形态：向后找第一条正文；遇到同级或更高级标题即视为空节。
            level = len(line) - len(line.lstrip("#"))
            for follower in lines[index + 1 : index + 1 + SECTION_BODY_WINDOW]:
                stripped = follower.strip()
                if not stripped:
                    continue
                if stripped.startswith("#"):
                    follower_level = len(stripped) - len(stripped.lstrip("#"))
                    if follower_level <= level:
                        break
                    continue
                return True
            continue
        # 粗体引导形态：同一列表项内已有正文（冒号后非空）即算。
        if len(line.split("：", 1)) > 1 and line.split("：", 1)[1].strip():
            return True
    return False


def is_legacy(name: str) -> bool:
    if name in LEGACY_NAMES:
        return True
    match = NUMBERED.match(name)
    if match is None:
        # 非 ADR-<n>-<slug>.md 的其他 markdown 不属决策文档，跳过。
        return True
    return int(match.group(1)) <= LEGACY_MAX_ADR


def main(argv: list[str]) -> int:
    verbose = "--verbose" in argv
    if not ADR_DIR.is_dir():
        print(f"FAIL: 缺少 ADR 目录 {ADR_DIR}")
        return 1

    failures: list[str] = []
    inspected = 0
    exempted = 0
    for path in sorted(ADR_DIR.glob("*.md")):
        name = path.name
        if name == "README.md" or is_legacy(name):
            exempted += 1
            if verbose:
                print(f"SKIP {name}（存量白名单）")
            continue
        inspected += 1
        text = path.read_text(encoding="utf-8")
        if not has_section(text.splitlines()):
            failures.append(
                f"FAIL: {name} 缺少“已知限制／失效条件”承载节"
                "（FR-763；标题或粗体引导 + 正文，二者其一）"
            )
        elif verbose:
            print(f"OK   {name}")

    if failures:
        for line in failures:
            print(line)
        print("提示：补节后重跑；确属存量遗漏时改 LEGACY_MAX_ADR 需在本脚本复审触发中说明。")
        return 1

    print(f"adr-limits: OK（新 ADR {inspected} 份合规，存量豁免 {exempted} 份）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
