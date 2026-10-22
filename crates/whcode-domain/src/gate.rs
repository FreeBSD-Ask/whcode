//! 门禁结果与 SDD 阶段转换记录（任务书 §3.1、§4.3、FR-302/308）。
//!
//! 门禁由**应用服务**校验（G-05、FR-308）；仅向模型追加提示词不构成门禁。阶段转换记录
//! 必须包含来源阶段、目标阶段、执行者、校验结果与时间戳。人工强制放行不改变本记录，
//! 必须另行写入审计（FR-312）。

use serde::{Deserialize, Serialize};

use crate::state::SddStage;

/// 门禁标识。Clarify / Analyze / Checklist 是质量步骤（§4.3.1），不是阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateId {
    /// 项目级约束一致性。
    Constitution,
    /// 高优先级歧义澄清：未解决则阻止进入 Plan。
    Clarify,
    /// 引用完整性/覆盖/矛盾检查：失败则阻止进入 Implement。
    Analyze,
    /// 可选人工质量审查清单；若项目策略设为强制则纳入审计。
    Checklist,
}

impl GateId {
    /// 稳定序列化名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Constitution => "constitution",
            Self::Clarify => "clarify",
            Self::Analyze => "analyze",
            Self::Checklist => "checklist",
        }
    }
}

impl std::fmt::Display for GateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 单条门禁判定结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateResult {
    /// 门禁标识。
    pub gate: GateId,
    /// 是否通过。
    pub passed: bool,
    /// 未通过原因；通过时为空。失败必须给出可定位理由（产物 ID、引用、行位）。
    pub reasons: Vec<String>,
}

impl GateResult {
    /// 通过。
    pub fn pass(gate: GateId) -> Self {
        Self {
            gate,
            passed: true,
            reasons: Vec::new(),
        }
    }

    /// 未通过，附单条原因。
    pub fn fail(gate: GateId, reason: impl Into<String>) -> Self {
        Self {
            gate,
            passed: false,
            reasons: vec![reason.into()],
        }
    }

    /// 未通过，附多条原因。
    pub fn fail_with(gate: GateId, reasons: Vec<String>) -> Self {
        debug_assert!(!reasons.is_empty(), "门禁失败必须给出至少一条原因");
        Self {
            gate,
            passed: false,
            reasons,
        }
    }
}

/// SDD 阶段转换记录（§3.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageTransition {
    /// 来源阶段。
    pub from: SddStage,
    /// 目标阶段。
    pub to: SddStage,
    /// 执行者（用户、智能体或系统组件标识）。
    pub actor: String,
    /// 本次转换的全部门禁判定。
    pub validation: Vec<GateResult>,
    /// Unix epoch 毫秒（UTC）；ISO-8601 渲染由前端负责。
    pub ts_ms: u64,
}

impl StageTransition {
    /// 构造转换记录。
    pub fn new(
        from: SddStage,
        to: SddStage,
        actor: impl Into<String>,
        validation: Vec<GateResult>,
        ts_ms: u64,
    ) -> Self {
        Self {
            from,
            to,
            actor: actor.into(),
            validation,
            ts_ms,
        }
    }

    /// 全部门禁通过才允许转换（FR-307/308）。
    pub fn allowed(&self) -> bool {
        self.validation.iter().all(|r| r.passed)
    }

    /// 未通过的门禁清单，供错误对象的 `details` 使用。
    pub fn failed_gates(&self) -> Vec<&GateResult> {
        self.validation.iter().filter(|r| !r.passed).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_is_blocked_when_any_gate_fails() {
        let record = StageTransition::new(
            SddStage::Specify,
            SddStage::Plan,
            "agent",
            vec![
                GateResult::pass(GateId::Constitution),
                GateResult::fail(GateId::Clarify, "存在未解决的高优先级澄清项 CL-003"),
            ],
            1_700_000_000_000,
        );
        assert!(!record.allowed());
        assert_eq!(record.failed_gates().len(), 1);
        assert_eq!(record.failed_gates()[0].gate, GateId::Clarify);
    }

    #[test]
    fn transition_record_serializes_five_elements() {
        let record = StageTransition::new(
            SddStage::Tasks,
            SddStage::Implement,
            "agent",
            vec![GateResult::pass(GateId::Analyze)],
            42,
        );
        let value = serde_json::to_value(&record).expect("serialize");
        assert_eq!(value["from"], serde_json::json!("tasks"));
        assert_eq!(value["to"], serde_json::json!("implement"));
        assert_eq!(value["actor"], serde_json::json!("agent"));
        assert_eq!(value["validation"][0]["gate"], serde_json::json!("analyze"));
        assert_eq!(value["ts_ms"], serde_json::json!(42));
    }

    #[test]
    fn empty_validation_is_allowed_but_not_a_substitute_for_gates() {
        // 记录本身不强制非空；门禁是否必需由阶段定义（FR-302）决定。
        let record = StageTransition::new(SddStage::Plan, SddStage::Tasks, "agent", vec![], 0);
        assert!(record.allowed());
    }
}
