//! # SDD 门禁强制执行（T074/T078、FR-307/FR-308/FR-312、V-202/V-203/V-206）
//!
//! 门禁由**程序**实施（FR-308：仅提示词要求不构成门禁）：
//!
//! - [`request_transition`]：每次阶段转换经状态机校验（`transition_allowed`）+
//!   门禁检查（FR-307）——高优先级未解决澄清项阻止 Plan（V-202）、Analyze 未通过
//!   阻止 Implement（V-203）；失败返回 `gate_failed`（`retryable = false`），
//!   `details` 含未通过门禁清单与澄清项 ID。
//! - [`force_advance`]（T078、FR-312、V-206）：人工越过门禁**必须带 `--reason`**
//!   （缺失即拒绝），审计记录含 actor / reason / 跳过的门禁 / 产物版本 / 时间戳。
//!
//! 产物版本口径：转换时的 SDD 产物版本由调用方传入（`artifact_version`），
//! 审计记录原样登记。
//!
//! 边界：门禁检查的产物状态（澄清项、Analyze 结果）由调用方以 [`TransitionContext`]
//! 显式提供——由历史文本推断属 FR-313 禁止项，角色与状态一律来自状态机与解析产物。

use duckdb::Connection;
use whcode_domain::{AuditAction, AuditRecord, AuditSink, ErrorCode, WhError};
use whcode_speckit::{SddStage, WorkflowConfig};
use whcode_store::directory::{self, SessionRow};

use crate::authz::{Actor, Capability, Policy, require};

/// 转换时的门禁上下文（由调用方从解析产物显式提供，FR-313 禁止推断）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransitionContext {
    /// 未解决的高优先级澄清项 ID（V-202 的门禁输入）。
    pub unresolved_high_priority: Vec<String>,
    /// Analyze 是否通过（V-203 的门禁输入）。
    pub analyze_passed: bool,
    /// Constitution 强制约束是否满足（V-201 的门禁输入）。
    pub constitution_passed: bool,
}

fn gate_failed(message: String, gate: &str, details: serde_json::Value) -> WhError {
    WhError {
        code: ErrorCode::GateFailed,
        message,
        retryable: false,
        details: Some(serde_json::json!({
            "gate": gate,
            "failed": details,
        })),
    }
}

/// 请求阶段转换（FR-307/308）：状态机校验 + 门禁检查 + 阶段持久化。
///
/// # Errors
///
/// 权限拒绝、非法转换（`invalid_state`）、门禁未通过（`gate_failed`，V-202/V-203）、
/// 会话不存在或存储失败时返回 [`WhError`]。
pub fn request_transition(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    workflow: &WorkflowConfig,
    to: SddStage,
    context: &TransitionContext,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SpecAdvance)?;
    let session = directory::get_session(conn, session_id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("会话不存在：{session_id}")))?;
    let from = SddStage::parse(&session.stage)?;
    if !workflow.transition_allowed(from, to) {
        return Err(WhError::invalid_state(format!(
            "非法阶段转换 {from} → {to}（FR-308：转换须为状态机的成功/失败转换）"
        )));
    }
    if to == SddStage::Plan
        && workflow.clarify_blocks_plan
        && !context.unresolved_high_priority.is_empty()
    {
        return Err(gate_failed(
            format!(
                "高优先级澄清项未解决（{} 项），Plan 转换被阻止（FR-307）",
                context.unresolved_high_priority.len()
            ),
            "clarify",
            serde_json::json!({
                "unresolved": context.unresolved_high_priority,
                "from": from.as_str(),
                "to": to.as_str(),
            }),
        ));
    }
    if to == SddStage::Implement && workflow.analyze_blocks_implement && !context.analyze_passed {
        return Err(gate_failed(
            "Analyze 未通过，Implement 转换被阻止（FR-307、V-203）".to_string(),
            "analyze",
            serde_json::json!({ "from": from.as_str(), "to": to.as_str() }),
        ));
    }
    if to == SddStage::Plan && workflow.clarify_blocks_plan && !context.constitution_passed {
        return Err(gate_failed(
            "Constitution 强制约束未满足，转换被阻止（FR-301、V-201）".to_string(),
            "constitution",
            serde_json::json!({ "from": from.as_str(), "to": to.as_str() }),
        ));
    }
    directory::update_stage(conn, session_id, to.as_str())?;
    directory::get_session(conn, session_id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("会话不存在：{session_id}")))
}

/// force-advance 命令参数（T078）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForceAdvance<'a> {
    /// 目标会话。
    pub session_id: &'a str,
    /// 目标阶段。
    pub to: SddStage,
    /// 强制放行理由（必填，FR-312）。
    pub reason: &'a str,
    /// 产物版本（入审计，V-206）。
    pub artifact_version: u32,
}

/// 人工强制越过门禁（T078、FR-312、V-206）：必须带 reason，审计含完整要素。
///
/// # Errors
///
/// 权限拒绝、`reason` 为空、非法转换或审计写入失败时返回 [`WhError`]；
/// **审计失败时转换不得生效**（`AuditSink` 契约）。
pub fn force_advance(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    workflow: &WorkflowConfig,
    command: &ForceAdvance<'_>,
    sink: &dyn AuditSink,
    now_ms: u64,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SpecAdvance)?;
    if command.reason.trim().is_empty() {
        return Err(WhError::invalid_state(
            "force-advance 必须携带 reason：人工越过门禁必须记录理由（FR-312）",
        ));
    }
    let session_id = command.session_id;
    let to = command.to;
    let session = directory::get_session(conn, session_id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("会话不存在：{session_id}")))?;
    let from = SddStage::parse(&session.stage)?;
    if !workflow.transition_allowed(from, to) {
        return Err(WhError::invalid_state(format!(
            "非法阶段转换 {from} → {to}（FR-308：强制放行也须为状态机定义的转换）"
        )));
    }
    let record = AuditRecord {
        id: format!("A-override-{session_id}-{now_ms}"),
        actor: actor.id.clone(),
        action: AuditAction::GateOverride,
        target: format!("session:{session_id} {from} → {to}"),
        detail: serde_json::json!({
            "reason": command.reason,
            "skipped_gates": ["clarify", "analyze", "constitution"],
            "artifact_version": command.artifact_version,
            "from": from.as_str(),
            "to": to.as_str(),
        }),
        ts_ms: now_ms,
    };
    sink.record(&record)?;
    directory::update_stage(conn, session_id, to.as_str())?;
    directory::get_session(conn, session_id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("会话不存在：{session_id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use whcode_domain::CollectingAuditSink;
    use whcode_speckit::default_workflow;
    use whcode_store::{apply_migrations, create_session_with_id, directory};

    fn setup() -> (Connection, WorkflowConfig, Actor) {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        create_session_with_id(&conn, "S-1", "门禁", "active", 1_000).expect("创建");
        (
            conn,
            default_workflow(),
            actor_with_all_capabilities("tester"),
        )
    }

    /// 沿状态机合法链路推进到目标阶段（无门禁约束时）。
    fn advance_to(
        conn: &mut Connection,
        actor: &Actor,
        workflow: &WorkflowConfig,
        target: SddStage,
    ) -> SessionRow {
        let context = TransitionContext {
            unresolved_high_priority: vec![],
            analyze_passed: true,
            constitution_passed: true,
        };
        let mut session = directory::get_session(conn, "S-1")
            .expect("读取")
            .expect("存在");
        loop {
            let from = SddStage::parse(&session.stage).expect("阶段");
            if from == target {
                return session;
            }
            let next = workflow.next_stage(from).expect("下一阶段");
            session =
                request_transition(conn, &DefaultPolicy, actor, "S-1", workflow, next, &context)
                    .expect("合法推进");
        }
    }

    #[test]
    fn unresolved_clarifications_block_plan_transition() {
        let (mut conn, workflow, actor) = setup();
        // 先合法推进到 Clarify（specify → clarify）。
        advance_to(&mut conn, &actor, &workflow, SddStage::Clarify);

        let mut context = TransitionContext {
            unresolved_high_priority: vec!["CLAR-001".to_string()],
            analyze_passed: true,
            constitution_passed: true,
        };
        let err = request_transition(
            &mut conn,
            &DefaultPolicy,
            &actor,
            "S-1",
            &workflow,
            SddStage::Plan,
            &context,
        )
        .expect_err("未解决澄清必须阻止 Plan（V-202）");
        assert_eq!(err.code, ErrorCode::GateFailed);
        assert!(!err.retryable);
        assert_eq!(
            err.details.as_ref().expect("details")["failed"]["unresolved"][0],
            "CLAR-001"
        );

        // 解决后放行。
        context.unresolved_high_priority.clear();
        let session = request_transition(
            &mut conn,
            &DefaultPolicy,
            &actor,
            "S-1",
            &workflow,
            SddStage::Plan,
            &context,
        )
        .expect("放行");
        assert_eq!(session.stage, "plan");
    }

    #[test]
    fn analyze_failure_blocks_implement_transition() {
        let (mut conn, workflow, actor) = setup();
        // 先合法推进到 Analyze 阶段（specify → … → tasks → analyze）；
        // 门禁挡的是 Analyze → Implement 的转换（FR-307）。
        advance_to(&mut conn, &actor, &workflow, SddStage::Analyze);
        let context = TransitionContext {
            unresolved_high_priority: vec![],
            analyze_passed: false,
            constitution_passed: true,
        };

        let err = request_transition(
            &mut conn,
            &DefaultPolicy,
            &actor,
            "S-1",
            &workflow,
            SddStage::Implement,
            &context,
        )
        .expect_err("Analyze 未通过必须阻止 Implement（V-203）");
        assert_eq!(err.code, ErrorCode::GateFailed);
        assert_eq!(err.details.as_ref().expect("details")["gate"], "analyze");

        // Analyze 通过后放行。
        let context = TransitionContext {
            unresolved_high_priority: vec![],
            analyze_passed: true,
            constitution_passed: true,
        };
        let session = request_transition(
            &mut conn,
            &DefaultPolicy,
            &actor,
            "S-1",
            &workflow,
            SddStage::Implement,
            &context,
        )
        .expect("放行");
        assert_eq!(session.stage, "implement");
    }

    #[test]
    fn force_advance_requires_reason_and_audits_full_elements() {
        let mut conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        create_session_with_id(&conn, "S-1", "门禁", "active", 1_000).expect("创建");
        let workflow = default_workflow();
        let actor = actor_with_all_capabilities("tester");
        let sink = CollectingAuditSink::new();

        // 无 reason 必须拒绝（FR-312）。
        let err = force_advance(
            &mut conn,
            &DefaultPolicy,
            &actor,
            &workflow,
            &ForceAdvance {
                session_id: "S-1",
                to: SddStage::Specify,
                reason: "  ",
                artifact_version: 1,
            },
            &sink,
            1_000,
        )
        .expect_err("无 reason 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 有 reason：审计含 actor/reason/跳过门禁/产物版本/时间戳（V-206）。
        let session = force_advance(
            &mut conn,
            &DefaultPolicy,
            &actor,
            &workflow,
            &ForceAdvance {
                session_id: "S-1",
                to: SddStage::Specify,
                reason: "阻塞解除",
                artifact_version: 7,
            },
            &sink,
            1_000,
        )
        .expect("强制放行");
        assert_eq!(session.stage, "specify");
        assert_eq!(sink.len(), 1);
        let record = &sink.records()[0];
        assert_eq!(record.action, whcode_domain::AuditAction::GateOverride);
        assert_eq!(record.detail["reason"], "阻塞解除");
        assert_eq!(record.detail["artifact_version"], 7);
        assert_eq!(record.ts_ms, 1_000);
        assert!(
            record.detail["skipped_gates"]
                .as_array()
                .expect("跳过门禁清单")
                .len()
                > 1,
            "审计须含跳过的门禁清单"
        );

        // 非法转换仍拒绝（强制放行也须为状态机定义的转换，FR-308）。
        let err = force_advance(
            &mut conn,
            &DefaultPolicy,
            &actor,
            &workflow,
            &ForceAdvance {
                session_id: "S-1",
                to: SddStage::Converge,
                reason: "x",
                artifact_version: 1,
            },
            &sink,
            2_000,
        )
        .expect_err("非法转换必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = directory::get_session;
    }
}
