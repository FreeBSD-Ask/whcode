//! # 工具审批四态与 fail-closed 判定（T264、FR-754、ADR-060 项 A）
//!
//! 边界：本模块只定义**类型与判定**，不访问数据库、不发送 UI 事件、不读取工具参数；
//! 审计由调用方经 [`crate::audit::AuditSink`] 写入，执行接线由 T255／T256（ADR-057 B3a／B3b）
//! 完成——本批（ADR-060 项 A）**不得**在 turn 路径强制 `ask`。
//!
//! 语义（fail-closed）：
//! - 仅 [`ApprovalOutcome::AllowedOnce`] 放行，其余三态一律按拒绝处理；
//! - 无应答者、应答者异常或未作答，一律 [`ApprovalOutcome::Unavailable`]——**不得**降级为
//!   允许，也不得与“明确拒绝”混同（两者对模型与审计的意义不同）；
//! - 一次授权只覆盖被问的那一次动作（`allowed_once`），不产生长期规则；
//! - 审批请求只携带 `exec_id` 关联已展示的工具调用，**不复制工具参数**，避免展示面与执行面
//!   参数漂移（TOCTOU 型欺骗）。

use serde::{Deserialize, Serialize};

use crate::error::WhError;

/// 待审批队列上限（有界；超出按背压拒绝，不静默丢弃）。
pub const MAX_PENDING_APPROVALS: usize = 64;
/// 审批标识（请求 id／`exec_id`／工具名）长度上限。
pub const MAX_APPROVAL_ID_LEN: usize = 96;

/// 审批结果：封闭四态，调用方必须对非 `allowed_once` 一律拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOutcome {
    /// 仅授权被问的这一次动作。
    AllowedOnce,
    /// 明确拒绝。
    Rejected,
    /// 请求被撤回或取消（含系统侧超时回收）。
    Cancelled,
    /// 无可用应答者（未接线、应答者异常、无人作答）。
    Unavailable,
}

impl ApprovalOutcome {
    /// 稳定序列化名（审计、事件与四端协议共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AllowedOnce => "allowed_once",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
        }
    }

    /// 是否放行。**唯一**放行态是 [`Self::AllowedOnce`]。
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::AllowedOnce)
    }

    /// 解析稳定名；未知值返回 `None`（调用方不得把未知值当允许）。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allowed_once" => Some(Self::AllowedOnce),
            "rejected" => Some(Self::Rejected),
            "cancelled" => Some(Self::Cancelled),
            "unavailable" => Some(Self::Unavailable),
            _ => None,
        }
    }
}

/// 工具级审批规则。
///
/// 本批只到“工具级”；参数级规则与“记住规则”的持久化属 T255／T256。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRule {
    /// 直接放行（仍以一次性授权表达，便于审计对账）。
    Allow,
    /// 询问人；无人可问即 `unavailable`。
    Ask,
    /// 直接拒绝。
    Deny,
}

impl ApprovalRule {
    /// 稳定序列化名。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// 解析规则名；未知值返回 `None`。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "ask" => Some(Self::Ask),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// 应答者能表达的三态。
///
/// “无人作答”不是一种应答——它由 `None` 表达并映射为
/// [`ApprovalOutcome::Unavailable`]，避免把“没人可问”落成“拒绝”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAnswer {
    /// 批准这一次。
    Approve,
    /// 拒绝。
    Reject,
    /// 撤回（人主动取消该请求）。
    Cancel,
}

/// 一次审批请求（不含工具参数，只带 `exec_id`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// 请求标识（稳定、唯一、可审计）。
    pub id: String,
    /// 被审批的工具执行标识（关联已展示的工具调用）。
    pub exec_id: String,
    /// 工具稳定名。
    pub tool: String,
    /// 发起者（会话或 actor 标识）。
    pub actor: String,
    /// 命中的规则。
    pub rule: ApprovalRule,
    /// 给人看的理由（可选；审计文本另由 `reason` 承担）。
    pub display_reason: Option<String>,
    /// 请求创建时刻（毫秒）。
    pub created_at_ms: u64,
}

impl ApprovalRequest {
    /// 构造并校验：三个标识非空、不超长；`display_reason` 不超 [`MAX_APPROVAL_ID_LEN`]×4。
    ///
    /// # Errors
    ///
    /// 任一标识为空或超长时返回 [`WhError::invalid_state`] 语义的错误；构造失败即请求不成立，
    /// 调用方不得继续等待应答。
    pub fn new(
        id: impl Into<String>,
        exec_id: impl Into<String>,
        tool: impl Into<String>,
        actor: impl Into<String>,
        rule: ApprovalRule,
        display_reason: Option<String>,
        created_at_ms: u64,
    ) -> Result<Self, WhError> {
        let id = id.into();
        let exec_id = exec_id.into();
        let tool = tool.into();
        let actor = actor.into();
        require_token(&id, "审批请求 id")?;
        require_token(&exec_id, "exec_id")?;
        require_token(&tool, "工具名")?;
        if actor.trim().is_empty() {
            return Err(WhError::invalid_state("审批请求的 actor 不得为空"));
        }
        if let Some(reason) = &display_reason
            && reason.chars().count() > MAX_APPROVAL_ID_LEN * 4
        {
            return Err(WhError::invalid_state(format!(
                "审批展示理由过长（上限 {} 字符）",
                MAX_APPROVAL_ID_LEN * 4
            )));
        }
        Ok(Self {
            id,
            exec_id,
            tool,
            actor,
            rule,
            display_reason,
            created_at_ms,
        })
    }
}

/// 按规则与应答计算结果。**唯一**放行路径是 `allow` 或 `ask + approve`。
///
/// `answer = None` 表示无人可问或应答者异常，映射为 `unavailable`（fail-closed）。
#[must_use]
pub fn outcome_for(rule: ApprovalRule, answer: Option<ApprovalAnswer>) -> ApprovalOutcome {
    match (rule, answer) {
        (ApprovalRule::Allow, _) => ApprovalOutcome::AllowedOnce,
        (ApprovalRule::Deny, _) => ApprovalOutcome::Rejected,
        (ApprovalRule::Ask, Some(ApprovalAnswer::Approve)) => ApprovalOutcome::AllowedOnce,
        (ApprovalRule::Ask, Some(ApprovalAnswer::Reject)) => ApprovalOutcome::Rejected,
        (ApprovalRule::Ask, Some(ApprovalAnswer::Cancel)) => ApprovalOutcome::Cancelled,
        (ApprovalRule::Ask, None) => ApprovalOutcome::Unavailable,
    }
}

/// 一次请求的生命周期结论（等待中／已决）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalState {
    /// 等待应答。
    Pending,
    /// 已决。
    Decided(ApprovalOutcome),
}

fn require_token(value: &str, field: &str) -> Result<(), WhError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(WhError::invalid_state(format!("{field} 不得为空")));
    }
    if trimmed.chars().count() > MAX_APPROVAL_ID_LEN {
        return Err(WhError::invalid_state(format!(
            "{field} 超过长度上限 {MAX_APPROVAL_ID_LEN}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allowed_once_grants() {
        assert!(ApprovalOutcome::AllowedOnce.is_allowed());
        for outcome in [
            ApprovalOutcome::Rejected,
            ApprovalOutcome::Cancelled,
            ApprovalOutcome::Unavailable,
        ] {
            assert!(!outcome.is_allowed(), "{} 不得放行", outcome.as_str());
        }
    }

    #[test]
    fn outcome_matrix_is_fail_closed() {
        assert_eq!(
            outcome_for(ApprovalRule::Allow, None),
            ApprovalOutcome::AllowedOnce
        );
        assert_eq!(
            outcome_for(ApprovalRule::Deny, Some(ApprovalAnswer::Approve)),
            ApprovalOutcome::Rejected
        );
        assert_eq!(
            outcome_for(ApprovalRule::Ask, Some(ApprovalAnswer::Approve)),
            ApprovalOutcome::AllowedOnce
        );
        assert_eq!(
            outcome_for(ApprovalRule::Ask, Some(ApprovalAnswer::Reject)),
            ApprovalOutcome::Rejected
        );
        assert_eq!(
            outcome_for(ApprovalRule::Ask, Some(ApprovalAnswer::Cancel)),
            ApprovalOutcome::Cancelled
        );
        // 无人可问：既不是允许，也不与“明确拒绝”混同。
        assert_eq!(
            outcome_for(ApprovalRule::Ask, None),
            ApprovalOutcome::Unavailable
        );
    }

    #[test]
    fn request_rejects_empty_and_overlong_identifiers() {
        assert!(
            ApprovalRequest::new("", "TX-1", "read_file", "S-1", ApprovalRule::Ask, None, 0)
                .is_err()
        );
        assert!(
            ApprovalRequest::new(
                "AP-1",
                "   ",
                "read_file",
                "S-1",
                ApprovalRule::Ask,
                None,
                0
            )
            .is_err()
        );
        let long = "x".repeat(MAX_APPROVAL_ID_LEN + 1);
        assert!(
            ApprovalRequest::new("AP-1", "TX-1", long, "S-1", ApprovalRule::Ask, None, 0).is_err()
        );
        assert!(
            ApprovalRequest::new(
                "AP-1",
                "TX-1",
                "read_file",
                "S-1",
                ApprovalRule::Ask,
                Some("写入工作区外文件".to_string()),
                1
            )
            .is_ok()
        );
    }

    #[test]
    fn names_round_trip_and_unknown_is_none() {
        for outcome in [
            ApprovalOutcome::AllowedOnce,
            ApprovalOutcome::Rejected,
            ApprovalOutcome::Cancelled,
            ApprovalOutcome::Unavailable,
        ] {
            assert_eq!(ApprovalOutcome::parse(outcome.as_str()), Some(outcome));
        }
        for rule in [ApprovalRule::Allow, ApprovalRule::Ask, ApprovalRule::Deny] {
            assert_eq!(ApprovalRule::parse(rule.as_str()), Some(rule));
        }
        assert_eq!(ApprovalOutcome::parse("approved"), None);
        assert_eq!(ApprovalRule::parse("prompt"), None);
    }
}
