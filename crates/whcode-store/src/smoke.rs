//! M0 数据库冒烟路径（tasks.md T022；V-002、ADR-010）。
//!
//! 覆盖四步：**创建 / 事务写读 / 关闭重启 / 迁移**。这是 M0 的**冒烟**，不是 M1 的完整
//! schema——14 张表在 T032 交付；此处只验证"能创建、能在事务内写读、重启后已提交数据仍在、
//! 版本化迁移可重复执行"这一最小机制。
//!
//! 纪律（ADR-010、V-001/V-002）：
//! - 结果必须逐平台留证；本模块只产出**本机**证据，不得代替其他平台的运行验证；
//! - 引擎版本取自运行期 `SELECT version()`，写入依赖记录（不得以 Cargo 版本号代替实测值）。

use std::path::Path;

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 冒烟步骤名（顺序即执行顺序，供报告与检查点引用）。
pub const SMOKE_STEPS: [&str; 4] = [
    "create",
    "transaction_write_read",
    "close_reopen",
    "migration_idempotent",
];

/// 冒烟用的最小键值表（非 M1 正式 schema）。
const SMOKE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS smoke_kv (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
)";

/// 迁移版本表（M0 只验证机制；正式 DDL 见 data-model.md §1 与 T032）。
const MIGRATION_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schema_migrations (
    version    BIGINT PRIMARY KEY,
    applied_at TIMESTAMP NOT NULL,
    checksum   TEXT NOT NULL
)";

/// 单步留证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeStep {
    /// 步骤名（取值见 [`SMOKE_STEPS`]）。
    pub name: &'static str,
    /// 该步骤的实测说明。
    pub detail: String,
}

impl SmokeStep {
    fn new(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            detail: detail.into(),
        }
    }
}

/// 冒烟报告（逐步骤留证）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmokeReport {
    /// 运行期实测的 DuckDB 引擎版本（`SELECT version()`）。
    pub duckdb_version: String,
    /// 数据库文件路径。
    pub path: String,
    /// 收尾 CHECKPOINT 后的数据库文件字节数。
    pub db_bytes: u64,
    /// 各步骤结果。
    pub steps: Vec<SmokeStep>,
}

impl SmokeReport {
    /// 是否四步齐备（仅用于自检；不等于跨平台验收通过）。
    pub fn covers_all_steps(&self) -> bool {
        SMOKE_STEPS
            .iter()
            .all(|name| self.steps.iter().any(|step| step.name == *name))
    }
}

fn db_err(step: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("数据库冒烟 {step} 失败：{err}"))
}

/// 探测当前**已链接**的 DuckDB 引擎版本（内存库，不落盘）。
///
/// 供依赖记录（V-001）与 `doctor` 使用：必须以运行期实测值入账，不得以 crate 版本号、
/// 归档文件名或构建脚本推断值代替（报告纪律，NFR-201—210 同源要求）。
///
/// # Errors
///
/// 无法打开内存库或查询版本时返回 `io_error`。
pub fn engine_version() -> Result<String, WhError> {
    let conn = Connection::open_in_memory().map_err(|err| db_err("engine_version", err))?;
    conn.query_row("SELECT version()", [], |row| row.get(0))
        .map_err(|err| db_err("engine_version", err))
}

/// 对指定路径执行完整冒烟。路径所在的父目录必须已存在。
///
/// # Errors
///
/// 任一步骤失败即返回 [`WhError`]（错误码 `io_error`，`retryable = true`：重试不会造成
/// 副作用重复——迁移与写入均为幂等语义）。
pub fn run(path: &Path) -> Result<SmokeReport, WhError> {
    let mut steps = Vec::with_capacity(SMOKE_STEPS.len());

    // 步骤 1：创建 / 打开，并取得引擎版本实测值。
    let mut conn = Connection::open(path).map_err(|err| db_err("create", err))?;
    let duckdb_version: String = conn
        .query_row("SELECT version()", [], |row| row.get(0))
        .map_err(|err| db_err("create", err))?;
    steps.push(SmokeStep::new(
        "create",
        format!("创建/打开 {}；引擎版本 {duckdb_version}", path.display()),
    ));

    // 步骤 2：事务写读（写与读在同一事务内，提交后才认为确认持久化，§7.3-5）。
    {
        let tx = conn
            .transaction()
            .map_err(|err| db_err("transaction_write_read", err))?;
        tx.execute_batch(SMOKE_SCHEMA)
            .map_err(|err| db_err("transaction_write_read", err))?;
        tx.execute(
            "INSERT OR REPLACE INTO smoke_kv (k, v) VALUES (?, ?)",
            params!["m0-key", "m0-value"],
        )
        .map_err(|err| db_err("transaction_write_read", err))?;
        let value: String = tx
            .query_row(
                "SELECT v FROM smoke_kv WHERE k = ?",
                params!["m0-key"],
                |row| row.get(0),
            )
            .map_err(|err| db_err("transaction_write_read", err))?;
        if value != "m0-value" {
            return Err(WhError::internal(format!(
                "事务内读回不一致：期望 m0-value，实际 {value}"
            )));
        }
        tx.commit()
            .map_err(|err| db_err("transaction_write_read", err))?;
    }
    steps.push(SmokeStep::new(
        "transaction_write_read",
        "事务内 INSERT + SELECT 一致，已提交",
    ));

    // 步骤 3：关闭重启（丢弃连接后重新打开，验证已提交数据仍在）。
    drop(conn);
    let conn = Connection::open(path).map_err(|err| db_err("close_reopen", err))?;
    let value: String = conn
        .query_row(
            "SELECT v FROM smoke_kv WHERE k = ?",
            params!["m0-key"],
            |row| row.get(0),
        )
        .map_err(|err| db_err("close_reopen", err))?;
    if value != "m0-value" {
        return Err(WhError::internal(format!(
            "重启后读回不一致：期望 m0-value，实际 {value}"
        )));
    }
    steps.push(SmokeStep::new(
        "close_reopen",
        "关闭连接后重新打开，已提交数据可读",
    ));

    // 步骤 4：迁移（版本化表 + 同一版本重复执行不报错、不产生重复行，§7.4）。
    conn.execute_batch(MIGRATION_SCHEMA)
        .map_err(|err| db_err("migration_idempotent", err))?;
    for _ in 0..2 {
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at, checksum)
             VALUES (?, CAST(now() AS TIMESTAMP), ?)
             ON CONFLICT (version) DO NOTHING",
            params![1_i64, "m0-smoke"],
        )
        .map_err(|err| db_err("migration_idempotent", err))?;
    }
    let applied: i64 = conn
        .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .map_err(|err| db_err("migration_idempotent", err))?;
    if applied != 1 {
        return Err(WhError::invalid_state(format!(
            "迁移幂等失败：schema_migrations 行数为 {applied}，期望 1"
        )));
    }
    steps.push(SmokeStep::new(
        "migration_idempotent",
        "同一 schema 版本重复应用两次，仅一行记录",
    ));

    // 收尾：显式 CHECKPOINT（同步 WAL）并记录文件规模，供空间报告引用（FR-510）。
    conn.execute_batch("CHECKPOINT")
        .map_err(|err| db_err("migration_idempotent", err))?;
    drop(conn);
    let db_bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);

    Ok(SmokeReport {
        duckdb_version,
        path: path.display().to_string(),
        db_bytes,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 在系统临时目录下建一个独占子目录，返回 (目录, 数据库路径)。
    fn temp_db(tag: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("whcode-smoke-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let db = dir.join("smoke.duckdb");
        (dir, db)
    }

    #[test]
    fn smoke_covers_all_steps_on_fresh_file() {
        let (dir, db) = temp_db("fresh");
        let report = run(&db).expect("新库冒烟应通过");
        assert!(report.covers_all_steps(), "步骤缺项：{:?}", report.steps);
        assert_eq!(report.steps.len(), SMOKE_STEPS.len());
        assert!(
            !report.duckdb_version.is_empty(),
            "必须取得引擎版本实测值（V-001 依赖记录）"
        );
        assert!(report.db_bytes > 0, "CHECKPOINT 后文件应有大小");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smoke_is_repeatable_on_existing_file() {
        let (dir, db) = temp_db("repeat");
        run(&db).expect("首次冒烟应通过");
        let second = run(&db).expect("对已有库重复冒烟应通过（迁移幂等）");
        assert!(second.covers_all_steps());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smoke_reports_engine_version_from_runtime() {
        let (dir, db) = temp_db("version");
        let report = run(&db).expect("冒烟应通过");
        // 运行期版本形如 "v1.5.6"：断言结构而非单一子串，避免硬编码文本也能“通过”。
        let raw = report.duckdb_version.trim_start_matches('v');
        let parts: Vec<&str> = raw.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "引擎版本应为三段式：{}",
            report.duckdb_version
        );
        assert!(
            parts.iter().all(|part| part.parse::<u32>().is_ok()),
            "引擎版本各段必须为数字：{}",
            report.duckdb_version
        );
        assert_eq!(parts[0], "1", "主版本应为 1：{}", report.duckdb_version);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_version_probe_reports_structured_version() {
        let version = engine_version().expect("内存库应可探测引擎版本");
        let raw = version.trim_start_matches('v');
        assert!(
            raw.split('.').count() == 3 && raw.split('.').all(|p| p.parse::<u32>().is_ok()),
            "引擎版本必须是三段式数字：{version}"
        );
    }

    #[test]
    fn missing_parent_directory_is_reported_as_io_error() {
        let db = std::env::temp_dir()
            .join(format!("whcode-smoke-{}-absent", std::process::id()))
            .join("nested")
            .join("smoke.duckdb");
        let err = run(&db).expect_err("父目录不存在必须失败");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.retryable, "IO 失败可安全重试");
    }
}
