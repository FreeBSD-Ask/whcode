//! # 任务存储原语（M2/T048—T052 支撑、ADR-018）
//!
//! 任务投影与状态变更的 SQL **只**在本模块；`whcode-app` 的任务命令经这些原语编排。
//! 读侧纪律：
//!
//! - 列表/详情一律**有界**（常量上限，无分页兜底），不使用 `OFFSET`；
//! - 产物只取**引用元数据**，不取 `content` 大对象列（R-05 引用化）；
//! - `status` 列按 domain `TaskStatus` 稳定名解析，未知取值显式报错，不静默跳过。
//!
//! 边界：只接受 [`duckdb::Connection`]；重试/中止的编排与权限判定属 `whcode-app`。

use std::collections::BTreeMap;

use duckdb::{Connection, params, params_from_iter};
use whcode_domain::{ErrorCode, TaskStatus, WhError};

fn task_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 任务行（`tasks` 表只读投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    /// 任务 ID。
    pub id: String,
    /// 所属会话。
    pub session_id: String,
    /// 标题。
    pub title: String,
    /// SDD 阶段（自由文本；阶段枚举属 M4）。
    pub stage: String,
    /// 任务状态（domain `TaskStatus` 稳定名）。
    pub status: TaskStatus,
    /// 父任务 ID（层级依赖，FR-406）。
    pub parent_id: Option<String>,
    /// 验收条件 JSON 原文（解析属调用方）。
    pub acceptance_json: String,
    /// 重试计数。
    pub retry_count: i64,
    /// 最近更新时间（Unix 毫秒）。
    pub updated_at_ms: i64,
}

/// 检查点行（步骤投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRow {
    /// 检查点 ID。
    pub id: String,
    /// 步骤序号。
    pub step: i64,
    /// 增量 JSON 原文。
    pub delta_json: String,
    /// 消息游标（可空）。
    pub message_cursor: Option<String>,
    /// 产物引用 JSON 原文。
    pub artifact_refs_json: String,
    /// 时间（Unix 毫秒）。
    pub ts_ms: i64,
}

/// 产物**引用**行——不含 `content` 大对象列（R-05）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRefRow {
    /// 产物 ID。
    pub id: String,
    /// 阶段。
    pub stage: String,
    /// 产物类型。
    pub artifact_type: String,
    /// 引用 JSON 原文。
    pub refs_json: String,
    /// 是否已通过校验。
    pub validated: bool,
    /// 产物版本。
    pub version: i64,
}

/// 任务详情的检查点条数上限（有界读取）。
pub const MAX_DETAIL_CHECKPOINTS: usize = 100;

/// 任务详情的最近事件条数上限。
pub const MAX_RECENT_EVENTS: usize = 20;

/// 活跃任务清单上限。
pub const MAX_ACTIVE_TASKS: usize = 100;

/// 会话任务清单上限（含全部状态的有界读取；converge 差异视图共用）。
pub const MAX_SESSION_TASKS: usize = 200;

/// 把 `status` 列的稳定名解析回 `TaskStatus`；未知取值显式报错。
fn status_from_str(value: &str) -> Result<TaskStatus, WhError> {
    const ALL: [TaskStatus; 9] = [
        TaskStatus::Pending,
        TaskStatus::Ready,
        TaskStatus::Running,
        TaskStatus::Blocked,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Aborted,
        TaskStatus::Interrupted,
        TaskStatus::Unknown,
    ];
    ALL.into_iter()
        .find(|status| status.as_str() == value)
        .ok_or_else(|| {
            invalid(&format!(
                "未知任务状态：{value}（须为 domain TaskStatus 稳定名）"
            ))
        })
}

/// 任务投影列；`acceptance_json` 是 VARIANT（schema v7），读取时显式转回 JSON 文本
/// （ADR-036）。列名保持原名，调用方拿到的仍是 JSON 原文。
const TASK_COLUMNS: &str = "id, session_id, title, stage, status, parent_id, \
                            CAST(CAST(acceptance_json AS JSON) AS VARCHAR) AS acceptance_json, \
                            retry_count, epoch_us(updated_at)";

fn read_task_row(row: &duckdb::Row<'_>) -> Result<TaskRow, WhError> {
    let status_raw: String = row.get(4).map_err(|err| task_err("读取任务状态", err))?;
    let status = status_from_str(&status_raw)?;
    Ok(TaskRow {
        id: row.get(0).map_err(|err| task_err("读取任务", err))?,
        session_id: row.get(1).map_err(|err| task_err("读取任务", err))?,
        title: row.get(2).map_err(|err| task_err("读取任务", err))?,
        stage: row.get(3).map_err(|err| task_err("读取任务", err))?,
        status,
        parent_id: row.get(5).map_err(|err| task_err("读取任务", err))?,
        acceptance_json: row.get(6).map_err(|err| task_err("读取任务", err))?,
        retry_count: row.get(7).map_err(|err| task_err("读取任务", err))?,
        updated_at_ms: row
            .get::<_, i64>(8)
            .map_err(|err| task_err("读取任务", err))?
            .div_euclid(1000),
    })
}

fn load_task(conn: &Connection, id: &str) -> Result<Option<TaskRow>, WhError> {
    let sql = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| task_err("查询任务", err))?;
    let mut rows = stmt
        .query(params![id])
        .map_err(|err| task_err("查询任务", err))?;
    let Some(row) = rows.next().map_err(|err| task_err("查询任务", err))? else {
        return Ok(None);
    };
    read_task_row(row).map(Some)
}

/// 新建任务参数（ID 由调用方提供，重复即约束错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask<'a> {
    /// 任务 ID。
    pub id: &'a str,
    /// 所属会话。
    pub session_id: &'a str,
    /// 标题。
    pub title: &'a str,
    /// SDD 阶段。
    pub stage: &'a str,
    /// 父任务（层级依赖）。
    pub parent_id: Option<&'a str>,
    /// 验收条件 JSON 原文。
    pub acceptance_json: &'a str,
}

/// 插入任务（初始状态 `pending`）。
///
/// # Errors
///
/// 字段为空或数据库写入失败时返回 [`WhError`]。
pub fn insert_task(conn: &Connection, task: &NewTask<'_>, now_ms: u64) -> Result<(), WhError> {
    if task.id.trim().is_empty() || task.session_id.trim().is_empty() {
        return Err(invalid("任务 ID 与会话 ID 不得为空"));
    }
    if task.title.trim().is_empty() || task.stage.trim().is_empty() {
        return Err(invalid("任务标题与阶段不得为空"));
    }
    // 文本层先校验再入库：VARIANT 的 `CAST` 同样会拒绝非法 JSON，但在这里给出
    // 带字段名的结构化错误，并显式拒绝顶层 JSON null（否则会得到被 NOT NULL
    // 拒绝的空变体，错误信息不指向字段）。
    let acceptance: serde_json::Value = serde_json::from_str(task.acceptance_json)
        .map_err(|err| WhError::parse_error(format!("acceptance_json 不是合法 JSON：{err}")))?;
    if acceptance.is_null() {
        return Err(invalid("acceptance_json 不得为顶层 JSON null"));
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    conn.execute(
        "INSERT INTO tasks (id, session_id, title, stage, status, parent_id,
                            acceptance_json, retry_count, updated_at)
         VALUES (?, ?, ?, ?, 'pending', ?, CAST(? AS JSON)::VARIANT, 0, epoch_ms(?))",
        params![
            task.id,
            task.session_id,
            task.title,
            task.stage,
            task.parent_id,
            task.acceptance_json,
            now
        ],
    )
    .map_err(|err| task_err("插入任务", err))?;
    Ok(())
}

/// 读取单个任务；不存在返回 `None`。
///
/// # Errors
///
/// 状态列含未知取值或查询失败时返回 [`WhError`]。
pub fn get_task(conn: &Connection, id: &str) -> Result<Option<TaskRow>, WhError> {
    load_task(conn, id)
}

/// 更新任务状态与重试计数；不存在返回 `not_found`。
///
/// # Errors
///
/// 会话不存在或数据库更新失败时返回 [`WhError`]。
pub fn update_task_status(
    conn: &Connection,
    id: &str,
    status: TaskStatus,
    retry_count: i64,
    now_ms: u64,
) -> Result<(), WhError> {
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let affected = conn
        .execute(
            "UPDATE tasks SET status = ?, retry_count = ?, updated_at = epoch_ms(?) WHERE id = ?",
            params![status.as_str(), retry_count, now, id],
        )
        .map_err(|err| task_err("更新任务状态", err))?;
    if affected == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("任务不存在：{id}"),
        ));
    }
    Ok(())
}

fn scope_clause(session_id: Option<&str>) -> (String, Vec<String>) {
    match session_id {
        Some(id) => (" WHERE session_id = ?".to_string(), vec![id.to_string()]),
        None => (String::new(), Vec::new()),
    }
}

/// 阶段分布（`GROUP BY stage`）；`session_id = None` 为工作区级。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn stage_distribution(
    conn: &Connection,
    session_id: Option<&str>,
) -> Result<BTreeMap<String, i64>, WhError> {
    let (clause, binds) = scope_clause(session_id);
    let sql = format!("SELECT stage, count(*) FROM tasks{clause} GROUP BY stage ORDER BY stage");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| task_err("阶段分布", err))?;
    let rows = stmt
        .query_map(params_from_iter(binds.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|err| task_err("阶段分布", err))?;
    let mut map = BTreeMap::new();
    for row in rows {
        let (stage, count) = row.map_err(|err| task_err("阶段分布", err))?;
        map.insert(stage, count);
    }
    Ok(map)
}

/// 状态分布（`GROUP BY status`）；未知状态取值显式报错。
///
/// # Errors
///
/// 状态列含未知取值或查询失败时返回 [`WhError`]。
pub fn status_distribution(
    conn: &Connection,
    session_id: Option<&str>,
) -> Result<BTreeMap<String, i64>, WhError> {
    let (clause, binds) = scope_clause(session_id);
    let sql = format!("SELECT status, count(*) FROM tasks{clause} GROUP BY status ORDER BY status");
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| task_err("状态分布", err))?;
    let rows = stmt
        .query_map(params_from_iter(binds.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|err| task_err("状态分布", err))?;
    let mut map = BTreeMap::new();
    for row in rows {
        let (status, count) = row.map_err(|err| task_err("状态分布", err))?;
        status_from_str(&status)?;
        map.insert(status, count);
    }
    Ok(map)
}

/// 活跃任务（`status = running`），最近更新在前，有界 [`MAX_ACTIVE_TASKS`]。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn active_tasks(conn: &Connection, session_id: Option<&str>) -> Result<Vec<TaskRow>, WhError> {
    let (clause, binds) = match session_id {
        Some(id) => (
            " WHERE session_id = ? AND status = 'running'".to_string(),
            vec![id.to_string()],
        ),
        None => (" WHERE status = 'running'".to_string(), Vec::new()),
    };
    let sql = format!(
        "SELECT {TASK_COLUMNS} FROM tasks{clause}
         ORDER BY updated_at DESC, id DESC LIMIT {MAX_ACTIVE_TASKS}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| task_err("活跃任务", err))?;
    let mut rows = stmt
        .query(params_from_iter(binds.iter()))
        .map_err(|err| task_err("活跃任务", err))?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next().map_err(|err| task_err("活跃任务", err))? {
        tasks.push(read_task_row(row)?);
    }
    Ok(tasks)
}

/// 会话的全部任务（**含全部状态**），最近更新在前，有界：`limit` 与
/// [`MAX_SESSION_TASKS`] 取小者，不使用 `OFFSET`。
///
/// 与 [`active_tasks`] 的差别：不过滤状态——converge 差异核对需要看到
/// pending/completed/failed 等全部事实，而不是只有 running。
///
/// # Errors
///
/// 状态列含未知取值或查询失败时返回 [`WhError`]。
pub fn tasks_of_session(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<TaskRow>, WhError> {
    // limit 由调用方提供但封顶于常量上限；整数内插无注入面（同 active_tasks 风格）。
    let effective = limit.min(MAX_SESSION_TASKS);
    let sql = format!(
        "SELECT {TASK_COLUMNS} FROM tasks WHERE session_id = ?
         ORDER BY updated_at DESC, id DESC LIMIT {effective}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| task_err("会话任务", err))?;
    let mut rows = stmt
        .query(params![session_id])
        .map_err(|err| task_err("会话任务", err))?;
    let mut tasks = Vec::new();
    while let Some(row) = rows.next().map_err(|err| task_err("会话任务", err))? {
        tasks.push(read_task_row(row)?);
    }
    Ok(tasks)
}

/// 任务的检查点（步骤），按 `step` 升序，有界 [`MAX_DETAIL_CHECKPOINTS`]。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn task_checkpoints(conn: &Connection, task_id: &str) -> Result<Vec<CheckpointRow>, WhError> {
    let sql = format!(
        "SELECT id, step, CAST(CAST(delta_json AS JSON) AS VARCHAR) AS delta_json,
                message_cursor,
                CAST(CAST(artifact_refs_json AS JSON) AS VARCHAR) AS artifact_refs_json,
                epoch_us(ts)
         FROM checkpoints WHERE task_id = ? ORDER BY step ASC, id ASC
         LIMIT {MAX_DETAIL_CHECKPOINTS}"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| task_err("查询检查点", err))?;
    let mut rows = stmt
        .query(params![task_id])
        .map_err(|err| task_err("查询检查点", err))?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(|err| task_err("查询检查点", err))? {
        items.push(CheckpointRow {
            id: row.get(0).map_err(|err| task_err("读取检查点", err))?,
            step: row.get(1).map_err(|err| task_err("读取检查点", err))?,
            delta_json: row.get(2).map_err(|err| task_err("读取检查点", err))?,
            message_cursor: row.get(3).map_err(|err| task_err("读取检查点", err))?,
            artifact_refs_json: row.get(4).map_err(|err| task_err("读取检查点", err))?,
            ts_ms: row
                .get::<_, i64>(5)
                .map_err(|err| task_err("读取检查点", err))?
                .div_euclid(1000),
        });
    }
    Ok(items)
}

/// 任务的产物**引用**（不含 `content` 大对象，R-05），有界读取。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn task_artifacts(conn: &Connection, task_id: &str) -> Result<Vec<ArtifactRefRow>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, stage, artifact_type,
                    CAST(CAST(refs_json AS JSON) AS VARCHAR) AS refs_json, validated, version
             FROM stage_artifacts WHERE task_id = ? ORDER BY version DESC, id ASC LIMIT 100",
        )
        .map_err(|err| task_err("查询产物", err))?;
    let mut rows = stmt
        .query(params![task_id])
        .map_err(|err| task_err("查询产物", err))?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(|err| task_err("查询产物", err))? {
        items.push(ArtifactRefRow {
            id: row.get(0).map_err(|err| task_err("读取产物", err))?,
            stage: row.get(1).map_err(|err| task_err("读取产物", err))?,
            artifact_type: row.get(2).map_err(|err| task_err("读取产物", err))?,
            refs_json: row.get(3).map_err(|err| task_err("读取产物", err))?,
            validated: row.get(4).map_err(|err| task_err("读取产物", err))?,
            version: row.get(5).map_err(|err| task_err("读取产物", err))?,
        });
    }
    Ok(items)
}

/// 任务的最近事件（`seq` 降序），有界 [`MAX_RECENT_EVENTS`]。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn recent_task_events(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<crate::query::EventRow>, WhError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {EVENT_PROJECTION} FROM events WHERE task_id = ?
             ORDER BY seq DESC LIMIT {MAX_RECENT_EVENTS}"
        ))
        .map_err(|err| task_err("查询任务事件", err))?;
    let mut rows = stmt
        .query(params![task_id])
        .map_err(|err| task_err("查询任务事件", err))?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().map_err(|err| task_err("查询任务事件", err))? {
        items.push(map_event_row(row).map_err(|err| task_err("读取任务事件", err))?);
    }
    Ok(items)
}

/// 事件投影列；`payload_json` 是 VARIANT（schema v6），读取时显式转回 JSON 文本（ADR-034）。
const EVENT_PROJECTION: &str = "seq, id, session_id, task_id, request_id, tool_call_id, kind, \
                                CAST(CAST(payload_json AS JSON) AS VARCHAR) AS payload_json, \
                                epoch_us(ts), persistence_level";

fn map_event_row(row: &duckdb::Row<'_>) -> duckdb::Result<crate::query::EventRow> {
    Ok(crate::query::EventRow {
        seq: row.get(0)?,
        id: row.get(1)?,
        session_id: row.get(2)?,
        task_id: row.get(3)?,
        request_id: row.get(4)?,
        tool_call_id: row.get(5)?,
        kind: row.get(6)?,
        payload_json: row.get(7)?,
        ts_us: row.get(8)?,
        persistence_level: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;
    use duckdb::params;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_checkpoint(conn: &Connection, id: &str, task_id: &str, step: i64) {
        conn.execute(
            "INSERT INTO checkpoints (id, task_id, step, delta_json, message_cursor,
                                      artifact_refs_json, ts)
             VALUES (?, ?, ?, CAST('{}' AS JSON)::VARIANT, NULL, CAST('[]' AS JSON)::VARIANT,
                     current_timestamp)",
            params![id, task_id, step],
        )
        .expect("插入检查点");
    }

    fn insert_artifact(conn: &Connection, id: &str, task_id: &str) {
        conn.execute(
            "INSERT INTO stage_artifacts (id, session_id, task_id, stage, artifact_type,
                                          content, refs_json, parser_version, validated,
                                          version, created_at)
             VALUES (?, 'S-1', ?, 'plan', 'spec', '大对象正文不进投影',
                     CAST('[]' AS JSON)::VARIANT, 'v1', TRUE, 1, current_timestamp)",
            params![id, task_id],
        )
        .expect("插入产物");
    }

    fn insert_event(conn: &Connection, seq: i64, task_id: &str, payload: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-1', ?, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}"), task_id, payload],
        )
        .expect("插入事件");
    }

    #[test]
    fn insert_get_update_roundtrip_and_missing_reports() {
        let conn = migrated();
        insert_task(
            &conn,
            &NewTask {
                id: "T-1",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("插入");
        let task = get_task(&conn, "T-1").expect("读取").expect("存在");
        assert_eq!(task.status, TaskStatus::Pending);
        assert_eq!(task.retry_count, 0);
        assert_eq!(task.updated_at_ms, 1_000);

        update_task_status(&conn, "T-1", TaskStatus::Running, 2, 2_000).expect("更新");
        let task = get_task(&conn, "T-1").expect("读取").expect("存在");
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.retry_count, 2);

        assert!(get_task(&conn, "T-missing").expect("读取缺失").is_none());
        let err =
            update_task_status(&conn, "T-missing", TaskStatus::Failed, 0, 1).expect_err("缺失");
        assert_eq!(err.code, ErrorCode::NotFound);
        for (id, session, title, stage) in [
            ("", "S-1", "t", "s"),
            ("T-2", "", "t", "s"),
            ("T-2", "S-1", "", "s"),
            ("T-2", "S-1", "t", ""),
        ] {
            let task = NewTask {
                id,
                session_id: session,
                title,
                stage,
                parent_id: None,
                acceptance_json: "[]",
            };
            let err = insert_task(&conn, &task, 1).expect_err("空字段必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState);
        }
    }

    #[test]
    fn insert_task_rejects_invalid_and_top_level_null_acceptance_without_rows() {
        let conn = migrated();
        for (payload, expected) in [
            ("{broken", ErrorCode::ParseError),
            ("null", ErrorCode::InvalidState),
        ] {
            let task = NewTask {
                id: "T-bad",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: payload,
            };
            let err = insert_task(&conn, &task, 1_000).expect_err("非法验收条件必须拒绝");
            assert_eq!(err.code, expected, "拒绝码必须指向字段语义：{payload}");
        }
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))
            .expect("统计任务");
        assert_eq!(rows, 0, "校验失败不得留下任何任务行");
    }

    #[test]
    fn unknown_status_value_is_rejected_not_silently_mapped() {
        let conn = migrated();
        insert_task(
            &conn,
            &NewTask {
                id: "T-1",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("插入");
        conn.execute("UPDATE tasks SET status = 'shrug' WHERE id = 'T-1'", [])
            .expect("写入未知状态");
        let err = get_task(&conn, "T-1").expect_err("未知状态必须显式报错");
        assert!(err.to_string().contains("shrug"), "{err}");
    }

    #[test]
    fn distributions_and_active_tasks_cover_workspace_and_session_scopes() {
        let conn = migrated();
        for (id, session, stage, ts) in [
            ("T-1", "S-1", "plan", 1_000),
            ("T-2", "S-1", "plan", 2_000),
            ("T-3", "S-1", "implement", 3_000),
            ("T-4", "S-2", "plan", 4_000),
        ] {
            insert_task(
                &conn,
                &NewTask {
                    id,
                    session_id: session,
                    title: "任务",
                    stage,
                    parent_id: None,
                    acceptance_json: "[]",
                },
                ts,
            )
            .expect("插入任务");
        }
        update_task_status(&conn, "T-1", TaskStatus::Completed, 0, 3_100).expect("完成");
        update_task_status(&conn, "T-2", TaskStatus::Blocked, 0, 3_200).expect("阻塞");
        update_task_status(&conn, "T-3", TaskStatus::Running, 0, 3_300).expect("执行中");

        let stages = stage_distribution(&conn, None).expect("工作区阶段分布");
        assert_eq!(stages.get("plan"), Some(&3));
        assert_eq!(stages.get("implement"), Some(&1));
        let session_stages = stage_distribution(&conn, Some("S-1")).expect("会话阶段分布");
        assert_eq!(session_stages.get("plan"), Some(&2));
        assert!(
            stage_distribution(&conn, Some("S-none"))
                .expect("空会话")
                .is_empty()
        );

        let statuses = status_distribution(&conn, None).expect("状态分布");
        assert_eq!(statuses.get("completed"), Some(&1));
        assert_eq!(statuses.get("blocked"), Some(&1));
        assert_eq!(statuses.get("running"), Some(&1));

        let active = active_tasks(&conn, None).expect("工作区活跃任务");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "T-3");
        assert!(
            active_tasks(&conn, Some("S-2"))
                .expect("S-2 活跃")
                .is_empty()
        );
    }

    #[test]
    fn tasks_of_session_returns_all_statuses_ordered_and_bounded() {
        let conn = migrated();
        for (id, session, ts) in [
            ("T-1", "S-A", 1_000),
            ("T-2", "S-A", 2_000),
            ("T-3", "S-A", 3_000),
            ("T-4", "S-A", 4_000),
            ("T-5", "S-B", 5_000),
        ] {
            insert_task(
                &conn,
                &NewTask {
                    id,
                    session_id: session,
                    title: "任务",
                    stage: "plan",
                    parent_id: None,
                    acceptance_json: "[]",
                },
                ts,
            )
            .expect("插入任务");
        }
        update_task_status(&conn, "T-1", TaskStatus::Completed, 0, 6_100).expect("完成");
        update_task_status(&conn, "T-2", TaskStatus::Failed, 0, 6_200).expect("失败");
        update_task_status(&conn, "T-3", TaskStatus::Running, 0, 6_300).expect("执行中");

        // 含全部状态，按 updated_at 倒序；不跨会话。
        let all = tasks_of_session(&conn, "S-A", MAX_SESSION_TASKS).expect("会话任务");
        let ids: Vec<&str> = all.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["T-3", "T-2", "T-1", "T-4"], "updated_at 倒序");
        let statuses: Vec<TaskStatus> = all.iter().map(|row| row.status).collect();
        assert!(
            statuses.contains(&TaskStatus::Pending)
                && statuses.contains(&TaskStatus::Running)
                && statuses.contains(&TaskStatus::Failed)
                && statuses.contains(&TaskStatus::Completed),
            "全部状态都在：{statuses:?}"
        );
        assert!(all.iter().all(|row| row.session_id == "S-A"));

        // limit 生效：取倒序前缀。
        let limited = tasks_of_session(&conn, "S-A", 2).expect("限额任务");
        let ids: Vec<&str> = limited.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, vec!["T-3", "T-2"]);

        // 常量封顶：limit 再大也只取 MAX_SESSION_TASKS 条（205 条插入取 200）。
        for index in 0..(MAX_SESSION_TASKS + 5) {
            insert_task(
                &conn,
                &NewTask {
                    id: &format!("B-{index}"),
                    session_id: "S-Bulk",
                    title: "批量",
                    stage: "plan",
                    parent_id: None,
                    acceptance_json: "[]",
                },
                u64::try_from(index + 1).expect("序号非负"),
            )
            .expect("批量插入");
        }
        let bulk = tasks_of_session(&conn, "S-Bulk", usize::MAX).expect("封顶任务");
        assert_eq!(bulk.len(), MAX_SESSION_TASKS, "读取必须封顶于常量上限");

        // limit = 0 是显式的空。
        assert!(
            tasks_of_session(&conn, "S-A", 0)
                .expect("零限额")
                .is_empty()
        );
    }

    #[test]
    fn detail_projections_are_bounded_and_exclude_content() {
        let conn = migrated();
        insert_task(
            &conn,
            &NewTask {
                id: "T-1",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("T-1");
        for step in 0..(MAX_DETAIL_CHECKPOINTS as i64 + 10) {
            insert_checkpoint(&conn, &format!("C-{step}"), "T-1", step);
        }
        insert_artifact(&conn, "A-1", "T-1");
        for seq in 0..(MAX_RECENT_EVENTS as i64 + 5) {
            insert_event(&conn, seq, "T-1", "{\"n\":1}");
        }

        let checkpoints = task_checkpoints(&conn, "T-1").expect("检查点");
        assert_eq!(
            checkpoints.len(),
            MAX_DETAIL_CHECKPOINTS,
            "检查点读取必须有界"
        );
        assert!(
            checkpoints
                .windows(2)
                .all(|pair| pair[0].step <= pair[1].step),
            "步骤必须按 step 升序"
        );

        let artifacts = task_artifacts(&conn, "T-1").expect("产物引用");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].artifact_type, "spec");
        assert!(artifacts[0].validated);
        // ArtifactRefRow 根本没有 content 字段——大对象不出存储层（R-05）。

        let events = recent_task_events(&conn, "T-1").expect("最近事件");
        assert_eq!(events.len(), MAX_RECENT_EVENTS, "事件读取必须有界");
        assert!(
            events.windows(2).all(|pair| pair[0].seq > pair[1].seq),
            "最近事件必须按 seq 降序"
        );
    }
}
