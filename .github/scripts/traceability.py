"""FR ↔ 任务 ↔ 验收检查点 三方追溯门禁（Check 127、FR-305/FR-306）。

判据（任一不满足即失败）：
1. spec 中的每个 FR 至少被 tasks.md 引用一次（需求 → 任务）；
2. spec 中的每个 FR 至少被 checklist.md 引用一次（需求 → 验收）；
3. tasks.md 与 checklist.md 引用的 FR 必须在 spec 中存在（防“幻影需求”）；
4. checklist.md 引用的任务编号必须在 tasks.md 中存在（防“幻影任务”）；
5. checklist 编号连续且唯一（1..N）；
6. “追溯缺口登记”节中的条目必须声明 FR、责任人与退出条件；登记失效（恢复完整覆盖）须删除。

支持项目既有的写法：`FR-201—210`（区间）与 `FR-502/503`（共用前缀的并列）。
用法：`python .github/scripts/traceability.py`；`python .github/scripts/traceability.py --selftest` 自检登记解析；
失败时退出码 1 并列出断链。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SPEC = ROOT / "specs/001-whcode-harness/spec.md"
TASKS = ROOT / "specs/001-whcode-harness/tasks.md"
CHECKLIST = ROOT / "specs/001-whcode-harness/checklist.md"

FR_RANGE = re.compile(r"\bFR-(\d{3})\s*[—–~-]\s*(?:FR-)?(\d{3})\b")
FR_COMPOSITE = re.compile(r"\bFR-(\d{3})((?:/\d{3})+)?\b")
TASK_PATTERN = re.compile(r"\bT(\d{3})\b")
CHECK_PATTERN = re.compile(r"^- \[[ x]\] Check (\d+):", re.MULTILINE)
GAP_PATTERN = re.compile(r"(?ms)^- 缺口 (G-\d+)：(.*?)(?=^- 缺口 |\Z)")
# 缺口登记节：其中的 FR 引用**不构成验收覆盖**（登记 ≠ 通过），扫描覆盖时必须剔除。
GAP_SECTION = re.compile(r"(?ms)^## 追溯缺口登记.*\Z")


def split_gap_section(text: str) -> tuple[str, str]:
    """把 checklist 切成“正文”与“缺口登记节”。"""
    match = GAP_SECTION.search(text)
    if match is None:
        return text, ""
    return text[: match.start()], match.group(0)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def fr_hits(text: str) -> dict[str, list[int]]:
    """FR 引用 → 行号列表；展开区间与 `/` 并列写法。"""
    hits: dict[str, list[int]] = {}

    def record(value: str, line_number: int) -> None:
        hits.setdefault(value, []).append(line_number)

    for line_number, line in enumerate(text.splitlines(), 1):
        consumed: list[tuple[int, int]] = []
        for match in FR_RANGE.finditer(line):
            start, end = int(match.group(1)), int(match.group(2))
            if 0 < end - start <= 100:
                for value in range(start, end + 1):
                    record(f"{value:03d}", line_number)
                consumed.append(match.span())
        for match in FR_COMPOSITE.finditer(line):
            if any(begin <= match.start() < end for begin, end in consumed):
                continue
            record(match.group(1), line_number)
            for extra in (match.group(2) or "").split("/"):
                if extra:
                    record(extra, line_number)
    return hits


def task_hits(text: str) -> dict[str, list[int]]:
    hits: dict[str, list[int]] = {}
    for line_number, line in enumerate(text.splitlines(), 1):
        for match in TASK_PATTERN.finditer(line):
            hits.setdefault(match.group(1), []).append(line_number)
    return hits


def parse_gaps(text: str) -> tuple[dict[str, str], list[str]]:
    """解析“追溯缺口登记”节：缺口编号 → 声明的 FR 列表，以及登记本身的缺陷。"""
    gaps: dict[str, str] = {}
    problems: list[str] = []
    for match in GAP_PATTERN.finditer(text):
        gap_id, block = match.group(1), match.group(2)
        declared = block.split("｜")[0]
        frs = re.findall(r"FR-(\d{3})", declared)
        if not frs:
            problems.append(f"缺口 {gap_id} 未在首段声明 FR 编号")
        if not re.search(r"责任人：\s*\S", block):
            problems.append(f"缺口 {gap_id} 缺少责任人")
        if not re.search(r"退出条件：\s*\S", block):
            problems.append(f"缺口 {gap_id} 缺少退出条件")
        for fr in frs:
            gaps[fr] = gap_id
    return gaps, problems


def self_test() -> int:
    """登记解析自检：缺责任人/缺退出条件/无 FR/正常四项（`--selftest`）。"""
    cases = [
        ("完整登记", "- 缺口 G-01：FR-402 ｜ 责任人：X ｜ 退出条件：Y\n", []),
        ("缺责任人", "- 缺口 G-02：FR-403 ｜ 退出条件：Y\n", ["缺口 G-02 缺少责任人"]),
        ("缺退出条件", "- 缺口 G-03：FR-404 ｜ 责任人：X\n", ["缺口 G-03 缺少退出条件"]),
        ("无 FR", "- 缺口 G-04：现状 ｜ 责任人：X ｜ 退出条件：Y\n", ["缺口 G-04 未在首段声明 FR 编号"]),
    ]
    failures = 0
    for name, text, expected in cases:
        _, problems = parse_gaps(text)
        status = "ok" if problems == expected else "FAIL"
        if problems != expected:
            failures += 1
            print(f"{status} {name}：{problems} != {expected}")
        else:
            print(f"{status} {name}")
    print("自检结果：", "全部通过" if failures == 0 else f"{failures} 项失败")
    return 1 if failures else 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return self_test()
    spec_text = read(SPEC)
    tasks_text = read(TASKS)
    checklist_text = read(CHECKLIST)

    spec_frs = set(fr_hits(spec_text))
    task_fr_hits = fr_hits(tasks_text)
    check_body, gap_section = split_gap_section(checklist_text)
    check_fr_hits = fr_hits(check_body)
    task_ids = set(task_hits(tasks_text))
    check_task_hits = task_hits(check_body)
    registered, gap_problems = parse_gaps(gap_section)

    failures: list[str] = list(gap_problems)

    missing_in_tasks = sorted(
        fr for fr in spec_frs if fr not in task_fr_hits and fr not in registered
    )
    if missing_in_tasks:
        failures.append(
            "FR 未被任务覆盖（且未登记缺口）："
            + "、".join(f"FR-{fr}" for fr in missing_in_tasks)
        )

    missing_in_checks = sorted(
        fr for fr in spec_frs if fr not in check_fr_hits and fr not in registered
    )
    if missing_in_checks:
        failures.append(
            "FR 未被验收检查覆盖（且未登记缺口）："
            + "、".join(f"FR-{fr}" for fr in missing_in_checks)
        )

    phantom_gaps = sorted(fr for fr in registered if fr not in spec_frs)
    if phantom_gaps:
        failures.append(
            "缺口登记引用了 spec 中不存在的 FR："
            + "、".join(f"FR-{fr}" for fr in phantom_gaps)
        )

    stale_gaps = sorted(
        fr
        for fr in registered
        if fr not in phantom_gaps and fr in task_fr_hits and fr in check_fr_hits
    )
    if stale_gaps:
        failures.append(
            "缺口登记已失效（任务与验收均已覆盖，须删除登记）："
            + "、".join(f"FR-{fr}" for fr in stale_gaps)
        )

    phantom_frs = sorted(
        fr for fr in set(task_fr_hits) | set(check_fr_hits) if fr not in spec_frs
    )
    if phantom_frs:
        failures.append(
            "引用了 spec 中不存在的 FR：" + "、".join(f"FR-{fr}" for fr in phantom_frs)
        )

    phantom_tasks = sorted(t for t in check_task_hits if t not in task_ids)
    if phantom_tasks:
        failures.append(
            "checklist 引用了 tasks.md 中不存在的任务："
            + "、".join(f"T{t}" for t in phantom_tasks)
        )

    numbers = [int(match.group(1)) for match in CHECK_PATTERN.finditer(check_body)]
    expected = set(range(1, len(numbers) + 1))
    if len(numbers) != len(set(numbers)):
        failures.append("checklist 存在重复编号")
    if set(numbers) != expected:
        missing = sorted(expected - set(numbers))
        extra = sorted(set(numbers) - expected)
        failures.append(f"checklist 编号不连续（缺 {missing[:10]}，越界 {extra[:10]}）")

    # ADR-046 证据态分布（登记语义，不阻断；阻断校验在 check_counts.py）。
    state_pattern = re.compile(r"^- \[[ x]\] Check \d+:(?: \[([a-z-]+)\])?", re.MULTILINE)
    state_dist: dict[str, int] = {}
    for state in state_pattern.findall(check_body):
        key = state or "present"
        state_dist[key] = state_dist.get(key, 0) + 1
    state_summary = " ".join(
        f"{k}={state_dist.get(k, 0)}"
        for k in (
            "present", "wired", "exercised", "outcome-supported",
            "missing", "unobserved", "not-applicable",
        )
    )

    print(
        "追溯统计："
        f"FR={len(spec_frs)}，任务={len(task_ids)}，检查点={len(numbers)}；"
        f"FR→任务缺 {len(missing_in_tasks)}，FR→验收缺 {len(missing_in_checks)}，"
        f"幻影 FR {len(phantom_frs)}，幻影任务 {len(phantom_tasks)}，"
        f"已登记缺口 {len(registered)}"
    )
    print(f"证据态分布：{state_summary}（ADR-046）")
    if registered:
        detail = "、".join(f"FR-{fr}→{gap}" for fr, gap in sorted(registered.items()))
        print(f"已登记缺口（不通过，仅免于门禁失败）：{detail}")
    if failures:
        for failure in failures:
            print(f"断链：{failure}")
        return 1
    print("追溯一致：无断链")
    return 0


if __name__ == "__main__":
    sys.exit(main())
