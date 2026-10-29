#!/usr/bin/env python3
"""知难行易落点门禁：宪法原则 X 的文档落点与任务块标注完整性（结构检查）。

性质：本脚本只验证宪法原则 X 的**落点与链接完整**、任务块标注存在：
  1. 各承载文档仍存在指向 `docs/guides/knowing-before-doing.md` 的落点，以及与
     原则 X 相关的必需小节标题与版本行字面量；
  2. `specs/001-whcode-harness/tasks.md` 中每个未勾选任务块至少出现一次“任务认知卡”
     （标注置于任务块末，避免拆断跨行任务描述）。

局限（必须承认，不得掩盖）：
  - 它**不验证“知”是否正确**：无法判断任务认知卡的方向、目标、用户意图、判据与
    非目标是否真实、是否与用户意图一致；文字存在不等于认知正确。
  - 本门禁通过只表示“结构完整”，**不得以其通过宣称理解正确**。
  - 已勾选任务块不检查（历史任务不追溯补写认知卡）。

复审触发：本脚本失败即复审 `docs/guides/knowing-before-doing.md` 与
`docs/adr/ADR-049-constitution-knowing-before-doing.md`；先判定是文档漂移还是脚本
该改，不得改脚本迁就漂移（落点被删、标题改名、任务模板不再要求认知卡均属复审触发）。

用法：
    python .github/scripts/check-knowing-first.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# 每项为（相对仓库根路径，必需子串列表）；子串为字面匹配，不做语义解读。
TOUCHPOINTS: list[tuple[str, list[str]]] = [
    (
        ".specify/memory/constitution.md",
        [
            "### X. 知难行易",
            "docs/guides/knowing-before-doing.md",
            "**Version**: 1.2.0",
            "**Last Amended**: 2026-10-29",
            "问题：",
            "备选方案：",
            "影响范围：",
            "迁移计划：",
        ],
    ),
    (
        "docs/adr/ADR-049-constitution-knowing-before-doing.md",
        ["知难行易", "已批准", "复审触发"],
    ),
    (
        "docs/guides/knowing-before-doing.md",
        [
            "## 1. 来源与证据",
            "## 2. 命题",
            "## 3. 任务认知卡",
            "## 4. 门禁落点",
            "## 5. 反例",
            "## 6. 正负和解",
            "## 7. 复审触发",
            "## 8. 局限",
        ],
    ),
    (
        "AGENTS.md",
        [
            "知难行易（任务认知先于手段）",
            "docs/guides/knowing-before-doing.md",
            "check-knowing-first.py",
            "任务认知卡",
        ],
    ),
    ("docs/guides/philosophy.md", ["## 11. 知难行易", "knowing-before-doing.md"]),
    (
        "docs/guides/yike-philosophy.md",
        ["知难行易", "file_id 22, block 3—4", "knowing-before-doing.md"],
    ),
    ("docs/guides/software-engineering-process.md", ["知难行易（原则 X）", "任务认知卡"]),
    ("docs/guides/requirements-engineering.md", ["## 2.1 任务认知卡先于 FR", "任务认知卡先于 FR"]),
    ("docs/guides/pr-checklist.md", ["任务认知先于手段"]),
    ("docs/guides/README.md", ["knowing-before-doing.md"]),
    ("engineering-doctrine/00-philosophy.md", ["## 11. 知难行易", "任务认知卡"]),
    ("engineering-doctrine/01-process.md", ["任务认知卡"]),
    ("engineering-doctrine/templates/AGENTS.template.md", ["知难行易"]),
    ("engineering-doctrine/templates/spec.template.md", ["## 0. 任务认知卡"]),
    ("engineering-doctrine/templates/pr-checklist.template.md", ["任务认知先于手段"]),
    ("engineering-doctrine/templates/tasks.template.md", ["任务认知卡"]),
    (".specify/templates/tasks-template.md", ["Task Cognition Precondition"]),
    (
        "specs/001-whcode-harness/tasks.md",
        ["## 全局执行前置：知难行易（任务认知卡）", "T001—T220", "已勾选任务不补写"],
    ),
]

TASKS_REL = "specs/001-whcode-harness/tasks.md"
TASK_HEAD = re.compile(r"^- \[ \] T(\d+)")


def load(rel: str, texts: dict[str, str | None], failures: list[str]) -> str | None:
    """按相对路径读取文本（缓存）；不存在即登记失败并返回 None。"""
    if rel in texts:
        return texts[rel]
    path = ROOT / rel
    if not path.is_file():
        failures.append(f"FAIL: {rel} 不存在")
        texts[rel] = None
        return None
    texts[rel] = path.read_text(encoding="utf-8", errors="replace")
    return texts[rel]


def check_tasks(text: str, failures: list[str]) -> tuple[int, int]:
    """按行解析未勾选任务块，返回（未勾选任务数，带认知卡标注数）。"""
    lines = text.splitlines()
    total = len(lines)
    unchecked = 0
    annotated = 0
    index = 0
    while index < total:
        match = TASK_HEAD.match(lines[index])
        if match is None:
            index += 1
            continue
        task = match.group(1)
        end = index + 1
        while end < total:
            line = lines[end]
            if line.startswith("- [") or line.startswith("#") or not line.strip():
                break
            end += 1
        block = "\n".join(lines[index:end])
        unchecked += 1
        if "任务认知卡" in block:
            annotated += 1
        else:
            failures.append(f"FAIL: 未勾选任务 T{task} 缺少认知卡标注")
        index = end
    return unchecked, annotated


def main() -> int:
    for stream in (sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8", errors="replace")

    failures: list[str] = []
    texts: dict[str, str | None] = {}

    for rel, required in TOUCHPOINTS:
        text = load(rel, texts, failures)
        if text is None:
            continue
        for sub in required:
            if sub not in text:
                failures.append(f"FAIL: {rel} 缺少 {sub}")

    tasks_text = load(TASKS_REL, texts, failures)
    annotated = 0
    if tasks_text is not None:
        _, annotated = check_tasks(tasks_text, failures)

    if failures:
        for line in failures:
            print(line)
        return 1

    count = len(TOUCHPOINTS)
    print(f"knowing-first: OK（{count} 个落点、{annotated} 个未勾选任务带认知卡标注）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
