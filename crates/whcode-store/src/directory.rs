//! # 会话目录存储原语（M2/T046 支撑、ADR-018）
//!
//! 会话 CRUD 的 SQL **只**在本模块；`whcode-app` 的会话命令经这些原语编排，
//! 不内嵌 SQL（ADR-018）。归档复用 [`crate::purge::set_archived`]，删除复用
//! [`crate::purge`] 的预览/确认/审计/活跃保护全链（FR-509），本模块不做删除。
//!
//! 会话 ID 由 schema v4 的序列 `whcode_session_id_seq` 自动分配（`S-` 前缀），
//! 单调且跨重启不重复；调用方不提供 ID。
//!
//! 边界：只接受 [`duckdb::Connection`]；业务规则与权限判定属 `whcode-app`。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

use crate::query;

/// 会话行（`sessions` 表只读投影；时间戳以 Unix 毫秒暴露）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SessionRow {
    /// 会话 ID（`S-` 前缀，序列自动分配）。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 创建时间（Unix 毫秒）。
    pub created_at_ms: i64,
    /// 最近活跃时间（Unix 毫秒）。
    pub last_active_at_ms: i64,
    /// 模型标识（M2 允许为空）。
    pub model: Option<String>,
    /// 是否已归档；归档会话不参与活跃尾部（data-model.md §1）。
    pub archived: bool,
    /// 状态原文（M2 取值 `active`；状态机属 SDD/M4）。
    pub status: String,
    /// SDD 阶段（v5 列；稳定名由 `whcode-speckit::workflow::SddStage` 定义，M4）。
    /// v5 列为可空（DuckDB ALTER 限制），NULL 由本层兜底为 `specify`。
    pub stage: String,
}

fn dir_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

fn row_to_session(row: &duckdb::Row<'_>) -> duckdb::Result<SessionRow> {
    Ok(SessionRow {
        id: row.get(0)?,
        title: row.get(1)?,
        created_at_ms: row.get::<_, i64>(2)?.div_euclid(1000),
        last_active_at_ms: row.get::<_, i64>(3)?.div_euclid(1000),
        model: row.get(4)?,
        archived: row.get(5)?,
        status: row.get(6)?,
        stage: row
            .get::<_, Option<String>>(7)?
            .unwrap_or_else(|| "specify".to_string()),
    })
}

const SESSION_COLUMNS: &str = "id, title, epoch_us(created_at), epoch_us(last_active_at),                                model, archived, status, stage";

/// 创建会话，返回含自动分配 ID 的完整行。
///
/// # Errors
///
/// 标题或状态为空、数据库写入失败时返回 [`WhError`]。
pub fn create_session(
    conn: &Connection,
    title: &str,
    model: Option<&str>,
    status: &str,
    now_ms: u64,
) -> Result<SessionRow, WhError> {
    if title.trim().is_empty() {
        return Err(invalid("会话标题不得为空"));
    }
    if status.trim().is_empty() {
        return Err(invalid("会话状态不得为空"));
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let id: String = conn
        .query_row(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
             VALUES ('S-' || nextval('whcode_session_id_seq'), ?, epoch_ms(?), epoch_ms(?), ?, FALSE, ?)
             RETURNING id",
            params![title, now, now, model, status],
            |row| row.get(0),
        )
        .map_err(|err| dir_err("创建会话", err))?;
    Ok(SessionRow {
        id,
        title: title.to_string(),
        created_at_ms: now,
        last_active_at_ms: now,
        model: model.map(str::to_string),
        archived: false,
        status: status.to_string(),
        stage: "specify".to_string(),
    })
}

/// 以调用方给定的 ID 创建会话（数据集夹具/导入路径）；ID 重复即约束错误。
///
/// # Errors
///
/// ID/标题/状态为空或数据库写入失败时返回 [`WhError`]。
pub fn create_session_with_id(
    conn: &Connection,
    id: &str,
    title: &str,
    status: &str,
    now_ms: u64,
) -> Result<SessionRow, WhError> {
    if id.trim().is_empty() {
        return Err(invalid("会话 ID 不得为空"));
    }
    if title.trim().is_empty() {
        return Err(invalid("会话标题不得为空"));
    }
    if status.trim().is_empty() {
        return Err(invalid("会话状态不得为空"));
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
         VALUES (?, ?, epoch_ms(?), epoch_ms(?), NULL, FALSE, ?)",
        params![id, title, now, now, status],
    )
    .map_err(|err| dir_err("创建会话", err))?;
    Ok(SessionRow {
        id: id.to_string(),
        title: title.to_string(),
        created_at_ms: now,
        last_active_at_ms: now,
        model: None,
        archived: false,
        status: status.to_string(),
        stage: "specify".to_string(),
    })
}

fn load_one(conn: &Connection, sql: &str, id: &str) -> Result<Option<SessionRow>, WhError> {
    let mut stmt = conn.prepare(sql).map_err(|err| dir_err("查询会话", err))?;
    let mut rows = stmt
        .query(params![id])
        .map_err(|err| dir_err("查询会话", err))?;
    match rows.next().map_err(|err| dir_err("查询会话", err))? {
        Some(row) => Ok(Some(
            row_to_session(row).map_err(|err| dir_err("读取会话", err))?,
        )),
        None => Ok(None),
    }
}

/// 读取单个会话；不存在返回 `None`。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn get_session(conn: &Connection, id: &str) -> Result<Option<SessionRow>, WhError> {
    load_one(
        conn,
        &format!("SELECT {SESSION_COLUMNS} FROM sessions WHERE id = ?"),
        id,
    )
}

/// 列出会话（元数据级行，最新在前），`include_archived` 控制是否含归档会话。
///
/// `limit` 由调用方给定并受 [`MAX_LIST_LIMIT`] 约束——列表有界，不做全量兜底。
///
/// # Errors
///
/// `limit` 越界或查询失败时返回 [`WhError`]。
pub const MAX_LIST_LIMIT: usize = 1000;

pub fn list_sessions(
    conn: &Connection,
    include_archived: bool,
    limit: usize,
) -> Result<Vec<SessionRow>, WhError> {
    if limit == 0 || limit > MAX_LIST_LIMIT {
        return Err(invalid(&format!(
            "list limit 须在 1..={MAX_LIST_LIMIT}，实际 {limit}"
        )));
    }
    let filter = if include_archived {
        ""
    } else {
        " WHERE archived = FALSE"
    };
    let sql = format!(
        "SELECT {SESSION_COLUMNS} FROM sessions{filter}
         ORDER BY created_at DESC, id DESC LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql).map_err(|err| dir_err("列出会话", err))?;
    let mut rows = stmt.query([]).map_err(|err| dir_err("列出会话", err))?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().map_err(|err| dir_err("列出会话", err))? {
        sessions.push(row_to_session(row).map_err(|err| dir_err("读取会话", err))?);
    }
    Ok(sessions)
}

/// 重命名会话；不存在返回 `not_found`。
///
/// # Errors
///
/// 会话不存在、标题为空或数据库更新失败时返回 [`WhError`]。
pub fn rename_session(conn: &Connection, id: &str, new_title: &str) -> Result<(), WhError> {
    if new_title.trim().is_empty() {
        return Err(invalid("会话标题不得为空"));
    }
    let affected = conn
        .execute(
            "UPDATE sessions SET title = ? WHERE id = ?",
            params![new_title, id],
        )
        .map_err(|err| dir_err("重命名会话", err))?;
    if affected == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{id}"),
        ));
    }
    Ok(())
}

/// 更新会话最近活跃时间；不存在返回 `not_found`。
///
/// # Errors
///
/// 会话不存在或数据库更新失败时返回 [`WhError`]。
pub fn touch_session(conn: &Connection, id: &str, now_ms: u64) -> Result<(), WhError> {
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let affected = conn
        .execute(
            "UPDATE sessions SET last_active_at = epoch_ms(?) WHERE id = ?",
            params![now, id],
        )
        .map_err(|err| dir_err("更新会话活跃时间", err))?;
    if affected == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{id}"),
        ));
    }
    Ok(())
}

/// 更新会话 SDD 阶段（M4/T074 状态机转换的存储原语）。
///
/// 阶段取值的合法性由调用方（`whcode-speckit::workflow::SddStage::parse`）校验，
/// 本原语只做持久化；不存在返回 `not_found`。
///
/// # Errors
///
/// 会话不存在或数据库更新失败时返回 [`WhError`]。
pub fn update_stage(conn: &Connection, id: &str, stage: &str) -> Result<(), WhError> {
    let affected = conn
        .execute(
            "UPDATE sessions SET stage = ? WHERE id = ?",
            params![stage, id],
        )
        .map_err(|err| dir_err("更新会话阶段", err))?;
    if affected == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{id}"),
        ));
    }
    Ok(())
}

/// 工作区事件游标：`events.seq` 的当前最大值；空表为 0。
///
/// 快照的 `event_cursor`（FR-314）与断线补拉（FR-605）以该值为基线。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn event_cursor(conn: &Connection) -> Result<i64, WhError> {
    query::latest_seq(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn create_assigns_monotonic_prefixed_ids() {
        let conn = migrated();
        let first = create_session(&conn, "一", None, "active", 1_000).expect("创建");
        let second = create_session(&conn, "二", Some("gpt-x"), "active", 2_000).expect("创建");
        assert!(first.id.starts_with("S-"), "{first:?}");
        assert!(second.id.starts_with("S-"));
        assert_ne!(first.id, second.id, "序列必须保证 ID 唯一");
        assert_eq!(first.created_at_ms, 1_000);
        assert_eq!(second.model.as_deref(), Some("gpt-x"));

        let loaded = get_session(&conn, &first.id).expect("读取").expect("存在");
        assert_eq!(loaded.title, "一");
        assert_eq!(loaded.status, "active");
        assert!(!loaded.archived);
    }

    #[test]
    fn create_rejects_empty_title_and_status() {
        let conn = migrated();
        for (title, status) in [("", "active"), ("  ", "active"), ("x", "")] {
            let err =
                create_session(&conn, title, None, status, 1).expect_err("空标题/状态必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState);
        }
    }

    #[test]
    fn list_orders_newest_first_and_filters_archived() {
        let mut conn = migrated();
        let a = create_session(&conn, "a", None, "active", 1_000).expect("a");
        let b = create_session(&conn, "b", None, "active", 2_000).expect("b");
        crate::purge::set_archived(&mut conn, &[a.id.as_str()], true, "directory-test")
            .expect("归档");

        let visible = list_sessions(&conn, false, 10).expect("列表");
        assert_eq!(
            visible.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec![b.id.as_str()]
        );

        let all = list_sessions(&conn, true, 10).expect("含归档列表");
        assert_eq!(all.len(), 2, "归档会话须在 include_archived 时出现");
        assert_eq!(all[0].id, b.id, "最新在前");

        let err = list_sessions(&conn, true, 0).expect_err("limit=0 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = list_sessions(&conn, true, MAX_LIST_LIMIT + 1).expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn rename_and_touch_report_missing_sessions() {
        let conn = migrated();
        let s = create_session(&conn, "旧名", None, "active", 1_000).expect("创建");

        rename_session(&conn, &s.id, "新名").expect("重命名");
        let loaded = get_session(&conn, &s.id).expect("读取").expect("存在");
        assert_eq!(loaded.title, "新名");

        touch_session(&conn, &s.id, 9_000).expect("更新活跃时间");
        let loaded = get_session(&conn, &s.id).expect("读取").expect("存在");
        assert_eq!(loaded.last_active_at_ms, 9_000);

        let err = rename_session(&conn, "S-missing", "x").expect_err("缺失会话必须 not_found");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = touch_session(&conn, "S-missing", 1).expect_err("缺失会话必须 not_found");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = rename_session(&conn, &s.id, "  ").expect_err("空标题必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn event_cursor_reflects_max_seq() {
        let conn = migrated();
        assert_eq!(event_cursor(&conn).expect("空库游标"), 0);
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (7, 'E-7', NULL, NULL, NULL, NULL, 'SessionStarted', CAST('{}' AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            [],
        )
        .expect("插入事件");
        assert_eq!(event_cursor(&conn).expect("游标"), 7);
    }
}
