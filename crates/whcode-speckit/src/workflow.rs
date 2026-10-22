//! # 版本化工作流配置与阶段定义（T068、FR-302、contracts/sdd-artifacts.md §1—2）
//!
//! SDD 是**应用层状态机**，不是线性五阶段（契约 §1）。阶段、门禁与转换规则从
//! 版本化工作流配置读取（FR-302）；每次转换由应用服务校验（FR-308、M4/T074），
//! 仅提示词要求不构成门禁。
//!
//! 核心交付循环（契约 §1）：
//! `Constitution → Specify → Plan → Tasks → Implement → Converge`，
//! 质量检查：Clarify（未解决高优先级歧义阻止 Plan）、Analyze（失败阻止 Implement）、
//! Checklist（可选人工审查）、Converge（实现后对照检查差距）。
//!
//! 边界：配置是**常量**（版本化、随代码交付）；运行时改写属工作流升级（须新版本）。

use whcode_domain::GateId;

/// 工作流配置版本；阶段定义变更必须递增。
pub const WORKFLOW_VERSION: u32 = 1;

/// SDD 阶段（应用层状态机节点，contracts/sdd-artifacts.md §1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SddStage {
    /// Constitution：项目级约束。
    Constitution,
    /// Specify：需求基线。
    Specify,
    /// Clarify：高优先级歧义澄清。
    Clarify,
    /// Plan：技术方案与决策。
    Plan,
    /// Tasks：任务分解。
    Tasks,
    /// Analyze：引用完整性/覆盖/矛盾检查。
    Analyze,
    /// Implement：实现。
    Implement,
    /// Checklist：可选人工质量审查。
    Checklist,
    /// Converge：实现后对照规范检查差距。
    Converge,
}

impl SddStage {
    /// 稳定序列化名（数据库/JSON 共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Constitution => "constitution",
            Self::Specify => "specify",
            Self::Clarify => "clarify",
            Self::Plan => "plan",
            Self::Tasks => "tasks",
            Self::Analyze => "analyze",
            Self::Implement => "implement",
            Self::Checklist => "checklist",
            Self::Converge => "converge",
        }
    }

    /// 由稳定名解析；未知取值显式报错（不静默映射）。
    ///
    /// # Errors
    ///
    /// 未知阶段名时返回 [`whcode_domain::WhError`]。
    pub fn parse(value: &str) -> Result<Self, whcode_domain::WhError> {
        use SddStage::{
            Analyze, Checklist, Clarify, Constitution, Converge, Implement, Plan, Specify, Tasks,
        };
        const ALL: [SddStage; 9] = [
            Constitution,
            Specify,
            Clarify,
            Plan,
            Tasks,
            Analyze,
            Implement,
            Checklist,
            Converge,
        ];
        ALL.into_iter()
            .find(|stage| stage.as_str() == value)
            .ok_or_else(|| {
                whcode_domain::WhError::invalid_state(format!(
                    "未知 SDD 阶段：{value}（须为状态机稳定名）"
                ))
            })
    }
}

impl std::fmt::Display for SddStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 阶段定义（FR-302：允许输入、必需产物、校验器、成功/失败转换）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageDefinition {
    /// 阶段。
    pub stage: SddStage,
    /// 允许输入（产物类型稳定名）。
    pub allowed_inputs: &'static [&'static str],
    /// 必需产物（转换成功的前提）。
    pub required_artifacts: &'static [&'static str],
    /// 校验器（门禁标识 + 校验名）。
    pub validators: &'static [GateId],
    /// 成功转换的目标阶段。
    pub success_transition: SddStage,
    /// 失败转换的目标阶段（回退/阻塞）。
    pub failure_transition: SddStage,
}

/// 工作流配置（版本化，FR-302）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowConfig {
    /// 配置版本。
    pub version: u32,
    /// 阶段定义（按状态机顺序）。
    pub stages: Vec<StageDefinition>,
    /// Clarify 未解决高优先级澄清项是否阻止 Plan（FR-307，V-202）。
    pub clarify_blocks_plan: bool,
    /// Analyze 未通过是否阻止 Implement（FR-307，V-203）。
    pub analyze_blocks_implement: bool,
}

/// 默认工作流（契约 §1 核心循环 + 质量检查；版本 [`WORKFLOW_VERSION`]）。
#[must_use]
pub fn default_workflow() -> WorkflowConfig {
    WorkflowConfig {
        version: WORKFLOW_VERSION,
        stages: vec![
            StageDefinition {
                stage: SddStage::Constitution,
                allowed_inputs: &["constitution"],
                required_artifacts: &["constitution"],
                validators: &[GateId::Constitution],
                success_transition: SddStage::Specify,
                failure_transition: SddStage::Constitution,
            },
            StageDefinition {
                stage: SddStage::Specify,
                allowed_inputs: &["spec", "requirements"],
                required_artifacts: &["spec"],
                validators: &[],
                success_transition: SddStage::Clarify,
                failure_transition: SddStage::Specify,
            },
            StageDefinition {
                stage: SddStage::Clarify,
                allowed_inputs: &["clarifications"],
                required_artifacts: &[],
                validators: &[GateId::Clarify],
                success_transition: SddStage::Plan,
                failure_transition: SddStage::Clarify,
            },
            StageDefinition {
                stage: SddStage::Plan,
                allowed_inputs: &["plan", "decisions"],
                required_artifacts: &["plan"],
                validators: &[],
                success_transition: SddStage::Tasks,
                failure_transition: SddStage::Plan,
            },
            StageDefinition {
                stage: SddStage::Tasks,
                allowed_inputs: &["tasks"],
                required_artifacts: &["tasks"],
                validators: &[],
                success_transition: SddStage::Analyze,
                failure_transition: SddStage::Tasks,
            },
            StageDefinition {
                stage: SddStage::Analyze,
                allowed_inputs: &["tasks", "plan", "spec"],
                required_artifacts: &[],
                validators: &[GateId::Analyze],
                success_transition: SddStage::Implement,
                failure_transition: SddStage::Tasks,
            },
            StageDefinition {
                stage: SddStage::Implement,
                allowed_inputs: &["artifacts", "checkpoints"],
                required_artifacts: &[],
                validators: &[],
                success_transition: SddStage::Checklist,
                failure_transition: SddStage::Implement,
            },
            StageDefinition {
                stage: SddStage::Checklist,
                allowed_inputs: &["checklist"],
                required_artifacts: &[],
                validators: &[GateId::Checklist],
                success_transition: SddStage::Converge,
                failure_transition: SddStage::Converge,
            },
            StageDefinition {
                stage: SddStage::Converge,
                allowed_inputs: &["converge-report"],
                required_artifacts: &["converge-report"],
                validators: &[],
                success_transition: SddStage::Converge,
                failure_transition: SddStage::Implement,
            },
        ],
        clarify_blocks_plan: true,
        analyze_blocks_implement: true,
    }
}

impl WorkflowConfig {
    /// 读取阶段定义；未知阶段返回 `None`。
    #[must_use]
    pub fn stage(&self, stage: SddStage) -> Option<&StageDefinition> {
        self.stages
            .iter()
            .find(|definition| definition.stage == stage)
    }

    /// 状态机转换是否合法（`from → to` 是该阶段的成功/失败转换之一）。
    #[must_use]
    pub fn transition_allowed(&self, from: SddStage, to: SddStage) -> bool {
        self.stage(from).is_some_and(|definition| {
            definition.success_transition == to || definition.failure_transition == to
        })
    }

    /// 下一成功阶段；无定义返回 `None`。
    #[must_use]
    pub fn next_stage(&self, from: SddStage) -> Option<SddStage> {
        self.stage(from)
            .map(|definition| definition.success_transition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_workflow_matches_contract_loop() {
        let workflow = default_workflow();
        assert_eq!(workflow.version, WORKFLOW_VERSION);
        // 核心循环：Constitution → Specify → (Clarify) → Plan → Tasks → (Analyze) →
        // Implement → (Checklist) → Converge。
        assert!(workflow.transition_allowed(SddStage::Constitution, SddStage::Specify));
        assert!(workflow.transition_allowed(SddStage::Specify, SddStage::Clarify));
        assert!(workflow.transition_allowed(SddStage::Clarify, SddStage::Plan));
        assert!(workflow.transition_allowed(SddStage::Plan, SddStage::Tasks));
        assert!(workflow.transition_allowed(SddStage::Tasks, SddStage::Analyze));
        assert!(workflow.transition_allowed(SddStage::Analyze, SddStage::Implement));
        assert!(workflow.transition_allowed(SddStage::Implement, SddStage::Checklist));
        assert!(workflow.transition_allowed(SddStage::Checklist, SddStage::Converge));
        // 失败转换：Analyze 失败回 Tasks。
        assert!(workflow.transition_allowed(SddStage::Analyze, SddStage::Tasks));

        // 非法转换：Specify 直接跳 Implement 被拒绝（状态机，FR-308）。
        assert!(!workflow.transition_allowed(SddStage::Specify, SddStage::Implement));
        assert!(!workflow.transition_allowed(SddStage::Plan, SddStage::Converge));
    }

    #[test]
    fn stage_definitions_carry_four_elements() {
        let workflow = default_workflow();
        let plan = workflow.stage(SddStage::Plan).expect("Plan 定义");
        assert!(!plan.allowed_inputs.is_empty(), "FR-302：允许输入");
        assert!(!plan.required_artifacts.is_empty(), "FR-302：必需产物");
        // Plan 无门禁校验器（Clarify/Analyze 承担）；Clarify 必须有 Clarify 门禁。
        assert!(plan.validators.is_empty());
        let clarify = workflow.stage(SddStage::Clarify).expect("Clarify 定义");
        assert!(clarify.validators.contains(&GateId::Clarify));
        let analyze = workflow.stage(SddStage::Analyze).expect("Analyze 定义");
        assert!(analyze.validators.contains(&GateId::Analyze));
        assert!(
            workflow.clarify_blocks_plan && workflow.analyze_blocks_implement,
            "默认策略：Clarify 阻止 Plan、Analyze 阻止 Implement（FR-307）"
        );
    }

    #[test]
    fn stage_parse_is_strict_on_unknown_names() {
        assert_eq!(SddStage::parse("plan").expect("plan"), SddStage::Plan);
        assert_eq!(
            SddStage::parse("converge").expect("converge"),
            SddStage::Converge
        );
        let err = SddStage::parse("shrug").expect_err("未知阶段必须拒绝");
        assert!(err.to_string().contains("shrug"));
    }
}
