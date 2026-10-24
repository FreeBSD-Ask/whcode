//! # 事件查询与导出视图（T065、FR-205）
//!
//! 事件 JSON/JSONL 查询与导出，**导出注明脱敏策略**（`trace_level` + `redacted`
//! + 完整性标记）。
//!
//! 本模块是 CUI 对应用服务的展示包装（前端禁止触库，ADR-003/005、C-03）；查询核心
//! 在 `whcode-app::events`。
//!
//! 边界：JSON 信封含 `schema_version`（T085 接线到统一信封）；载荷已按级别脱敏，
//! `redacted` 下疑似秘密字段不出现在导出（V-110）。

use duckdb::Connection;
use serde_json::json;
use whcode_app::{Actor, Policy};
use whcode_domain::{SCHEMA_VERSION, TraceLevel};

/// 事件 JSON 导出（脱敏策略随对象注明，FR-205）。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`whcode_domain::WhError`]。
pub fn events_json(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    after_seq: i64,
    limit: usize,
    level: TraceLevel,
) -> Result<serde_json::Value, whcode_domain::WhError> {
    let export =
        whcode_app::events_export(conn, policy, actor, session_id, after_seq, limit, level)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "session_id": export.session_id,
        "trace_level": export.trace_level,
        "redacted": export.redacted,
        "integrity": export.integrity.to_json(),
        "count": export.events.len(),
        "events": export.events,
    }))
}

/// 事件 JSONL 导出：首行为导出头（脱敏策略注明），其后每事件一行。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`whcode_domain::WhError`]。
pub fn events_jsonl(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    after_seq: i64,
    limit: usize,
    level: TraceLevel,
) -> Result<String, whcode_domain::WhError> {
    let export =
        whcode_app::events_export(conn, policy, actor, session_id, after_seq, limit, level)?;
    let header = json!({
        "schema_version": SCHEMA_VERSION,
        "session_id": export.session_id,
        "trace_level": export.trace_level,
        "redacted": export.redacted,
        "integrity": export.integrity.to_json(),
        "count": export.events.len(),
    });
    let mut lines = vec![header.to_string()];
    for event in &export.events {
        lines.push(serde_json::to_string(event).map_err(|err| {
            whcode_domain::WhError::parse_error(format!("事件序列化失败：{err}"))
        })?);
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_MIGRATIONS;
    use duckdb::{Connection, params};
    use whcode_app::{DefaultPolicy, actor_with_all_capabilities};

    fn setup() -> (Connection, DefaultPolicy, whcode_app::Actor) {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '导出', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("创建");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (1, 'E-1', 'S-1', NULL, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            params![r#"{"api_key":"sk-secret","prompt":"正文"}"#],
        )
        .expect("插入事件");
        (conn, DefaultPolicy, actor_with_all_capabilities("tester"))
    }

    #[test]
    fn events_json_annotates_policy_and_redacts_secrets() {
        let (conn, policy, actor) = setup();
        let view = events_json(&conn, &policy, &actor, "S-1", 0, 100, TraceLevel::Redacted)
            .expect("JSON 导出");
        assert_eq!(view["schema_version"], SCHEMA_VERSION);
        assert_eq!(
            view["trace_level"], "redacted",
            "脱敏策略必须注明（FR-205）"
        );
        assert_eq!(view["redacted"], true);
        assert_eq!(view["count"], 1);
        let serialized = serde_json::to_string(&view).expect("序列化");
        assert!(
            !serialized.contains("sk-secret"),
            "redacted 下秘密不得出现（V-110）"
        );
        assert!(serialized.contains("正文"), "普通内容保留");
    }

    #[test]
    fn events_jsonl_first_line_is_header_with_policy() {
        let (conn, policy, actor) = setup();
        let jsonl = events_jsonl(&conn, &policy, &actor, "S-1", 0, 100, TraceLevel::Redacted)
            .expect("JSONL 导出");
        let mut lines = jsonl.lines();
        let header: serde_json::Value =
            serde_json::from_str(lines.next().expect("头部行")).expect("头部可解析");
        assert_eq!(header["trace_level"], "redacted");
        assert_eq!(header["count"], 1);
        assert_eq!(jsonl.lines().count(), 2, "头部 + 1 条事件");
    }
}
