//! # 检查点写入、任务状态转移事务与恢复读取（G-01、FR-402/403/404、ADR-018）
//!
//! 检查点三条件在本模块落地：
//!
//! - **FR-402 增量差分**：`delta_json` 存每步差分对象而非全量历史；单条差分有
//!   [`MAX_DELTA_JSON_BYTES`] 上限，产物引用不超过 [`MAX_ARTIFACT_REFS`] 个；
//!   每次写入后把该任务的检查点裁剪到 domain `DEFAULT_RETAIN`（保留 `step` 最大的
//!   N 个，删除 `step` 最小的多余行），同任务检查点行数有界、不随步数增长；
//! - **FR-403 同事务原子提交**：[`task_transition`] 把任务状态变化、关键事件与
//!   检查点引用放进**同一个 duckdb 事务**；当前状态不等于 `from_status`、事件或
//!   检查点写入任一步失败时整体回滚，三表不留半程状态；
//! - **FR-404 按引用恢复**：[`recover_from_checkpoint`] 只加载检查点行、其引用的
//!   产物元数据与消息游标；**完整历史不在恢复路径加载**——全部检查点经
//!   `crate::tasks::task_checkpoints`、全部事件经应用服务 `events_export` 按需
//!   有界读取。
//!
//! 边界：SQL 只在本 crate（ADR-018）；权限判定属 `whcode-app`；裁剪目标取 domain
//! 默认保留数 [`DEFAULT_RETAIN`]（`MAX_RETAINED` 仍是策略配置的校验上界；运行时
//! 自定义 `retain` 的执行点在 M8 调度器，本层不引入策略依赖）。引用的产物行可能
//! 已随会话清理而消失——恢复时悬空引用直接缺席（正确的空），不作错误。

use duckdb::{Connection, params, params_from_iter};
use whcode_domain::checkpoint::DEFAULT_RETAIN;
use whcode_domain::{ErrorCode, MAX_PAYLOAD_BYTES, TaskStatus, WhError};

use crate::tasks::{ArtifactRefRow, CheckpointRow};

/// 单条检查点差分 `delta_json` 的字节上限（64 KiB）。
pub const MAX_DELTA_JSON_BYTES: usize = 64 * 1024;

/// 单个检查点引用的产物数量上限。
pub const MAX_ARTIFACT_REFS: usize = 100;

/// 检查点写入参数（FR-402：`delta_json` 须是单步差分对象，不是全量历史）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointInsert {
    /// 所属任务 ID（必须已存在）。
    pub task_id: String,
    /// 步骤序号（非负）。
    pub step: i64,
    /// 增量状态差分 JSON（合法 JSON，≤ [`MAX_DELTA_JSON_BYTES`] 字节）。
    pub delta_json: String,
    /// 消息游标（上下文恢复点，可空）。
    pub message_cursor: Option<String>,
    /// 产物引用 JSON 数组（元素为产物 ID 字符串或含 `id` 字符串字段的对象），
    /// 至多 [`MAX_ARTIFACT_REFS`] 个。
    pub artifact_refs_json: String,
}

/// 状态转移伴随的事件写入参数（FR-403：与状态变化同事务提交）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventInsertInfo {
    /// 所属会话（可空：任务级事件允许无会话归属）。
    pub session_id: Option<String>,
    /// 事件类型稳定名（`events.kind`；取值口径同任务书 §8.3 事件目录）。
    pub kind: String,
    /// 载荷 JSON 原文（合法 JSON，≤ `MAX_PAYLOAD_BYTES` 字节，R-05）。
    pub payload_json: String,
    /// 关联请求 ID（可空）。
    pub request_id: Option<String>,
    /// 关联工具调用 ID（可空）。
    pub tool_call_id: Option<String>,
    /// 持久化级别稳定名（`full`/`metadata`/`redacted`/`omitted_by_policy`）。
    pub persistence_level: String,
}

/// 一次任务状态转移（FR-403）：状态变化、事件与可选检查点原子提交。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTransition {
    /// 目标任务 ID。
    pub task_id: String,
    /// 期望的当前状态；不符即拒绝并回滚（防并发写覆盖的乐观校验）。
    pub from_status: TaskStatus,
    /// 目标状态。
    pub to_status: TaskStatus,
    /// 与本次状态变化同事务写入的事件。
    pub event: EventInsertInfo,
    /// 可选检查点（危险操作前/里程碑）；`None` 表示本次不留检查点。
    pub checkpoint: Option<CheckpointInsert>,
}

/// 状态转移回执（三表提交成功后的确认，FR-403）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskTransitionReport {
    /// 任务 ID。
    pub task_id: String,
    /// 转移后的任务状态。
    pub to_status: TaskStatus,
    /// 本次写入的事件 `seq`（单调递增）。
    pub event_seq: i64,
    /// 写入的检查点 ID（本次未留检查点时为 `None`）。
    pub checkpoint_id: Option<String>,
}

/// 检查点恢复集（FR-404）：检查点行、引用的产物元数据与上下文游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRecovery {
    /// 检查点行本身（含差分 `delta_json` 原文）。
    pub checkpoint: CheckpointRow,
    /// 检查点引用的产物元数据（不含大对象正文，R-05）；按 `artifact_refs_json`
    /// 中的引用 ID 联查 `stage_artifacts`，至多解析 [`MAX_ARTIFACT_REFS`] 个。
    /// 已被清理的产物引用不出现在结果中——悬空引用属正确的空，不是损坏。
    pub artifacts: Vec<ArtifactRefRow>,
    /// 上下文消息游标（原样返回；`None` 表示该检查点未记录游标）。
    pub message_cursor: Option<String>,
}

fn cp_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::invalid_state(context)
}

fn not_found(context: String) -> WhError {
    WhError::new(ErrorCode::NotFound, context)
}

/// 解析产物引用 JSON 数组中的引用 ID；元素须为字符串或含 `id` 字符串字段的对象。
fn artifact_ref_ids(artifact_refs_json: &str) -> Result<Vec<String>, WhError> {
    let value: serde_json::Value = serde_json::from_str(artifact_refs_json)
        .map_err(|err| WhError::parse_error(format!("artifact_refs_json 不是合法 JSON：{err}")))?;
    let items = value
        .as_array()
        .ok_or_else(|| WhError::parse_error("artifact_refs_json 必须是 JSON 数组"))?;
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let id = match item {
            serde_json::Value::String(id) => id.clone(),
            serde_json::Value::Object(map) => map
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| WhError::parse_error("产物引用对象缺少字符串字段 id"))?
                .to_string(),
            _ => {
                return Err(WhError::parse_error(
                    "产物引用元素须为字符串或含 id 字段的对象",
                ));
            }
        };
        ids.push(id);
    }
    Ok(ids)
}

/// 校验检查点写入参数（FR-402 有界性：差分体积、引用数量、步骤非负）。
fn validate_checkpoint(insert: &CheckpointInsert) -> Result<(), WhError> {
    if insert.task_id.trim().is_empty() {
        return Err(invalid("检查点所属任务 ID 不得为空"));
    }
    if insert.step < 0 {
        return Err(invalid(&format!(
            "检查点 step 必须非负（当前 {}）",
            insert.step
        )));
    }
    serde_json::from_str::<serde_json::Value>(&insert.delta_json)
        .map_err(|err| WhError::parse_error(format!("delta_json 不是合法 JSON：{err}")))?;
    if insert.delta_json.len() > MAX_DELTA_JSON_BYTES {
        return Err(invalid(&format!(
            "delta_json 超过 {MAX_DELTA_JSON_BYTES} 字节上限（当前 {}）：\
             差分须拆成更小粒度，不得复制全量历史（FR-402）",
            insert.delta_json.len()
        )));
    }
    let refs = artifact_ref_ids(&insert.artifact_refs_json)?;
    if refs.len() > MAX_ARTIFACT_REFS {
        return Err(invalid(&format!(
            "产物引用数量超过 {MAX_ARTIFACT_REFS} 上限（当前 {}）",
            refs.len()
        )));
    }
    Ok(())
}

/// 生成检查点 ID：`cp-<task>-<step>-<毫秒>`。
fn checkpoint_id(task_id: &str, step: i64, now_ms: u64) -> String {
    format!("cp-{task_id}-{step}-{now_ms}")
}

/// 裁剪：保留 `step` 最大的 [`DEFAULT_RETAIN`] 个，删除 `step` 最小的多余行。
///
/// 目标值取 domain `DEFAULT_RETAIN`（`retain_recent` 的“至少保留 1 个”语义由常量
/// 下界保证）；裁剪与插入同事务，回滚时不留半程状态。注意：插入 `step` 小于既有
/// 检查点的行可能立即被裁剪——保留语义以 `step` 为准，调用方须单调推进步骤。
fn prune_checkpoints(conn: &Connection, task_id: &str) -> Result<(), WhError> {
    let retain = DEFAULT_RETAIN.max(1) as i64;
    conn.execute(
        "DELETE FROM checkpoints WHERE task_id = ? AND id NOT IN (
             SELECT id FROM checkpoints WHERE task_id = ?
             ORDER BY step DESC, id DESC LIMIT ?)",
        params![task_id, task_id, retain],
    )
    .map_err(|err| cp_err("裁剪检查点", err))?;
    Ok(())
}

/// 在调用方给定的连接/事务上写入检查点并裁剪；任务存在性由调用方保证。
fn write_checkpoint(conn: &Connection, insert: &CheckpointInsert) -> Result<String, WhError> {
    validate_checkpoint(insert)?;
    let id = checkpoint_id(&insert.task_id, insert.step, crate::writer::now_ms());
    conn.execute(
        "INSERT INTO checkpoints (id, task_id, step, delta_json, message_cursor,
                                  artifact_refs_json, ts)
         VALUES (?, ?, ?, ?, ?, ?, current_timestamp)",
        params![
            id,
            insert.task_id,
            insert.step,
            insert.delta_json,
            insert.message_cursor,
            insert.artifact_refs_json,
        ],
    )
    .map_err(|err| cp_err("写入检查点", err))?;
    prune_checkpoints(conn, &insert.task_id)?;
    Ok(id)
}

/// 写入一条检查点（FR-402）并把该任务的历史裁剪到有界数量，返回检查点 ID。
///
/// 校验：任务必须存在、`step` 非负、`delta_json` 与 `artifact_refs_json` 为合法
/// JSON、`delta_json` ≤ [`MAX_DELTA_JSON_BYTES`]、产物引用 ≤ [`MAX_ARTIFACT_REFS`]。
///
/// # Errors
///
/// 上述任一校验失败或数据库写入失败时返回 [`WhError`]。
pub fn insert_checkpoint(conn: &Connection, insert: &CheckpointInsert) -> Result<String, WhError> {
    let exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE id = ?",
            params![insert.task_id],
            |row| row.get(0),
        )
        .map_err(|err| cp_err("核对任务存在性", err))?;
    if exists == 0 {
        return Err(not_found(format!("任务不存在：{}", insert.task_id)));
    }
    write_checkpoint(conn, insert)
}

/// 校验事件写入参数（载荷有界且为合法 JSON，R-05/FR-208）。
fn validate_event(event: &EventInsertInfo) -> Result<(), WhError> {
    if event.kind.trim().is_empty() {
        return Err(invalid("事件 kind 不得为空"));
    }
    serde_json::from_str::<serde_json::Value>(&event.payload_json)
        .map_err(|err| WhError::parse_error(format!("事件载荷不是合法 JSON：{err}")))?;
    if event.payload_json.len() > MAX_PAYLOAD_BYTES {
        return Err(invalid(&format!(
            "事件载荷超过 {MAX_PAYLOAD_BYTES} 字节上限：须改走内容表/引用（R-05）"
        )));
    }
    if event.persistence_level.trim().is_empty() {
        return Err(invalid("事件 persistence_level 不得为空"));
    }
    Ok(())
}

/// 任务状态转移 + 事件 + 可选检查点的同事务原子提交（FR-403）。
///
/// 事务内步骤：核对当前状态 == `from_status`（不符返回 `invalid_state`，整体回滚）→
/// 更新 `tasks.status`/`updated_at` → 写入事件行（`seq` 取当前最大值 + 1，与
/// `crate::writer` 写路径同法；事件 ID 取 `EV-<seq>`）→ 有检查点则写入并裁剪。
/// 任一步失败整体回滚，三表不留半程状态。
///
/// # Errors
///
/// 任务 ID 为空、任务不存在（`not_found`）、状态不符（`invalid_state`）、事件或
/// 检查点校验失败或数据库读写失败时返回 [`WhError`]。
pub fn task_transition(
    conn: &mut Connection,
    transition: &TaskTransition,
) -> Result<TaskTransitionReport, WhError> {
    if transition.task_id.trim().is_empty() {
        return Err(invalid("任务 ID 不得为空"));
    }
    validate_event(&transition.event)?;
    if let Some(checkpoint) = &transition.checkpoint
        && checkpoint.task_id != transition.task_id
    {
        return Err(invalid(&format!(
            "检查点 task_id（{}）与转移目标任务（{}）不一致",
            checkpoint.task_id, transition.task_id
        )));
    }

    let tx = conn
        .transaction()
        .map_err(|err| cp_err("开始状态转移事务", err))?;

    let current: String = tx
        .query_row(
            "SELECT status FROM tasks WHERE id = ?",
            params![transition.task_id],
            |row| row.get(0),
        )
        .map_err(|err| match err {
            duckdb::Error::QueryReturnedNoRows => {
                not_found(format!("任务不存在：{}", transition.task_id))
            }
            other => cp_err("读取任务状态", other),
        })?;
    if current != transition.from_status.as_str() {
        return Err(invalid(&format!(
            "任务 {} 当前状态 {current} 与期望 from_status {} 不符，转移已回滚",
            transition.task_id,
            transition.from_status.as_str()
        )));
    }

    tx.execute(
        "UPDATE tasks SET status = ?, updated_at = current_timestamp WHERE id = ?",
        params![transition.to_status.as_str(), transition.task_id],
    )
    .map_err(|err| cp_err("更新任务状态", err))?;

    let seq: i64 = tx
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
            row.get(0)
        })
        .map_err(|err| cp_err("分配事件序号", err))?;
    let next_seq = seq.saturating_add(1);
    tx.execute(
        "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                             kind, payload_json, ts, persistence_level)
         VALUES (?, ?, ?, ?, ?, ?, ?, CAST(? AS JSON)::VARIANT, current_timestamp, ?)",
        params![
            next_seq,
            format!("EV-{next_seq}"),
            transition.event.session_id,
            transition.task_id,
            transition.event.request_id,
            transition.event.tool_call_id,
            transition.event.kind,
            transition.event.payload_json,
            transition.event.persistence_level,
        ],
    )
    .map_err(|err| cp_err("写入转移事件", err))?;

    let checkpoint_id = match &transition.checkpoint {
        Some(checkpoint) => Some(write_checkpoint(&tx, checkpoint)?),
        None => None,
    };

    tx.commit().map_err(|err| cp_err("提交状态转移事务", err))?;

    Ok(TaskTransitionReport {
        task_id: transition.task_id.clone(),
        to_status: transition.to_status,
        event_seq: next_seq,
        checkpoint_id,
    })
}

fn map_checkpoint_row(row: &duckdb::Row<'_>) -> duckdb::Result<CheckpointRow> {
    Ok(CheckpointRow {
        id: row.get(0)?,
        step: row.get(1)?,
        delta_json: row.get(2)?,
        message_cursor: row.get(3)?,
        artifact_refs_json: row.get(4)?,
        ts_ms: row.get::<_, i64>(5)?.div_euclid(1000),
    })
}

/// 读取任务的指定检查点，或 `step` 最大者（`checkpoint_id = None`）。
fn load_checkpoint(
    conn: &Connection,
    task_id: &str,
    checkpoint_id: Option<&str>,
) -> Result<CheckpointRow, WhError> {
    match checkpoint_id {
        Some(id) => conn
            .query_row(
                "SELECT id, step, delta_json, message_cursor, artifact_refs_json, epoch_us(ts)
                 FROM checkpoints WHERE task_id = ? AND id = ?",
                params![task_id, id],
                map_checkpoint_row,
            )
            .map_err(|err| match err {
                duckdb::Error::QueryReturnedNoRows => {
                    not_found(format!("检查点不存在或不属于任务 {task_id}：{id}"))
                }
                other => cp_err("读取检查点", other),
            }),
        None => conn
            .query_row(
                "SELECT id, step, delta_json, message_cursor, artifact_refs_json, epoch_us(ts)
                 FROM checkpoints WHERE task_id = ?
                 ORDER BY step DESC, id DESC LIMIT 1",
                params![task_id],
                map_checkpoint_row,
            )
            .map_err(|err| match err {
                duckdb::Error::QueryReturnedNoRows => {
                    not_found(format!("任务 {task_id} 没有检查点"))
                }
                other => cp_err("读取检查点", other),
            }),
    }
}

/// 从检查点恢复（FR-404）：加载检查点行、其引用的产物元数据与消息游标。
///
/// `checkpoint_id` 缺省取该任务 `step` 最大的检查点。完整历史（全部检查点与事件）
/// **不在本函数加载**——按需经 `crate::tasks::task_checkpoints` 与应用服务
/// `events_export` 有界读取；本函数只返回引用可达的元数据。
///
/// # Errors
///
/// 任务不存在、检查点不存在（含不属于该任务）或数据库查询失败时返回
/// `not_found` / [`WhError`]。
pub fn recover_from_checkpoint(
    conn: &Connection,
    task_id: &str,
    checkpoint_id: Option<&str>,
) -> Result<CheckpointRecovery, WhError> {
    let exists: i64 = conn
        .query_row(
            "SELECT count(*) FROM tasks WHERE id = ?",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(|err| cp_err("核对任务存在性", err))?;
    if exists == 0 {
        return Err(not_found(format!("任务不存在：{task_id}")));
    }

    let checkpoint = load_checkpoint(conn, task_id, checkpoint_id)?;
    let message_cursor = checkpoint.message_cursor.clone();

    let mut ref_ids = artifact_ref_ids(&checkpoint.artifact_refs_json)?;
    // 有界读取（R-05）：插入路径已限制引用数；导入的历史行可能超限，按记录顺序
    // 取前 MAX_ARTIFACT_REFS 个，保证恢复读取的参数与结果集都有界。
    ref_ids.truncate(MAX_ARTIFACT_REFS);
    let mut artifacts = Vec::new();
    if !ref_ids.is_empty() {
        let placeholders = vec!["?"; ref_ids.len()].join(", ");
        let sql = format!(
            "SELECT id, stage, artifact_type, refs_json, validated, version
             FROM stage_artifacts WHERE id IN ({placeholders}) ORDER BY id"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| cp_err("查询检查点产物", err))?;
        let rows = stmt
            .query_map(params_from_iter(ref_ids.iter()), |row| {
                Ok(ArtifactRefRow {
                    id: row.get(0)?,
                    stage: row.get(1)?,
                    artifact_type: row.get(2)?,
                    refs_json: row.get(3)?,
                    validated: row.get(4)?,
                    version: row.get(5)?,
                })
            })
            .map_err(|err| cp_err("查询检查点产物", err))?;
        for row in rows {
            artifacts.push(row.map_err(|err| cp_err("读取检查点产物", err))?);
        }
    }

    Ok(CheckpointRecovery {
        checkpoint,
        artifacts,
        message_cursor,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::schema::apply_migrations;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_task(conn: &Connection, id: &str) {
        crate::tasks::insert_task(
            conn,
            &crate::tasks::NewTask {
                id,
                session_id: "S-1",
                title: "任务",
                stage: "implement",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("插入任务");
    }

    fn insert_artifact(conn: &Connection, id: &str, task_id: &str) {
        conn.execute(
            "INSERT INTO stage_artifacts (id, session_id, task_id, stage, artifact_type,
                                          content, refs_json, parser_version, validated,
                                          version, created_at)
             VALUES (?, 'S-1', ?, 'plan', 'spec', '大对象正文不进恢复路径', '[]', 'v1', TRUE, 1,
                     current_timestamp)",
            params![id, task_id],
        )
        .expect("插入产物");
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect("计数")
    }

    fn transition_event() -> EventInsertInfo {
        EventInsertInfo {
            session_id: Some("S-1".to_string()),
            kind: "TaskStatusChanged".to_string(),
            payload_json: r#"{"note":"转移"}"#.to_string(),
            request_id: None,
            tool_call_id: None,
            persistence_level: "redacted".to_string(),
        }
    }

    #[test]
    fn insert_checkpoint_rejects_invalid_inputs_with_structured_errors() {
        let conn = migrated();
        insert_task(&conn, "T-1");

        let base = |task_id: &str, delta: String, refs: String| CheckpointInsert {
            task_id: task_id.to_string(),
            step: 1,
            delta_json: delta,
            message_cursor: None,
            artifact_refs_json: refs,
        };

        // 任务不存在。
        let err = insert_checkpoint(&conn, &base("T-none", "{}".into(), "[]".into()))
            .expect_err("缺失任务必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);

        // 非法 JSON（差分与引用各一，引用还须是数组）。
        let err = insert_checkpoint(&conn, &base("T-1", "{broken".into(), "[]".into()))
            .expect_err("delta_json 非法必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        let err = insert_checkpoint(&conn, &base("T-1", "{}".into(), "{broken".into()))
            .expect_err("引用 JSON 非法必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        let err = insert_checkpoint(
            &conn,
            &base("T-1", "{}".into(), r#"{"not":"array"}"#.into()),
        )
        .expect_err("引用必须是数组");
        assert_eq!(err.code, ErrorCode::ParseError);

        // 超限差分与超限引用。
        let big_delta = format!(r#"{{"pad":"{}"}}"#, "x".repeat(MAX_DELTA_JSON_BYTES));
        let err = insert_checkpoint(&conn, &base("T-1", big_delta, "[]".into()))
            .expect_err("超限差分必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let many: Vec<String> = (0..=MAX_ARTIFACT_REFS).map(|i| format!("A-{i}")).collect();
        let err = insert_checkpoint(
            &conn,
            &base(
                "T-1",
                "{}".into(),
                serde_json::to_string(&many).expect("序列化"),
            ),
        )
        .expect_err("超限引用必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 负 step。
        let mut negative = base("T-1", "{}".into(), "[]".into());
        negative.step = -1;
        let err = insert_checkpoint(&conn, &negative).expect_err("负 step 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 校验失败不产生任何行。
        assert_eq!(count(&conn, "SELECT count(*) FROM checkpoints"), 0);
    }

    #[test]
    fn insert_checkpoint_prunes_to_default_retain_keeping_largest_steps() {
        let conn = migrated();
        insert_task(&conn, "T-1");

        let top = DEFAULT_RETAIN as i64 + 3;
        let mut last_id = String::new();
        for step in 1..=top {
            last_id = insert_checkpoint(
                &conn,
                &CheckpointInsert {
                    task_id: "T-1".into(),
                    step,
                    delta_json: serde_json::json!({ "step": step }).to_string(),
                    message_cursor: None,
                    artifact_refs_json: "[]".into(),
                },
            )
            .expect("插入检查点");
            assert!(
                last_id.starts_with("cp-T-1-"),
                "ID 形如 cp-<task>-<step>-<毫秒>"
            );
        }

        let checkpoints = crate::tasks::task_checkpoints(&conn, "T-1").expect("读取");
        assert_eq!(
            checkpoints.len(),
            DEFAULT_RETAIN,
            "检查点行数必须被裁剪到上限（有界增长，FR-402）"
        );
        assert_eq!(
            checkpoints.iter().map(|c| c.step).collect::<Vec<_>>(),
            ((top - DEFAULT_RETAIN as i64 + 1)..=top).collect::<Vec<_>>(),
            "step 最小的多余行被删除，保留 step 最大者"
        );
        assert!(
            checkpoints.iter().any(|c| c.id == last_id),
            "最新检查点保留"
        );
    }

    #[test]
    fn transition_commits_task_event_and_checkpoint_together() {
        let mut conn = migrated();
        insert_task(&conn, "T-1");
        insert_artifact(&conn, "A-1", "T-1");

        let report = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-1".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: transition_event(),
                checkpoint: Some(CheckpointInsert {
                    task_id: "T-1".into(),
                    step: 1,
                    delta_json: r#"{"from":"pending","to":"running"}"#.into(),
                    message_cursor: Some("cursor-1".into()),
                    artifact_refs_json: r#"["A-1"]"#.into(),
                }),
            },
        )
        .expect("转移");
        assert_eq!(report.task_id, "T-1");
        assert_eq!(report.to_status, TaskStatus::Running);
        assert_eq!(report.event_seq, 1);
        let checkpoint_id = report.checkpoint_id.expect("有检查点");
        assert!(checkpoint_id.starts_with("cp-T-1-1-"), "{checkpoint_id}");

        let task = crate::tasks::get_task(&conn, "T-1")
            .expect("可读")
            .expect("存在");
        assert_eq!(task.status, TaskStatus::Running, "任务状态同事务更新");

        let (seq, event_id, event_task, kind, payload, level): (
            i64,
            String,
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT seq, id, task_id, kind, CAST(CAST(payload_json AS JSON) AS VARCHAR),
                        persistence_level
                 FROM events LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("事件行");
        assert_eq!(seq, 1, "首个事件 seq 为 1");
        assert_eq!(event_id, "EV-1");
        assert_eq!(event_task, "T-1");
        assert_eq!(kind, "TaskStatusChanged");
        assert_eq!(payload, transition_event().payload_json);
        assert_eq!(level, "redacted");

        let checkpoints = crate::tasks::task_checkpoints(&conn, "T-1").expect("检查点");
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].id, checkpoint_id);
        assert_eq!(checkpoints[0].message_cursor.as_deref(), Some("cursor-1"));

        // 第二次转移（无检查点）：seq 单调 +1，回执不留检查点 ID。
        let second = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-1".into(),
                from_status: TaskStatus::Running,
                to_status: TaskStatus::Blocked,
                event: transition_event(),
                checkpoint: None,
            },
        )
        .expect("第二次转移");
        assert_eq!(second.event_seq, 2, "事件 seq 必须单调递增");
        assert!(second.checkpoint_id.is_none());
        assert_eq!(count(&conn, "SELECT count(*) FROM events"), 2);
    }

    #[test]
    fn transition_failure_rolls_back_task_event_and_checkpoint() {
        let conn = migrated();
        insert_task(&conn, "T-r");
        // 伪造约束失败：占用下一事件的 id（seq 取 max+1 = 2，事件 ID 生成 EV-2）。
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (1, 'EV-2', 'S-1', NULL, NULL, NULL, 'SessionStarted',
                     CAST('{}' AS JSON)::VARIANT, current_timestamp, 'redacted')",
            [],
        )
        .expect("种子事件");

        let mut conn = conn;
        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-r".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: transition_event(),
                checkpoint: Some(CheckpointInsert {
                    task_id: "T-r".into(),
                    step: 1,
                    delta_json: r#"{"from":"pending","to":"running"}"#.into(),
                    message_cursor: None,
                    artifact_refs_json: "[]".into(),
                }),
            },
        )
        .expect_err("事件约束失败必须整体失败");
        assert_eq!(err.code, ErrorCode::IoError);

        // 三表全回滚：任务状态未变、事件未增、检查点未增（FR-403）。
        let task = crate::tasks::get_task(&conn, "T-r")
            .expect("可读")
            .expect("存在");
        assert_eq!(task.status, TaskStatus::Pending, "任务状态必须回滚");
        assert_eq!(count(&conn, "SELECT count(*) FROM events"), 1, "事件未增");
        assert_eq!(
            count(&conn, "SELECT count(*) FROM checkpoints"),
            0,
            "检查点未增"
        );
    }

    #[test]
    fn transition_with_oversized_delta_rolls_back_task_and_event() {
        // 检查点校验发生在事务内（任务已更新、事件已写入之后）：
        // 校验失败同样必须整体回滚，不留半程状态。
        let conn = migrated();
        insert_task(&conn, "T-v");
        let big_delta = format!(r#"{{"pad":"{}"}}"#, "x".repeat(MAX_DELTA_JSON_BYTES));

        let mut conn = conn;
        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-v".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: transition_event(),
                checkpoint: Some(CheckpointInsert {
                    task_id: "T-v".into(),
                    step: 1,
                    delta_json: big_delta,
                    message_cursor: None,
                    artifact_refs_json: "[]".into(),
                }),
            },
        )
        .expect_err("超限差分必须整体失败");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let task = crate::tasks::get_task(&conn, "T-v")
            .expect("可读")
            .expect("存在");
        assert_eq!(task.status, TaskStatus::Pending, "任务状态必须回滚");
        assert_eq!(
            count(&conn, "SELECT count(*) FROM events"),
            0,
            "事件必须回滚"
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM checkpoints"), 0);
    }

    #[test]
    fn transition_rejects_invalid_json_and_oversized_payload_before_write() {
        // 载荷校验在事务开始之前：非法 JSON 与超限文本都必须整体拒绝、零写入。
        let conn = migrated();
        insert_task(&conn, "T-j");
        let mut conn = conn;

        let mut invalid = transition_event();
        invalid.payload_json = "{broken".to_string();
        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-j".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: invalid,
                checkpoint: None,
            },
        )
        .expect_err("非法 JSON 载荷必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);

        // 上限按原 JSON 文本字节数计算（VARIANT 转换之前的判据）。
        let mut oversized = transition_event();
        oversized.payload_json = format!(r#"{{"pad":"{}"}}"#, "x".repeat(MAX_PAYLOAD_BYTES));
        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-j".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: oversized,
                checkpoint: None,
            },
        )
        .expect_err("超限载荷必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("上限"), "错误须说明上限：{err}");

        assert_eq!(
            count(&conn, "SELECT count(*) FROM events"),
            0,
            "拒绝必须发生在写入之前，不得半写入"
        );
        let task = crate::tasks::get_task(&conn, "T-j")
            .expect("可读")
            .expect("存在");
        assert_eq!(task.status, TaskStatus::Pending, "任务状态不得变化");
    }

    #[test]
    fn transition_rejects_mismatched_state_and_missing_task_without_side_effects() {
        let mut conn = migrated();
        insert_task(&conn, "T-1");

        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-1".into(),
                from_status: TaskStatus::Running,
                to_status: TaskStatus::Completed,
                event: transition_event(),
                checkpoint: None,
            },
        )
        .expect_err("状态不符必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable);

        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "T-none".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: transition_event(),
                checkpoint: None,
            },
        )
        .expect_err("缺失任务必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);

        let err = task_transition(
            &mut conn,
            &TaskTransition {
                task_id: "  ".into(),
                from_status: TaskStatus::Pending,
                to_status: TaskStatus::Running,
                event: transition_event(),
                checkpoint: None,
            },
        )
        .expect_err("空任务 ID 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 拒绝路径零副作用。
        assert_eq!(count(&conn, "SELECT count(*) FROM events"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM checkpoints"), 0);
        let task = crate::tasks::get_task(&conn, "T-1")
            .expect("可读")
            .expect("存在");
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[test]
    fn twenty_transitions_keep_bounded_incremental_history() {
        // FR-402：每行存单步差分对象，20 次转移后行数仍为 DEFAULT_RETAIN——
        // 不随步数增长、不复制全量历史。
        let mut conn = migrated();
        insert_task(&conn, "T-d");

        let mut from = TaskStatus::Pending;
        for step in 1..=20i64 {
            let to = match from {
                TaskStatus::Pending => TaskStatus::Running,
                TaskStatus::Running => TaskStatus::Blocked,
                _ => TaskStatus::Running,
            };
            let delta =
                serde_json::json!({ "from": from.as_str(), "to": to.as_str(), "step": step })
                    .to_string();
            let report = task_transition(
                &mut conn,
                &TaskTransition {
                    task_id: "T-d".into(),
                    from_status: from,
                    to_status: to,
                    event: transition_event(),
                    checkpoint: Some(CheckpointInsert {
                        task_id: "T-d".into(),
                        step,
                        delta_json: delta,
                        message_cursor: Some(format!("cursor-{step}")),
                        artifact_refs_json: "[]".into(),
                    }),
                },
            )
            .expect("转移");
            assert_eq!(report.event_seq, step, "事件 seq 单调且无空洞");
            from = to;
        }

        let checkpoints = crate::tasks::task_checkpoints(&conn, "T-d").expect("读取");
        assert_eq!(checkpoints.len(), DEFAULT_RETAIN, "检查点行数有界");
        assert_eq!(
            checkpoints.iter().map(|c| c.step).collect::<Vec<_>>(),
            (11..=20).collect::<Vec<_>>(),
            "保留 step 最大的差分"
        );
        for checkpoint in &checkpoints {
            let value: serde_json::Value =
                serde_json::from_str(&checkpoint.delta_json).expect("差分可解析");
            let keys: BTreeSet<String> = value
                .as_object()
                .expect("差分是 JSON 对象")
                .keys()
                .cloned()
                .collect();
            assert_eq!(
                keys,
                BTreeSet::from(["from".to_string(), "to".to_string(), "step".to_string()]),
                "每行存的是单步差分，不是累计历史"
            );
            assert!(
                checkpoint.delta_json.len() <= MAX_DELTA_JSON_BYTES,
                "差分不超过上限"
            );
        }
        assert_eq!(
            count(&conn, "SELECT count(*) FROM events"),
            20,
            "完整轨迹在事件表，检查点只存差分引用"
        );
    }

    #[test]
    fn recover_defaults_to_max_step_resolves_refs_and_reports_missing() {
        let conn = migrated();
        insert_task(&conn, "T-1");
        insert_task(&conn, "T-2");
        insert_artifact(&conn, "A-1", "T-1");
        insert_artifact(&conn, "A-2", "T-1");

        let first = insert_checkpoint(
            &conn,
            &CheckpointInsert {
                task_id: "T-1".into(),
                step: 1,
                delta_json: r#"{"from":"pending","to":"running"}"#.into(),
                message_cursor: Some("cursor-1".into()),
                artifact_refs_json: r#"["A-1"]"#.into(),
            },
        )
        .expect("检查点 1");
        let last = insert_checkpoint(
            &conn,
            &CheckpointInsert {
                task_id: "T-1".into(),
                step: 5,
                delta_json: r#"{"from":"running","to":"blocked"}"#.into(),
                message_cursor: Some("cursor-5".into()),
                // 混合形式：对象 + 字符串 + 悬空引用（产物已清理的正确的空）。
                artifact_refs_json: r#"[{"id":"A-1"},"A-2","A-gone"]"#.into(),
            },
        )
        .expect("检查点 5");
        insert_checkpoint(
            &conn,
            &CheckpointInsert {
                task_id: "T-2".into(),
                step: 1,
                delta_json: "{}".into(),
                message_cursor: None,
                artifact_refs_json: "[]".into(),
            },
        )
        .expect("T-2 检查点");

        let recovery = recover_from_checkpoint(&conn, "T-1", None).expect("默认恢复");
        assert_eq!(recovery.checkpoint.id, last, "缺省取 step 最大者");
        assert_eq!(recovery.checkpoint.step, 5);
        assert_eq!(
            recovery.message_cursor.as_deref(),
            Some("cursor-5"),
            "游标原样返回"
        );
        let ids: Vec<&str> = recovery
            .artifacts
            .iter()
            .map(|artifact| artifact.id.as_str())
            .collect();
        assert_eq!(ids, vec!["A-1", "A-2"], "悬空引用缺席，联查按 id 排序");
        assert!(recovery.artifacts.iter().all(|a| a.stage == "plan"));

        let by_id = recover_from_checkpoint(&conn, "T-1", Some(&first)).expect("指定恢复");
        assert_eq!(by_id.checkpoint.id, first);
        assert_eq!(by_id.checkpoint.step, 1);
        assert_eq!(by_id.artifacts.len(), 1);
        assert_eq!(by_id.artifacts[0].id, "A-1");

        let err = recover_from_checkpoint(&conn, "T-none", None).expect_err("缺失任务");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err =
            recover_from_checkpoint(&conn, "T-1", Some("cp-missing")).expect_err("缺失检查点");
        assert_eq!(err.code, ErrorCode::NotFound);
        let other = recover_from_checkpoint(&conn, "T-2", None).expect("T-2 恢复");
        let err = recover_from_checkpoint(&conn, "T-1", Some(&other.checkpoint.id))
            .expect_err("跨任务检查点必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = recover_from_checkpoint(&conn, "T-2", Some(&first))
            .expect_err("T-2 不能用 T-1 的检查点");
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}
