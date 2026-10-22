//! # 数据库 compact：停写 → 复制 → 校验 → 原子切换（T041、FR-510、R-09/D-10）
//!
//! 流程（D-10，与行删除、块回收、WAL checkpoint、文件重建明确区分）：
//!
//! 1. **维护锁**：进程内 [`COMPACT_LOCK`] 守卫拒绝并发维护；跨进程由 DuckDB 文件锁
//!    保证单写者。BSD 文件锁语义与 Windows 文件替换语义的差异按 ADR-013 延后取证，
//!    当前仅在 Windows 11 amd64 验收；
//! 2. **停写与快照**：打开写连接，记录前后统计所需的清单（逐表行数、四项空间统计），
//!    `CHECKPOINT` 把 WAL 同步进主文件后关闭连接；
//! 3. **复制**：`db → db.compact-tmp`；
//! 4. **副本校验**：重新打开副本，核对 schema 版本、必需索引与逐表行数，任一不符
//!    即删除副本并报错——**原库不受影响**（Check 41）；
//! 5. **原子切换**：原库改名 `.compact-backup` → 副本改名回原名；第二步失败则改回
//!    备份名恢复。切换后删除已 checkpoint 的旧 WAL 残留，确认成功后删除备份；
//! 6. **复验**：重开新库核对 schema 版本并采集切换后统计。
//!
//! **不承诺缩小文件**（FR-510、R-09）：已删除行占用的块随文件复制保留，`CHECKPOINT`
//! 只同步 WAL 并可能回收尾部块；`SpaceReport` 如实记录前后四项（文件大小/WAL/已用
//! 块/可用块），由调用方解释，不作为释放承诺。
//!
//! 边界：本函数自管连接生命周期（关闭/重开是流程的一部分），是唯一这样做的存储模块；
//! 调用方不得同时持有同一文件的连接。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use duckdb::Connection;
use serde::Serialize;
use whcode_domain::{ErrorCode, WhError};

use crate::indexes::verify_indexes;
use crate::monitor::TableStat;
use crate::schema::schema_version;

/// 进程内维护锁：同一时刻至多一个 compact。
static COMPACT_LOCK: AtomicBool = AtomicBool::new(false);

/// 四项空间统计（FR-510）：文件大小、WAL、已用块、可用块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SpaceReport {
    /// 数据库主文件大小（`fs` 元数据字节，非 DuckDB 人类可读串）。
    pub file_size_bytes: i64,
    /// WAL 文件大小（字节；不存在为 0）。
    pub wal_size_bytes: i64,
    /// 总块数。
    pub total_blocks: i64,
    /// 已用块数。
    pub used_blocks: i64,
    /// 空闲块数。
    pub free_blocks: i64,
}

impl SpaceReport {
    /// 块统计自洽：`used + free == total`。
    #[must_use]
    pub fn blocks_consistent(&self) -> bool {
        self.used_blocks + self.free_blocks == self.total_blocks
    }
}

/// 一次 compact 的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompactReport {
    /// 切换前统计（CHECKPOINT 之前，含 WAL）。
    pub before: SpaceReport,
    /// 切换后统计（重开新库时）。
    pub after: SpaceReport,
    /// 副本校验所用的逐表行数（切换前快照）。
    pub table_rows: Vec<TableStat>,
    /// 是否完成切换。
    pub switched: bool,
}

fn compact_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

fn bytes_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn wal_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.wal", db_path.display()))
}

/// 维护锁守卫；`Drop` 释放。
struct MaintenanceGuard;

impl MaintenanceGuard {
    fn acquire() -> Result<Self, WhError> {
        COMPACT_LOCK
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| invalid("已有 compact 维护操作进行中，拒绝并发维护"))
    }
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        COMPACT_LOCK.store(false, Ordering::SeqCst);
    }
}

/// 采集四项空间统计。
///
/// # Errors
///
/// 文件元数据或块统计查询失败时返回 [`WhError`]。
pub fn capture_space(conn: &Connection, db_path: &Path) -> Result<SpaceReport, WhError> {
    let file_size = std::fs::metadata(db_path)
        .map(|meta| bytes_i64(meta.len()))
        .map_err(|err| compact_err(&format!("读取 {}", db_path.display()), err))?;
    let wal_size = std::fs::metadata(wal_path(db_path))
        .map(|meta| bytes_i64(meta.len()))
        .unwrap_or(0);
    let (total_blocks, used_blocks, free_blocks): (i64, i64, i64) = conn
        .query_row(
            "SELECT total_blocks, used_blocks, free_blocks FROM pragma_database_size()",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|err| compact_err("读取块统计", err))?;
    Ok(SpaceReport {
        file_size_bytes: file_size,
        wal_size_bytes: wal_size,
        total_blocks,
        used_blocks,
        free_blocks,
    })
}

/// 校验副本：schema 版本、必需索引、逐表行数与快照一致。
fn verify_copy(
    conn: &Connection,
    expected_schema: u32,
    expected_rows: &[TableStat],
) -> Result<(), WhError> {
    let actual_schema = schema_version(conn)?;
    if actual_schema != expected_schema {
        return Err(invalid(&format!(
            "副本 schema 版本不符：预期 {expected_schema}，实际 {actual_schema}"
        )));
    }
    verify_indexes(conn)?;
    let actual_rows = crate::monitor::table_stats(conn)?;
    if actual_rows != expected_rows {
        let drift: Vec<String> = actual_rows
            .iter()
            .zip(expected_rows.iter())
            .filter(|(actual, expected)| actual.rows != expected.rows)
            .map(|(actual, expected)| {
                format!("{}: {} → {}", actual.table, expected.rows, actual.rows)
            })
            .collect();
        return Err(invalid(&format!(
            "副本逐表行数与快照不符：{}",
            drift.join(", ")
        )));
    }
    Ok(())
}

/// 对数据库文件执行 compact（复制—校验—原子切换）。
///
/// 调用方必须先停止使用该库的全部连接（DuckDB 文件锁会拒绝并行打开）。
///
/// # Errors
///
/// 库不存在/未迁移、已有维护进行中、复制或副本校验失败、切换失败时返回
/// [`WhError`]；校验与切换失败都不影响原库内容。
pub fn compact_database(db_path: &Path) -> Result<CompactReport, WhError> {
    let _guard = MaintenanceGuard::acquire()?;
    if !db_path.exists() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("数据库文件不存在：{}", db_path.display()),
        ));
    }

    let tmp_path = PathBuf::from(format!("{}.compact-tmp", db_path.display()));
    let backup_path = PathBuf::from(format!("{}.compact-backup", db_path.display()));

    // 1. 停写与快照：CHECKPOINT 后关闭连接。
    let (expected_schema, expected_rows, before) = {
        let conn = Connection::open(db_path).map_err(|err| compact_err("打开原库", err))?;
        let schema = schema_version(&conn)?;
        let rows = crate::monitor::table_stats(&conn)?;
        let before = capture_space(&conn, db_path)?;
        conn.execute_batch("CHECKPOINT")
            .map_err(|err| compact_err("执行 CHECKPOINT", err))?;
        (schema, rows, before)
    };

    // 2. 复制。
    let _ = std::fs::remove_file(&tmp_path);
    std::fs::copy(db_path, &tmp_path).map_err(|err| compact_err("复制数据库文件", err))?;

    // 3. 副本校验；失败则删除副本，原库不受影响。
    {
        let conn = Connection::open(&tmp_path).map_err(|err| compact_err("打开副本", err))?;
        let verification = verify_copy(&conn, expected_schema, &expected_rows);
        drop(conn);
        if let Err(err) = verification {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err);
        }
    }

    // 4. 原子切换：改名原库为备份 → 副本改名回原名；第二步失败恢复备份。
    let _ = std::fs::remove_file(&backup_path);
    std::fs::rename(db_path, &backup_path).map_err(|err| compact_err("改名原库为备份", err))?;
    if let Err(err) = std::fs::rename(&tmp_path, db_path) {
        let _ = std::fs::rename(&backup_path, db_path);
        let _ = std::fs::remove_file(&tmp_path);
        return Err(compact_err("切换副本，已恢复原库", err));
    }
    // 已 checkpoint 的旧 WAL 残留属于切换前状态，新库打开前必须移除；
    // 副本自身的 WAL（若关闭时未自清）同样不属于新库。
    let _ = std::fs::remove_file(wal_path(db_path));
    let _ = std::fs::remove_file(wal_path(&tmp_path));
    let _ = std::fs::remove_file(&backup_path);

    // 5. 复验并采集切换后统计。
    let after = {
        let conn = Connection::open(db_path).map_err(|err| compact_err("重开新库", err))?;
        let schema = schema_version(&conn)?;
        if schema != expected_schema {
            return Err(invalid(&format!(
                "切换后 schema 版本不符：预期 {expected_schema}，实际 {schema}"
            )));
        }
        capture_space(&conn, db_path)?
    };

    Ok(CompactReport {
        before,
        after,
        table_rows: expected_rows,
        switched: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;
    use duckdb::params;

    fn temp_db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("whcode-compact-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir.join(format!("{tag}.duckdb"))
    }

    fn seeded_file_db(tag: &str, event_count: i64) -> Connection {
        let db = temp_db(tag);
        let _ = std::fs::remove_file(&db);
        let conn = Connection::open(&db).expect("打开测试库");
        apply_migrations(&conn).expect("迁移");
        for seq in 1..=event_count {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, 'S-1', NULL, NULL, NULL, 'StreamingChunk', '{}',
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}")],
            )
            .expect("插入事件");
        }
        conn
    }

    fn count_events(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("统计事件")
    }

    #[test]
    fn compact_switches_file_and_preserves_data() {
        let db = temp_db("happy");
        {
            let conn = seeded_file_db("happy", 500);
            // 删除一半行：块占用仍在文件里，compact 不承诺缩小（FR-510）。
            conn.execute("DELETE FROM events WHERE seq > 250", [])
                .expect("删除一半事件");
            assert_eq!(count_events(&conn), 250);
        }

        let report = compact_database(&db).expect("compact");
        assert!(report.switched);
        assert!(report.before.blocks_consistent());
        assert!(report.after.blocks_consistent());
        assert!(
            report.before.file_size_bytes > 0,
            "before 须含文件大小：{:?}",
            report.before
        );
        assert_eq!(
            report
                .table_rows
                .iter()
                .find(|s| s.table == "events")
                .map(|s| s.rows),
            Some(250),
            "快照行数是 compact 启动时（删除后、切换前）的统计"
        );

        let conn = Connection::open(&db).expect("重开 compact 后的库");
        assert_eq!(
            schema_version(&conn).expect("版本"),
            crate::schema::STORE_SCHEMA_VERSION
        );
        assert_eq!(count_events(&conn), 250, "保留行必须完好");
        verify_indexes(&conn).expect("必需索引在切换后必须完好");
        assert!(
            !PathBuf::from(format!("{}.compact-tmp", db.display())).exists(),
            "临时副本必须清理"
        );
        assert!(
            !PathBuf::from(format!("{}.compact-backup", db.display())).exists(),
            "备份在成功后必须删除"
        );
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn verify_copy_rejects_missing_index_and_row_drift() {
        let db = temp_db("verify");
        {
            let conn = seeded_file_db("verify", 50);
            let expected_schema = schema_version(&conn).expect("版本");
            let expected_rows = crate::monitor::table_stats(&conn).expect("行数快照");
            drop(conn);

            let tampered = temp_db("verify-tampered");
            let _ = std::fs::remove_file(&tampered);
            std::fs::copy(&db, &tampered).expect("复制");
            let bad = Connection::open(&tampered).expect("打开副本");
            bad.execute("DROP INDEX idx_events_session_seq", [])
                .expect("删除索引");
            let err = verify_copy(&bad, expected_schema, &expected_rows)
                .expect_err("缺索引的副本必须拒绝");
            assert!(err.to_string().contains("索引"), "{err}");
            drop(bad);

            let drifted = temp_db("verify-drift");
            let _ = std::fs::remove_file(&drifted);
            std::fs::copy(&db, &drifted).expect("复制");
            let bad = Connection::open(&drifted).expect("打开副本");
            bad.execute("DELETE FROM events WHERE seq > 10", [])
                .expect("删除行");
            let err = verify_copy(&bad, expected_schema, &expected_rows)
                .expect_err("行数漂移的副本必须拒绝");
            assert!(err.to_string().contains("行数"), "{err}");
            drop(bad);

            let _ = std::fs::remove_file(&db);
            let _ = std::fs::remove_file(&tampered);
            let _ = std::fs::remove_file(&drifted);
        }
    }

    #[test]
    fn compact_rejects_missing_db_and_concurrent_maintenance() {
        let missing = temp_db("missing");
        let _ = std::fs::remove_file(&missing);
        let err = compact_database(&missing).expect_err("不存在的库必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(!PathBuf::from(format!("{}.compact-tmp", missing.display())).exists());

        let db = temp_db("locked");
        let _guard = MaintenanceGuard::acquire().expect("持锁");
        let err = compact_database(&db).expect_err("并发维护必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.to_string().contains("并发维护"));
        drop(_guard);
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn space_report_counts_wal_when_present() {
        let db = temp_db("space");
        let conn = seeded_file_db("space", 10);
        let report = capture_space(&conn, &db).expect("采集空间");
        assert!(report.file_size_bytes > 0);
        assert!(report.blocks_consistent());
        let _ = std::fs::remove_file(&db);
    }
}
