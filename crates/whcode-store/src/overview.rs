//! # 会话概览预聚合与重建（T037、FR-506、§7.3-8）
//!
//! `session_overview` 是**可重建的预聚合表**，不是事实源：`tasks` 原始行才是事实源。
//! 本模块提供：
//!
//! - [`compute_overview`]：直接读 `tasks` 计算**强一致**结果（用于核对与重建）；
//! - [`rebuild_overview`] / [`rebuild_all_overviews`]：在事务内计算并 upsert 预聚合行；
//! - [`load_overview`]：读取**最终一致**的预聚合行，缺失返回 `None`。
//!
//! ## 一致性标注
//!
//! | 字段 | 一致性 | 说明 |
//! |---|---|---|
//! | `session_id` | 强一致 | 主键；必须对应 `sessions` 中的会话 |
//! | `by_stage_json` | 最终一致 | 由 `tasks.stage` 聚合，异步刷新期间可能滞后 |
//! | `total` / `done` / `blocked` | 最终一致 | 分别对应任务总数、`completed`、`blocked` |
//! | `refreshed_at` | 最终一致 | 上次重建时间；不是数据版本号 |
//!
//! 需要强一致计数的调用方应使用 [`compute_overview`]；界面概览使用 [`load_overview`]
//! 并接受 `refreshed_at` 指示的滞后。自动刷新节奏由应用层决定（T048 起），本模块不隐式
//! 在每次任务写入时更新，避免把预聚合变成第二个事实源。
//!
//! 边界：只读写 `session_overview` 与 `tasks`/`sessions` 的只读查询；不负责权限与展示。

use std::collections::BTreeMap;

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 强一致字段：键与会话存在性。
pub const OVERVIEW_STRONG_FIELDS: &[&str] = &["session_id"];

/// 最终一致字段：可由 `tasks` 重建的聚合与刷新时间。
pub const OVERVIEW_EVENTUAL_FIELDS: &[&str] =
    &["by_stage_json", "total", "done", "blocked", "refreshed_at"];

/// 会话概览（与 `session_overview` 表一一对应）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOverview {
    /// 会话 ID（主键，强一致）。
    pub session_id: String,
    /// 各阶段任务数（按阶段名排序，最终一致）。
    pub by_stage: BTreeMap<String, i64>,
    /// 任务总数（最终一致）。
    pub total: i64,
    /// `completed` 任务数（最终一致）。
    pub done: i64,
    /// `blocked` 任务数（最终一致）。
    pub blocked: i64,
    /// 上次重建的 Unix 微秒时间戳（最终一致）。
    pub refreshed_at_us: i64,
}

fn db_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}

/// 直接读 `tasks` 计算概览（强一致），不写库。
///
/// `refreshed_at_us` 为本次计算时间。
///
/// # Errors
///
/// 数据库查询或 `by_stage_json` 序列化失败时返回 [`WhError`]。
pub fn compute_overview(conn: &Connection, session_id: &str) -> Result<SessionOverview, WhError> {
    compute_with(conn, session_id)
}

/// 重建单个会话的预聚合行；会话不存在时返回 `not_found`。
///
/// 计算与 upsert 在同一事务内完成，失败时不留下半更新的概览行。
///
/// # Errors
///
/// 会话不存在、数据库查询或写入失败时返回 [`WhError`]。
pub fn rebuild_overview(
    conn: &mut Connection,
    session_id: &str,
) -> Result<SessionOverview, WhError> {
    let tx = conn
        .transaction()
        .map_err(|err| db_err("开始概览重建事务", err))?;
    if !session_exists(&tx, session_id)? {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在，无法重建概览：{session_id}"),
        ));
    }
    let overview = compute_with(&tx, session_id)?;
    write_overview(&tx, &overview)?;
    tx.commit().map_err(|err| db_err("提交概览重建事务", err))?;
    Ok(overview)
}

/// 重建全部会话的预聚合行（含无任务会话，计数为零）；返回重建的会话数。
///
/// # Errors
///
/// 数据库查询或写入失败时返回 [`WhError`]；整个重建在单一事务内完成。
pub fn rebuild_all_overviews(conn: &mut Connection) -> Result<usize, WhError> {
    let tx = conn
        .transaction()
        .map_err(|err| db_err("开始概览批量重建事务", err))?;
    let session_ids = {
        let mut stmt = tx
            .prepare("SELECT id FROM sessions ORDER BY id")
            .map_err(|err| db_err("读取会话清单", err))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| db_err("读取会话清单", err))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(|err| db_err("读取会话清单", err))?);
        }
        ids
    };

    for session_id in &session_ids {
        let overview = compute_with(&tx, session_id)?;
        write_overview(&tx, &overview)?;
    }
    tx.commit()
        .map_err(|err| db_err("提交概览批量重建事务", err))?;
    Ok(session_ids.len())
}

/// 读取预聚合概览（最终一致）；不存在返回 `None`。
///
/// # Errors
///
/// 数据库查询失败或 `by_stage_json` 损坏时返回 [`WhError`]。
pub fn load_overview(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionOverview>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT session_id, by_stage_json, total, done, blocked, epoch_us(refreshed_at)
             FROM session_overview WHERE session_id = ?",
        )
        .map_err(|err| db_err("准备概览查询", err))?;
    let mut rows = stmt
        .query(params![session_id])
        .map_err(|err| db_err("查询概览", err))?;
    let Some(row) = rows.next().map_err(|err| db_err("读取概览", err))? else {
        return Ok(None);
    };
    let by_stage_json: String = row.get(1).map_err(|err| db_err("读取概览阶段分布", err))?;
    let by_stage: BTreeMap<String, i64> = serde_json::from_str(&by_stage_json)
        .map_err(|err| WhError::parse_error(format!("概览 by_stage_json 无法解析：{err}")))?;
    Ok(Some(SessionOverview {
        session_id: row.get(0).map_err(|err| db_err("读取概览会话", err))?,
        by_stage,
        total: row.get(2).map_err(|err| db_err("读取概览总数", err))?,
        done: row.get(3).map_err(|err| db_err("读取概览完成数", err))?,
        blocked: row.get(4).map_err(|err| db_err("读取概览阻塞数", err))?,
        refreshed_at_us: row.get(5).map_err(|err| db_err("读取概览刷新时间", err))?,
    }))
}

fn session_exists(tx: &duckdb::Transaction<'_>, session_id: &str) -> Result<bool, WhError> {
    let count: i64 = tx
        .query_row(
            "SELECT count(*) FROM sessions WHERE id = ?",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(|err| db_err("检查会话存在性", err))?;
    Ok(count > 0)
}

fn compute_with(conn: &Connection, session_id: &str) -> Result<SessionOverview, WhError> {
    let (total, done, blocked): (i64, i64, i64) = conn
        .query_row(
            "SELECT count(*),
                    count(*) FILTER (WHERE status = 'completed'),
                    count(*) FILTER (WHERE status = 'blocked')
             FROM tasks WHERE session_id = ?",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|err| db_err("聚合任务状态", err))?;

    let by_stage = {
        let mut stmt = conn
            .prepare(
                "SELECT stage, count(*) FROM tasks
                 WHERE session_id = ? GROUP BY stage ORDER BY stage",
            )
            .map_err(|err| db_err("准备阶段聚合", err))?;
        let rows = stmt
            .query_map(params![session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|err| db_err("聚合任务阶段", err))?;
        let mut counts = BTreeMap::new();
        for row in rows {
            let (stage, count) = row.map_err(|err| db_err("聚合任务阶段", err))?;
            counts.insert(stage, count);
        }
        counts
    };

    let refreshed_at_us: i64 = conn
        .query_row("SELECT epoch_us(current_timestamp)", [], |row| row.get(0))
        .map_err(|err| db_err("读取概览刷新时间", err))?;

    Ok(SessionOverview {
        session_id: session_id.to_string(),
        by_stage,
        total,
        done,
        blocked,
        refreshed_at_us,
    })
}

/// 写入概览行（MERGE INTO upsert，DuckDB 2.0；ADR-034）。
///
/// `USING` 子查询只产出一行且按 `session_overview.session_id` 主键匹配，故至多匹配一条
/// 目标行；2.0 的“多源匹配同一目标行即报错”语义在这里是结构性保证，不依赖调用方去重。
/// 参数在 `USING` 内显式 `CAST`，避免 `MERGE` 上下文中的参数类型推断歧义。
fn write_overview(conn: &Connection, overview: &SessionOverview) -> Result<(), WhError> {
    let by_stage_json = serde_json::to_string(&overview.by_stage)
        .map_err(|err| WhError::internal(format!("概览阶段分布序列化失败：{err}")))?;
    conn.execute(
        "MERGE INTO session_overview AS t
         USING (
             SELECT CAST(? AS VARCHAR) AS session_id,
                    CAST(? AS VARCHAR) AS by_stage_json,
                    CAST(? AS BIGINT) AS total,
                    CAST(? AS BIGINT) AS done,
                    CAST(? AS BIGINT) AS blocked,
                    make_timestamp(CAST(? AS BIGINT)) AS refreshed_at
         ) AS src
         ON t.session_id = src.session_id
         WHEN MATCHED THEN UPDATE SET
             by_stage_json = src.by_stage_json,
             total = src.total,
             done = src.done,
             blocked = src.blocked,
             refreshed_at = src.refreshed_at
         WHEN NOT MATCHED THEN INSERT
             (session_id, by_stage_json, total, done, blocked, refreshed_at)
             VALUES (src.session_id, src.by_stage_json, src.total, src.done,
                     src.blocked, src.refreshed_at)",
        params![
            overview.session_id.as_str(),
            by_stage_json,
            overview.total,
            overview.done,
            overview.blocked,
            overview.refreshed_at_us
        ],
    )
    .map_err(|err| db_err("写入概览", err))?;
    Ok(())
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

    fn insert_session(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES (?, ?, current_timestamp, current_timestamp, 'active')",
            params![id, format!("会话 {id}")],
        )
        .expect("插入会话");
    }

    fn insert_task(conn: &Connection, id: &str, session: &str, stage: &str, status: &str) {
        conn.execute(
            "INSERT INTO tasks
             (id, session_id, title, stage, status, parent_id, acceptance_json, retry_count,
              updated_at)
             VALUES (?, ?, ?, ?, ?, NULL, '[]', 0, current_timestamp)",
            params![id, session, format!("任务 {id}"), stage, status],
        )
        .expect("插入任务");
    }

    fn fixture(conn: &Connection) {
        insert_session(conn, "S-1");
        insert_task(conn, "T-1", "S-1", "Implement", "completed");
        insert_task(conn, "T-2", "S-1", "Implement", "completed");
        insert_task(conn, "T-3", "S-1", "Implement", "blocked");
        insert_task(conn, "T-4", "S-1", "Plan", "running");
        insert_task(conn, "T-5", "S-1", "Plan", "failed");
    }

    #[test]
    fn computes_counts_and_stage_distribution_from_tasks() {
        let conn = migrated();
        fixture(&conn);

        let overview = compute_overview(&conn, "S-1").expect("计算概览");
        assert_eq!(overview.total, 5);
        assert_eq!(overview.done, 2, "仅 completed 计入完成数");
        assert_eq!(overview.blocked, 1, "仅 blocked 计入阻塞数");
        assert_eq!(
            overview.by_stage,
            BTreeMap::from([("Implement".to_string(), 3), ("Plan".to_string(), 2)])
        );
        assert!(overview.refreshed_at_us > 0);
    }

    #[test]
    fn rebuild_persists_row_and_load_matches_compute() {
        let mut conn = migrated();
        fixture(&conn);

        let rebuilt = rebuild_overview(&mut conn, "S-1").expect("重建概览");
        let loaded = load_overview(&conn, "S-1")
            .expect("读取概览")
            .expect("必有行");
        assert_eq!(loaded, rebuilt, "重建返回值必须与库内行一致");

        let recomputed = compute_overview(&conn, "S-1").expect("再计算");
        assert_eq!(loaded.total, recomputed.total);
        assert_eq!(loaded.done, recomputed.done);
        assert_eq!(loaded.blocked, recomputed.blocked);
        assert_eq!(loaded.by_stage, recomputed.by_stage);
    }

    #[test]
    fn stale_row_is_detected_and_rebuild_refreshes_it() {
        let mut conn = migrated();
        fixture(&conn);
        rebuild_overview(&mut conn, "S-1").expect("首次重建");

        conn.execute("UPDATE tasks SET status = 'completed' WHERE id = 'T-4'", [])
            .expect("更新任务");
        let stale = load_overview(&conn, "S-1").expect("读取").expect("必有行");
        assert_eq!(stale.done, 2, "预聚合允许滞后于原始数据");
        assert_eq!(
            compute_overview(&conn, "S-1").expect("强一致计算").done,
            3,
            "原始数据已变化"
        );

        let refreshed = rebuild_overview(&mut conn, "S-1").expect("再次重建");
        assert_eq!(refreshed.done, 3);
        assert_eq!(
            load_overview(&conn, "S-1")
                .expect("读取")
                .expect("必有行")
                .done,
            3
        );

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM session_overview", [], |row| {
                row.get(0)
            })
            .expect("统计概览行");
        assert_eq!(rows, 1, "重建必须是 upsert，不得重复插入");
    }

    /// 数据未变时重复刷新：MERGE 命中已存在行只更新，不得插入重复行，计数保持稳定。
    #[test]
    fn repeated_refresh_is_idempotent_and_keeps_single_row() {
        let mut conn = migrated();
        fixture(&conn);

        let first = rebuild_overview(&mut conn, "S-1").expect("首次重建");
        for _ in 0..3 {
            let again = rebuild_overview(&mut conn, "S-1").expect("重复重建");
            assert_eq!(again.session_id, first.session_id);
            assert_eq!(again.by_stage, first.by_stage, "数据未变时阶段分布必须一致");
            assert_eq!(again.total, first.total);
            assert_eq!(again.done, first.done);
            assert_eq!(again.blocked, first.blocked);
            assert!(
                again.refreshed_at_us >= first.refreshed_at_us,
                "刷新时间不得回退"
            );
        }

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM session_overview", [], |row| {
                row.get(0)
            })
            .expect("统计概览行");
        assert_eq!(rows, 1, "MERGE 必须 upsert 单行，不得重复插入");
    }

    #[test]
    fn rebuild_all_includes_sessions_without_tasks() {
        let mut conn = migrated();
        fixture(&conn);
        insert_session(&conn, "S-empty");

        let rebuilt = rebuild_all_overviews(&mut conn).expect("批量重建");
        assert_eq!(rebuilt, 2);

        let empty = load_overview(&conn, "S-empty")
            .expect("读取空会话")
            .expect("空会话也应有概览");
        assert_eq!(empty.total, 0);
        assert_eq!(empty.done, 0);
        assert_eq!(empty.blocked, 0);
        assert!(empty.by_stage.is_empty());

        assert_eq!(
            load_overview(&conn, "S-1")
                .expect("读取")
                .expect("必有行")
                .total,
            5
        );
    }

    #[test]
    fn missing_overview_returns_none_and_unknown_session_is_rejected() {
        let mut conn = migrated();
        assert_eq!(
            load_overview(&conn, "S-missing").expect("读取"),
            None,
            "未重建时不得伪造概览"
        );

        let err = rebuild_overview(&mut conn, "S-missing").expect_err("未知会话必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(!err.retryable, "未知会话不是可重试错误");

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM session_overview", [], |row| {
                row.get(0)
            })
            .expect("统计概览行");
        assert_eq!(rows, 0, "失败的重建不得留下概览行");
    }

    #[test]
    fn consistency_annotation_partitions_fields() {
        assert_eq!(OVERVIEW_STRONG_FIELDS, &["session_id"]);
        assert!(OVERVIEW_EVENTUAL_FIELDS.contains(&"total"));
        assert!(OVERVIEW_EVENTUAL_FIELDS.contains(&"by_stage_json"));
        assert!(OVERVIEW_EVENTUAL_FIELDS.contains(&"refreshed_at"));
        for field in OVERVIEW_STRONG_FIELDS {
            assert!(
                !OVERVIEW_EVENTUAL_FIELDS.contains(field),
                "字段不得同时标注为强一致与最终一致：{field}"
            );
        }
    }
}
