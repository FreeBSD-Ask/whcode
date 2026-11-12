//! # 交付物读取面（T285／FR-772／ADR-048 §7.2）
//!
//! `present_deliverables` 的登记是**累加型事实**：每次成功调用落一条
//! `DeliverablePresented` 事件。本模块把本会话全部登记聚合成有界视图：按首次登记顺序
//! 去重（同一路径重复登记只出现一次），保留首次登记的 `seq` 与产生它的 `tool_call_id`，
//! 便于审计回查。
//!
//! 边界：SQL 只在 `whcode-store`（`query::events_of_kind`）；本模块只做权限判定、解析与
//! 有界收口（事件读取 ≤ [`MAX_DELIVERABLE_EVENTS`]，条目 ≤ [`MAX_DELIVERABLE_ENTRIES`]）。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

use crate::authz::{Actor, Capability, Policy, require};

/// 读取的事件条数上限（有界；超出标 `truncated`，不静默丢）。
pub const MAX_DELIVERABLE_EVENTS: usize = 200;

/// 视图条目上限（去重后的路径数）。
pub const MAX_DELIVERABLE_ENTRIES: usize = 200;

/// 单个交付物条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deliverable {
    /// 工作区内相对路径（登记时已规范化）。
    pub path: String,
    /// 首次登记的 `seq`。
    pub seq: i64,
    /// 产生登记的 `tool_call_id`（缺失时为空串——旧夹具可能不带）。
    pub tool_call_id: String,
}

/// 交付物视图（按首次登记顺序去重）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverableView {
    /// 交付物（按首次登记顺序）。
    pub items: Vec<Deliverable>,
    /// 事件读取达到上限（可能存在更早或更多的登记未纳入）。
    pub truncated: bool,
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.into())
}

/// 读取会话交付物清单（累加去重）。
///
/// # Errors
///
/// 权限拒绝、会话不存在（`not_found`）、载荷损坏（`parse_error`）或存储失败。
pub fn list(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<DeliverableView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    let events = whcode_store::query::events_of_kind(
        conn,
        session_id,
        DELIVERABLE_EVENT_KIND,
        MAX_DELIVERABLE_EVENTS,
    )?;
    let truncated_events = events.len() == MAX_DELIVERABLE_EVENTS;
    let mut items: Vec<Deliverable> = Vec::new();
    let mut truncated = truncated_events;
    for (seq, payload) in events {
        let value: serde_json::Value = serde_json::from_str(&payload)
            .map_err(|err| WhError::parse_error(format!("交付物事件载荷损坏：{err}")))?;
        let paths = value
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid("交付物事件缺少 paths 数组"))?;
        let tool_call_id = value
            .get("tool_call_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        for path in paths {
            let Some(path) = path.as_str() else {
                return Err(invalid("交付物路径必须为字符串"));
            };
            if items.iter().any(|existing| existing.path == path) {
                continue;
            }
            if items.len() == MAX_DELIVERABLE_ENTRIES {
                truncated = true;
                break;
            }
            items.push(Deliverable {
                path: path.to_string(),
                seq,
                tool_call_id: tool_call_id.clone(),
            });
        }
    }
    Ok(DeliverableView { items, truncated })
}

/// 事件类型原文（与 `whcode_domain::EventKind::DeliverablePresented` 的稳定名一致）。
pub const DELIVERABLE_EVENT_KIND: &str = "DeliverablePresented";

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

    fn insert(conn: &Connection, seq: i64, session: &str, kind: &str, payload: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, ?, CAST(? AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            params![seq, format!("EV-{seq}"), session, kind, payload],
        )
        .expect("插入事件");
    }

    #[test]
    fn list_accumulates_dedups_in_first_seen_order_and_distinguishes_empty() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "交付", "active", 1_000).expect("创建");
        create_session_with_id(&conn, "S-empty", "空", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let empty = list(&conn, &policy, &actor, "S-empty").expect("无交付物是正确的空");
        assert!(empty.items.is_empty());
        assert!(!empty.truncated);

        insert(
            &conn,
            1,
            "S-1",
            DELIVERABLE_EVENT_KIND,
            r#"{"paths":["out/a.md","out/b.md"],"count":2,"tool_call_id":"call-1"}"#,
        );
        insert(
            &conn,
            2,
            "S-1",
            DELIVERABLE_EVENT_KIND,
            r#"{"paths":["out/b.md","out/c.md"],"count":2,"tool_call_id":"call-2"}"#,
        );
        // 其他类型事件不得被误读。
        insert(&conn, 3, "S-1", "TodoUpdated", r#"{"paths":["out/x.md"]}"#);

        let view = list(&conn, &policy, &actor, "S-1").expect("聚合");
        let paths: Vec<&str> = view.items.iter().map(|item| item.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["out/a.md", "out/b.md", "out/c.md"],
            "累加、按首次登记顺序、重复路径只出现一次"
        );
        assert_eq!(view.items[1].seq, 1, "保留首次登记 seq");
        assert_eq!(view.items[1].tool_call_id, "call-1");
        assert_eq!(view.items[2].seq, 2);

        let err = list(&conn, &policy, &actor, "S-404").expect_err("缺失会话");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn corrupted_payload_and_oversized_reads_are_explicit() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "交付", "active", 1_000).expect("创建");
        create_session_with_id(&conn, "S-2", "多", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        insert(
            &conn,
            1,
            "S-1",
            DELIVERABLE_EVENT_KIND,
            r#"{"paths":"不是数组"}"#,
        );
        let err = list(&conn, &policy, &actor, "S-1").expect_err("结构不符必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 事件数达到上限 → truncated（有界读取不静默丢）。
        for index in 1..=MAX_DELIVERABLE_EVENTS as i64 {
            // seq 全局唯一：S-1 已占用 1，这里从 2 起。
            let seq = index + 1;
            let payload =
                format!(r#"{{"paths":["out/{seq}.md"],"count":1,"tool_call_id":"call-{seq}"}}"#);
            insert(&conn, seq, "S-2", DELIVERABLE_EVENT_KIND, &payload);
        }
        let view = list(&conn, &policy, &actor, "S-2").expect("上限本身可用");
        assert!(view.truncated, "达到事件上限须标 truncated");
        assert_eq!(view.items.len(), MAX_DELIVERABLE_EVENTS);
    }
}
