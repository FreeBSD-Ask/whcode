//! 状态常量：任务、LLM 请求、运行时执行与 SDD 阶段（任务书 §3.1）。
//!
//! 本模块是这些枚举的**唯一出处**：代码、数据库、CUI JSON Schema 与 Web 协议必须
//! 使用同一套定义（§3.1 末条约束）。

use serde::{Deserialize, Serialize};

/// 任务状态（任务书 §3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// 已登记，依赖尚未满足。
    Pending,
    /// 依赖已满足，可被调度。
    Ready,
    /// 正在执行。
    Running,
    /// 被阻塞：等待人工决策、依赖失败或门禁未通过。
    Blocked,
    /// 已完成。仅当验收条件无 `fail`/`not_tested` 时才可进入（FR-310/311）。
    Completed,
    /// 执行失败。
    Failed,
    /// 被显式中止。
    Aborted,
    /// 被中断（进程或运行时终止），需恢复流程处理（FR-409）。
    Interrupted,
    /// 外部副作用状态不确定，需核对、幂等重试或人工决策（FR-410、ADR-007）。
    Unknown,
}

impl TaskStatus {
    /// 稳定序列化名，供数据库与 JSON 契约共用。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
        }
    }

    /// 是否处于终态。`Interrupted` 与 `Unknown` **不是**终态：它们要求恢复流程介入。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Aborted)
    }
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// LLM 请求状态（任务书 §3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmRequestStatus {
    /// 已入队，尚未发出。
    Queued,
    /// 请求已发出，等待首个响应字节。
    Started,
    /// 正在接收流式增量。
    Streaming,
    /// 正常结束。
    Completed,
    /// 失败（含供应商错误归一化后的结果）。
    Failed,
    /// 被中断（进程终止、取消或网络中断）。
    Interrupted,
}

impl LlmRequestStatus {
    /// 稳定序列化名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Started => "started",
            Self::Streaming => "streaming",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }
}

impl std::fmt::Display for LlmRequestStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 运行时执行状态（任务书 §3.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeExecutionStatus {
    /// 意图已提交、检查点已写入，子进程尚未启动。
    Prepared,
    /// 子进程已启动。
    Running,
    /// 正常结束。
    Completed,
    /// 执行失败（非零退出或被信号终止）。
    Failed,
    /// 超时终止（FR-409）。
    TimedOut,
    /// 被中断。
    Interrupted,
    /// 是否产生副作用不确定——不得重放，须核对（FR-410、ADR-007）。
    Unknown,
}

impl RuntimeExecutionStatus {
    /// 稳定序列化名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
        }
    }

    /// 是否处于终态。`Interrupted` 与 `Unknown` **不是**终态：前者要求恢复流程判定，
    /// 后者要求核对副作用是否已发生（FR-410、ADR-007）——二者都不得直接视为可重试或已完成。
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::TimedOut)
    }
}

impl std::fmt::Display for RuntimeExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// SDD 阶段（任务书 §4.3.1 核心交付循环）。
///
/// `Clarify`、`Analyze`、`Checklist` 是**门禁/质量步骤**，不是阶段，因此不在此枚举中；
/// 它们以 [`crate::gate::GateId`] 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SddStage {
    /// 项目级约束。
    Constitution,
    /// 需求规格。
    Specify,
    /// 实施计划。
    Plan,
    /// 任务列表。
    Tasks,
    /// 实现。
    Implement,
    /// 收敛核对。
    Converge,
}

impl SddStage {
    /// 稳定序列化名。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Constitution => "constitution",
            Self::Specify => "specify",
            Self::Plan => "plan",
            Self::Tasks => "tasks",
            Self::Implement => "implement",
            Self::Converge => "converge",
        }
    }
}

impl std::fmt::Display for SddStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_enum_matches_task_book() {
        let all = [
            TaskStatus::Pending,
            TaskStatus::Ready,
            TaskStatus::Running,
            TaskStatus::Blocked,
            TaskStatus::Completed,
            TaskStatus::Failed,
            TaskStatus::Aborted,
            TaskStatus::Interrupted,
            TaskStatus::Unknown,
        ];
        assert_eq!(all.len(), 9);
        let names: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            names,
            [
                "pending",
                "ready",
                "running",
                "blocked",
                "completed",
                "failed",
                "aborted",
                "interrupted",
                "unknown"
            ]
        );
    }

    #[test]
    fn interrupted_and_unknown_are_not_terminal() {
        assert!(!TaskStatus::Interrupted.is_terminal());
        assert!(!TaskStatus::Unknown.is_terminal());
        assert!(TaskStatus::Completed.is_terminal());
    }

    #[test]
    fn serde_uses_snake_case_names() {
        let json = serde_json::to_string(&TaskStatus::Unknown).expect("serialize");
        assert_eq!(json, "\"unknown\"");
        let parsed: RuntimeExecutionStatus =
            serde_json::from_str("\"timed_out\"").expect("deserialize");
        assert_eq!(parsed, RuntimeExecutionStatus::TimedOut);
    }

    #[test]
    fn llm_and_runtime_status_sets_are_complete() {
        assert_eq!(LlmRequestStatus::Queued.as_str(), "queued");
        assert_eq!(LlmRequestStatus::Interrupted.as_str(), "interrupted");
        assert_eq!(RuntimeExecutionStatus::Prepared.as_str(), "prepared");
        assert_eq!(SddStage::Implement.as_str(), "implement");
        assert!(!RuntimeExecutionStatus::Prepared.is_terminal());
    }
}
