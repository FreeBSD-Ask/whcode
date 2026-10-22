//! # SDD 门禁视图（T078、FR-312、FR-314）
//!
//! `force-advance --reason`：人工越过门禁必须携带 reason（缺失即拒绝），审计记录
//! 含 actor / reason / 跳过的门禁 / 产物版本 / 时间戳（V-206）。本模块是 CUI 对
//! 应用服务的展示包装（前端禁止触库，ADR-003/005）；JSON 信封含 `schema_version`
//! （T085 接线到统一信封）。

use duckdb::Connection;
use serde_json::json;
use whcode_app::{Actor, Policy};
use whcode_domain::{AuditSink, SCHEMA_VERSION};
use whcode_speckit::WorkflowConfig;

/// `force-advance` 视图：人工越过门禁并输出 JSON 结果（FR-312）。
///
/// # Errors
///
/// 无 `reason`、非法转换或审计失败时返回 [`whcode_domain::WhError`]（错误信封由
/// 上层统一包装，T085）。
pub fn force_advance(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    workflow: &WorkflowConfig,
    command: &whcode_app::ForceAdvance<'_>,
    sink: &dyn AuditSink,
    now_ms: u64,
) -> Result<serde_json::Value, whcode_domain::WhError> {
    let session = whcode_app::force_advance(conn, policy, actor, workflow, command, sink, now_ms)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "session_id": session.id,
        "stage": session.stage,
        "forced": true,
        "reason": command.reason,
        "note": "人工越过门禁已留审计（actor/reason/跳过门禁/产物版本/时间戳，FR-312）",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_MIGRATIONS;
    use duckdb::Connection;
    use whcode_app::{DefaultPolicy, ForceAdvance, actor_with_all_capabilities};
    use whcode_domain::CollectingAuditSink;
    use whcode_speckit::{SddStage, default_workflow};

    #[test]
    fn force_advance_view_requires_reason_and_outputs_envelope() {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '强制放行', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        let mut conn = conn;
        let workflow = default_workflow();
        let actor = actor_with_all_capabilities("tester");
        let sink = CollectingAuditSink::new();

        // 无 reason：应用层拒绝。
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
        assert!(err.to_string().contains("reason"));

        // 有 reason：JSON 信封含 schema_version 与审计说明。
        let view = force_advance(
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
        assert_eq!(view["schema_version"], SCHEMA_VERSION);
        assert_eq!(view["stage"], "specify");
        assert_eq!(view["forced"], true);
        assert_eq!(view["reason"], "阻塞解除");
        assert_eq!(sink.len(), 1, "审计已记录（V-206）");
    }
}
