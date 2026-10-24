#!/usr/bin/env python3
"""计数一致性门禁：FR/任务/Check 的实际数量必须与文档声明一致。"""

from __future__ import annotations

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = ROOT / "specs" / "001-whcode-harness" / "spec.md"
TASKS = ROOT / "specs" / "001-whcode-harness" / "tasks.md"
CHECKLIST = ROOT / "specs" / "001-whcode-harness" / "checklist.md"
PROCESS = ROOT / "docs" / "guides" / "software-engineering-process.md"


def check_task_check_consistency(checklist: str, tasks_text: str) -> list[str]:
    """任务↔Check 联动校验（2026-10-24 全局复核后加入）。

    只校验强不一致方向：检查点已取证通过（`[x]`），而组织行映射的对应任务仍未
    勾选——取证以任务交付为前提，该状态即登记漏勾（实测两例：T124、T158）。
    反向（任务已勾、检查点待取证）是合法中间态，不校验。
    映射来自 checklist 组织行（第一层实现完整性与第六层事务语义的区间声明）；
    区间不等长或无 "Task" 前缀的条目按截断/跳过处理——只少校验、不误报。
    """
    failures: list[str] = []
    org = re.search(r"\*\*组织\*\*：(.+?)→ 第二层", checklist, re.S)
    if not org:
        return ["未找到组织行，无法做任务↔Check 映射校验"]
    mapping: dict[int, int] = {}
    for m in re.finditer(r"Check (\d+)(?:—(\d+))? 对应 Task T(\d+)(?:—T(\d+))?", org.group(1)):
        c1, c2, t1, t2 = int(m.group(1)), m.group(2), int(m.group(3)), m.group(4)
        c_hi = int(c2) if c2 else c1
        t_hi = int(t2) if t2 else t1
        for c, tk in zip(range(c1, c_hi + 1), range(t1, t_hi + 1)):
            mapping[c] = tk
    checked_checks = {int(n) for n in re.findall(r"^- \[x\] Check (\d+):", checklist, re.M)}
    checked_tasks = {int(n) for n in re.findall(r"^- \[x\] T(\d+)", tasks_text, re.M)}
    for c in sorted(checked_checks):
        task = mapping.get(c)
        if task is not None and task not in checked_tasks:
            failures.append(
                f"Check {c} 已取证通过，但对应任务 T{task:03d} 未勾选（登记漏勾）"
            )
    return failures


def check_check_number_continuity(checklist: str) -> list[str]:
    """Check 编号连续性校验（2026-10-24 全局复核后加入）。

    全部 Check 编号必须连续（1..=max 无缺号）且无重复——新增编号漏登记
    （如 Check 261—269 逐条落盘时跳号）会直接挂门禁，不再依赖人工清点。
    """
    failures: list[str] = []
    numbers = [int(n) for n in re.findall(r"^- \[[ x]\] Check (\d+):", checklist, re.M)]
    if not numbers:
        return ["未找到任何 Check 条目"]
    duplicates = sorted({n for n in numbers if numbers.count(n) > 1})
    if duplicates:
        failures.append(f"Check 编号重复：{duplicates}")
    maximum = max(numbers)
    missing = sorted(set(range(1, maximum + 1)) - set(numbers))
    if missing:
        failures.append(f"Check 编号不连续：缺失 {missing}")
    return failures


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
    failures += check_task_check_consistency(checklist, tasks)
    failures += check_check_number_continuity(checklist)

    def expect(pattern: str, source: str, key: str, label: str) -> None:
        matches = re.findall(pattern, source)
        if not matches:
            failures.append(f"未找到声明：{label}")
            return
        # 复核记录保留各批次的历史口径（如“Check 1—258”），只有时间序最后的
        # 声明是当前口径——历史匹配不参与断言，避免文档史实触发门禁误报。
        if int(matches[-1]) != actual[key]:
            failures.append(f"{label} 声明 {matches[-1]}，实际 {actual[key]}")

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
