//! # 事件导出（T065 核心、FR-202/FR-205）
//!
//! 事件按持久化游标查询并导出为 JSON/JSONL（CUI 的 [`crate::replay`] 与
//! `whcode-cui::events_cmd` 复用本模块——前端禁止触库，查询必须经应用服务，
//! ADR-003/005、C-03）。
//!
//! **脱敏策略随导出注明**（FR-205）：导出对象携带 `trace_level` 与 `redacted`
//! 标记；`redacted`/`metadata` 级别下载荷经 [`whcode_llm::trace::sanitize`] 处理，
//! `off` 级别不导出载荷正文（`payload` 置空对象并标注）。`full` 级别原样导出，
//! 调用方须自行承担访问控制与警告（UI 警告属前端职责）。

use duckdb::Connection;
use serde::Serialize;
use whcode_domain::{ErrorCode, TraceLevel, WhError};
use whcode_llm::trace::{self, IntegrityFlags};
use whcode_store::query::{self, EventRow};

use crate::authz::{Actor, Capability, Policy, require};

/// 导出单页上限（继承 store 分页上限）。
pub const EXPORT_PAGE_LIMIT: usize = 1000;

/// 事件导出对象（脱敏策略随对象注明，FR-205）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventsExport {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 会话范围。
    pub session_id: String,
    /// 本次导出的追踪级别。
    pub trace_level: String,
    /// 是否已脱敏（`redacted` 级别为真）。
    pub redacted: bool,
    /// 完整性标记（FR-208：脱敏/截断痕迹可查询）。
    pub integrity: IntegrityFlags,
    /// 事件（载荷已按级别处理）。
    pub events: Vec<EventRow>,
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 导出会话事件（FR-202/205）：按游标之后增量查询，载荷按追踪级别脱敏。
///
/// # Errors
///
/// 权限拒绝、`limit` 越界或存储失败时返回 [`WhError`]。
pub fn events_export(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    after_seq: i64,
    limit: usize,
    level: TraceLevel,
) -> Result<EventsExport, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if limit == 0 || limit > EXPORT_PAGE_LIMIT {
        return Err(invalid(&format!(
            "export limit 须在 1..={EXPORT_PAGE_LIMIT}，实际 {limit}"
        )));
    }
    let page = query::events_page(
        conn,
        session_id,
        whcode_store::Direction::Forward,
        Some(query::EventCursor { seq: after_seq }),
        limit,
    )?;

    let events = match level {
        TraceLevel::Full => page.items,
        TraceLevel::Redacted | TraceLevel::Metadata | TraceLevel::Off => page
            .items
            .into_iter()
            .map(|mut event| {
                let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json)
                    .map_err(|err| {
                        WhError::parse_error(format!("事件 {} 载荷损坏：{err}", event.id))
                    })?;
                event.payload_json = match level {
                    TraceLevel::Off => "{}".to_string(),
                    other => {
                        let sanitized = trace::sanitize(&payload, other)
                            .payload
                            .unwrap_or_else(|| "{}".into());
                        sanitized.to_string()
                    }
                };
                Ok(event)
            })
            .collect::<Result<Vec<EventRow>, WhError>>()?,
    };

    Ok(EventsExport {
        schema_version: whcode_domain::SCHEMA_VERSION,
        session_id: session_id.to_string(),
        trace_level: level.as_str().to_string(),
        redacted: level == TraceLevel::Redacted,
        integrity: IntegrityFlags {
            redacted: level == TraceLevel::Redacted,
            missing_chunks: false,
            downsampled: false,
            persistence_failed: false,
        },
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::params;
    use whcode_store::{apply_migrations, create_session_with_id};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn seed_events(conn: &Connection, session_id: &str) {
        for (seq, payload) in [
            (1, r#"{"api_key":"sk-secret1","note":"正文"}"#),
            (2, r#"{"prompt":"普通载荷"}"#),
        ] {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', ?,
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}"), session_id, payload],
            )
            .expect("插入事件");
        }
    }

    #[test]
    fn export_annotates_trace_policy_and_redacts_at_redacted_level() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "导出", "active", 1_000).expect("创建");
        seed_events(&conn, "S-1");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let export = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Redacted,
        )
        .expect("导出");
        assert_eq!(
            export.trace_level, "redacted",
            "脱敏策略必须随导出注明（FR-205）"
        );
        assert!(export.redacted);
        assert!(export.integrity.redacted);
        let serialized = serde_json::to_string(&export).expect("序列化");
        assert!(
            !serialized.contains("sk-secret1"),
            "redacted 下秘密不得出现在导出"
        );
        assert!(serialized.contains("正文"), "普通内容保留");

        let full = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Full,
        )
        .expect("full 导出");
        let full_serialized = serde_json::to_string(&full).expect("序列化");
        assert!(full_serialized.contains("sk-secret1"), "full 原样导出");

        let off = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Off,
        )
        .expect("off 导出");
        assert!(off.events.iter().all(|event| event.payload_json == "{}"));
    }

    #[test]
    fn export_is_incremental_from_cursor_and_bounded() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "增量", "active", 1_000).expect("创建");
        seed_events(&conn, "S-1");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let first =
            events_export(&conn, &policy, &actor, "S-1", 0, 1, TraceLevel::Full).expect("第一页");
        assert_eq!(first.events.len(), 1);
        let second = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            1,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Full,
        )
        .expect("游标增量");
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].seq, 2);

        let err = events_export(&conn, &policy, &actor, "S-1", 0, 0, TraceLevel::Full)
            .expect_err("limit=0");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT + 1,
            TraceLevel::Full,
        )
        .expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
