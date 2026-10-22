//! # 阶段角色与允许操作显式注入（T079、FR-313）
//!
//! 每次模型调用必须显式注入当前阶段角色与允许操作；角色**由状态机提供**，
//! **不得**从历史文本推断（FR-313 禁止项）。[`role_for`] 是唯一出处。
//!
//! 边界：内存纯函数（工作流配置之上的投影）；不做持久化与权限判定（属 authz）。

use whcode_speckit::{SddStage, WorkflowConfig};

/// 阶段角色与允许操作（FR-313 注入项）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRole {
    /// 当前阶段。
    pub stage: SddStage,
    /// 角色名（显式注入给模型调用）。
    pub role_name: String,
    /// 允许操作（状态机定义，不来自历史文本）。
    pub allowed_operations: Vec<&'static str>,
}

/// 从工作流配置读取阶段角色（FR-313：由状态机提供，唯一出处）。
#[must_use]
pub fn role_for(_workflow: &WorkflowConfig, stage: SddStage) -> StageRole {
    // 角色映射当前为常量（M4 口径）；workflow 参数预留运行时改写的策略注入点。

    let (role_name, allowed_operations) = match stage {
        SddStage::Constitution => ("architect", vec!["edit_constitution"]),
        SddStage::Specify => ("analyst", vec!["write_spec"]),
        SddStage::Clarify => ("analyst", vec!["answer_clarifications"]),
        SddStage::Plan => ("architect", vec!["write_plan"]),
        SddStage::Tasks => ("project_manager", vec!["write_tasks"]),
        SddStage::Analyze => ("auditor", vec!["run_analyze"]),
        SddStage::Implement => ("engineer", vec!["write_code", "record_checkpoints"]),
        SddStage::Checklist => ("auditor", vec!["review_checklist"]),
        SddStage::Converge => ("auditor", vec!["run_converge"]),
    };
    StageRole {
        stage,
        role_name: role_name.to_string(),
        allowed_operations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_speckit::default_workflow;

    #[test]
    fn roles_come_from_state_machine_not_history() {
        let workflow = default_workflow();
        // FR-313：角色由状态机提供——同一阶段恒返回同一角色，与历史文本无关。
        for stage in [
            SddStage::Constitution,
            SddStage::Specify,
            SddStage::Clarify,
            SddStage::Plan,
            SddStage::Tasks,
            SddStage::Analyze,
            SddStage::Implement,
            SddStage::Checklist,
            SddStage::Converge,
        ] {
            let first = role_for(&workflow, stage);
            let second = role_for(&workflow, stage);
            assert_eq!(first, second, "阶段 {stage} 的角色必须确定性（状态机提供）");
            assert!(!first.role_name.is_empty());
            assert!(!first.allowed_operations.is_empty(), "允许操作必须显式注入");
        }
        let implement = role_for(&workflow, SddStage::Implement);
        assert_eq!(implement.role_name, "engineer");
        assert!(implement.allowed_operations.contains(&"write_code"));
        let auditor = role_for(&workflow, SddStage::Analyze);
        assert_eq!(auditor.role_name, "auditor");
    }
}
