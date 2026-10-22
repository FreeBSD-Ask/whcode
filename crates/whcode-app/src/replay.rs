//! # 断线补拉与重连（T066、FR-202/FR-605）
//!
//! [`replay_since`] 按持久化游标增量补拉事件（`events.seq` 排他游标，与在线流
//! 同一全序），返回有界页与游标。补拉语义（FR-605）：断线端以游标为基线，把
//! 游标之后的事件补齐——补拉结果与在线流的**同一事件集合**一致（V-103）。
//!
//! 边界：只读；不修改游标状态（游标由调用方持有并推进）。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};
use whcode_store::query::{self, EventCursor, EventRow};

use crate::authz::{Actor, Capability, Policy, require};

/// 单次补拉默认上限（继承 store 分页上限）。
pub const REPLAY_PAGE_LIMIT: usize = 1000;

/// 补拉页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPage {
    /// 游标之后的事件（按 `seq` 升序）。
    pub events: Vec<EventRow>,
    /// 继续补拉的游标；`None` 表示已到头（补拉完整）。
    pub next_cursor: Option<EventCursor>,
    /// 已到头（本轮补拉覆盖了游标之后的全部事件）。
    pub complete: bool,
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 按持久化游标补拉事件（FR-202/605）。
///
/// # Errors
///
/// 权限拒绝、`limit` 越界或存储失败时返回 [`WhError`]。
pub fn replay_since(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    after_seq: i64,
    limit: usize,
) -> Result<ReplayPage, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if limit == 0 || limit > REPLAY_PAGE_LIMIT {
        return Err(invalid(&format!(
            "replay limit 须在 1..={REPLAY_PAGE_LIMIT}，实际 {limit}"
        )));
    }
    let page = query::events_page(
        conn,
        session_id,
        whcode_store::Direction::Forward,
        Some(EventCursor { seq: after_seq }),
        limit,
    )?;
    Ok(ReplayPage {
        complete: page.next.is_none(),
        next_cursor: page.next,
        events: page.items,
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

    fn seed(conn: &Connection, session_id: &str, count: i64) {
        for seq in 1..=count {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', '{}',
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}"), session_id],
            )
            .expect("插入事件");
        }
    }

    #[test]
    fn replay_fetches_events_after_cursor_and_reports_completion() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "补拉", "active", 1_000).expect("创建");
        seed(&conn, "S-1", 10);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        // 客户端在 seq=3 处断线：补拉 4..=10。
        let page = replay_since(&conn, &policy, &actor, "S-1", 3, REPLAY_PAGE_LIMIT).expect("补拉");
        assert_eq!(page.events.len(), 7);
        assert_eq!(page.events[0].seq, 4, "补拉从游标之后开始（排他）");
        assert!(page.complete);
        assert!(page.next_cursor.is_none());

        // 有界补拉：limit=3 → 未到头，游标指向本页最后事件。
        let page = replay_since(&conn, &policy, &actor, "S-1", 3, 3).expect("有界补拉");
        assert_eq!(page.events.len(), 3);
        assert!(!page.complete);
        assert_eq!(page.next_cursor.expect("游标").seq, 6);

        // 游标之后无事件：空页且完整。
        let page =
            replay_since(&conn, &policy, &actor, "S-1", 10, REPLAY_PAGE_LIMIT).expect("空补拉");
        assert!(page.events.is_empty());
        assert!(page.complete);

        let err = replay_since(&conn, &policy, &actor, "S-1", 0, 0).expect_err("limit=0");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn replay_requires_capability() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "补拉", "active", 1_000).expect("创建");
        seed(&conn, "S-1", 3);
        let policy = DefaultPolicy;
        let actor = crate::authz::Actor::new("无能力", []);
        let err = replay_since(&conn, &policy, &actor, "S-1", 0, REPLAY_PAGE_LIMIT)
            .expect_err("无能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }
}
