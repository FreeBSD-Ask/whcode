//! # 检查点触发策略（FR-405）
//!
//! 定时、按步骤、危险操作前三类触发的间隔与保留数量可配置；[`CheckpointPolicy`]
//! 是纯值类型，不读时钟、不做 IO——调度器（M8 运行时装配）采样后调用
//! [`CheckpointPolicy::triggered_by`] 得到触发原因，历史裁剪用 [`retain_recent`]。
//!
//! 放在领域层的原因：配置（`whcode-bin`）与运行时（`whcode-runtime`）都需要同一份
//! 策略定义，且两者之间不新增依赖边（AGENTS §4 依赖纪律）。
//!
//! 边界：本模块只判定“是否触发、为什么触发”；检查点四要素记录（FR-706）在
//! `whcode-runtime::checkpoint`。

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, WhError};

/// 保留数量上限（有界；超出即拒绝配置，防止把保留窗口写成无意义的大数）。
pub const MAX_RETAINED: usize = 100;
/// 定时触发间隔上限（秒）。
pub const MAX_INTERVAL_SECONDS: u64 = 86_400;
/// 按步骤触发的步数上限。
pub const MAX_EVERY_STEPS: u32 = 10_000;

/// 默认定时触发间隔（秒）——可配置的基线，不是定理（宪法 §0.3-6）。
pub const DEFAULT_INTERVAL_SECONDS: u64 = 300;
/// 默认保留数量。
pub const DEFAULT_RETAIN: usize = 10;

/// 检查点触发原因（可观测：写入事件与诊断时说明“为什么在这里检查点”）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTrigger {
    /// 危险操作前强制触发。
    Dangerous,
    /// 距上次检查点达到定时间隔。
    Interval,
    /// 自上次检查点起达到步数阈值。
    Steps,
}

/// 触发判定输入（由调度器采样后传入；本模块不读时钟、不自行计数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CheckpointState {
    /// 距上次检查点的秒数。
    pub elapsed_seconds: u64,
    /// 自上次检查点以来的步数。
    pub steps_since_checkpoint: u32,
    /// 下一步是否为危险操作。
    pub next_step_dangerous: bool,
}

/// 检查点触发策略（FR-405）：定时 / 按步骤 / 危险操作前，间隔与保留数可配置。
///
/// 关闭某类触发用 `0`（定时与按步骤）；`before_dangerous = false` 关闭危险操作前触发。
/// 默认值：[`DEFAULT_INTERVAL_SECONDS`] 秒定时、按步骤关闭、危险操作前开启、
/// 保留 [`DEFAULT_RETAIN`] 个。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckpointPolicy {
    /// 定时触发间隔（秒；`0` = 关闭）。
    pub interval_seconds: u64,
    /// 每 N 步触发（`0` = 关闭）。
    pub every_steps: u32,
    /// 危险操作前是否强制触发。
    pub before_dangerous: bool,
    /// 保留的检查点数量（1..=[`MAX_RETAINED`]）。
    pub retain: usize,
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            interval_seconds: DEFAULT_INTERVAL_SECONDS,
            every_steps: 0,
            before_dangerous: true,
            retain: DEFAULT_RETAIN,
        }
    }
}

impl CheckpointPolicy {
    /// 校验配置边界（越界即拒绝，不静默夹取）。
    ///
    /// # Errors
    ///
    /// 间隔、步数或保留数越界时返回 `invalid_state`（消息写明合法范围）。
    pub fn validate(&self) -> Result<(), WhError> {
        if self.interval_seconds > MAX_INTERVAL_SECONDS {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!(
                    "检查点定时间隔不得超过 {MAX_INTERVAL_SECONDS} 秒（当前 {}）",
                    self.interval_seconds
                ),
            ));
        }
        if self.every_steps > MAX_EVERY_STEPS {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!(
                    "检查点步数阈值不得超过 {MAX_EVERY_STEPS}（当前 {}）",
                    self.every_steps
                ),
            ));
        }
        if !(1..=MAX_RETAINED).contains(&self.retain) {
            return Err(WhError::new(
                ErrorCode::InvalidState,
                format!(
                    "检查点保留数量必须在 1—{MAX_RETAINED} 之间（当前 {}）",
                    self.retain
                ),
            ));
        }
        Ok(())
    }

    /// 判定是否应创建检查点：危险操作优先，其次定时，最后按步骤。
    ///
    /// 返回 `None` 表示本次不触发；判定顺序固定，便于测试与审计复现。
    #[must_use]
    pub fn triggered_by(&self, state: &CheckpointState) -> Option<CheckpointTrigger> {
        if self.before_dangerous && state.next_step_dangerous {
            return Some(CheckpointTrigger::Dangerous);
        }
        if self.interval_seconds > 0 && state.elapsed_seconds >= self.interval_seconds {
            return Some(CheckpointTrigger::Interval);
        }
        if self.every_steps > 0 && state.steps_since_checkpoint >= self.every_steps {
            return Some(CheckpointTrigger::Steps);
        }
        None
    }
}

/// 只保留最近 `retain` 个检查点，返回被丢弃的数量。
///
/// 有界容器纪律（NFR-215）：历史不得无界累积；`retain` 为 `0` 时按 1 处理，
/// 保证至少保留最新一项而不是全部丢弃。
pub fn retain_recent<T>(items: &mut Vec<T>, retain: usize) -> usize {
    let retain = retain.max(1);
    let excess = items.len().saturating_sub(retain);
    if excess > 0 {
        items.drain(0..excess);
    }
    excess
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check 171：三类触发各有独立开关与阈值，判定顺序固定（危险 > 定时 > 按步骤）。
    #[test]
    fn policy_triggers_on_timer_steps_and_dangerous_actions() {
        let policy = CheckpointPolicy {
            interval_seconds: 60,
            every_steps: 10,
            before_dangerous: true,
            retain: 5,
        };
        policy.validate().expect("合法策略");

        let idle = CheckpointState {
            elapsed_seconds: 59,
            steps_since_checkpoint: 9,
            next_step_dangerous: false,
        };
        assert_eq!(policy.triggered_by(&idle), None, "未达任何阈值不触发");

        let danger = CheckpointState {
            next_step_dangerous: true,
            ..idle
        };
        assert_eq!(
            policy.triggered_by(&danger),
            Some(CheckpointTrigger::Dangerous),
            "危险操作优先于定时与步数"
        );

        let timed = CheckpointState {
            elapsed_seconds: 60,
            steps_since_checkpoint: 0,
            next_step_dangerous: false,
        };
        assert_eq!(
            policy.triggered_by(&timed),
            Some(CheckpointTrigger::Interval)
        );

        let stepped = CheckpointState {
            elapsed_seconds: 0,
            steps_since_checkpoint: 10,
            next_step_dangerous: false,
        };
        assert_eq!(
            policy.triggered_by(&stepped),
            Some(CheckpointTrigger::Steps)
        );

        // 关闭三类触发后永不触发（可撤销，不做“为你好”的强制）。
        let disabled = CheckpointPolicy {
            interval_seconds: 0,
            every_steps: 0,
            before_dangerous: false,
            retain: 1,
        };
        let everything = CheckpointState {
            elapsed_seconds: u64::MAX,
            steps_since_checkpoint: u32::MAX,
            next_step_dangerous: true,
        };
        assert_eq!(disabled.triggered_by(&everything), None);
    }

    /// Check 171：越界配置显式失败，不静默夹取。
    #[test]
    fn policy_rejects_out_of_range_values() {
        let cases = [
            CheckpointPolicy {
                interval_seconds: MAX_INTERVAL_SECONDS + 1,
                ..CheckpointPolicy::default()
            },
            CheckpointPolicy {
                every_steps: MAX_EVERY_STEPS + 1,
                ..CheckpointPolicy::default()
            },
            CheckpointPolicy {
                retain: 0,
                ..CheckpointPolicy::default()
            },
            CheckpointPolicy {
                retain: MAX_RETAINED + 1,
                ..CheckpointPolicy::default()
            },
        ];
        for policy in cases {
            let err = policy.validate().expect_err("越界必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState, "{policy:?}");
        }
        CheckpointPolicy::default().validate().expect("默认值合法");
    }

    /// Check 171：保留数量可配置且有界——只保留最新 N 个并报告丢弃数。
    #[test]
    fn retention_keeps_recent_items_and_reports_drops() {
        let mut items: Vec<u32> = (0..10).collect();
        assert_eq!(retain_recent(&mut items, 3), 7);
        assert_eq!(items, vec![7, 8, 9], "保留最新的 3 个");
        assert_eq!(retain_recent(&mut items, 3), 0, "未超限不丢弃");
        // 0 按 1 处理：至少保留最新一项，绝不“保留 0 个”。
        assert_eq!(retain_recent(&mut items, 0), 2);
        assert_eq!(items, vec![9]);
    }

    /// Check 171：策略参与 JSON 往返；缺字段时取默认值（旧配置不被破坏）。
    #[test]
    fn policy_roundtrips_through_json_with_defaults() {
        let policy = CheckpointPolicy {
            interval_seconds: 120,
            every_steps: 4,
            before_dangerous: false,
            retain: 7,
        };
        let json = serde_json::to_string(&policy).expect("序列化");
        let restored: CheckpointPolicy = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(policy, restored);

        let partial: CheckpointPolicy =
            serde_json::from_str(r#"{"every_steps": 2}"#).expect("缺字段取默认值");
        assert_eq!(partial.every_steps, 2);
        assert_eq!(partial.interval_seconds, DEFAULT_INTERVAL_SECONDS);
        assert!(partial.before_dangerous);
        assert_eq!(partial.retain, DEFAULT_RETAIN);
    }
}
