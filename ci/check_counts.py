#!/usr/bin/env python3
"""计数一致性门禁：FR/任务/Check 的实际数量必须与文档声明一致。"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
SPEC = ROOT / "specs" / "001-whcode-harness" / "spec.md"
TASKS = ROOT / "specs" / "001-whcode-harness" / "tasks.md"
CHECKLIST = ROOT / "specs" / "001-whcode-harness" / "checklist.md"
PROCESS = ROOT / "docs" / "guides" / "software-engineering-process.md"


def main() -> int:
    spec = SPEC.read_text(encoding="utf-8")
    tasks = TASKS.read_text(encoding="utf-8")
    checklist = CHECKLIST.read_text(encoding="utf-8")
    process = PROCESS.read_text(encoding="utf-8")

    fr = sorted(set(re.findall(r"^- \*\*(FR-\d{3})\*\*", spec, re.M)))
    task_ids = re.findall(r"^- \[[ x]\] (T\d+)", tasks, re.M)
    checks = re.findall(r"^- \[[ x]\] Check (\d+):", checklist, re.M)

    actual = {
        "fr": len(fr),
        "tasks": len(set(task_ids)),
        "checks": len(checks),
    }
    numbers = [int(t[1:]) for t in task_ids]
    expected_range = list(range(1, actual["tasks"] + 1))
    if sorted(set(numbers)) != expected_range:
        print(f"任务编号不连续：{sorted(set(numbers))[:5]}...", file=sys.stderr)
        return 1

    failures: list[str] = []

    def expect(pattern: str, source: str, key: str, label: str) -> None:
        matches = re.findall(pattern, source)
        if not matches:
            failures.append(f"未找到声明：{label}")
            return
        for value in matches:
            if int(value) != actual[key]:
                failures.append(f"{label} 声明 {value}，实际 {actual[key]}")

    expect(r"全部 (\d+) 条任务", checklist, "tasks", "checklist 任务数")
    expect(r"本清单 Check 1—(\d+)", checklist, "checks", "checklist Check 总数")
    expect(r"spec 全部 (\d+) 条 FR", checklist, "fr", "checklist FR 数")

    for task_claim, check_claim in re.findall(r"(\d+) 条任务、(\d+) 项 Check", process):
        if int(task_claim) != actual["tasks"]:
            failures.append(f"process 任务数声明 {task_claim}，实际 {actual['tasks']}")
        if int(check_claim) != actual["checks"]:
            failures.append(f"process Check 数声明 {check_claim}，实际 {actual['checks']}")

    expect(r"\| 验收检查点 \| Check 1—(\d+) \|", process, "checks", "process Check 总数")
    expect(r"Check 1—(\d+) 中仍有", process, "checks", "process Check 口径")
    expect(r"(\d+) 条 FR 定义", process, "fr", "process FR 数")

    print(
        f"actual: FR={actual['fr']} tasks={actual['tasks']} checks={actual['checks']}"
    )
    if failures:
        for item in failures:
            print(f"FAIL: {item}", file=sys.stderr)
        return 1
    print("counts consistent")
    return 0


if __name__ == "__main__":
    sys.exit(main())
