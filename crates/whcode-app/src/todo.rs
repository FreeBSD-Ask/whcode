//! # 会话待办读取面（T284／FR-771／ADR-048 修订）
//!
//! `todo_write` 的整表替换语义把“当前清单”定义为**最近一条 `TodoUpdated` 事件的载荷**；
//! 本模块把它解析成有界视图，供前端与工具侧核对。清单与 SDD 任务账本无关：不读 `tasks`、
//! 不产生任务、不参与门禁（隔离声明见 `whcode_tools::todo`）。
//!
//! 边界：SQL 只在 `whcode-store`（`query::latest_event_of_kind`）；本模块只做权限判定、
//! 解析与有界收口。

use std::collections::BTreeMap;

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

use crate::authz::{Actor, Capability, Policy, require};

/// 单个待办条目（内容 + 三态原文）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoEntry {
    /// 条目内容（写入时已去首尾空白，长度 ≤ `MAX_TODO_CONTENT_CHARS`）。
    pub content: String,
    /// 状态原文：`pending`／`in_progress`／`completed`。
    pub status: String,
}

/// 会话待办视图：最近一次整表快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoView {
    /// 快照事件序号（`None`＝该会话从未写过待办，是正确的空）。
    pub seq: Option<i64>,
    /// 条目（按快照顺序；三态）。
    pub items: Vec<TodoEntry>,
    /// 条目数超过上限时截断（正常路径不会发生：写入侧已在 50 条处拒绝）。
    pub truncated: bool,
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.into())
}

/// 读取会话当前待办（最近一次 `TodoUpdated`）。
///
/// # Errors
///
/// 权限拒绝、会话不存在（`not_found`）、载荷损坏（`parse_error`）或存储失败。
pub fn current(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<TodoView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    let Some((seq, payload)) =
        whcode_store::query::latest_event_of_kind(conn, session_id, TODO_EVENT_KIND)?
    else {
        return Ok(TodoView {
            seq: None,
            items: Vec::new(),
            truncated: false,
        });
    };
    let value: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|err| WhError::parse_error(format!("待办快照载荷损坏：{err}")))?;
    let raw_items = value
        .get("items")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("待办快照缺少 items 数组"))?;
    let mut items = Vec::with_capacity(raw_items.len().min(whcode_tools::MAX_TODO_ITEMS));
    for item in raw_items.iter().take(whcode_tools::MAX_TODO_ITEMS) {
        let content = item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("待办条目缺少 content"))?;
        let status = item
            .get("status")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid("待办条目缺少 status"))?;
        items.push(TodoEntry {
            content: content.to_string(),
            status: status.to_string(),
        });
    }
    Ok(TodoView {
        seq: Some(seq),
        truncated: raw_items.len() > items.len(),
        items,
    })
}

/// 快照事件类型原文（与 `whcode_domain::EventKind::TodoUpdated` 的稳定名一致）。
pub const TODO_EVENT_KIND: &str = "TodoUpdated";

/// 按状态统计条目数（未知状态归入 `other`，不丢弃计数）。
#[must_use]
pub fn counts_by_status(items: &[TodoEntry]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for item in items {
        *counts.entry(item.status.clone()).or_insert(0) += 1;
    }
    counts
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

    fn insert_snapshot(conn: &Connection, seq: i64, session: &str, kind: &str, payload: &str) {
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
    fn current_reads_latest_snapshot_and_distinguishes_empty() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "待办", "active", 1_000).expect("创建");
        create_session_with_id(&conn, "S-empty", "空", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let empty = current(&conn, &policy, &actor, "S-empty").expect("无待办是正确的空");
        assert_eq!(empty.seq, None);
        assert!(empty.items.is_empty());
        assert!(!empty.truncated);

        insert_snapshot(
            &conn,
            1,
            "S-1",
            TODO_EVENT_KIND,
            r#"{"items":[{"content":"旧","status":"pending"}],"pending":1,"in_progress":0,"completed":0}"#,
        );
        insert_snapshot(
            &conn,
            2,
            "S-1",
            TODO_EVENT_KIND,
            r#"{"items":[{"content":"写 ADR","status":"completed"},{"content":"补测试","status":"in_progress"}],"pending":0,"in_progress":1,"completed":1}"#,
        );
        // 其他类型事件不得被当作快照。
        insert_snapshot(&conn, 3, "S-1", "ProgressChanged", r#"{"items":[]}"#);

        let view = current(&conn, &policy, &actor, "S-1").expect("读取最近快照");
        assert_eq!(view.seq, Some(2), "整表替换：最新一条即当前状态");
        assert_eq!(
            view.items,
            vec![
                TodoEntry {
                    content: "写 ADR".to_string(),
                    status: "completed".to_string(),
                },
                TodoEntry {
                    content: "补测试".to_string(),
                    status: "in_progress".to_string(),
                },
            ]
        );
        let counts = counts_by_status(&view.items);
        assert_eq!(counts.get("completed"), Some(&1));
        assert_eq!(counts.get("in_progress"), Some(&1));

        let err = current(&conn, &policy, &actor, "S-404").expect_err("缺失会话");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn corrupted_snapshot_is_a_parse_error_not_an_empty_view() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "待办", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        insert_snapshot(&conn, 1, "S-1", TODO_EVENT_KIND, r#"{"items":"不是数组"}"#);
        let err = current(&conn, &policy, &actor, "S-1").expect_err("结构不符必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
