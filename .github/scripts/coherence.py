#!/usr/bin/env python3
"""融洽性求值器：对可判定片段逐条计算 M ⊨ γ（ADR-085）。

性质：静态检查。回答的是“程序在当前工作区快照上能否判定该条款被满足”，不回答
“条款在语义上是否被正确执行”。不可判定项登记为 registered，恒返回 unknown。

判据（任一满足即按阶段语义处理）：
  - direct:<谓词>   —— 本脚本内实现的结构谓词；
  - delegate:<脚本> —— 复用既有门禁的退出码（0 满足，非 0 反例）；
  - registered:     —— 不可判定项，返回 unknown（不计反例，不算通过）。

阶段语义：
  pre   开工前置：任一 severity=hard 的反例即退出 1；元公理（COH-M1-*）失败同样退出 1。
  mid   批次提交前：默认只记录；--strict 时任一反例退出 1；--independent 启用反向路径。
  end   收口：默认只记录；--strict 时任一反例退出 1；元公理失败始终退出 1。

用法：
    python .github/scripts/coherence.py --phase pre
    python .github/scripts/coherence.py --phase mid --independent
    python .github/scripts/coherence.py --phase end --strict
    python .github/scripts/coherence.py --ledger-check
    python .github/scripts/coherence.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
AXIOMS_PATH = ROOT / "specs/001-whcode-harness/coherence/axioms.toml"
SYMBOLS_PATH = ROOT / "specs/001-whcode-harness/coherence/symbols.toml"
LEDGER_PATH = ROOT / "specs/001-whcode-harness/coherence/counterexamples.md"
REPORT_DIR = ROOT / "artifacts/coherence"
SPEC_PATH = ROOT / "specs/001-whcode-harness/spec.md"
TASKS_PATH = ROOT / "specs/001-whcode-harness/tasks.md"
CHECKLIST_PATH = ROOT / "specs/001-whcode-harness/checklist.md"
PROCESS_PATH = ROOT / "docs/guides/software-engineering-process.md"
AGENTS_PATH = ROOT / "AGENTS.md"
ADR_DIR = ROOT / "docs/adr"

PHASES = ("pre", "mid", "end")
REQUIRED_FIELDS = ("id", "axiom", "statement", "source", "tier", "phase", "severity", "evaluator")
VALID_TIERS = ("I", "III", "IV")
VALID_SEVERITY = ("hard", "soft")
META_PREFIX = "COH-M1-"
DELEGATE_TIMEOUT_SECONDS = 900

FR_HEAD = re.compile(r"^- \*\*(FR-\d{3})\*\*", re.M)
FR_RANGE = re.compile(r"\bFR-(\d{3})\s*[—–~-]\s*(?:FR-)?(\d{3})\b")
FR_COMPOSITE = re.compile(r"\bFR-(\d{3})((?:/\d{3})+)?\b")
TASK_HEAD = re.compile(r"^- \[([ x])\] (T\d{3})", re.M)
CHECK_HEAD = re.compile(r"^- \[([ x])\] Check (\d+):(?: \[([a-z-]+)\])?", re.M)
ADR_FILE = re.compile(r"^ADR-(\d+)(?:-([A-Za-z0-9._-]+))?\.md$")
MEANS_TOKEN = re.compile(
    r"^(`?)((?:\.github/scripts|crates|docs|specs|tests)/[A-Za-z0-9._/-]+)\1$"
)
INLINE_CODE = re.compile(r"`([^`]+)`")
VAGUE = ("基本完成", "整体良好", "基本通过")
EVIDENCE_MARKERS = (
    "取证",
    "复核",
    "更正",
    "补正",
    "实测",
    "三证",
    "通过",
    "证据",
    "审查",
    "断言",
    "已取得",
)


@dataclass
class Result:
    id: str
    axiom: str
    tier: str
    severity: str
    status: str  # satisfied | counterexample | unknown | skipped
    detail: str


@dataclass
class Context:
    """一次求值的只读快照。所有解析结果缓存，保证同一快照内自洽。"""

    phase: str
    _cache: dict[str, str] = field(default_factory=dict)
    _delegates: dict[tuple[str, tuple[str, ...]], tuple[bool, str]] = field(default_factory=dict)
    counterexamples: list[Result] = field(default_factory=list)

    def text(self, path: Path) -> str:
        key = str(path)
        if key not in self._cache:
            self._cache[key] = path.read_text(encoding="utf-8")
        return self._cache[key]

    @property
    def spec(self) -> str:
        return self.text(SPEC_PATH)

    @property
    def tasks(self) -> str:
        return self.text(TASKS_PATH)

    @property
    def checklist(self) -> str:
        return self.text(CHECKLIST_PATH)

    def delegate(self, script: str, args: tuple[str, ...]) -> tuple[bool, str]:
        key = (script, args)
        if key in self._delegates:
            return self._delegates[key]
        if not re.fullmatch(r"[A-Za-z0-9_-]+\.py", script):
            self._delegates[key] = (False, f"门禁脚本名非法：{script}")
            return self._delegates[key]
        path = ROOT / ".github/scripts" / script
        if not path.is_file():
            self._delegates[key] = (False, f"门禁脚本不存在：.github/scripts/{script}")
            return self._delegates[key]
        try:
            completed = subprocess.run(
                [sys.executable, str(path), *args],
                cwd=ROOT,
                shell=False,
                capture_output=True,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=DELEGATE_TIMEOUT_SECONDS,
                check=False,
            )
        except subprocess.TimeoutExpired:
            self._delegates[key] = (False, f"{script} 超时（>{DELEGATE_TIMEOUT_SECONDS}s）")
            return self._delegates[key]
        tail = (completed.stdout + completed.stderr).strip().splitlines()[-3:]
        detail = f"{script} 退出码 {completed.returncode}"
        if tail:
            detail += "；" + " / ".join(line.strip() for line in tail if line.strip())
        self._delegates[key] = (completed.returncode == 0, detail)
        return self._delegates[key]


# ---------------- 通用解析 ----------------


def fr_hits(text: str) -> dict[str, int]:
    """FR 引用计数：展开区间与 `/` 并列写法。"""
    hits: dict[str, int] = {}
    for line in text.splitlines():
        consumed: list[tuple[int, int]] = []
        for match in FR_RANGE.finditer(line):
            start, end = int(match.group(1)), int(match.group(2))
            if 0 < end - start <= 100:
                for value in range(start, end + 1):
                    key = f"{value:03d}"
                    hits[key] = hits.get(key, 0) + 1
                consumed.append(match.span())
        for match in FR_COMPOSITE.finditer(line):
            if any(begin <= match.start() < end for begin, end in consumed):
                continue
            key = match.group(1)
            hits[key] = hits.get(key, 0) + 1
            for extra in (match.group(2) or "").split("/"):
                if extra:
                    hits[extra] = hits.get(extra, 0) + 1
    return hits


def task_blocks(tasks: str) -> list[tuple[str, bool, str]]:
    """任务块：编号、是否勾选、块正文。"""
    heads = list(TASK_HEAD.finditer(tasks))
    blocks: list[tuple[str, bool, str]] = []
    for index, match in enumerate(heads):
        end = heads[index + 1].start() if index + 1 < len(heads) else len(tasks)
        # 下一个二级标题也算块边界。
        section = tasks.find("\n## ", match.end(), end)
        if section != -1:
            end = section
        blocks.append((match.group(2), match.group(1) == "x", tasks[match.start() : end]))
    return blocks


def check_blocks(checklist: str) -> list[tuple[int, bool, str, str | None]]:
    """Check 块：编号、是否勾选、块正文、证据态。"""
    heads = list(CHECK_HEAD.finditer(checklist))
    blocks: list[tuple[int, bool, str, str | None]] = []
    for index, match in enumerate(heads):
        end = heads[index + 1].start() if index + 1 < len(heads) else len(checklist)
        section = checklist.find("\n## ", match.end(), end)
        trailer = checklist.find("\n---", match.end(), end)
        if section != -1:
            end = min(end, section)
        if trailer != -1:
            end = min(end, trailer)
        blocks.append((int(match.group(2)), match.group(1) == "x", checklist[match.start() : end], match.group(3)))
    return blocks


def means_tokens(block: str) -> list[str]:
    tokens: list[str] = []
    for raw in INLINE_CODE.findall(block):
        value = raw.strip()
        if not MEANS_TOKEN.match(f"`{value}`"):
            continue
        if any(char in value for char in "*{}<>…"):
            continue
        tokens.append(value.rstrip("/"))
    return tokens


def adr_files() -> dict[int, Path]:
    mapping: dict[int, Path] = {}
    if not ADR_DIR.is_dir():
        return mapping
    for path in sorted(ADR_DIR.glob("ADR-*.md")):
        match = ADR_FILE.match(path.name)
        if match:
            mapping[int(match.group(1))] = path
    return mapping


# ---------------- 直接谓词 ----------------


def pred_meta_axiom_table(ctx: Context, axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    seen: set[str] = set()
    for entry in axioms:
        missing = [name for name in REQUIRED_FIELDS if not entry.get(name)]
        if missing:
            failures.append(f"{entry.get('id', '<无 id>')} 缺字段 {'/'.join(missing)}")
            continue
        ident = str(entry["id"])
        if ident in seen:
            failures.append(f"{ident} 编号重复")
        seen.add(ident)
        tier = entry["tier"]
        evaluator = entry["evaluator"]
        if tier not in VALID_TIERS:
            failures.append(f"{ident} tier 非法：{tier}")
        if entry["severity"] not in VALID_SEVERITY:
            failures.append(f"{ident} severity 非法：{entry['severity']}")
        phases = entry["phase"]
        if not phases or any(item not in PHASES for item in phases):
            failures.append(f"{ident} phase 非法：{phases}")
        if tier == "IV" and not evaluator.startswith("registered:"):
            failures.append(f"{ident} tier IV 必须是 registered 求值")
        if tier == "I" and evaluator.startswith("registered:"):
            failures.append(f"{ident} tier I 不得只登记为 registered")
        if not evaluator.startswith(("direct:", "delegate:", "registered:")):
            failures.append(f"{ident} evaluator 型别未知：{evaluator}")
    return (not failures, "；".join(failures) if failures else f"{len(axioms)} 条条目字段合规")


def pred_meta_anchors(ctx: Context, axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for entry in axioms:
        source = str(entry.get("source", ""))
        if " :: " not in source:
            failures.append(f"{entry.get('id')} source 缺锚点分隔符")
            continue
        name, needle = source.split(" :: ", 1)
        path = ROOT / name.strip()
        if not path.is_file():
            failures.append(f"{entry.get('id')} 源文件不存在：{name.strip()}")
            continue
        if needle.strip() not in ctx.text(path):
            failures.append(f"{entry.get('id')} 锚点未命中：{needle.strip()}")
    return (not failures, "；".join(failures[:5]) if failures else f"{len(axioms)} 条锚点可定位")


def parse_ledger(text: str) -> list[dict[str, str]]:
    entries: list[dict[str, str]] = []
    current: dict[str, str] | None = None
    for line in text.splitlines():
        match = re.match(r"^- id[：:]\s*(.+)$", line.strip())
        if match:
            if current is not None:
                entries.append(current)
            current = {"id": match.group(1).strip()}
            continue
        if current is None:
            continue
        field_match = re.match(r"^\s+(axiom|evidence|owner|exit|status)[：:]\s*(.+)$", line)
        if field_match:
            current[field_match.group(1)] = field_match.group(2).strip()
    if current is not None:
        entries.append(current)
    return entries


def pred_ledger_fields(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    entries = parse_ledger(ctx.text(LEDGER_PATH))
    failures: list[str] = []
    for entry in entries:
        missing = [name for name in ("axiom", "evidence", "owner", "exit", "status") if not entry.get(name)]
        if missing:
            failures.append(f"{entry.get('id')} 缺字段 {'/'.join(missing)}")
            continue
        if entry["status"].startswith("closed") and "修复证据" not in entry["status"]:
            failures.append(f"{entry.get('id')} closed 条目缺修复证据")
    return (not failures, "；".join(failures) if failures else f"{len(entries)} 条台账条目字段合规")


def ledger_axiom_matches(entry_axiom: str, group: str) -> bool:
    """台账的 axiom 字段允许写全编号（COH-A1-05）或组号（A1）。"""
    return entry_axiom == group or entry_axiom.startswith(f"COH-{group}-")


def pred_ledger_coverage(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    open_axioms = {
        entry.get("axiom", "")
        for entry in parse_ledger(ctx.text(LEDGER_PATH))
        if entry.get("status", "").startswith("open")
    }
    uncovered = sorted(
        {
            item.axiom
            for item in ctx.counterexamples
            if not any(ledger_axiom_matches(entry, item.axiom) for entry in open_axioms)
        }
    )
    return (
        not uncovered,
        "未登记反例所在公理：" + "、".join(uncovered) if uncovered else "当前反例均有台账条目",
    )


def pred_phase_reports(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    if ctx.phase != "end":
        return True, "非 end 阶段不适用"
    reports = list(REPORT_DIR.glob("run-mid-*.json")) if REPORT_DIR.is_dir() else []
    if reports:
        return True, f"mid 报告 {len(reports)} 份"
    return False, "缺少同批次 mid 报告"


def pred_check_sources(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for number, _checked, block, _state in check_blocks(ctx.checklist):
        if not any(token in block for token in ("来源：", "FR-", "ADR-")):
            failures.append(f"Check {number} 无来源引用")
    return (not failures, "；".join(failures[:5]) if failures else "全部 Check 含来源引用")


def pred_check_evidence_recorded(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for number, checked, block, _state in check_blocks(ctx.checklist):
        if checked and not any(token in block for token in EVIDENCE_MARKERS):
            failures.append(f"Check {number} 已勾选但无取证／复核记录")
    return (not failures, "；".join(failures[:5]) if failures else "已勾选 Check 均有取证记录")


def pred_check_decidable_wording(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for number, _checked, block, _state in check_blocks(ctx.checklist):
        for word in VAGUE:
            if word in block:
                failures.append(f"Check {number} 含不可判定表述“{word}”")
    return (not failures, "；".join(failures[:5]) if failures else "Check 判据无不可判定表述")


def pred_fr_closure(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    fr_ids = sorted({value.removeprefix("FR-") for value in set(FR_HEAD.findall(ctx.spec))})
    task_refs = fr_hits(ctx.tasks)
    check_refs = fr_hits(ctx.checklist)
    failures = [
        f"FR-{ident} 缺任务" for ident in fr_ids if ident not in task_refs
    ] + [f"FR-{ident} 缺 Check" for ident in fr_ids if ident not in check_refs]
    return (not failures, "；".join(failures[:5]) if failures else f"{len(fr_ids)} 条 FR 均有任务与 Check")


def pred_fr_adr_exists(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    mapping = adr_files()
    failures: list[str] = []
    for match in re.finditer(r"(?ms)^- \*\*(FR-\d{3})\*\*:?.*?(?=^- \*\*FR-\d{3}\*\*|\Z)", ctx.spec):
        body = match.group(0)
        for adr in set(re.findall(r"ADR-(\d{3})", body)):
            if int(adr) not in mapping:
                failures.append(f"{match.group(1)} 引用的 ADR-{adr} 无文件")
    return (not failures, "；".join(failures[:5]) if failures else "FR 引用的 ADR 均在档")


def pred_card_present(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures = [
        ident for ident, checked, block in task_blocks(ctx.tasks) if not checked and "任务认知卡" not in block
    ]
    return (
        not failures,
        "未勾选任务缺认知卡：" + "、".join(failures) if failures else "未勾选任务均有认知卡",
    )


def pred_platform_cfg_single(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for crate in sorted((ROOT / "crates").glob("whcode-*")):
        if crate.name == "whcode-platform":
            continue
        for path in (crate / "src").rglob("*.rs"):
            text = path.read_text(encoding="utf-8")
            if "target_os" in text or "CARGO_CFG_" in text:
                failures.append(str(path.relative_to(ROOT)))
    return (
        not failures,
        "平台分支出现在：" + "、".join(failures[:5]) if failures else "平台分支只在 whcode-platform",
    )


def pred_frontend_no_store(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """前端不得自行打开磁盘数据库；允许透传装配根注入的 Connection 类型与测试内存库。"""
    failures: list[str] = []
    opener = re.compile(r"Connection::open\((?!\s*\))")
    for name in ("whcode-tui", "whcode-cui", "whcode-web", "whcode-ssh"):
        for path in (ROOT / "crates" / name / "src").rglob("*.rs"):
            text = path.read_text(encoding="utf-8")
            if opener.search(text):
                failures.append(str(path.relative_to(ROOT)))
    return (
        not failures,
        "前端自行打开数据库：" + "、".join(failures[:5]) if failures else "前端不自行打开数据库",
    )


def pred_unsafe_absent(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for path in (ROOT / "crates").glob("whcode-*/src/**/*.rs"):
        text = path.read_text(encoding="utf-8")
        for needle in ("unsafe {", "unsafe fn ", "unsafe impl ", "mem::forget"):
            if needle in text:
                failures.append(f"{path.relative_to(ROOT)} 含 {needle.strip()}")
    return (not failures, "；".join(failures[:5]) if failures else "自有 crate 无 unsafe／mem::forget")


def pred_means_exist(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for ident, _checked, block in task_blocks(ctx.tasks):
        if int(ident[1:]) < 360:
            continue
        for token in means_tokens(block):
            if not (ROOT / token).exists():
                failures.append(f"{ident} 引用的 {token} 不存在")
    return (not failures, "；".join(failures[:5]) if failures else "T360 起任务引用的手段均存在")


def pred_adr_review_trigger(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    failures: list[str] = []
    for number, path in sorted(adr_files().items()):
        if number <= 62:
            continue
        text = path.read_text(encoding="utf-8")
        if "复审" not in text and "失效条件" not in text:
            failures.append(path.name)
    return (not failures, "；".join(failures[:5]) if failures else "63 起 ADR 均含复审触发或失效条件")


def pred_adr_numbering(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    mapping = adr_files()
    if not mapping:
        return False, "docs/adr 无编号 ADR"
    numbers = sorted(mapping)
    missing = [value for value in range(1, numbers[-1] + 1) if value not in mapping]
    if missing:
        return False, f"ADR 编号不连续：缺 {missing[:5]}"
    return True, f"ADR 1—{numbers[-1]} 编号连续"


def load_symbols() -> dict:
    """加载规则符号注册表；文件缺失返回空表，由谓词判为反例（不伪造通过）。"""
    if not SYMBOLS_PATH.is_file():
        return {}
    with SYMBOLS_PATH.open("rb") as handle:
        return tomllib.load(handle)


def symbol_violations(symbols: dict, docs: dict[str, str]) -> list[str]:
    """符号命名空间 purity 检查（纯函数，供负向自检）。

    规则：
      1. 每个 rule 的 canonical_source 锚点须在目标文件中逐字命中；
      2. 每个 guard 文档中不得出现 forbidden 命名空间的编号；
      3. 每个 guard 文档的 require_all 编号须全部在场。
    """
    failures: list[str] = []
    rules = symbols.get("rule", [])
    ids = {str(item.get("id", "")) for item in rules}
    if not rules:
        return ["符号注册表无 rule 条目"]

    for item in rules:
        ident = str(item.get("id", "<无 id>"))
        source = str(item.get("canonical_source", ""))
        namespace = str(item.get("namespace", ""))
        if namespace not in ("inference", "coherence"):
            failures.append(f"{ident} namespace 非法：{namespace}")
        if " :: " not in source:
            failures.append(f"{ident} canonical_source 缺锚点分隔符")
            continue
        name, needle = source.split(" :: ", 1)
        text = docs.get(name.strip())
        if text is None:
            failures.append(f"{ident} 规范源文档未载入：{name.strip()}")
        elif needle.strip() not in text:
            failures.append(f"{ident} 规范锚点未命中：{needle.strip()}")

    for guard in symbols.get("guard", []):
        document = str(guard.get("document", ""))
        text = docs.get(document)
        if text is None:
            failures.append(f"guard 文档未载入：{document}")
            continue
        pattern = str(guard.get("pattern", ""))
        forbidden = str(guard.get("forbidden", ""))
        try:
            hits = set(re.findall(pattern, text))
        except re.error as exc:
            failures.append(f"guard 正则非法（{document}）：{exc}")
            hits = set()
        if hits:
            failures.append(f"{document} 含越界命名空间编号（{forbidden}）：" + "、".join(sorted(hits)))
        for required in guard.get("require_all", []):
            ident = str(required)
            if ident not in ids:
                failures.append(f"{document} require_all 引用未注册符号：{ident}")
            elif not re.search(rf"(?<![A-Za-z0-9]){re.escape(ident)}(?![0-9])", text):
                failures.append(f"{document} 缺规范编号：{ident}")
    return failures


def pred_symbol_unambiguous(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """ADR-101：R/CR 编号在各自书写域内唯一，锚点可定位。"""
    symbols = load_symbols()
    if not symbols:
        return False, "symbols.toml 缺失或为空"
    docs: dict[str, str] = {}
    seen_paths: set[str] = set()
    for item in symbols.get("rule", []):
        source = str(item.get("canonical_source", ""))
        if " :: " in source:
            seen_paths.add(source.split(" :: ", 1)[0].strip())
    for guard in symbols.get("guard", []):
        seen_paths.add(str(guard.get("document", "")))
    for name in seen_paths:
        path = ROOT / name
        docs[name] = ctx.text(path) if path.is_file() else ""
    failures = symbol_violations(symbols, docs)
    if failures:
        return False, "；".join(failures[:5])
    return True, f"{len(symbols.get('rule', []))} 个符号锚点与命名空间边界一致"


def orphan_adr_ids(adr_numbers: dict[int, Path], ledger_text: str) -> list[int]:
    """已跟踪 ADR 若在三份账本（spec/tasks/checklist）零引用即为孤立（纯函数）。"""
    return sorted(
        number
        for number, path in adr_numbers.items()
        if f"ADR-{number:03d}" not in ledger_text
        and path.stem not in ledger_text
    )


def implies_fr_violations(checks: list[tuple[int, bool, str, str | None]], spec_fr: set[str]) -> list[str]:
    """Check 的 implies_fr 声明须指向在档 FR 且本 Check 确实引用它（纯函数）。"""
    failures: list[str] = []
    for number, _checked, body, _state in checks:
        # 剔除声明本身，检查判据正文是否另有该 FR 引用（否则声明自证，不构成蕴含）。
        prose = re.sub(r"implies_fr=FR-\d{3}", "", body)
        for declared in re.findall(r"implies_fr=(FR-\d{3})", body):
            ident = declared.removeprefix("FR-")
            if ident not in spec_fr:
                failures.append(f"Check {number} 的 {declared} 在 spec 不存在")
            elif declared not in prose:
                failures.append(f"Check {number} 声明 {declared} 但判据正文未另有引用")
    return failures


def pred_implies_fr(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """判据蕴含字段一致性（ADR-101 支柱五）。"""
    spec_fr = {value.removeprefix("FR-") for value in set(FR_HEAD.findall(ctx.spec))}
    failures = implies_fr_violations(check_blocks(ctx.checklist), spec_fr)
    if failures:
        return False, "；".join(failures[:5])
    declared = sum(
        len(re.findall(r"implies_fr=FR-\d{3}", body))
        for _n, _c, body, _s in check_blocks(ctx.checklist)
    )
    return True, f"{declared} 条 implies_fr 声明均指向在档 FR 且正文自洽"


def pred_orphan_adr(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """原子提交门禁（ADR-101 支柱三）：已入库 ADR 必须被账本引用。

    未跟踪文件不在此列——它们不是仓库规范的一部分，由 COH-CE-009 一类台账条目单独登记。
    """
    completed = subprocess.run(
        ["git", "ls-files", "docs/adr/ADR-*.md"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    tracked = {Path(line).name for line in completed.stdout.splitlines() if line.strip()}
    mapping = {
        number: path
        for number, path in adr_files().items()
        if path.name in tracked
    }
    # 合法账本：三份主账本 ＋ constitution ＋ 决策矩阵 ＋ 其他 ADR 正文。
    # 只认三份主账本会把被 constitution 与源码规则引用的 ADR-008 误判为孤立。
    ledger_parts = [ctx.spec, ctx.tasks, ctx.checklist]
    for name in (".specify/memory/constitution.md",):
        path = ROOT / name
        if path.is_file():
            ledger_parts.append(ctx.text(path))
    for path in (ROOT / "docs/matrices").glob("*.md"):
        ledger_parts.append(ctx.text(path))
    for number, path in adr_files().items():
        if number not in mapping:
            ledger_parts.append(ctx.text(path))
    orphans = orphan_adr_ids(mapping, "\n".join(ledger_parts))
    if orphans:
        return False, "孤立 ADR：" + "、".join(f"ADR-{n:03d}" for n in orphans[:10])
    return True, f"{len(mapping)} 份已跟踪 ADR 均被账本引用"


def phantom_fr_in_source(source_text: dict[str, str], spec_fr: set[str]) -> list[str]:
    """源码注释引用的 FR 若不在 spec 集合即为幻影（纯函数）。"""
    found: set[str] = set()
    # 负向后顾排除 NFR-xxx（否则 "NFR-215" 会被误截为 "FR-215"）。
    pattern = re.compile(r"(?<!N)FR-(\d{3})")
    for name, text in source_text.items():
        for matched in pattern.findall(text):
            if matched not in spec_fr:
                found.add(f"{name}:FR-{matched}")
    return sorted(found)


def pred_phantom_fr(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """源码 FR 引用须在 spec 在档（ADR-101 支柱三）。

    仅看 git 已跟踪源码；未跟踪工作树内容由台账登记，不使 pre 硬阻断在建工作。
    """
    completed = subprocess.run(
        ["git", "ls-files", "crates/**/*.rs"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    tracked = [line for line in completed.stdout.splitlines() if line.strip()]
    source_text: dict[str, str] = {}
    for name in tracked:
        path = ROOT / name
        if path.is_file():
            source_text[name] = ctx.text(path)
    spec_fr = {value.removeprefix("FR-") for value in set(FR_HEAD.findall(ctx.spec))}
    phantoms = phantom_fr_in_source(source_text, spec_fr)
    if phantoms:
        return False, "幻影 FR 引用：" + "、".join(phantoms[:10])
    return True, f"已跟踪源码引用的 FR 均在 spec 在档（{len(spec_fr)} 条）"


def pred_authority_layers(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """ADR-086：四处声明必须同时在场且指向同一层级。"""
    checks = [
        ("docs/adr/ADR-086-rule-authority-hierarchy.md", ("权威层级", "公理最高")),
        ("AGENTS.md", ("权威层级", "公理最高", "入口与目录", "软件工程理论")),
        ("docs/guides/axiom-system.md", ("权威层级", "公理最高")),
        ("docs/guides/coherence-audit.md", ("权威层级", "ADR-086")),
    ]
    failures: list[str] = []
    for name, needles in checks:
        path = ROOT / name
        if not path.is_file():
            failures.append(f"缺文件：{name}")
            continue
        text = ctx.text(path)
        for needle in needles:
            if needle not in text:
                failures.append(f"{name} 缺表述：{needle}")
    return (not failures, "；".join(failures[:5]) if failures else "四处权威层级声明一致")


def eol_violations(items: list[tuple[str, bytes]]) -> tuple[list[str], list[str]]:
    """返回（CRLF 文件、BOM 文件）；含 NUL 的二进制跳过。纯函数，供负向自检。"""
    crlf: list[str] = []
    bom: list[str] = []
    for name, data in items:
        if b"\x00" in data:
            continue
        if data.startswith(b"\xef\xbb\xbf"):
            bom.append(name)
        if b"\r\n" in data:
            crlf.append(name)
    return crlf, bom


def parse_eol_entry(record: str) -> tuple[str, str, str]:
    """解析 `git ls-files --eol -z` 的一条记录 →（index 行尾、worktree 行尾、路径）。"""
    meta, _, name = record.partition("\t")
    index_eol = "unknown"
    worktree_eol = "unknown"
    for field in meta.split():
        if field.startswith("i/"):
            index_eol = field[2:]
        elif field.startswith("w/"):
            worktree_eol = field[2:]
    return index_eol, worktree_eol, name


def pred_eol_tracked(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    """只查 git 跟踪文件的**提交内容**行尾与 BOM。

    判定对象是索引（提交内容），不是工作树：CI 的平台 job 会就地重写
    `artifacts/ssh-acceptance/*.txt`（CRLF），工作树因此与提交树不一致；按工作树判会把
    运行期改写当成仓库卫生违规，且该文件在本机不存在时又会漏判（2026-11-17 两轮实测）。
    行尾取 `git ls-files --eol` 的 `i/` 字段；BOM 读工作树字节，文件不存在时读索引 blob。
    二进制（`i/binary` 或含 NUL）跳过。
    """
    completed = subprocess.run(
        ["git", "ls-files", "--eol", "-z"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    records = [record for record in completed.stdout.split("\0") if record]
    crlf: list[str] = []
    bom: list[str] = []
    checked = 0
    for record in records:
        index_eol, _worktree_eol, name = parse_eol_entry(record)
        if index_eol in ("binary", "unknown"):
            continue
        checked += 1
        if index_eol in ("crlf", "mixed"):
            crlf.append(name)
            continue
        path = ROOT / name
        try:
            data = path.read_bytes() if path.is_file() else _index_blob(name)
        except OSError:
            continue
        if data is None or b"\x00" in data:
            continue
        if data.startswith(b"\xef\xbb\xbf"):
            bom.append(name)
    if crlf or bom:
        detail = f"跟踪文件 CRLF {len(crlf)}、BOM {len(bom)}"
        if crlf:
            detail += "；CRLF 前 3：" + "、".join(crlf[:3])
        if bom:
            detail += "；BOM 前 3：" + "、".join(bom[:3])
        return False, detail
    return True, f"{checked} 个跟踪文本文件的提交内容无 CRLF 与 BOM"


def _index_blob(name: str) -> bytes | None:
    """读取索引中的文件内容；失败返回 `None`（不把读不到当作通过）。"""
    completed = subprocess.run(
        ["git", "show", f":{name}"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return completed.stdout


INDEX_FIELD = re.compile(r"index=[^\s，；、]+")
RUN_INDEX = re.compile(r"run\s+`?\d{6,}")
SCOPE_FIELD = re.compile(r"scope=")
BEHAVIOR_TOKENS = ("实测", "p95", "p99", "RSS", "线程", "句柄", "CPU", "吞吐", "冒烟", "端到端")
BUDGET_TOKENS = ("≤", "预算", "p95", "p99", "RSS", "线程", "句柄", "CPU", "DPI", "缩放", "吞吐")
NEGATIVE_CHECK_TOKENS = ("未启用", "不启用")
NEGATIVE_FR_TOKENS = ("禁止", "不得", "无任意", "默认不", "不提供", "不创建", "不引入", "只有", "显式降级")


def pred_evidence_indexed(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    missing: list[str] = []
    for number, checked, block, _state in check_blocks(ctx.checklist):
        if not checked or not any(token in block for token in BEHAVIOR_TOKENS):
            continue
        if INDEX_FIELD.search(block) or RUN_INDEX.search(block):
            continue
        missing.append(str(number))
    if missing:
        return False, f"{len(missing)} 条行为／性能类 Check 缺 index（前 30）：" + "、".join(missing[:30])
    return True, "行为／性能类已勾 Check 均有 index 或 run 引用"


def pred_evidence_scoped(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    missing: list[str] = []
    for number, checked, block, _state in check_blocks(ctx.checklist):
        if not checked or not any(token in block for token in BUDGET_TOKENS):
            continue
        if SCOPE_FIELD.search(block):
            continue
        missing.append(str(number))
    if missing:
        return False, f"{len(missing)} 条预算／平台类 Check 缺 scope（前 30）：" + "、".join(missing[:30])
    return True, "预算／平台类已勾 Check 均有 scope"


def fr_blocks(spec: str):
    heads = list(FR_HEAD.finditer(spec))
    for index, match in enumerate(heads):
        end = heads[index + 1].start() if index + 1 < len(heads) else len(spec)
        yield match.group(1), spec[match.start() : end]


def positive_fr_negative_checks(ctx: Context) -> list[tuple[str, list[int]]]:
    """正面 FR 若只由负极性 Check 覆盖，返回（FR 编号，Check 编号列表）。"""
    blocks = list(check_blocks(ctx.checklist))
    results: list[tuple[str, list[int]]] = []
    for fr_id, fr_text in fr_blocks(ctx.spec):
        # 显式极性字段优先（ADR-101 支柱五）：条件性延后能力由负极性 Check 合法覆盖。
        if "polarity=conditional" in fr_text or "polarity=negative" in fr_text:
            continue
        if any(token in fr_text for token in NEGATIVE_FR_TOKENS):
            continue
        covers = [(number, body) for number, _checked, body, _state in blocks if fr_id in body]
        if not covers:
            continue
        negative = [
            number for number, body in covers if any(token in body for token in NEGATIVE_CHECK_TOKENS)
        ]
        if len(negative) == len(covers):
            results.append((fr_id, negative))
    return results


def pred_positive_coverage(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    bad = positive_fr_negative_checks(ctx)
    if bad:
        detail = "；".join(
            f"{fr} 仅由 Check {','.join(map(str, checks))}（负极性）覆盖" for fr, checks in bad[:10]
        )
        return False, detail
    return True, "正面 FR 均有非负极性验收覆盖"


def pred_polarity_declared(ctx: Context, _axioms: list[dict]) -> tuple[bool, str]:
    blocks = {number: body for number, _checked, body, _state in check_blocks(ctx.checklist)}
    missing: list[str] = []
    for fr_id, checks in positive_fr_negative_checks(ctx):
        for number in checks:
            body = blocks.get(number, "")
            if "polarity=negative" not in body and "负极性" not in body:
                missing.append(f"{fr_id}/Check {number}")
    if missing:
        return False, f"{len(missing)} 条负极性覆盖未声明：" + "、".join(missing[:10])
    return True, "负极性覆盖均已声明"


DIRECT_PREDICATES = {
    "meta_axiom_table": pred_meta_axiom_table,
    "meta_anchors": pred_meta_anchors,
    "ledger_fields": pred_ledger_fields,
    "ledger_coverage": pred_ledger_coverage,
    "phase_reports": pred_phase_reports,
    "check_sources": pred_check_sources,
    "check_evidence_recorded": pred_check_evidence_recorded,
    "check_decidable_wording": pred_check_decidable_wording,
    "fr_closure": pred_fr_closure,
    "fr_adr_exists": pred_fr_adr_exists,
    "card_present": pred_card_present,
    "platform_cfg_single": pred_platform_cfg_single,
    "frontend_no_store": pred_frontend_no_store,
    "unsafe_absent": pred_unsafe_absent,
    "means_exist": pred_means_exist,
    "adr_review_trigger": pred_adr_review_trigger,
    "adr_numbering": pred_adr_numbering,
    "authority_layers": pred_authority_layers,
    "symbol_unambiguous": pred_symbol_unambiguous,
    "orphan_adr": pred_orphan_adr,
    "phantom_fr": pred_phantom_fr,
    "implies_fr": pred_implies_fr,
    "eol_tracked": pred_eol_tracked,
    "evidence_indexed": pred_evidence_indexed,
    "evidence_scoped": pred_evidence_scoped,
    "positive_coverage": pred_positive_coverage,
    "polarity_declared": pred_polarity_declared,
}


# ---------------- 求值主流程 ----------------


def evaluate_entry(ctx: Context, entry: dict, axioms: list[dict]) -> Result:
    ident = str(entry["id"])
    axiom = str(entry["axiom"])
    tier = str(entry["tier"])
    severity = str(entry["severity"])
    evaluator = str(entry["evaluator"])
    if ctx.phase not in entry["phase"]:
        return Result(ident, axiom, tier, severity, "skipped", f"不参与 {ctx.phase} 阶段")
    if evaluator.startswith("direct:"):
        name = evaluator.split(":", 1)[1]
        predicate = DIRECT_PREDICATES.get(name)
        if predicate is None:
            return Result(ident, axiom, tier, severity, "counterexample", f"未知谓词：{name}")
        ok, detail = predicate(ctx, axioms)
    elif evaluator.startswith("delegate:"):
        script = evaluator.split(":", 1)[1]
        args = tuple(str(item) for item in entry.get("args", []))
        ok, detail = ctx.delegate(script, args)
    else:
        reason = evaluator.split(":", 1)[1]
        return Result(ident, axiom, tier, severity, "unknown", reason)
    return Result(ident, axiom, tier, severity, "satisfied" if ok else "counterexample", detail)


def reverse_counts(ctx: Context) -> dict:
    """反向路径一：从清单头与过程表的声明反推数量，与直接解析的产物计数比对。"""
    fr_actual = len(set(FR_HEAD.findall(ctx.spec)))
    checks_actual = len(CHECK_HEAD.findall(ctx.checklist))
    tasks_actual = len({ident for ident, _checked, _block in task_blocks(ctx.tasks)})
    declarations: dict[str, list[int]] = {"fr": [], "tasks": [], "checks": []}
    for value in re.findall(r"spec 全部 (\d+) 条 FR", ctx.checklist):
        declarations["fr"].append(int(value))
    for value in re.findall(r"全部 (\d+) 条任务", ctx.checklist):
        declarations["tasks"].append(int(value))
    for value in re.findall(r"本清单 Check 1—(\d+)", ctx.checklist):
        declarations["checks"].append(int(value))
    for task_claim, check_claim in re.findall(r"(\d+) 条任务、(\d+) 项 Check", ctx.text(PROCESS_PATH)):
        declarations["tasks"].append(int(task_claim))
        declarations["checks"].append(int(check_claim))
    disagreements: list[str] = []
    actual = {"fr": fr_actual, "tasks": tasks_actual, "checks": checks_actual}
    for key, values in declarations.items():
        if values and values[-1] != actual[key]:
            disagreements.append(f"{key} 声明 {values[-1]}，实际 {actual[key]}")
    return {
        "kind": "counts",
        "actual": actual,
        "declared": {key: (values[-1] if values else None) for key, values in declarations.items()},
        "disagreements": disagreements,
    }


def reverse_tracked_means(ctx: Context) -> dict:
    """反向路径二：手段路径以 git ls-files（已跟踪）为存在性来源，而非工作区 Path.exists。"""
    completed = subprocess.run(
        ["git", "ls-files"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    tracked = set(completed.stdout.splitlines())
    disagreements: list[str] = []
    for ident, _checked, block in task_blocks(ctx.tasks):
        if int(ident[1:]) < 360:
            continue
        for token in means_tokens(block):
            if (ROOT / token).exists() and token not in tracked:
                disagreements.append(f"{ident} 引用的 {token} 未被版本控制")
    return {"kind": "tracked_means", "tracked": len(tracked), "disagreements": disagreements}


def independent_report(ctx: Context) -> dict:
    paths = [reverse_counts(ctx), reverse_tracked_means(ctx)]
    disagreements = [item for path in paths for item in path["disagreements"]]
    return {"paths": paths, "disagreements": disagreements}


def git_state() -> tuple[str, bool]:
    head = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=ROOT,
        shell=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    return head.stdout.strip() or "unknown", bool(status.stdout.strip())


def load_axioms() -> list[dict]:
    with AXIOMS_PATH.open("rb") as handle:
        data = tomllib.load(handle)
    return list(data.get("axiom", []))


def run(phase: str, independent: bool, strict: bool) -> int:
    axioms = load_axioms()
    ctx = Context(phase=phase)

    results: list[Result] = []
    for entry in axioms:
        if str(entry.get("id")) == "COH-M1-04":
            continue  # 依赖本次运行的反例集合，最后求值
        results.append(evaluate_entry(ctx, entry, axioms))
    ctx.counterexamples = [item for item in results if item.status == "counterexample"]
    coverage_entry = next((entry for entry in axioms if entry.get("id") == "COH-M1-04"), None)
    if coverage_entry is not None:
        coverage_result = evaluate_entry(ctx, coverage_entry, axioms)
        results.append(coverage_result)
        # 覆盖判定的反例也属本次运行的反例集合：否则它只进计数、不进打印与未登记清单。
        if coverage_result.status == "counterexample":
            ctx.counterexamples.append(coverage_result)

    independent_data = independent_report(ctx) if independent else None
    if independent_data is not None:
        for item in independent_data["disagreements"]:
            results.append(
                Result("COH-IND-01", "M", "I", "soft", "counterexample", item)
            )
            ctx.counterexamples.append(results[-1])

    ledger_entries = parse_ledger(ctx.text(LEDGER_PATH))
    ledger_summary = {
        "open": sorted(
            entry.get("id", "") for entry in ledger_entries if entry.get("status", "").startswith("open")
        ),
        "closed": sorted(
            entry.get("id", "") for entry in ledger_entries if entry.get("status", "").startswith("closed")
        ),
    }
    open_axioms = {
        entry.get("axiom", "")
        for entry in ledger_entries
        if entry.get("status", "").startswith("open")
    }
    uncovered = [
        item
        for item in ctx.counterexamples
        if not any(ledger_axiom_matches(entry, item.axiom) for entry in open_axioms)
    ]
    summary = {
        "total": len(results),
        "satisfied": sum(1 for item in results if item.status == "satisfied"),
        "counterexample": sum(1 for item in results if item.status == "counterexample"),
        "registered": len(ctx.counterexamples) - len(uncovered),
        "unregistered": len(uncovered),
        "unknown": sum(1 for item in results if item.status == "unknown"),
        "skipped": sum(1 for item in results if item.status == "skipped"),
    }
    head, dirty = git_state()
    report = {
        "schema": "coherence-run/v1",
        "phase": phase,
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "head": head,
        "worktree_dirty": dirty,
        "summary": summary,
        "results": [item.__dict__ for item in sorted(results, key=lambda value: value.id)],
        "counterexamples": [
            {"id": item.id, "axiom": item.axiom, "message": item.detail}
            for item in sorted(ctx.counterexamples, key=lambda value: value.id)
        ],
        "ledger": ledger_summary,
        "independent": independent_data,
    }

    REPORT_DIR.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    report_path = REPORT_DIR / f"run-{phase}-{stamp}.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    print(
        f"coherence {phase}: total={summary['total']} satisfied={summary['satisfied']} "
        f"counterexample={summary['counterexample']} registered={summary['registered']} "
        f"unregistered={summary['unregistered']} unknown={summary['unknown']} "
        f"skipped={summary['skipped']} worktree_dirty={dirty} head={head}"
    )
    for item in sorted(ctx.counterexamples, key=lambda value: value.id):
        print(f"CE {item.id} [{item.axiom}/{item.tier}/{item.severity}] {item.detail}")
    if independent_data is not None:
        print(
            "independent: "
            f"paths={len(independent_data['paths'])} "
            f"disagreements={len(independent_data['disagreements'])}"
        )
        for item in independent_data["disagreements"]:
            print(f"IND {item}")
    print(f"report: {report_path.relative_to(ROOT)}")

    meta_failures = [
        item
        for item in ctx.counterexamples
        if item.id.startswith(META_PREFIX) and item.id != "COH-M1-05"
    ]
    uncovered_hard = [item for item in uncovered if item.severity == "hard"]
    if meta_failures:
        return 1
    if phase == "pre" and uncovered_hard:
        return 1
    if strict and uncovered:
        return 1
    return 0


def ledger_check() -> int:
    ctx = Context(phase="pre")
    axioms = load_axioms()
    ok, detail = pred_ledger_fields(ctx, axioms)
    print(("OK " if ok else "FAIL ") + detail)
    return 0 if ok else 1


# ---------------- 自检（不读写工作区产物） ----------------


def selftest() -> int:
    failures: list[str] = []

    def expect(condition: bool, label: str) -> None:
        if not condition:
            failures.append(label)

    hits = fr_hits("覆盖 FR-101—103 与 FR-204/205")
    expect(set(hits) == {"101", "102", "103", "204", "205"}, "FR 区间与并列展开")
    ledger = parse_ledger(
        "- id：X-1\n  axiom：A1\n  evidence：e\n  owner：o\n  exit：x\n  status：open\n"
    )
    expect(ledger == [{"id": "X-1", "axiom": "A1", "evidence": "e", "owner": "o", "exit": "x", "status": "open"}], "台账解析")
    expect(means_tokens("见 `crates/whcode-app/src/lib.rs` 与 `local/x.py`") == ["crates/whcode-app/src/lib.rs"], "手段路径过滤")
    expect(means_tokens("`docs/adr/ADR-*.md` `crates/<crate>/src`") == [], "通配与占位过滤")
    expect(
        eol_violations([("ok.md", b"a\n"), ("bad.md", b"a\r\n")]) == (["bad.md"], []),
        "CRLF 负向探针",
    )
    expect(
        eol_violations([("bom.md", b"\xef\xbb\xbfa\n")]) == ([], ["bom.md"]),
        "BOM 负向探针",
    )
    expect(
        eol_violations([("bin.dat", b"a\x00\r\n")]) == ([], []),
        "二进制跳过",
    )
    expect(
        parse_eol_entry("i/crlf    w/crlf    attr/text=auto eol=lf\tx.md")
        == ("crlf", "crlf", "x.md"),
        "行尾记录解析（CRLF）",
    )
    expect(
        parse_eol_entry("i/lf    w/missing   -text\tdata.bin") == ("lf", "missing", "data.bin"),
        "行尾记录解析（工作树缺失）",
    )

    # 符号命名空间负向自检（ADR-101）：注入裸 R4 与缺锚点须被捕获。
    probe_symbols = {
        "rule": [
            {
                "id": "R4",
                "namespace": "inference",
                "canonical_source": "a.md :: | R4 | 禁跳与例外",
            },
            {
                "id": "CR4",
                "namespace": "coherence",
                "canonical_source": "b.md :: **CR4（降级）**",
            },
        ],
        "guard": [
            {
                "document": "b.md",
                "forbidden": "inference",
                "pattern": r"\*\*R[1-8]（|`(?:FR-|)R[1-8]`",
                "require_all": ["CR4"],
            }
        ],
    }
    expect(
        symbol_violations(probe_symbols, {"a.md": "| R4 | 禁跳与例外", "b.md": "**CR4（降级）**"}) == [],
        "符号命名空间正向探针",
    )
    dirty_docs = {"a.md": "| R4 | 禁跳与例外", "b.md": "**CR4（降级）** 与 **R4（禁跳）** 不同"}
    expect(
        any("越界命名空间" in item for item in symbol_violations(probe_symbols, dirty_docs)),
        "裸 R4 越界负向探针",
    )
    broken_anchor = dict(probe_symbols)
    broken_anchor["rule"] = [
        {**probe_symbols["rule"][0], "canonical_source": "a.md :: 不存在的锚点"},
        probe_symbols["rule"][1],
    ]
    expect(
        any("规范锚点未命中" in item for item in symbol_violations(broken_anchor, {"a.md": "| R4 | 禁跳与例外", "b.md": "**CR4（降级）**"})),
        "符号锚点缺失负向探针",
    )

    # 判据蕴含字段负向自检（ADR-101 支柱五）。
    probe_checks = [(1, True, "覆盖 FR-101（implies_fr=FR-101）", None)]
    expect(implies_fr_violations(probe_checks, {"101"}) == [], "implies_fr 正向探针")
    expect(
        len(implies_fr_violations(probe_checks, set())) == 1,
        "implies_fr 幻影 FR 负向探针",
    )
    expect(
        len(implies_fr_violations([(2, True, "implies_fr=FR-999（无正文引用）", None)], {"999"})) == 1,
        "implies_fr 正文不自洽负向探针",
    )

    # 原子提交门禁负向自检（ADR-101 支柱三）。
    adr_map = {1: Path("docs/adr/ADR-001-x.md"), 2: Path("docs/adr/ADR-002-y.md")}
    expect(orphan_adr_ids(adr_map, "见 ADR-001") == [2], "孤立 ADR 负向探针")
    expect(orphan_adr_ids(adr_map, "ADR-001 与 ADR-002") == [], "孤立 ADR 正向探针")
    expect(
        phantom_fr_in_source({"a.rs": "// FR-999"}, {"101"}) == ["a.rs:FR-999"],
        "幻影 FR 负向探针",
    )
    expect(
        phantom_fr_in_source({"a.rs": "// FR-101"}, {"101"}) == [],
        "幻影 FR 正向探针",
    )
    expect(
        phantom_fr_in_source({"a.rs": "// NFR-215"}, {"101"}) == [],
        "NFR 前缀不误报探针",
    )

    if failures:
        for item in failures:
            print(f"FAIL {item}")
        return 1
    print("selftest: OK")
    return 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="融洽性求值器（ADR-085）")
    parser.add_argument("--phase", choices=PHASES)
    parser.add_argument("--independent", action="store_true", help="启用反向求值路径并对账")
    parser.add_argument("--strict", action="store_true", help="mid/end 阶段任一反例即退出 1")
    parser.add_argument("--ledger-check", action="store_true", help="只校验反例台账字段")
    parser.add_argument("--selftest", action="store_true", help="运行解析器自检")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    if args.ledger_check:
        return ledger_check()
    if not args.phase:
        parser.error("需要 --phase pre|mid|end，或 --ledger-check／--selftest")
    return run(args.phase, args.independent, args.strict)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
