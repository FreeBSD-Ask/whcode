//! # 工具审批 broker 后端（T264、FR-754、ADR-060 项 A）
//!
//! 职责：维护**有界**的待审批请求表，按工具级规则生成请求、接收应答、给出封闭四态结论，
//! 并以 `approval_asked`／`approval_decided` 成对写入审计。
//!
//! 边界（ADR-060 §2.2 冻结）：
//! - **不接线 turn**：`execute_tool_call` 的审批接入属 T255／T256（ADR-057 B3a／B3b）；
//!   本模块默认不被 turn 调用，强制 `ask` 会让无 UI 场景全部工具变 `unavailable`（功能性回归）。
//! - **不做 SQL**：审计经 [`AuditSink`] 写入（ADR-018），持久化实现属 `whcode-store`。
//! - **不复制工具参数**：请求只带 `exec_id`，参数留在已展示的工具调用上。
//! - **不新增线程**：等待由调用方（UI 事件循环或测试）驱动；本模块只做状态机。

use std::collections::BTreeMap;

use whcode_domain::approval::{
    ApprovalAnswer, ApprovalOutcome, ApprovalRequest, ApprovalRule, MAX_PENDING_APPROVALS,
    outcome_for,
};
use whcode_domain::{AuditAction, AuditRecord, AuditSink, ErrorCode, WhError};

/// 有界的审批请求表。
///
/// 生命周期：`request()` 生成请求并落 `approval_asked`；`decide()`／`expire()` 给出结论并落
/// `approval_decided`。审计写入失败时状态**不**推进（请求不入表／不删除），符合“审计失败阻断”。
#[derive(Debug)]
pub struct ApprovalBroker {
    actor: String,
    pending: BTreeMap<String, ApprovalRequest>,
    seq: u64,
}

impl ApprovalBroker {
    /// 以发起者身份创建空 broker。
    #[must_use]
    pub fn new(actor: impl Into<String>) -> Self {
        Self {
            actor: actor.into(),
            pending: BTreeMap::new(),
            seq: 0,
        }
    }

    /// 待审批数量（有界性断言与 UI 展示用）。
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 按 id 取待审批请求。
    #[must_use]
    pub fn get(&self, request_id: &str) -> Option<&ApprovalRequest> {
        self.pending.get(request_id)
    }

    /// 当前全部待审批请求（稳定顺序，供 UI 列表）。
    pub fn iter_pending(&self) -> impl Iterator<Item = &ApprovalRequest> {
        self.pending.values()
    }

    /// 生成一次审批请求并写 `approval_asked` 审计。
    ///
    /// # Errors
    ///
    /// - 标识非法：`invalid_state`；
    /// - 待审批表已满 [`MAX_PENDING_APPROVALS`]：`backpressure`（可重试，不静默丢弃）；
    /// - 审计写入失败：原文错误（请求**不**入表，调用方不得视为已发起）。
    pub fn request(
        &mut self,
        sink: &dyn AuditSink,
        exec_id: &str,
        tool: &str,
        rule: ApprovalRule,
        display_reason: Option<String>,
        now_ms: u64,
    ) -> Result<ApprovalRequest, WhError> {
        if self.pending.len() >= MAX_PENDING_APPROVALS {
            return Err(WhError::backpressure(format!(
                "待审批请求已达上限 {MAX_PENDING_APPROVALS}：请先处理或撤回既有请求（不静默丢弃）"
            )));
        }
        self.seq = self.seq.wrapping_add(1);
        let id = format!("AP-{:06}-{now_ms}", self.seq);
        let request = ApprovalRequest::new(
            id,
            exec_id,
            tool,
            self.actor.clone(),
            rule,
            display_reason,
            now_ms,
        )?;
        let record = AuditRecord {
            id: format!("A-approval-asked-{}", request.id),
            actor: self.actor.clone(),
            action: AuditAction::ApprovalAsked,
            target: format!("tool:{} exec:{}", request.tool, request.exec_id),
            detail: serde_json::json!({
                "request_id": request.id,
                "exec_id": request.exec_id,
                "tool": request.tool,
                "rule": request.rule.as_str(),
                "display_reason": request.display_reason,
            }),
            ts_ms: now_ms,
        };
        sink.record(&record)?;
        self.pending.insert(request.id.clone(), request.clone());
        Ok(request)
    }

    /// 以应答（或“无人作答”＝`None`）结算请求，写 `approval_decided` 审计并返回四态结论。
    ///
    /// # Errors
    ///
    /// - 请求不存在（已被结算或从未发起）：`not_found`；
    /// - 审计写入失败：原文错误（请求**保留**在表中，可重试结算）。
    pub fn decide(
        &mut self,
        sink: &dyn AuditSink,
        request_id: &str,
        answer: Option<ApprovalAnswer>,
        note: Option<String>,
        now_ms: u64,
    ) -> Result<ApprovalOutcome, WhError> {
        let request = self.pending.get(request_id).cloned().ok_or_else(|| {
            WhError::new(
                ErrorCode::NotFound,
                format!("审批请求不存在或已结算：{request_id}"),
            )
        })?;
        let outcome = outcome_for(request.rule, answer);
        let record = AuditRecord {
            id: format!("A-approval-decided-{request_id}"),
            actor: self.actor.clone(),
            action: AuditAction::ApprovalDecided,
            target: format!("tool:{} exec:{}", request.tool, request.exec_id),
            detail: serde_json::json!({
                "request_id": request_id,
                "exec_id": request.exec_id,
                "tool": request.tool,
                "rule": request.rule.as_str(),
                "answer": answer.map(|value| match value {
                    ApprovalAnswer::Approve => "approve",
                    ApprovalAnswer::Reject => "reject",
                    ApprovalAnswer::Cancel => "cancel",
                }),
                "outcome": outcome.as_str(),
                "note": note,
            }),
            ts_ms: now_ms,
        };
        sink.record(&record)?;
        self.pending.remove(request_id);
        Ok(outcome)
    }

    /// 系统侧撤回（超时回收、会话取消）：结论固定为 `cancelled`，与“无人可问”区分。
    ///
    /// # Errors
    ///
    /// 同 [`Self::decide`]：请求不存在为 `not_found`；审计失败时请求保留。
    pub fn expire(
        &mut self,
        sink: &dyn AuditSink,
        request_id: &str,
        now_ms: u64,
    ) -> Result<ApprovalOutcome, WhError> {
        let request = self.pending.get(request_id).cloned().ok_or_else(|| {
            WhError::new(
                ErrorCode::NotFound,
                format!("审批请求不存在或已结算：{request_id}"),
            )
        })?;
        let record = AuditRecord {
            id: format!("A-approval-decided-{request_id}"),
            actor: self.actor.clone(),
            action: AuditAction::ApprovalDecided,
            target: format!("tool:{} exec:{}", request.tool, request.exec_id),
            detail: serde_json::json!({
                "request_id": request_id,
                "exec_id": request.exec_id,
                "tool": request.tool,
                "rule": request.rule.as_str(),
                "answer": serde_json::Value::Null,
                "outcome": ApprovalOutcome::Cancelled.as_str(),
                "note": "expired",
            }),
            ts_ms: now_ms,
        };
        sink.record(&record)?;
        self.pending.remove(request_id);
        Ok(ApprovalOutcome::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::CollectingAuditSink;

    struct FailingSink;

    impl AuditSink for FailingSink {
        fn record(&self, _record: &AuditRecord) -> Result<(), WhError> {
            Err(WhError::internal("审计不可用（测试替身）"))
        }
    }

    fn broker() -> ApprovalBroker {
        ApprovalBroker::new("S-1")
    }

    #[test]
    fn ask_without_answer_is_unavailable_not_rejected() {
        let sink = CollectingAuditSink::new();
        let mut broker = broker();
        let request = broker
            .request(&sink, "TX-1", "write_file", ApprovalRule::Ask, None, 10)
            .expect("发起");
        let outcome = broker
            .decide(&sink, &request.id, None, None, 11)
            .expect("结算");
        assert_eq!(outcome, ApprovalOutcome::Unavailable);
        assert!(!outcome.is_allowed());
        assert_eq!(broker.pending_len(), 0);
    }

    #[test]
    fn answer_matrix() {
        let sink = CollectingAuditSink::new();
        for (answer, expected) in [
            (Some(ApprovalAnswer::Approve), ApprovalOutcome::AllowedOnce),
            (Some(ApprovalAnswer::Reject), ApprovalOutcome::Rejected),
            (Some(ApprovalAnswer::Cancel), ApprovalOutcome::Cancelled),
        ] {
            let mut broker = broker();
            let request = broker
                .request(&sink, "TX-2", "shell", ApprovalRule::Ask, None, 1)
                .expect("发起");
            assert_eq!(
                broker.decide(&sink, &request.id, answer, None, 2).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn allow_and_deny_rules_do_not_need_an_answer() {
        let sink = CollectingAuditSink::new();
        let mut broker = broker();
        let allow = broker
            .request(&sink, "TX-3", "read_file", ApprovalRule::Allow, None, 1)
            .unwrap();
        assert_eq!(
            broker.decide(&sink, &allow.id, None, None, 2).unwrap(),
            ApprovalOutcome::AllowedOnce
        );
        let deny = broker
            .request(&sink, "TX-4", "shell", ApprovalRule::Deny, None, 3)
            .unwrap();
        assert_eq!(
            broker
                .decide(&sink, &deny.id, Some(ApprovalAnswer::Approve), None, 4)
                .unwrap(),
            ApprovalOutcome::Rejected
        );
    }

    #[test]
    fn pending_table_is_bounded() {
        let sink = CollectingAuditSink::new();
        let mut broker = broker();
        for index in 0..MAX_PENDING_APPROVALS {
            broker
                .request(
                    &sink,
                    &format!("TX-{index}"),
                    "shell",
                    ApprovalRule::Ask,
                    None,
                    index as u64,
                )
                .expect("容量内必须成功");
        }
        assert_eq!(broker.pending_len(), MAX_PENDING_APPROVALS);
        let err = broker
            .request(&sink, "TX-over", "shell", ApprovalRule::Ask, None, 99)
            .expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert_eq!(broker.pending_len(), MAX_PENDING_APPROVALS);
    }

    #[test]
    fn audit_failure_blocks_state_change() {
        let sink = FailingSink;
        let mut broker = broker();
        assert!(
            broker
                .request(&sink, "TX-5", "shell", ApprovalRule::Ask, None, 1)
                .is_err()
        );
        assert_eq!(broker.pending_len(), 0, "审计失败时请求不得入表");

        let ok_sink = CollectingAuditSink::new();
        let request = broker
            .request(&ok_sink, "TX-6", "shell", ApprovalRule::Ask, None, 2)
            .unwrap();
        assert!(
            broker
                .decide(&sink, &request.id, Some(ApprovalAnswer::Approve), None, 3)
                .is_err()
        );
        assert_eq!(
            broker.pending_len(),
            1,
            "审计失败时请求必须保留，可重试结算"
        );
        assert_eq!(
            broker
                .decide(
                    &ok_sink,
                    &request.id,
                    Some(ApprovalAnswer::Approve),
                    None,
                    4
                )
                .unwrap(),
            ApprovalOutcome::AllowedOnce
        );
    }

    #[test]
    fn expire_is_cancelled_and_unknown_id_is_not_found() {
        let sink = CollectingAuditSink::new();
        let mut broker = broker();
        let request = broker
            .request(&sink, "TX-7", "shell", ApprovalRule::Ask, None, 1)
            .unwrap();
        assert_eq!(
            broker.expire(&sink, &request.id, 2).unwrap(),
            ApprovalOutcome::Cancelled
        );
        let err = broker.expire(&sink, &request.id, 3).expect_err("已结算");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn audit_pair_carries_exec_id_and_no_arguments() {
        let sink = CollectingAuditSink::new();
        let mut broker = broker();
        let request = broker
            .request(
                &sink,
                "TX-8",
                "write_file",
                ApprovalRule::Ask,
                Some("写入工作区".to_string()),
                1,
            )
            .unwrap();
        broker
            .decide(&sink, &request.id, Some(ApprovalAnswer::Approve), None, 2)
            .unwrap();
        let records = sink.records();
        assert_eq!(records.len(), 2, "asked 与 decided 必须成对");
        assert_eq!(records[0].action, AuditAction::ApprovalAsked);
        assert_eq!(records[1].action, AuditAction::ApprovalDecided);
        for record in &records {
            assert_eq!(
                record.detail.get("exec_id").and_then(|v| v.as_str()),
                Some("TX-8")
            );
            assert!(
                record.detail.get("arguments").is_none() && record.detail.get("params").is_none(),
                "审批审计不得复制工具参数"
            );
        }
    }
}
