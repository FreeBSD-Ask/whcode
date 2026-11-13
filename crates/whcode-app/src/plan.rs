//! # 计划模式应用面（T287／FR-774／ADR-067）
//!
//! 计划模式是**会话状态**：最新一条 `PlanModeChanged` 事件即当前值（log-only，可重放，
//! 分叉与恢复自动继承）。本模块提供读取、用户侧切换与 turn 侧的引导判定。
//!
//! 语义边界（ADR-067 决策 2／6）：软引导，不阻止任何工具调用；不产生 SDD 任务、不参与门禁。
//!
//! 边界：SQL 只在 `whcode-store`（`plan::record_mode`／`plan::current_mode`）；
//! 本模块只做权限判定、会话存在性校验与视图封装。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

use crate::authz::{Actor, Capability, Policy, require};

/// 计划模式视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    /// 当前是否处于计划模式。
    pub active: bool,
    /// 决定当前状态的事件序号（`None`＝该会话从未切换过，视为关闭且是正确的空）。
    pub seq: Option<i64>,
}

/// 读取会话计划模式状态（最新一条 `PlanModeChanged`）。
///
/// # Errors
///
/// 权限拒绝（`permission_denied`）、会话不存在（`not_found`）、载荷损坏（`parse_error`）。
pub fn current(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<PlanView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    match whcode_store::plan::current_mode(conn, session_id)? {
        Some((seq, active)) => Ok(PlanView {
            active,
            seq: Some(seq),
        }),
        None => Ok(PlanView {
            active: false,
            seq: None,
        }),
    }
}

/// 用户侧切换计划模式（CUI `session plan`／Web `POST /api/plan`）。
///
/// 用户即决策者，**不经审批**（ADR-067 决策 4）；仍落 `PlanModeChanged`（`source = user`），
/// 因此切换可重放、可审计。
///
/// # Errors
///
/// 权限拒绝、会话不存在、写入失败或读回不一致时返回错误（不伪造成功）。
pub fn set(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    active: bool,
) -> Result<PlanView, WhError> {
    require(policy, actor, Capability::SessionPlan)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    whcode_store::plan::record_mode(conn, session_id, active, "user", None)?;
    current(conn, policy, actor, session_id)
}

/// turn 是否处于计划模式（注入引导段的判定；调用方已持有连接与权限上下文）。
///
/// 读取失败按“未激活”处理并**不吞掉错误**的语义：由调用方决定是否继续 turn——
/// turn 把此处失败视为装配缺口，记录 `PersistenceError` 后继续（引导缺失不阻断对话）。
///
/// # Errors
///
/// 存储读取或载荷解析失败。
pub fn is_active(conn: &Connection, session_id: &str) -> Result<bool, WhError> {
    Ok(whcode_store::plan::current_mode(conn, session_id)?.is_some_and(|(_, active)| active))
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

    #[test]
    fn current_reads_latest_state_and_distinguishes_never_toggled() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "计划", "active", 1_000).expect("创建");
        create_session_with_id(&conn, "S-empty", "空", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let empty = current(&conn, &policy, &actor, "S-empty").expect("从未切换是正确的空");
        assert!(!empty.active);
        assert_eq!(empty.seq, None);

        insert(
            &conn,
            1,
            "S-1",
            "PlanModeChanged",
            r#"{"active":true,"source":"user"}"#,
        );
        insert(
            &conn,
            2,
            "S-1",
            "PlanModeChanged",
            r#"{"active":false,"source":"model","tool_call_id":"call-1"}"#,
        );
        // 其他类型事件不得被当作状态。
        insert(&conn, 3, "S-1", "TodoUpdated", r#"{"active":true}"#);
        let view = current(&conn, &policy, &actor, "S-1").expect("最新一条即当前状态");
        assert!(!view.active, "整值替换：最新一条为当前值");
        assert_eq!(view.seq, Some(2));

        let err = current(&conn, &policy, &actor, "S-404").expect_err("缺失会话");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn user_toggle_requires_write_capability_and_records_source_user() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "计划", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let view = set(&conn, &policy, &actor, "S-1", true).expect("开启");
        assert!(view.active);
        assert_eq!(view.seq, Some(1));
        let source: String = conn
            .query_row(
                "SELECT CAST(CAST(payload_json AS JSON)->>'$.source' AS VARCHAR) FROM events
                 WHERE session_id='S-1' AND kind='PlanModeChanged' ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("载荷");
        assert_eq!(source, "user", "用户通道来源必须是 user");
        let off = set(&conn, &policy, &actor, "S-1", false).expect("关闭");
        assert!(!off.active);
        assert_eq!(off.seq, Some(2));

        let weak = Actor::new("weak", []);
        let err = set(&conn, &policy, &weak, "S-1", true).expect_err("无能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }

    #[test]
    fn corrupted_state_is_a_parse_error_not_silently_off() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "计划", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        insert(&conn, 1, "S-1", "PlanModeChanged", r#"{"active":"yes"}"#);
        let err = current(&conn, &policy, &actor, "S-1").expect_err("结构不符必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = is_active(&conn, "S-1").expect_err("损坏状态不得静默按关闭处理");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
