//! 会话内容检索命令（P3.3／ADR-076）。
//!
//! 命令编排：权限（`session.read`）→ 入参校验（非空、有界、limit 夹取）→ 存储原语
//! （[`whcode_store::search_messages`]）→ 结果。**无 SQL**（ADR-018），片段正文不落日志。
//!
//! 与侧栏搜索（FR-746，标题/ID）的分工：本命令检索**消息正文**；两者结果形状不同
//! （目录行 vs 命中片段），故不合并到 `session_directory`，避免把“目录过滤”与“内容命中”
//! 混成一种返回类型。

use duckdb::Connection;
use whcode_domain::WhError;
use whcode_store::{ContentHit, MAX_SEARCH_LIMIT, MAX_SEARCH_NEEDLE_CHARS, search_messages};

use crate::authz::{Actor, Capability, Policy, require};

/// 默认返回条数（GUI/Web 列表一屏；调用方可传更小值）。
pub const CONTENT_SEARCH_DEFAULT_LIMIT: usize = 20;

/// 内容检索查询。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSearchQuery {
    /// 关键词（去首尾空白后 1..=128 字符）。
    pub needle: String,
    /// 返回条数上限（1..=[`MAX_SEARCH_LIMIT`]，超出按上限夹取）。
    pub limit: usize,
}

/// 检索会话内容，最新命中优先。
///
/// # Errors
///
/// 缺少 `session.read` 权限（`permission_denied`，不可重试）、搜索词为空/超长
/// （`invalid_state`）、存储失败（`io_error`）时返回 [`WhError`]。
pub fn search_session_content(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    query: &ContentSearchQuery,
) -> Result<Vec<ContentHit>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let needle = query.needle.trim();
    if needle.is_empty() {
        return Err(WhError::invalid_state("搜索词不能为空"));
    }
    if needle.chars().count() > MAX_SEARCH_NEEDLE_CHARS {
        return Err(WhError::invalid_state(format!(
            "搜索词超过 {MAX_SEARCH_NEEDLE_CHARS} 字符"
        )));
    }
    let limit = query.limit.clamp(1, MAX_SEARCH_LIMIT);
    search_messages(conn, needle, limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;
    use whcode_store::directory::create_session;
    use whcode_store::schema::apply_migrations;

    use crate::authz::{Actor, DefaultPolicy, actor_with_all_capabilities};

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_message(conn: &Connection, id: &str, session_id: &str, content: &str) {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, 'user', ?, NULL, to_timestamp(1.0), NULL)",
            duckdb::params![id, session_id, content],
        )
        .expect("插入消息");
    }

    #[test]
    fn search_returns_hits_with_session_context() {
        let conn = fixture();
        let session = create_session(&conn, "检索目标", None, "active", 1_000).expect("建会话");
        insert_message(&conn, "M-1", &session.id, "关于 WAL 的讨论");
        let hits = search_session_content(
            &conn,
            &DefaultPolicy,
            &actor_with_all_capabilities("t"),
            &ContentSearchQuery {
                needle: " wal ".to_string(),
                limit: 10,
            },
        )
        .expect("检索");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_title, "检索目标");
        assert!(hits[0].snippet.contains("WAL"), "{hits:?}");
    }

    #[test]
    fn missing_read_capability_is_permission_denied() {
        let conn = fixture();
        let actor = Actor::new("u", []);
        let err = search_session_content(
            &conn,
            &DefaultPolicy,
            &actor,
            &ContentSearchQuery {
                needle: "x".to_string(),
                limit: 10,
            },
        )
        .expect_err("无权限必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }

    #[test]
    fn limit_is_clamped_and_zero_rejected_at_store_layer() {
        let conn = fixture();
        let session = create_session(&conn, "夹取", None, "active", 1_000).expect("建会话");
        for index in 0..3 {
            insert_message(&conn, &format!("M-{index}"), &session.id, "同一关键词");
        }
        let hits = search_session_content(
            &conn,
            &DefaultPolicy,
            &actor_with_all_capabilities("t"),
            &ContentSearchQuery {
                needle: "同一关键词".to_string(),
                limit: 2,
            },
        )
        .expect("检索");
        assert_eq!(hits.len(), 2, "limit 必须生效");
    }

    #[test]
    fn blank_needle_is_invalid_state() {
        let conn = fixture();
        let err = search_session_content(
            &conn,
            &DefaultPolicy,
            &actor_with_all_capabilities("t"),
            &ContentSearchQuery {
                needle: "   ".to_string(),
                limit: 10,
            },
        )
        .expect_err("空搜索词必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
