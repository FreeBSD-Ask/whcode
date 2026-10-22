//! # 数据库监控与查询计划分析（T038、FR-507、NFR-106）
//!
//! 一次采集输出四类信息：
//! 1. **数据库统计**：引擎版本、schema 版本、库名/大小、块统计、内存占用与上限；
//! 2. **表规模**：`main` schema 下各表精确行数；
//! 3. **WAL 状态**：WAL 大小、自动 checkpoint 阈值、checkpoint 阈值；
//! 4. **写者队列**：排队深度与容量（由调用方从 [`WriterService`](crate::WriterService) 读取后传入）。
//!
//! 另提供 [`plan_query`]：对**只读**查询执行 `EXPLAIN` 或 `EXPLAIN ANALYZE`，供
//! `db explain` 与诊断使用。`EXPLAIN ANALYZE` 会实际执行查询，因此写语句一律拒绝
//! （`permission_denied`），不依赖调用方自觉。
//!
//! 测量纪律（§5.2）：大小字段保留 DuckDB 的人类可读字符串（如 `512.0 KiB`），不在此层
//! 做有损解析；需要字节数的调用方须显式换算并在报告中写明口径。所有指标不依赖图形界面
//! （NFR-106）。

use duckdb::Connection;
use serde::Serialize;
use whcode_domain::{ErrorCode, WhError};

use crate::schema::schema_version;
use crate::service::WriterService;

/// 数据库级统计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatabaseStats {
    /// DuckDB 引擎版本。
    pub engine_version: String,
    /// 已应用的 schema 版本。
    pub schema_version: u32,
    /// 数据库名（文件库为文件名，不含扩展名）。
    pub database_name: String,
    /// 数据库文件大小（DuckDB 原文，如 `512.0 KiB`）。
    pub database_size: String,
    /// 块大小（字节）。
    pub block_size: i64,
    /// 总块数。
    pub total_blocks: i64,
    /// 已用块数。
    pub used_blocks: i64,
    /// 空闲块数。
    pub free_blocks: i64,
    /// DuckDB 缓冲管理器占用（原文）。
    pub memory_usage: String,
    /// `memory_limit` 设置（原文）。
    pub memory_limit: String,
}

/// 单表规模。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableStat {
    /// 表名。
    pub table: String,
    /// 精确行数（`count(*)`，非估算）。
    pub rows: i64,
}

/// WAL 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WalStatus {
    /// 当前 WAL 大小（DuckDB 原文）。
    pub wal_size: String,
    /// `wal_autocheckpoint` 设置（原文）。
    pub wal_autocheckpoint: String,
    /// `checkpoint_threshold` 设置（原文）。
    pub checkpoint_threshold: String,
}

/// 写者队列快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WriterStats {
    /// 排队等待写线程处理的请求数。
    pub queued: usize,
    /// 有界队列容量。
    pub capacity: usize,
}

impl WriterStats {
    /// 从写者服务读取队列深度与容量。
    pub fn from_service(service: &WriterService) -> Self {
        Self {
            queued: service.queued_len(),
            capacity: service.capacity(),
        }
    }
}

/// 查询计划的一行（DuckDB `EXPLAIN` 的 `(key, value)`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanRow {
    /// 计划键（如 `physical_plan`）。
    pub key: String,
    /// 计划文本。
    pub value: String,
}

/// 一次查询计划分析。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryPlan {
    /// 被分析的 SQL。
    pub sql: String,
    /// 是否使用 `EXPLAIN ANALYZE`（会实际执行查询）。
    pub analyze: bool,
    /// 计划行。
    pub rows: Vec<PlanRow>,
}

/// 监控报告（四类信息 + 可选查询计划）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorReport {
    /// 数据库统计。
    pub database: DatabaseStats,
    /// 各表行数。
    pub tables: Vec<TableStat>,
    /// WAL 状态。
    pub wal: WalStatus,
    /// 写者队列快照；未提供写者时为 `None`。
    pub writer: Option<WriterStats>,
    /// 查询计划；未提供查询时为空。
    pub plans: Vec<QueryPlan>,
}

fn db_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}

/// 采集数据库统计、表规模与 WAL 状态，并为给定只读查询附上 `EXPLAIN`。
///
/// # Errors
///
/// 数据库查询失败，或 `queries` 中含非只读语句时返回 [`WhError`]。
pub fn collect(
    conn: &Connection,
    writer: Option<WriterStats>,
    queries: &[&str],
) -> Result<MonitorReport, WhError> {
    let database = database_stats(conn)?;
    let tables = table_stats(conn)?;
    let wal = wal_status(conn)?;
    let mut plans = Vec::with_capacity(queries.len());
    for sql in queries {
        plans.push(plan_query(conn, sql, false)?);
    }
    Ok(MonitorReport {
        database,
        tables,
        wal,
        writer,
        plans,
    })
}

/// 读取数据库统计。
///
/// # Errors
///
/// `PRAGMA database_size` 或 schema 版本不可读时返回 `io_error`。
pub fn database_stats(conn: &Connection) -> Result<DatabaseStats, WhError> {
    let engine_version: String = conn
        .query_row("SELECT version()", [], |row| row.get(0))
        .map_err(|err| db_err("读取引擎版本", err))?;
    let (
        database_name,
        database_size,
        block_size,
        total_blocks,
        used_blocks,
        free_blocks,
        memory_usage,
        memory_limit,
    ): (String, String, i64, i64, i64, i64, String, String) = conn
        .query_row(
            "SELECT database_name, database_size, block_size, total_blocks, used_blocks,
                    free_blocks, memory_usage, memory_limit
             FROM pragma_database_size()",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .map_err(|err| db_err("读取数据库大小", err))?;

    Ok(DatabaseStats {
        engine_version,
        schema_version: schema_version(conn)?,
        database_name,
        database_size,
        block_size,
        total_blocks,
        used_blocks,
        free_blocks,
        memory_usage,
        memory_limit,
    })
}

/// 统计 `main` schema 下各表精确行数（按表名排序）。
///
/// # Errors
///
/// 表目录或行数查询失败时返回 `io_error`。
pub fn table_stats(conn: &Connection) -> Result<Vec<TableStat>, WhError> {
    let names = {
        let mut stmt = conn
            .prepare(
                "SELECT table_name FROM duckdb_tables()
                 WHERE schema_name = 'main' AND internal = false AND temporary = false
                 ORDER BY table_name",
            )
            .map_err(|err| db_err("准备表清单查询", err))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|err| db_err("读取表清单", err))?;
        let mut names = Vec::new();
        for row in rows {
            names.push(row.map_err(|err| db_err("读取表清单", err))?);
        }
        names
    };

    let mut stats = Vec::with_capacity(names.len());
    for name in names {
        let sql = format!("SELECT count(*) FROM {}", quote_identifier(&name));
        let rows: i64 = conn
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|err| db_err("统计表行数", err))?;
        stats.push(TableStat { table: name, rows });
    }
    Ok(stats)
}

/// 读取 WAL 状态。
///
/// # Errors
///
/// `PRAGMA database_size` 或 WAL 设置不可读时返回 `io_error`。
pub fn wal_status(conn: &Connection) -> Result<WalStatus, WhError> {
    let wal_size: String = conn
        .query_row("SELECT wal_size FROM pragma_database_size()", [], |row| {
            row.get(0)
        })
        .map_err(|err| db_err("读取 WAL 大小", err))?;
    let wal_autocheckpoint: String = conn
        .query_row("SELECT current_setting('wal_autocheckpoint')", [], |row| {
            row.get(0)
        })
        .map_err(|err| db_err("读取 wal_autocheckpoint", err))?;
    let checkpoint_threshold: String = conn
        .query_row(
            "SELECT current_setting('checkpoint_threshold')",
            [],
            |row| row.get(0),
        )
        .map_err(|err| db_err("读取 checkpoint_threshold", err))?;
    Ok(WalStatus {
        wal_size,
        wal_autocheckpoint,
        checkpoint_threshold,
    })
}

/// 对只读查询执行 `EXPLAIN` 或 `EXPLAIN ANALYZE`。
///
/// # Errors
///
/// 非只读语句返回 `permission_denied`（不可重试）；查询失败返回 `io_error`。
pub fn plan_query(conn: &Connection, sql: &str, analyze: bool) -> Result<QueryPlan, WhError> {
    ensure_read_only(sql)?;
    let prefix = if analyze {
        "EXPLAIN ANALYZE "
    } else {
        "EXPLAIN "
    };
    let mut stmt = conn
        .prepare(&format!("{prefix}{sql}"))
        .map_err(|err| db_err("准备查询计划", err))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(PlanRow {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })
        .map_err(|err| db_err("读取查询计划", err))?;
    let mut plan = Vec::new();
    for row in rows {
        plan.push(row.map_err(|err| db_err("读取查询计划", err))?);
    }
    Ok(QueryPlan {
        sql: sql.to_string(),
        analyze,
        rows: plan,
    })
}

/// 只允许 `SELECT`/`WITH`/`VALUES` 开头的查询，避免 `EXPLAIN ANALYZE` 执行写语句。
fn ensure_read_only(sql: &str) -> Result<(), WhError> {
    let head = sql
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if matches!(head.as_str(), "SELECT" | "WITH" | "VALUES") {
        Ok(())
    } else {
        Err(WhError::permission_denied(
            "db explain 仅接受只读查询（SELECT/WITH/VALUES）；拒绝执行写语句或未知语句",
        ))
    }
}

/// 以双引号包裹标识符并转义内部双引号（目录中的表名同样不当作可信 SQL）。
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{STORE_SCHEMA_VERSION, TABLE_COUNT, apply_migrations};
    use duckdb::params;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_event(conn: &Connection, seq: i64) {
        conn.execute(
            "INSERT INTO events
             (seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts,
              persistence_level)
             VALUES (?, ?, 'S-1', NULL, NULL, NULL, 'StreamingChunk', '{}',
                     current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}")],
        )
        .expect("插入事件");
    }

    #[test]
    fn collect_reports_database_tables_wal_writer_and_plans() {
        let conn = migrated();
        insert_event(&conn, 1);
        insert_event(&conn, 2);

        let writer = Some(WriterStats {
            queued: 3,
            capacity: 16,
        });
        let report = collect(
            &conn,
            writer,
            &["SELECT seq FROM events WHERE session_id = 'S-1' ORDER BY seq LIMIT 10"],
        )
        .expect("采集监控");

        assert!(!report.database.engine_version.is_empty());
        assert_eq!(report.database.schema_version, STORE_SCHEMA_VERSION);
        assert!(!report.database.database_name.is_empty());
        assert!(!report.database.database_size.is_empty());
        assert!(
            report.database.used_blocks + report.database.free_blocks
                == report.database.total_blocks,
            "块统计必须自洽"
        );
        assert!(!report.database.memory_usage.is_empty());
        assert!(!report.database.memory_limit.is_empty());

        assert_eq!(report.tables.len(), TABLE_COUNT);
        let events = report
            .tables
            .iter()
            .find(|stat| stat.table == "events")
            .expect("必须有 events 表");
        assert_eq!(events.rows, 2);

        assert!(!report.wal.wal_size.is_empty());
        assert!(!report.wal.wal_autocheckpoint.is_empty());
        assert!(!report.wal.checkpoint_threshold.is_empty());

        assert_eq!(
            report.writer,
            Some(WriterStats {
                queued: 3,
                capacity: 16
            })
        );
        assert_eq!(report.plans.len(), 1, "必须包含查询计划");
        assert!(!report.plans[0].rows.is_empty(), "计划不得为空");
    }

    #[test]
    fn table_stats_are_exact_counts() {
        let conn = migrated();
        insert_event(&conn, 1);
        insert_event(&conn, 2);
        insert_event(&conn, 3);
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES ('M-1', 'S-1', 'user', 'ref', NULL, current_timestamp, NULL)",
            [],
        )
        .expect("插入消息");

        let stats = table_stats(&conn).expect("表统计");
        assert_eq!(
            stats
                .iter()
                .find(|stat| stat.table == "events")
                .map(|s| s.rows),
            Some(3)
        );
        assert_eq!(
            stats
                .iter()
                .find(|stat| stat.table == "messages")
                .map(|s| s.rows),
            Some(1)
        );
        assert_eq!(stats.len(), TABLE_COUNT);
    }

    #[test]
    fn collect_without_writer_or_queries_has_optional_sections_empty() {
        let conn = migrated();
        let report = collect(&conn, None, &[]).expect("采集监控");
        assert_eq!(report.writer, None);
        assert!(report.plans.is_empty());
    }

    #[test]
    fn plan_query_returns_explain_rows_and_analyze_variant() {
        let conn = migrated();
        insert_event(&conn, 1);

        let plan = plan_query(&conn, "SELECT seq FROM events", false).expect("EXPLAIN");
        assert!(!plan.rows.is_empty());
        assert!(!plan.analyze);

        let analyzed = plan_query(&conn, "SELECT seq FROM events", true).expect("EXPLAIN ANALYZE");
        assert!(!analyzed.rows.is_empty());
        assert!(analyzed.analyze);
    }

    #[test]
    fn plan_query_rejects_write_statements() {
        let conn = migrated();
        for sql in [
            "DELETE FROM events",
            "INSERT INTO events (seq) VALUES (1)",
            "UPDATE events SET kind = 'x'",
            "DROP TABLE events",
        ] {
            let err = plan_query(&conn, sql, true).expect_err("写语句必须拒绝");
            assert_eq!(err.code, ErrorCode::PermissionDenied, "SQL：{sql}");
            assert!(!err.retryable);
        }
    }
}
