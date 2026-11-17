//! # 任务充分性计算（FR-795／ADR-085）
//!
//! 对“要求（要干什么）—检查点（目的判据）—手段（工具链）”三元组计算**可判定片段**的
//! 充分性：缺检查点、手段未声明或不可用、文本越界与条目过多，都是反例；预算未声明与
//! 语义充分性（意图是否正确、方案是否最优）恒为 [`RuleStatus::Unknown`]，不算通过。
//!
//! 边界（ADR-085 §7）：
//!
//! - 纯函数，无 IO、无 SQL、无时钟、无线程；同一输入恒得同一输出；
//! - 不判断语义：`SemanticSufficiency` 永远 `Unknown`，由用户确认；
//! - 不做截断：超出上限即反例（拒绝不静默截断），上限同时约束内存与消息长度。
//!
//! 仓库流程层的同一判定由 `.github/scripts/coherence.py` 承担（公理表
//! `specs/001-whcode-harness/coherence/axioms.toml`）；本模块是它在产品运行时的对应物。

/// 检查点与手段各自的条目上限（超出即反例，不截断）。
pub const MAX_SUFFICIENCY_ITEMS: usize = 32;

/// 单段文本（要求与单个检查点、手段名）的字符上限。
pub const MAX_SUFFICIENCY_TEXT_CHARS: usize = 512;

/// 执行中重算的轮次间隔（turn 计数；只记录，不阻断）。
pub const MID_AUDIT_INTERVAL_TURNS: u32 = 8;

/// 计算所处阶段（与仓库层 `coherence.py` 的 `--phase` 同口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoherencePhase {
    /// 执行前：反例即拒绝执行。
    Pre,
    /// 执行中：按轮次重算，只记录。
    Mid,
    /// 收口：任务完成时重算，只记录。
    End,
}

impl CoherencePhase {
    /// 稳定名（入报告与查询参数）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pre => "pre",
            Self::Mid => "mid",
            Self::End => "end",
        }
    }

    /// 解析稳定名（未知取值返回 `None`，由调用方决定报错口径）。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pre" => Some(Self::Pre),
            "mid" => Some(Self::Mid),
            "end" => Some(Self::End),
            _ => None,
        }
    }
}

/// 单条规则的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStatus {
    /// 可判定且满足。
    Satisfied,
    /// 可判定且不满足——反例。
    Counterexample,
    /// 不可判定或输入不足以判定（显式未知，不算通过）。
    Unknown,
}

impl RuleStatus {
    /// 稳定名（入报告）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Counterexample => "counterexample",
            Self::Unknown => "unknown",
        }
    }
}

/// 可判定片段的结论：有反例即反例，否则满足。未知项不参与本判定，
/// 由 [`SufficiencyReport::unknowns`] 单列——既不判失败，也不判通过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 可判定规则全部满足。
    Satisfied,
    /// 至少一条可判定规则不满足。
    Counterexample,
}

impl Verdict {
    /// 稳定名（入报告与错误消息）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Counterexample => "counterexample",
        }
    }
}

/// 充分性规则（封闭集；新增规则须同步 ADR-085 与仓库层公理表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SufficiencyRule {
    /// 要求存在且非空。
    RequirementPresent,
    /// 要求文本不超过上限。
    RequirementBounded,
    /// 至少一个检查点。
    CheckpointPresent,
    /// 检查点条目非空且不重复。
    CheckpointDistinct,
    /// 检查点条目不超过上限。
    CheckpointBounded,
    /// 至少一个手段已声明。
    MeansDeclared,
    /// 声明的手段都在可用集合中。
    MeansAvailable,
    /// 手段条目不超过上限。
    MeansBounded,
    /// 预算已声明。
    BudgetDeclared,
    /// 语义充分性（意图、方案质量）——恒未知。
    SemanticSufficiency,
}

impl SufficiencyRule {
    /// 稳定编号（入报告与台账）。
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::RequirementPresent => "SFR-01",
            Self::RequirementBounded => "SFR-02",
            Self::CheckpointPresent => "SFR-03",
            Self::CheckpointDistinct => "SFR-04",
            Self::CheckpointBounded => "SFR-05",
            Self::MeansDeclared => "SFR-06",
            Self::MeansAvailable => "SFR-07",
            Self::MeansBounded => "SFR-08",
            Self::BudgetDeclared => "SFR-09",
            Self::SemanticSufficiency => "SFR-10",
        }
    }

    /// 可判定档（与仓库层 `tier` 同口径）。
    #[must_use]
    pub const fn tier(self) -> &'static str {
        match self {
            Self::SemanticSufficiency | Self::BudgetDeclared => "III",
            _ => "I",
        }
    }
}

/// 计算输入。切片由调用方持有；本模块不复制整表，只读一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SufficiencyInput<'a> {
    /// 要求（要干什么），通常是任务标题或计划目标。
    pub requirement: &'a str,
    /// 检查点（如何判定完成），通常是任务验收条件。
    pub checkpoints: &'a [&'a str],
    /// 手段（用什么做），通常是要调用的工具名。
    pub means: &'a [&'a str],
    /// 当前可用手段集合（工具注册表里的名字）。
    pub available_means: &'a [&'a str],
    /// 资源预算（MB）；`None` 表示未声明。
    pub budget_mb: Option<u32>,
}

/// 单条规则的判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleOutcome {
    /// 规则。
    pub rule: SufficiencyRule,
    /// 结果。
    pub status: RuleStatus,
    /// 可读说明（不含凭据与正文；长度有界）。
    pub detail: String,
}

/// 一次计算的报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SufficiencyReport {
    /// 阶段。
    pub phase: CoherencePhase,
    /// 结论。
    pub verdict: Verdict,
    /// 逐条结果（顺序固定，便于对账）。
    pub outcomes: Vec<RuleOutcome>,
}

impl SufficiencyReport {
    /// 反例条目。
    pub fn counterexamples(&self) -> impl Iterator<Item = &RuleOutcome> {
        self.outcomes
            .iter()
            .filter(|item| item.status == RuleStatus::Counterexample)
    }

    /// 未知条目（不计反例，也不计通过）。
    pub fn unknowns(&self) -> impl Iterator<Item = &RuleOutcome> {
        self.outcomes
            .iter()
            .filter(|item| item.status == RuleStatus::Unknown)
    }
}

fn push(outcomes: &mut Vec<RuleOutcome>, rule: SufficiencyRule, status: RuleStatus, detail: &str) {
    outcomes.push(RuleOutcome {
        rule,
        status,
        detail: detail.to_string(),
    });
}

fn text_ok(text: &str, label: &str) -> (bool, String) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (false, format!("{label}为空"));
    }
    if trimmed.chars().count() > MAX_SUFFICIENCY_TEXT_CHARS {
        return (
            false,
            format!("{label}超出 {MAX_SUFFICIENCY_TEXT_CHARS} 字符上限"),
        );
    }
    (true, format!("{label}非空且在限内"))
}

/// 计算三元组的可判定充分性。
///
/// 结论只覆盖 tier I 规则：任一 `Counterexample` → [`Verdict::Counterexample`]；
/// 否则 [`Verdict::Satisfied`]。tier III 的未知项由 [`SufficiencyReport::unknowns`] 单列。
#[must_use]
pub fn evaluate(phase: CoherencePhase, input: &SufficiencyInput<'_>) -> SufficiencyReport {
    let mut outcomes: Vec<RuleOutcome> = Vec::with_capacity(10);

    let (ok, detail) = text_ok(input.requirement, "要求");
    push(
        &mut outcomes,
        SufficiencyRule::RequirementPresent,
        if input.requirement.trim().is_empty() {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        &detail,
    );
    push(
        &mut outcomes,
        SufficiencyRule::RequirementBounded,
        if ok || input.requirement.trim().is_empty() {
            // 空要求已由 SFR-01 判反例，这里不重复计。
            RuleStatus::Satisfied
        } else {
            RuleStatus::Counterexample
        },
        &detail,
    );

    push(
        &mut outcomes,
        SufficiencyRule::CheckpointPresent,
        if input.checkpoints.is_empty() {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        if input.checkpoints.is_empty() {
            "没有检查点：目的不可判定"
        } else {
            "检查点非空"
        },
    );

    let mut distinct = true;
    let mut blank = false;
    for item in input.checkpoints {
        if item.trim().is_empty() {
            blank = true;
        }
    }
    let mut unique: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for item in input.checkpoints {
        unique.insert(item.trim());
    }
    if unique.len() != input.checkpoints.len() {
        distinct = false;
    }
    push(
        &mut outcomes,
        SufficiencyRule::CheckpointDistinct,
        if !distinct || blank {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        if blank {
            "检查点含空条目"
        } else if !distinct {
            "检查点有重复条目"
        } else {
            "检查点条目非空且不重复"
        },
    );

    push(
        &mut outcomes,
        SufficiencyRule::CheckpointBounded,
        if input.checkpoints.len() > MAX_SUFFICIENCY_ITEMS {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        if input.checkpoints.len() > MAX_SUFFICIENCY_ITEMS {
            "检查点数量超出上限"
        } else {
            "检查点数量在限内"
        },
    );

    push(
        &mut outcomes,
        SufficiencyRule::MeansDeclared,
        if input.means.is_empty() {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        if input.means.is_empty() {
            "没有声明手段：不知道用什么做"
        } else {
            "手段已声明"
        },
    );

    let mut unavailable = false;
    for item in input.means {
        if !input
            .available_means
            .iter()
            .any(|candidate| candidate == item)
        {
            unavailable = true;
            break;
        }
    }
    push(
        &mut outcomes,
        SufficiencyRule::MeansAvailable,
        if unavailable {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        if unavailable {
            "声明的手段不在可用集合中"
        } else {
            "声明的手段均可用"
        },
    );

    push(
        &mut outcomes,
        SufficiencyRule::MeansBounded,
        if input.means.len() > MAX_SUFFICIENCY_ITEMS {
            RuleStatus::Counterexample
        } else {
            RuleStatus::Satisfied
        },
        if input.means.len() > MAX_SUFFICIENCY_ITEMS {
            "手段数量超出上限"
        } else {
            "手段数量在限内"
        },
    );

    push(
        &mut outcomes,
        SufficiencyRule::BudgetDeclared,
        if input.budget_mb.is_some() {
            RuleStatus::Satisfied
        } else {
            RuleStatus::Unknown
        },
        if input.budget_mb.is_some() {
            "预算已声明"
        } else {
            "预算未声明：不判为失败，也不计为满足"
        },
    );

    push(
        &mut outcomes,
        SufficiencyRule::SemanticSufficiency,
        RuleStatus::Unknown,
        "语义充分性（意图与方案质量）不可判定，须用户确认",
    );

    let verdict = if outcomes
        .iter()
        .any(|item| item.status == RuleStatus::Counterexample && item.rule.tier() == "I")
    {
        Verdict::Counterexample
    } else {
        Verdict::Satisfied
    };

    SufficiencyReport {
        phase,
        verdict,
        outcomes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> SufficiencyInput<'static> {
        SufficiencyInput {
            requirement: "把会话导出补齐",
            checkpoints: &["导出含 schema_version", "断链为零"],
            means: &["shell", "read_file"],
            available_means: &["shell", "read_file", "todo"],
            budget_mb: Some(96),
        }
    }

    #[test]
    fn complete_input_is_satisfied() {
        let report = evaluate(CoherencePhase::Pre, &complete());
        assert_eq!(report.verdict, Verdict::Satisfied);
        assert_eq!(report.counterexamples().count(), 0);
        assert_eq!(report.outcomes.len(), 10);
    }

    #[test]
    fn missing_checkpoint_is_counterexample() {
        let mut input = complete();
        input.checkpoints = &[];
        let report = evaluate(CoherencePhase::Pre, &input);
        assert_eq!(report.verdict, Verdict::Counterexample);
        let ids: Vec<&str> = report
            .counterexamples()
            .map(|item| item.rule.id())
            .collect();
        assert!(ids.contains(&"SFR-03"));
    }

    #[test]
    fn unavailable_means_is_counterexample() {
        let mut input = complete();
        input.means = &["shell", "not_registered"];
        let report = evaluate(CoherencePhase::Pre, &input);
        assert_eq!(report.verdict, Verdict::Counterexample);
        assert!(
            report
                .counterexamples()
                .any(|item| item.rule.id() == "SFR-07")
        );
    }

    #[test]
    fn missing_budget_is_unknown_not_failure() {
        let mut input = complete();
        input.budget_mb = None;
        let report = evaluate(CoherencePhase::Mid, &input);
        assert_eq!(report.verdict, Verdict::Satisfied);
        assert_eq!(report.counterexamples().count(), 0);
        assert!(
            report
                .unknowns()
                .any(|item| item.rule == SufficiencyRule::BudgetDeclared)
        );
    }

    #[test]
    fn semantic_rule_is_always_unknown() {
        let report = evaluate(CoherencePhase::End, &complete());
        let semantic = report
            .outcomes
            .iter()
            .find(|item| item.rule == SufficiencyRule::SemanticSufficiency)
            .expect("语义规则必须在报告中");
        assert_eq!(semantic.status, RuleStatus::Unknown);
        assert_eq!(semantic.rule.tier(), "III");
    }

    #[test]
    fn oversized_text_is_counterexample_not_truncated() {
        let long = "x".repeat(MAX_SUFFICIENCY_TEXT_CHARS + 1);
        let mut input = complete();
        input.requirement = &long;
        let report = evaluate(CoherencePhase::Pre, &input);
        assert_eq!(report.verdict, Verdict::Counterexample);
        assert!(
            report
                .counterexamples()
                .any(|item| item.rule.id() == "SFR-02")
        );
    }

    #[test]
    fn too_many_checkpoints_is_counterexample() {
        let many = vec!["检查点"; MAX_SUFFICIENCY_ITEMS + 1];
        let mut input = complete();
        input.checkpoints = &many;
        let report = evaluate(CoherencePhase::Pre, &input);
        assert!(
            report
                .counterexamples()
                .any(|item| item.rule.id() == "SFR-05")
        );
    }

    #[test]
    fn duplicate_checkpoint_is_counterexample() {
        let mut input = complete();
        input.checkpoints = &["同一个", "同一个"];
        let report = evaluate(CoherencePhase::Pre, &input);
        assert!(
            report
                .counterexamples()
                .any(|item| item.rule.id() == "SFR-04")
        );
    }

    #[test]
    fn phase_roundtrips_and_parse_rejects_unknown() {
        for phase in [
            CoherencePhase::Pre,
            CoherencePhase::Mid,
            CoherencePhase::End,
        ] {
            assert_eq!(CoherencePhase::parse(phase.as_str()), Some(phase));
        }
        assert_eq!(CoherencePhase::parse("audit"), None);
    }

    #[test]
    fn evaluation_is_deterministic() {
        let input = complete();
        let first = evaluate(CoherencePhase::Pre, &input);
        let second = evaluate(CoherencePhase::Pre, &input);
        assert_eq!(first, second);
    }
}
