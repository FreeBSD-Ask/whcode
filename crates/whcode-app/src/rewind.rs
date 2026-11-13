//! # 对话剪枝应用面（T288／FR-775／ADR-068）
//!
//! 回退是**边界事件**而非删除：最新一条 `SessionRewound` 给出 `(target_seq, rewind_seq)`，
//! 历史有效区为 `s <= target_seq || s > rewind_seq`。事件与消息全部保留（导出、审计、
//! 三态视图仍完整），只有模型上下文与对话尾按有效区派生。
//!
//! 边界：SQL 只在 `whcode-store`（`rewind::record_rewind`／`latest_rewind`）；本模块只做
//! 权限判定、会话存在性校验与派生过滤。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

use crate::authz::{Actor, Capability, Policy, require};

/// 剪枝视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindView {
    /// 有效区上界（`0`＝回退到会话起点；`None`＝从未回退）。
    pub target_seq: Option<i64>,
    /// 最新一条回退事件自身的 `seq`（其后的新事件属于有效区）。
    pub rewind_seq: Option<i64>,
}

/// 读取当前剪枝边界。
///
/// # Errors
///
/// 权限拒绝、会话不存在（`not_found`）、载荷损坏（`parse_error` 或 `invalid_state`）。
pub fn current(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<RewindView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    match whcode_store::rewind::latest_rewind(conn, session_id)? {
        Some((target, rewind)) => Ok(RewindView {
            target_seq: Some(target),
            rewind_seq: Some(rewind),
        }),
        None => Ok(RewindView {
            target_seq: None,
            rewind_seq: None,
        }),
    }
}

/// 回退对话到指定 turn 边界（用户侧通道）。
///
/// # Errors
///
/// 权限拒绝（需 `session.rewind`）、会话不存在、目标不是合法边界（`invalid_state`，
/// 消息可操作）或写入失败。
pub fn rewind(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    to_seq: i64,
) -> Result<RewindView, WhError> {
    require(policy, actor, Capability::SessionRewind)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    whcode_store::rewind::record_rewind(conn, session_id, to_seq, "user")?;
    current(conn, policy, actor, session_id)
}

/// 读取派生过滤所需的边界（不做权限判定；调用方已持有会话上下文）。
///
/// # Errors
///
/// 存储读取失败或载荷损坏。
pub fn active_boundary(conn: &Connection, session_id: &str) -> Result<Option<(i64, i64)>, WhError> {
    whcode_store::rewind::latest_rewind(conn, session_id)
}

/// 序号是否落在有效区（`s <= target || s > rewind_seq`；无边界＝全部有效）。
#[must_use]
pub fn seq_is_active(seq: i64, boundary: Option<(i64, i64)>) -> bool {
    match boundary {
        None => true,
        Some((target, rewind)) => seq <= target || seq > rewind,
    }
}

/// 从消息 ID（`MSG-<n>`）解析序号；非该形态返回 `None`（外部夹具不参与过滤判定）。
#[must_use]
pub fn message_seq(id: &str) -> Option<i64> {
    id.strip_prefix("MSG-").and_then(|rest| rest.parse().ok())
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

    fn turn_finished(conn: &Connection, seq: i64, session: &str) {
        insert(
            conn,
            seq,
            session,
            "TurnFinished",
            r#"{"termination_reason":"model_finished"}"#,
        );
    }

    #[test]
    fn rewind_requires_turn_boundary_and_moves_backwards() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "回退", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        insert(&conn, 1, "S-1", "TurnStarted", r#"{}"#);
        turn_finished(&conn, 2, "S-1");
        insert(&conn, 3, "S-1", "TurnStarted", r#"{}"#);
        turn_finished(&conn, 4, "S-1");

        let err = rewind(&conn, &policy, &actor, "S-1", 4).expect_err("回退到末尾是空操作");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("无操作"), "{err}");

        let err = rewind(&conn, &policy, &actor, "S-1", 1).expect_err("半截 turn 不是边界");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("TurnFinished"), "{err}");

        let err = rewind(&conn, &policy, &actor, "S-1", 99).expect_err("不存在");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let view = rewind(&conn, &policy, &actor, "S-1", 2).expect("合法边界");
        assert_eq!(view.target_seq, Some(2));
        assert_eq!(view.rewind_seq, Some(5));

        let boundary = active_boundary(&conn, "S-1").expect("边界可读");
        assert_eq!(boundary, Some((2, 5)));
        // 有效区：2 及其之前 + 回退事件之后（目前为空）。
        assert!(seq_is_active(1, boundary));
        assert!(seq_is_active(2, boundary));
        assert!(!seq_is_active(3, boundary));
        assert!(!seq_is_active(4, boundary));
        assert!(
            seq_is_active(6, boundary),
            "回退事件之后新增的 turn 属于有效区"
        );
    }

    #[test]
    fn rewind_requires_write_capability_and_rejects_missing_session() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "回退", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let weak = Actor::new("weak", []);
        let err = rewind(&conn, &policy, &weak, "S-1", 0).expect_err("无能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        let actor = actor_with_all_capabilities("tester");
        let err = rewind(&conn, &policy, &actor, "S-404", 0).expect_err("缺失会话");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn corrupted_boundary_is_not_silently_ignored() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "回退", "active", 1_000).expect("创建");
        insert(
            &conn,
            1,
            "S-1",
            "SessionRewound",
            r#"{"requested_by":"user"}"#,
        );
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let err = current(&conn, &policy, &actor, "S-1").expect_err("缺 target_seq 必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn message_seq_parses_only_stable_ids() {
        assert_eq!(message_seq("MSG-42"), Some(42));
        assert_eq!(message_seq("MSG-x"), None);
        assert_eq!(message_seq("EV-42"), None);
    }
}
