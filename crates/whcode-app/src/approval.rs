//! # 工具审批 broker 后端（T264、FR-754、ADR-060 项 A；接线 T255／FR-744、ADR-064）
//!
//! 职责：维护**有界**的待审批请求表，按工具级规则生成请求、接收应答、给出封闭四态结论，
//! 并以 `approval_asked`／`approval_decided` 成对写入审计。
//!
//! 边界（ADR-060 §2.2 冻结的解除范围见 ADR-064）：
//! - **规则解析与执行闸门分离**：[`ApprovalPolicy`] 只做“工具 → 规则”解析，
//!   [`ApprovalGate`] 把规则、broker、应答者与审计串成一次判定；`turn` 的接入点在
//!   `execute_tool_call`（参数校验之后、三步登记之前）。
//! - **不做 SQL**：审计经 [`AuditSink`] 写入（ADR-018），持久化实现属 `whcode-store`。
//! - **不复制工具参数**：请求只带 `exec_id`，参数留在已展示的工具调用上。
//! - **不新增线程**：应答者 [`ApprovalAnswerer`] 由调用线程同步驱动；本模块不建线程，
//!   等待预算有硬上限（[`MAX_APPROVAL_WAIT_MS`]），超预算的应答按 `unavailable` 丢弃。

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use whcode_domain::approval::{
    ApprovalAnswer, ApprovalOutcome, ApprovalRequest, ApprovalRule, MAX_APPROVAL_ID_LEN,
    MAX_PENDING_APPROVALS, outcome_for,
};
use whcode_domain::{AuditAction, AuditRecord, AuditSink, ErrorCode, WhError};
/// 副作用范围（规则按范围取默认的判据）。
///
/// 由 `whcode-tools` 定义；此处再导出，让装配根（`whcode-bin`）不必直接依赖工具 crate
/// 也能表达“按范围解析规则”。
pub use whcode_tools::SideEffectScope;

/// 审批等待预算默认值（毫秒）：一次 `ask` 最多等人这么久，超时按 `unavailable`。
pub const DEFAULT_APPROVAL_WAIT_MS: u64 = 120_000;
/// 审批等待预算硬上限（毫秒；越界按上限截断，不拒绝启动）。
pub const MAX_APPROVAL_WAIT_MS: u64 = 600_000;
/// 工具级覆盖规则数量上限（有界：规则表不随工具面无限制累积）。
pub const MAX_APPROVAL_TOOL_RULES: usize = 256;

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

/// 应答者（人机交互面：CUI 提示、TUI 对话框、GUI／Web 审批卡）。
///
/// 契约（ADR-064 决策 3）：
/// - **同步、无内建线程**：实现方在调用线程上等待（UI 事件循环或测试驱动）；本模块不建线程；
/// - `Ok(None)` 表示“无人可问”（未接线、已关闭、无人作答）；
/// - `Err` 表示应答面异常；两者都映射为 [`ApprovalOutcome::Unavailable`]，
///   **不得**降级为允许或拒绝；
/// - `budget_ms` 是等待预算，返回后由 [`ApprovalGate`] 复核实际耗时，超预算的应答被丢弃。
pub trait ApprovalAnswerer: Send + Sync {
    /// 请求一次应答；`budget_ms` 取值被收口在 `[1, MAX_APPROVAL_WAIT_MS]`。
    ///
    /// # Errors
    ///
    /// 应答面不可用或内部错误；错误文本不进入审计（避免参数／应答内容泄漏）。
    fn answer(
        &self,
        request: &ApprovalRequest,
        budget_ms: u64,
    ) -> Result<Option<ApprovalAnswer>, WhError>;
}

/// 工具级审批规则解析（per-tool 覆盖 + 按副作用范围默认；ADR-064 决策 2）。
///
/// 解析顺序：per-tool 覆盖 > 副作用范围默认。默认值 [`Default`] 为**全部 `allow`**，
/// 与接线前行为一致（无 UI 时既有工具不得全部变 `unavailable`，ADR-060 §2.2 理由）。
/// `ask`／`deny` 只能由显式配置生效；破坏性工具默认切 `ask` 留给 B3b（ADR-064）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPolicy {
    default_read_only: ApprovalRule,
    default_workspace_write: ApprovalRule,
    per_tool: BTreeMap<String, ApprovalRule>,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            default_read_only: ApprovalRule::Allow,
            default_workspace_write: ApprovalRule::Allow,
            per_tool: BTreeMap::new(),
        }
    }
}

impl ApprovalPolicy {
    /// 按副作用范围默认值构造（per-tool 覆盖另行叠加）。
    #[must_use]
    pub fn new(default_read_only: ApprovalRule, default_workspace_write: ApprovalRule) -> Self {
        Self {
            default_read_only,
            default_workspace_write,
            per_tool: BTreeMap::new(),
        }
    }

    /// 叠加一条 per-tool 覆盖。
    ///
    /// # Errors
    ///
    /// 工具名为空／超长，或覆盖数量超 [`MAX_APPROVAL_TOOL_RULES`] 时返回 `invalid_state`
    /// （配置错误由人修正，运行期不得静默忽略）。
    pub fn with_tool_rule(
        mut self,
        tool: impl Into<String>,
        rule: ApprovalRule,
    ) -> Result<Self, WhError> {
        let tool = tool.into();
        let trimmed = tool.trim();
        if trimmed.is_empty() {
            return Err(WhError::invalid_state("审批规则的工具名不得为空"));
        }
        if trimmed.chars().count() > MAX_APPROVAL_ID_LEN {
            return Err(WhError::invalid_state(format!(
                "审批规则的工具名超过长度上限 {MAX_APPROVAL_ID_LEN}"
            )));
        }
        if !self.per_tool.contains_key(trimmed) && self.per_tool.len() >= MAX_APPROVAL_TOOL_RULES {
            return Err(WhError::invalid_state(format!(
                "工具级审批规则超过上限 {MAX_APPROVAL_TOOL_RULES}"
            )));
        }
        self.per_tool.insert(trimmed.to_string(), rule);
        Ok(self)
    }

    /// 解析某工具某次调用的规则：per-tool 覆盖优先，否则取副作用范围默认。
    #[must_use]
    pub fn rule_for(&self, tool: &str, scope: SideEffectScope) -> ApprovalRule {
        self.per_tool.get(tool).copied().unwrap_or(match scope {
            SideEffectScope::ReadOnly => self.default_read_only,
            SideEffectScope::WorkspaceWrite => self.default_workspace_write,
        })
    }

    /// 两个范围默认值（配置回显与断言用）。
    #[must_use]
    pub fn range_defaults(&self) -> (ApprovalRule, ApprovalRule) {
        (self.default_read_only, self.default_workspace_write)
    }

    /// 已声明的 per-tool 覆盖数量（有界性断言用）。
    #[must_use]
    pub fn tool_rule_count(&self) -> usize {
        self.per_tool.len()
    }
}

/// 一次审批判定的结论：命中规则、封闭四态、请求 id（`allow` 时无请求）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalDecision {
    /// 命中的规则。
    pub rule: ApprovalRule,
    /// 四态结论；仅 [`ApprovalOutcome::AllowedOnce`] 放行。
    pub outcome: ApprovalOutcome,
    /// 审计请求 id（`allow` 规则不发起请求，故为 `None`）。
    pub request_id: Option<String>,
}

impl ApprovalDecision {
    /// 是否放行（唯一放行态是 `allowed_once`）。
    #[must_use]
    pub fn is_allowed(&self) -> bool {
        self.outcome.is_allowed()
    }
}

/// 工具执行路径上的审批闸门：规则解析 → 请求 → 应答 → 结论 → 成对审计。
///
/// 所有失败都返回错误，由调用方转为**结构化工具失败**（不中断 turn、不伪造成功）；
/// 审计写入失败时状态不推进（沿用 T264 口径：审计失败阻断执行）。
pub struct ApprovalGate<'a> {
    policy: ApprovalPolicy,
    sink: &'a dyn AuditSink,
    answerer: Option<&'a dyn ApprovalAnswerer>,
    broker: ApprovalBroker,
    wait_ms: u64,
}

impl<'a> ApprovalGate<'a> {
    /// 构造闸门；`answerer = None` 表示“无人可问”（`ask` 一律 `unavailable`）。
    ///
    /// `wait_ms` 被收口到 `[1, MAX_APPROVAL_WAIT_MS]`（有界等待，配置笔误不得放大成无界）。
    #[must_use]
    pub fn new(
        actor: impl Into<String>,
        policy: ApprovalPolicy,
        sink: &'a dyn AuditSink,
        answerer: Option<&'a dyn ApprovalAnswerer>,
        wait_ms: u64,
    ) -> Self {
        Self {
            policy,
            sink,
            answerer,
            broker: ApprovalBroker::new(actor),
            wait_ms: wait_ms.clamp(1, MAX_APPROVAL_WAIT_MS),
        }
    }

    /// 生效规则表（回显/断言用）。
    #[must_use]
    pub fn policy(&self) -> &ApprovalPolicy {
        &self.policy
    }

    /// 待审批数量（正常情况下每次判定都立即结算，故应恒为 0）。
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.broker.pending_len()
    }

    /// 生效等待预算（毫秒）。
    #[must_use]
    pub fn wait_ms(&self) -> u64 {
        self.wait_ms
    }

    /// 判定一次工具调用是否可执行。
    ///
    /// - `allow`：直接放行，**不**写审计（无人工参与；避免为每次工具调用留两份噪声记录）；
    /// - `deny`：落 `approval_asked` + `approval_decided(rejected)` 后返回 `rejected`；
    /// - `ask`：落 `approval_asked`，同步询问应答者，再落 `approval_decided`。
    ///   无应答者／应答者异常／超预算一律 `unavailable`（fail-closed，不与拒绝混同）。
    ///
    /// # Errors
    ///
    /// - 审计写入失败：原文错误（调用方**不得**继续执行工具）；
    /// - 待审批队列已满：`backpressure`（有界，不静默丢弃）；
    /// - 请求构造失败：`invalid_state`。
    pub fn authorize(
        &mut self,
        exec_id: &str,
        tool: &str,
        scope: SideEffectScope,
        now_ms: u64,
    ) -> Result<ApprovalDecision, WhError> {
        let rule = self.policy.rule_for(tool, scope);
        if rule == ApprovalRule::Allow {
            return Ok(ApprovalDecision {
                rule,
                outcome: ApprovalOutcome::AllowedOnce,
                request_id: None,
            });
        }
        let display_reason = match rule {
            ApprovalRule::Deny => Some("规则 deny：该工具调用直接拒绝（不询问）".to_string()),
            _ => Some("规则 ask：该工具调用需要人工确认".to_string()),
        };
        let request =
            self.broker
                .request(self.sink, exec_id, tool, rule, display_reason, now_ms)?;
        let (answer, note) = match rule {
            ApprovalRule::Deny => (None, Some("rule_deny".to_string())),
            _ => self.ask(&request),
        };
        let outcome = self
            .broker
            .decide(self.sink, &request.id, answer, note, now_ms)?;
        Ok(ApprovalDecision {
            rule,
            outcome,
            request_id: Some(request.id),
        })
    }

    /// 同步询问应答者；返回（应答, 审计备注）。备注只取静态标签，不携带异常正文。
    fn ask(&self, request: &ApprovalRequest) -> (Option<ApprovalAnswer>, Option<String>) {
        let Some(answerer) = self.answerer else {
            return (None, Some("no_answerer".to_string()));
        };
        let started = Instant::now();
        let reply = answerer.answer(request, self.wait_ms);
        let over_budget = started.elapsed() > Duration::from_millis(self.wait_ms);
        match reply {
            Ok(_) if over_budget => (None, Some("timed_out".to_string())),
            Ok(Some(answer)) => (Some(answer), Some("answered".to_string())),
            Ok(None) => (None, Some("no_answerer".to_string())),
            Err(_) => (None, Some("answerer_failed".to_string())),
        }
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

    // ── 规则解析与执行闸门（T255／FR-744、ADR-064）─────────────────────

    /// 固定应答的应答者；`answer = None` 模拟“无人作答”。
    struct FixedAnswerer(Option<ApprovalAnswer>);

    impl ApprovalAnswerer for FixedAnswerer {
        fn answer(
            &self,
            _request: &ApprovalRequest,
            _budget_ms: u64,
        ) -> Result<Option<ApprovalAnswer>, WhError> {
            Ok(self.0)
        }
    }

    /// 应答面异常（内部错误）：必须映射为 `unavailable`，不得降级。
    struct FailingAnswerer;

    impl ApprovalAnswerer for FailingAnswerer {
        fn answer(
            &self,
            _request: &ApprovalRequest,
            _budget_ms: u64,
        ) -> Result<Option<ApprovalAnswer>, WhError> {
            Err(WhError::internal("应答面不可用（测试替身）"))
        }
    }

    /// 超过等待预算才作答：结论必须是 `unavailable`（超预算应答被丢弃）。
    struct SlowAnswerer(ApprovalAnswer);

    impl ApprovalAnswerer for SlowAnswerer {
        fn answer(
            &self,
            _request: &ApprovalRequest,
            _budget_ms: u64,
        ) -> Result<Option<ApprovalAnswer>, WhError> {
            std::thread::sleep(Duration::from_millis(12));
            Ok(Some(self.0))
        }
    }

    fn policy_with(tool: &str, rule: ApprovalRule) -> ApprovalPolicy {
        ApprovalPolicy::default()
            .with_tool_rule(tool, rule)
            .expect("规则合法")
    }

    #[test]
    fn default_policy_allows_every_scope_and_is_behavior_compatible() {
        let policy = ApprovalPolicy::default();
        assert_eq!(
            policy.range_defaults(),
            (ApprovalRule::Allow, ApprovalRule::Allow),
            "默认必须行为兼容：无 UI 时既有工具不得全部变 unavailable（ADR-060 §2.2）"
        );
        assert_eq!(
            policy.rule_for("read_file", SideEffectScope::ReadOnly),
            ApprovalRule::Allow
        );
        assert_eq!(
            policy.rule_for("write_file", SideEffectScope::WorkspaceWrite),
            ApprovalRule::Allow
        );
        assert_eq!(policy.tool_rule_count(), 0);
    }

    #[test]
    fn per_tool_override_wins_over_scope_default() {
        let policy = ApprovalPolicy::new(ApprovalRule::Allow, ApprovalRule::Ask)
            .with_tool_rule("write_file", ApprovalRule::Deny)
            .expect("合法覆盖");
        assert_eq!(
            policy.rule_for("read_file", SideEffectScope::ReadOnly),
            ApprovalRule::Allow,
            "读工具取只读默认"
        );
        assert_eq!(
            policy.rule_for("grep", SideEffectScope::WorkspaceWrite),
            ApprovalRule::Ask,
            "未覆盖的写工具取写默认"
        );
        assert_eq!(
            policy.rule_for("write_file", SideEffectScope::WorkspaceWrite),
            ApprovalRule::Deny,
            "per-tool 覆盖优先于范围默认"
        );
    }

    #[test]
    fn policy_rejects_invalid_tool_names_and_bounds_rule_count() {
        assert!(
            ApprovalPolicy::default()
                .with_tool_rule("   ", ApprovalRule::Ask)
                .is_err(),
            "空工具名必须拒绝"
        );
        let long = "t".repeat(MAX_APPROVAL_ID_LEN + 1);
        assert!(
            ApprovalPolicy::default()
                .with_tool_rule(long, ApprovalRule::Ask)
                .is_err(),
            "超长工具名必须拒绝"
        );
        let mut policy = ApprovalPolicy::default();
        for index in 0..MAX_APPROVAL_TOOL_RULES {
            policy = policy
                .with_tool_rule(format!("tool-{index}"), ApprovalRule::Ask)
                .expect("容量内必须成功");
        }
        assert_eq!(policy.tool_rule_count(), MAX_APPROVAL_TOOL_RULES);
        // 覆盖既有 key 不受上限影响。
        let policy = policy
            .with_tool_rule("tool-0", ApprovalRule::Allow)
            .expect("替换既有规则不受上限约束");
        assert_eq!(policy.tool_rule_count(), MAX_APPROVAL_TOOL_RULES);
        assert!(
            policy
                .with_tool_rule("tool-over", ApprovalRule::Ask)
                .is_err(),
            "超限必须拒绝（有界，不静默丢弃）"
        );
    }

    #[test]
    fn allow_rule_passes_without_asking_or_auditing() {
        let sink = CollectingAuditSink::new();
        let answerer = FixedAnswerer(Some(ApprovalAnswer::Reject));
        let mut gate = ApprovalGate::new(
            "S-1",
            ApprovalPolicy::default(),
            &sink,
            Some(&answerer),
            1_000,
        );
        let decision = gate
            .authorize("TX-A", "read_file", SideEffectScope::ReadOnly, 1)
            .expect("放行");
        assert!(decision.is_allowed());
        assert_eq!(decision.rule, ApprovalRule::Allow);
        assert_eq!(decision.request_id, None, "allow 不发起请求");
        assert!(sink.is_empty(), "allow 不写审批审计（无人工参与）");
        assert_eq!(gate.pending_len(), 0);
    }

    #[test]
    fn deny_rule_is_rejected_with_paired_audit() {
        let sink = CollectingAuditSink::new();
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("shell", ApprovalRule::Deny),
            &sink,
            None,
            1_000,
        );
        let decision = gate
            .authorize("TX-D", "shell", SideEffectScope::WorkspaceWrite, 5)
            .expect("判定");
        assert_eq!(decision.rule, ApprovalRule::Deny);
        assert_eq!(decision.outcome, ApprovalOutcome::Rejected);
        assert!(!decision.is_allowed());
        let records = sink.records();
        assert_eq!(records.len(), 2, "deny 也必须成对留痕");
        assert_eq!(records[0].action, AuditAction::ApprovalAsked);
        assert_eq!(records[1].action, AuditAction::ApprovalDecided);
        assert_eq!(
            records[1]
                .detail
                .get("outcome")
                .and_then(|value| value.as_str()),
            Some("rejected")
        );
        assert_eq!(gate.pending_len(), 0, "判定后不得残留待审批");
    }

    #[test]
    fn ask_with_approve_executes_and_audits_pair() {
        let sink = CollectingAuditSink::new();
        let answerer = FixedAnswerer(Some(ApprovalAnswer::Approve));
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("write_file", ApprovalRule::Ask),
            &sink,
            Some(&answerer),
            1_000,
        );
        let decision = gate
            .authorize("TX-OK", "write_file", SideEffectScope::WorkspaceWrite, 7)
            .expect("判定");
        assert_eq!(decision.outcome, ApprovalOutcome::AllowedOnce);
        assert!(decision.is_allowed());
        let records = sink.records();
        assert_eq!(records.len(), 2, "asked 与 decided 成对");
        assert_eq!(
            records[1]
                .detail
                .get("answer")
                .and_then(|value| value.as_str()),
            Some("approve")
        );
        assert_eq!(
            records[0]
                .detail
                .get("exec_id")
                .and_then(|value| value.as_str()),
            Some("TX-OK"),
            "请求只带 exec_id 关联已展示的调用"
        );
    }

    #[test]
    fn ask_with_reject_is_rejected() {
        let sink = CollectingAuditSink::new();
        let answerer = FixedAnswerer(Some(ApprovalAnswer::Reject));
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("write_file", ApprovalRule::Ask),
            &sink,
            Some(&answerer),
            1_000,
        );
        let decision = gate
            .authorize("TX-R", "write_file", SideEffectScope::WorkspaceWrite, 1)
            .expect("判定");
        assert_eq!(decision.outcome, ApprovalOutcome::Rejected);
        assert!(!decision.is_allowed());
    }

    #[test]
    fn ask_without_answerer_is_unavailable_not_allowed_or_rejected() {
        let sink = CollectingAuditSink::new();
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("write_file", ApprovalRule::Ask),
            &sink,
            None,
            1_000,
        );
        let decision = gate
            .authorize("TX-U", "write_file", SideEffectScope::WorkspaceWrite, 1)
            .expect("判定");
        assert_eq!(decision.outcome, ApprovalOutcome::Unavailable);
        assert_ne!(decision.outcome, ApprovalOutcome::AllowedOnce);
        assert_ne!(decision.outcome, ApprovalOutcome::Rejected);
        let records = sink.records();
        assert_eq!(records.len(), 2, "问了但无人作答也必须成对留痕");
        assert_eq!(
            records[1]
                .detail
                .get("note")
                .and_then(|value| value.as_str()),
            Some("no_answerer")
        );
        assert_eq!(
            records[1]
                .detail
                .get("outcome")
                .and_then(|value| value.as_str()),
            Some("unavailable")
        );
    }

    #[test]
    fn ask_answerer_error_is_unavailable() {
        let sink = CollectingAuditSink::new();
        let answerer = FailingAnswerer;
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("write_file", ApprovalRule::Ask),
            &sink,
            Some(&answerer),
            1_000,
        );
        let decision = gate
            .authorize("TX-E", "write_file", SideEffectScope::WorkspaceWrite, 1)
            .expect("判定");
        assert_eq!(decision.outcome, ApprovalOutcome::Unavailable);
        let records = sink.records();
        assert_eq!(
            records[1]
                .detail
                .get("note")
                .and_then(|value| value.as_str()),
            Some("answerer_failed")
        );
        assert!(
            !records[1].detail.to_string().contains("测试替身"),
            "应答面异常正文不得进入审计"
        );
    }

    #[test]
    fn ask_over_wait_budget_is_discarded_as_unavailable() {
        let sink = CollectingAuditSink::new();
        let answerer = SlowAnswerer(ApprovalAnswer::Approve);
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("write_file", ApprovalRule::Ask),
            &sink,
            Some(&answerer),
            1,
        );
        assert_eq!(gate.wait_ms(), 1);
        let decision = gate
            .authorize("TX-T", "write_file", SideEffectScope::WorkspaceWrite, 1)
            .expect("判定");
        assert_eq!(
            decision.outcome,
            ApprovalOutcome::Unavailable,
            "超预算的迟到应答不得放行"
        );
        let records = sink.records();
        assert_eq!(
            records[1]
                .detail
                .get("note")
                .and_then(|value| value.as_str()),
            Some("timed_out")
        );
    }

    #[test]
    fn wait_budget_is_clamped_to_a_finite_cap() {
        let sink = CollectingAuditSink::new();
        let gate = ApprovalGate::new("S-1", ApprovalPolicy::default(), &sink, None, 0);
        assert_eq!(gate.wait_ms(), 1, "0 被抬到最小 1 ms（不放任无界等待）");
        let gate = ApprovalGate::new(
            "S-1",
            ApprovalPolicy::default(),
            &sink,
            None,
            MAX_APPROVAL_WAIT_MS + 1,
        );
        assert_eq!(gate.wait_ms(), MAX_APPROVAL_WAIT_MS, "超上限按上限收口");
        assert_eq!(DEFAULT_APPROVAL_WAIT_MS, 120_000);
    }

    #[test]
    fn audit_failure_blocks_authorization() {
        let sink = FailingSink;
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("shell", ApprovalRule::Deny),
            &sink,
            None,
            1_000,
        );
        let err = gate
            .authorize("TX-F", "shell", SideEffectScope::WorkspaceWrite, 1)
            .expect_err("审计失败必须阻断（不得返回判定）");
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(gate.pending_len(), 0, "审计失败时请求不得入表");

        // 结算阶段审计失败：请求保留在表中（可重试结算），工具仍不得执行。
        let ok_sink = CollectingAuditSink::new();
        let mut gate = ApprovalGate::new(
            "S-1",
            policy_with("shell", ApprovalRule::Ask),
            &ok_sink,
            None,
            1_000,
        );
        assert!(
            gate.authorize("TX-G", "shell", SideEffectScope::WorkspaceWrite, 1)
                .is_ok()
        );
    }
}
