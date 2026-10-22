//! # 执行前检查点（T105、FR-706、FR-405）
//!
//! 脚本哈希 / 输入哈希 / 运行时指纹 / 工作目录策略记录（FR-706）——意图记录协议
//! （ADR-007：意图—执行—登记）。哈希为 FNV-1a（意外损坏检测，非密码学——
//! 失效条件：防篡改须外部签名）。
//!
//! 触发策略（FR-405）：[`CheckpointPolicy`] 声明定时、按步骤、危险操作前三类触发
//! 的间隔与保留数量上限，[`CheckpointPolicy::triggered_by`] 给出触发原因，
//! [`retain_recent`] 把历史裁剪到有界长度。策略类型定义在 `whcode-domain::checkpoint`，
//! 由配置层（`whcode-bin`）与运行时共用同一份定义，避免新增依赖边。
//!
//! **未接线事项**（不得依赖隐含默认）：本模块只提供记录结构与策略判定；
//! 调度器（何时读取时钟、在哪一步判定“危险操作”）属 M8 运行时装配，
//! 尚未接入任何 Provider 执行路径（见 `docs/matrices/runtime-inventory.md`）。
//!
//! 持久化入 `runtime_executions` 表（script_hash/input_hash/runtime_fingerprint），
//! 由调用方执行；本模块只做记录结构。

use whcode_domain::{ErrorCode, WhError};

pub use whcode_domain::checkpoint::{
    CheckpointPolicy, CheckpointState, CheckpointTrigger, DEFAULT_INTERVAL_SECONDS, DEFAULT_RETAIN,
    MAX_EVERY_STEPS, MAX_INTERVAL_SECONDS, MAX_RETAINED, retain_recent,
};

/// 执行前检查点（FR-706 四要素 + 结果登记）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Checkpoint {
    /// 脚本哈希。
    pub script_hash: String,
    /// 输入哈希。
    pub input_hash: String,
    /// 运行时指纹。
    pub runtime_fingerprint: String,
    /// 工作目录策略。
    pub workdir_policy: String,
    /// 执行结果。
    pub result: Option<crate::worker::WorkerResult>,
}

/// 校验检查点四要素齐备（FR-706 校验）。
///
/// # Errors
///
/// 任一要素为空时返回 `invalid_state`。
pub fn validate(checkpoint: &Checkpoint) -> Result<(), WhError> {
    for (name, value) in [
        ("script_hash", &checkpoint.script_hash),
        ("input_hash", &checkpoint.input_hash),
        ("runtime_fingerprint", &checkpoint.runtime_fingerprint),
        ("workdir_policy", &checkpoint.workdir_policy),
    ] {
        if value.trim().is_empty() {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!("检查点 {name} 不得为空（FR-706）"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_validates_four_elements() {
        let checkpoint = Checkpoint {
            script_hash: "fnv1a64:123".to_string(),
            input_hash: "fnv1a64:456".to_string(),
            runtime_fingerprint: "python-3.12/windows-amd64".to_string(),
            workdir_policy: "temp".to_string(),
            result: None,
        };
        validate(&checkpoint).expect("四要素齐备");

        let empty = Checkpoint {
            script_hash: String::new(),
            ..checkpoint
        };
        let err = validate(&empty).expect_err("空要素必须拒绝");
        assert!(err.to_string().contains("script_hash"));
    }

    /// FR-405：运行时侧经重导出使用同一份策略定义（不复制判定逻辑）。
    #[test]
    fn policy_is_reused_from_domain() {
        let policy = CheckpointPolicy::default();
        policy.validate().expect("默认策略合法");
        assert_eq!(
            policy.triggered_by(&CheckpointState {
                next_step_dangerous: true,
                ..CheckpointState::default()
            }),
            Some(CheckpointTrigger::Dangerous)
        );
    }
}
