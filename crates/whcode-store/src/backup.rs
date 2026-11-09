//! # 预迁移备份与备份校验（P3.4／FR-767／ADR-063）
//!
//! 口径（`docs/guides/disaster-recovery.md` §3 的文件级还原是兜底，不是常规路径）：
//!
//! 1. **时点**：只在“库文件已存在且 schema 版本落后于目标版本”时复制一次；新库与
//!    同版本重开不复制（不制造无意义副本）。
//! 2. **完整性**：复制同时流式计算 SHA-256（64 KiB 读取缓冲，内存不随库大小增长），
//!    落盘后核对字节数；校验和供 [`verify_backup`] 与发布前演练复算。
//! 3. **有界**：`<workspace>/backups/` 只保留最近 [`MAX_BACKUPS`] 份，超出按文件名内
//!    零填充时间戳裁剪——不出现“随启动次数线性增长”的目录。
//! 4. **fail-closed**：备份失败即由调用方阻断迁移，不出现“无备份却已升版”的状态
//!    （data-model §5、NFR-103）。
//!
//! 边界：本模块只做**文件级**复制与校验，不解析 DuckDB 页；“备份能被打开且行数一致”
//! 由恢复演练测试（`backup_can_be_opened_after_copying_live_database`）承担。
//! 备份内容是完整数据库文件，与 `db archive --export` 的导出 JSON 是两条通道：
//! 前者用于迁移回退，后者用于带离与留档（`export.rs`）。

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use duckdb::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use whcode_domain::{ErrorCode, WhError};

use crate::schema::schema_version;

/// 备份目录名（相对库文件所在目录，即工作区根）。
pub const BACKUP_DIR_NAME: &str = "backups";

/// 保留的备份份数上限（含本次新建），超出按时间戳裁剪。
pub const MAX_BACKUPS: usize = 5;

/// 复制与校验的读取缓冲字节数（有界，不随库大小增长）。
pub const BACKUP_BUFFER_BYTES: usize = 64 * 1024;

/// 目录扫描上限：裁剪时最多检查的条目数（防止异常目录拖住启动）。
const MAX_SCAN_ENTRIES: usize = 256;

/// 备份文件后缀（`<库文件名>.v0007-<毫秒时间戳>.bak`）。
const BACKUP_SUFFIX: &str = ".bak";

/// 一次备份的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupRecord {
    /// 来源库文件。
    pub source: String,
    /// 备份文件路径。
    pub path: String,
    /// 备份内容的 SHA-256（小写十六进制）。
    pub sha256: String,
    /// 备份字节数。
    pub bytes: u64,
    /// 备份时的 schema 版本（迁移前的版本）。
    pub schema_version: u32,
    /// 创建时间（毫秒，调用方时间源）。
    pub created_at_ms: u64,
}

impl BackupRecord {
    /// 迁移前备份的提示文案（不含库内容，只含路径、迁移前版本、字节数与校验和）。
    #[must_use]
    pub fn notice(&self) -> String {
        format!(
            "迁移前已备份工作区（迁移前 schema v{}）：{}（{} 字节，SHA-256 {}）",
            self.schema_version, self.path, self.bytes, self.sha256
        )
    }
}

fn backup_err(context: &str, err: impl std::fmt::Display) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 备份文件路径：`<库文件名>.v<四位版本>-<十三位毫秒>.bak`。
///
/// 时间戳零填充到十三位，保证同一目录内文件名按字典序即时间序（裁剪依赖此性质）。
#[must_use]
pub fn backup_file_name(db_file_name: &str, schema_version: u32, created_at_ms: u64) -> String {
    format!("{db_file_name}.v{schema_version:04}-{created_at_ms:013}{BACKUP_SUFFIX}")
}

/// 库文件对应的备份目录（与库文件同级）。
#[must_use]
pub fn backup_directory(db_path: &Path) -> PathBuf {
    db_path.parent().map_or_else(
        || PathBuf::from(BACKUP_DIR_NAME),
        |parent| parent.join(BACKUP_DIR_NAME),
    )
}

/// 把库文件复制到备份目录，返回校验和记录；超出保留上限时裁剪最旧副本。
///
/// 先写 `<目标>.partial` 再改名，避免崩溃留下“看起来完整”的半截文件；每次备份前
/// 清掉遗留的 `.partial`（同样受扫描上限约束，不随崩溃次数累积）。
/// 调用方必须保证库文件**未被打开**：Windows 上 DuckDB 独占持有库文件，连接期间复制
/// 会得到 sharing violation（`pre_migration_backup` 已按此约束组织时序）。
///
/// # Errors
///
/// 源文件不存在（`not_found`）、不是普通文件（`invalid_state`）、目录创建/复制/
/// 改名/裁剪失败（`io_error`）时返回 [`WhError`]。
pub fn backup_database(
    db_path: &Path,
    backup_dir: &Path,
    schema_version: u32,
    created_at_ms: u64,
) -> Result<BackupRecord, WhError> {
    let metadata = fs::metadata(db_path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            WhError::new(
                ErrorCode::NotFound,
                format!("备份源数据库不存在：{}", db_path.display()),
            )
        } else {
            backup_err(&format!("读取 {}", db_path.display()), err)
        }
    })?;
    if !metadata.is_file() {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!("备份源不是普通文件：{}", db_path.display()),
        ));
    }
    fs::create_dir_all(backup_dir).map_err(|err| backup_err("创建备份目录", err))?;
    clean_partials(backup_dir)?;
    let file_name = db_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            WhError::new(
                ErrorCode::InvalidState,
                format!("库文件名不是有效 UTF-8：{}", db_path.display()),
            )
        })?;
    let destination = backup_dir.join(backup_file_name(file_name, schema_version, created_at_ms));
    let partial = backup_dir.join(format!(
        "{}.partial",
        backup_file_name(file_name, schema_version, created_at_ms)
    ));

    let copied = match copy_with_sha256(db_path, &partial) {
        Ok(copied) => copied,
        Err(err) => {
            let _ = fs::remove_file(&partial);
            return Err(err);
        }
    };
    let (bytes, sha256) = copied;
    if bytes != metadata.len() {
        let _ = fs::remove_file(&partial);
        return Err(WhError::new(
            ErrorCode::IoError,
            format!(
                "备份字节数不符：源 {} 字节，副本 {bytes} 字节",
                metadata.len()
            ),
        ));
    }
    if let Err(err) = fs::rename(&partial, &destination) {
        let _ = fs::remove_file(&partial);
        return Err(backup_err("提交备份文件", err));
    }
    prune_backups(backup_dir, MAX_BACKUPS)?;
    Ok(BackupRecord {
        source: db_path.display().to_string(),
        path: destination.display().to_string(),
        sha256,
        bytes,
        schema_version,
        created_at_ms,
    })
}

/// 迁移前备份：库文件存在且版本落后时复制一次并返回记录；否则返回 `None`。
///
/// 在调用方打开**主连接之前**执行：Windows 上 DuckDB 以独占方式持有库文件，
/// 连接存续期间无法做文件级复制。本函数自带短生命周期连接——读版本、`CHECKPOINT`
/// 把 WAL 合入主文件，**释放连接后**才复制，因此备份包含最近一次提交。
/// `CHECKPOINT` 失败即返回错误，调用方**不得**继续迁移。
///
/// # Errors
///
/// 打开数据库、读取 schema 版本、`CHECKPOINT` 或复制失败时返回 [`WhError`]。
pub fn pre_migration_backup(
    db_path: &Path,
    target_version: u32,
    created_at_ms: u64,
) -> Result<Option<BackupRecord>, WhError> {
    if !db_path.exists() {
        // 新库：没有可备份的内容（首次迁移由空库开始）。
        return Ok(None);
    }
    let conn = Connection::open(db_path).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("迁移前打开数据库失败（{}）：{err}", db_path.display()),
        )
    })?;
    let Some(current) = current_schema_version(&conn)? else {
        // 迁移表不存在＝尚未迁移过的空库（DuckDB 打开时即创建文件），不制造无意义副本。
        return Ok(None);
    };
    if current >= target_version {
        return Ok(None);
    }
    conn.execute_batch("CHECKPOINT").map_err(|err| {
        WhError::new(
            ErrorCode::InvalidState,
            format!("迁移前 CHECKPOINT 失败，拒绝在没有完整备份的情况下迁移：{err}"),
        )
    })?;
    // Windows 下必须先释放文件句柄，否则复制会得到 sharing violation。
    drop(conn);
    let record = backup_database(db_path, &backup_directory(db_path), current, created_at_ms)?;
    Ok(Some(record))
}

/// 复算备份文件 SHA-256 并与记录比对，返回字节数。
///
/// # Errors
///
/// 文件不存在（`not_found`）、读取失败（`io_error`）、校验和不符（`invalid_state`）
/// 时返回 [`WhError`]。
pub fn verify_backup(path: &Path, expected_sha256: &str) -> Result<u64, WhError> {
    let (bytes, actual) = hash_file(path)?;
    if actual != expected_sha256 {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "备份校验和不符：{}（期望 {expected_sha256}，实际 {actual}）",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

/// 清理遗留的半截备份（`.partial`）；同样受扫描上限约束，不随崩溃次数累积。
fn clean_partials(backup_dir: &Path) -> Result<(), WhError> {
    let entries = fs::read_dir(backup_dir).map_err(|err| backup_err("读取备份目录", err))?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_SCAN_ENTRIES {
            break;
        }
        let entry = entry.map_err(|err| backup_err("读取备份目录项", err))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(".partial") {
            fs::remove_file(entry.path()).map_err(|err| backup_err("清理半截备份", err))?;
        }
    }
    Ok(())
}

/// 按时间序裁剪备份目录，只保留最新 `keep` 份；返回被删除的路径。
///
/// # Errors
///
/// 目录读取或删除失败时返回 [`WhError`]。
pub fn prune_backups(backup_dir: &Path, keep: usize) -> Result<Vec<PathBuf>, WhError> {
    let mut names: Vec<String> = Vec::new();
    let entries = fs::read_dir(backup_dir).map_err(|err| backup_err("读取备份目录", err))?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_SCAN_ENTRIES {
            break;
        }
        let entry = entry.map_err(|err| backup_err("读取备份目录项", err))?;
        if !entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(BACKUP_SUFFIX) {
            names.push(name.to_string());
        }
    }
    names.sort();
    let mut removed = Vec::new();
    let excess = names.len().saturating_sub(keep);
    for name in names.into_iter().take(excess) {
        let path = backup_dir.join(&name);
        fs::remove_file(&path).map_err(|err| backup_err("裁剪旧备份", err))?;
        removed.push(path);
    }
    Ok(removed)
}

/// 读取当前 schema 版本；`schema_migrations` 尚未创建时返回 `None`（新库）。
fn current_schema_version(conn: &Connection) -> Result<Option<u32>, WhError> {
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM information_schema.tables
             WHERE table_schema = 'main' AND table_name = 'schema_migrations'",
            [],
            |row| row.get(0),
        )
        .map_err(|err| {
            WhError::new(
                ErrorCode::InvalidState,
                format!("读取 schema 迁移表存在性失败：{err}"),
            )
        })?;
    if tables == 0 {
        return Ok(None);
    }
    schema_version(conn).map(Some)
}

fn copy_with_sha256(source: &Path, destination: &Path) -> Result<(u64, String), WhError> {
    let mut reader = File::open(source).map_err(|err| backup_err("打开源数据库", err))?;
    let mut writer = File::create(destination).map_err(|err| backup_err("创建备份文件", err))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BACKUP_BUFFER_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| backup_err("读取源数据库", err))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|err| backup_err("写入备份", err))?;
        total = total.saturating_add(read as u64);
    }
    writer
        .sync_all()
        .map_err(|err| backup_err("落盘备份", err))?;
    Ok((total, hex(&hasher.finalize())))
}

fn hash_file(path: &Path) -> Result<(u64, String), WhError> {
    let mut reader = File::open(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            WhError::new(
                ErrorCode::NotFound,
                format!("备份文件不存在：{}", path.display()),
            )
        } else {
            backup_err("打开备份文件", err)
        }
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BACKUP_BUFFER_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|err| backup_err("读取备份文件", err))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(read as u64);
    }
    Ok((total, hex(&hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util;
    use duckdb::params;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn write_file(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("创建目录");
        }
        fs::write(path, bytes).expect("写文件");
    }

    /// 复制与校验：已知内容的 SHA-256 与标准向量一致；篡改一个字节即判定不符。
    #[test]
    fn backup_matches_known_sha256_and_detects_tampering() {
        let dir = test_util::dir("backup-sha");
        let db = dir.join("whcode.duckdb");
        write_file(&db, b"abc");
        let record =
            backup_database(&db, &dir.join(BACKUP_DIR_NAME), 7, 1_700_000_000_001).expect("备份");
        assert_eq!(record.sha256, ABC_SHA256, "SHA-256 必须与标准向量一致");
        assert_eq!(record.bytes, 3);
        assert_eq!(record.schema_version, 7);
        assert_eq!(
            record.path,
            dir.join(BACKUP_DIR_NAME)
                .join("whcode.duckdb.v0007-1700000000001.bak")
                .display()
                .to_string()
        );
        assert!(Path::new(&record.path).is_file(), "备份文件必须已落盘");
        assert_eq!(
            verify_backup(Path::new(&record.path), ABC_SHA256).expect("校验"),
            3
        );

        write_file(Path::new(&record.path), b"abd");
        let err = verify_backup(Path::new(&record.path), ABC_SHA256).expect_err("篡改必须被发现");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.to_string().contains("校验和不符"),
            "错误须可操作：{err}"
        );
    }

    /// 保留上限：超过 [`MAX_BACKUPS`] 份时按时间戳删除最旧副本。
    #[test]
    fn retention_keeps_bounded_number_of_backups() {
        let dir = test_util::dir("backup-retention");
        let db = dir.join("whcode.duckdb");
        write_file(&db, b"seed");
        let backups = dir.join(BACKUP_DIR_NAME);
        for step in 0..(MAX_BACKUPS as u64 + 3) {
            backup_database(&db, &backups, 8, 1_700_000_000_000 + step).expect("备份");
        }
        let files: Vec<String> = fs::read_dir(&backups)
            .expect("读目录")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                Some(entry.file_name().to_string_lossy().to_string())
            })
            .collect();
        assert_eq!(files.len(), MAX_BACKUPS, "保留份数必须有界");
        // 最旧的 3 份已被裁剪（时间戳 000..002）。
        assert!(
            !files.iter().any(|name| name.contains("-1700000000002.bak")),
            "最旧副本应被删除：{files:?}"
        );
        assert!(
            files.iter().any(|name| name.contains("-1700000000007.bak")),
            "最新副本必须保留：{files:?}"
        );
    }

    /// 预迁移判定：新库与同版本不备份；落后版本备份一次且带迁移前版本号。
    #[test]
    fn pre_migration_backup_only_runs_when_version_lags() {
        let dir = test_util::dir("backup-premigrate");
        let db = dir.join("whcode.duckdb");
        let conn = Connection::open(&db).expect("打开库");
        crate::apply_migrations(&conn).expect("迁移至当前版本");

        // 同版本重开：不备份（函数自带连接，故先释放测试连接）。
        drop(conn);
        let record = pre_migration_backup(&db, crate::STORE_SCHEMA_VERSION, 1_700_000_000_000)
            .expect("预迁移检查");
        assert!(record.is_none(), "同版本不得制造副本");

        // 落一条数据，然后把版本表回退一版，模拟待迁移的旧库。
        let conn = Connection::open(&db).expect("打开库");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-M', '迁移前', current_timestamp, current_timestamp, 'idle')",
            [],
        )
        .expect("插入会话");
        conn.execute(
            "DELETE FROM schema_migrations WHERE version = ?",
            params![i64::from(crate::STORE_SCHEMA_VERSION)],
        )
        .expect("回退版本记录");
        drop(conn);
        let record = pre_migration_backup(&db, crate::STORE_SCHEMA_VERSION, 1_700_000_000_001)
            .expect("预迁移检查")
            .expect("落后版本必须备份");
        assert_eq!(
            record.schema_version,
            crate::STORE_SCHEMA_VERSION - 1,
            "记录迁移前版本"
        );
        assert!(Path::new(&record.path).is_file());

        // 新库（文件存在但迁移表未创建）：不备份——这是首次启动的真实时点。
        let fresh = dir.join("fresh").join("whcode.duckdb");
        fs::create_dir_all(fresh.parent().expect("父目录")).expect("建目录");
        let fresh_conn = Connection::open(&fresh).expect("打开新库");
        assert!(fresh.exists(), "DuckDB 打开时即创建文件");
        drop(fresh_conn);
        let record =
            pre_migration_backup(&fresh, crate::STORE_SCHEMA_VERSION, 1).expect("新库检查");
        assert!(record.is_none(), "新库不得备份");
    }

    /// 恢复演练：备份文件可被 DuckDB 打开，schema 版本与行数与来源一致。
    #[test]
    fn backup_can_be_opened_after_copying_live_database() {
        let dir = test_util::dir("backup-drill");
        let db = dir.join("whcode.duckdb");
        let conn = Connection::open(&db).expect("打开库");
        crate::apply_migrations(&conn).expect("迁移");
        for index in 0..3 {
            conn.execute(
                "INSERT INTO sessions (id, title, created_at, last_active_at, status)
                 VALUES (?, '演练', current_timestamp, current_timestamp, 'idle')",
                params![format!("S-DRILL-{index}")],
            )
            .expect("插入会话");
        }
        conn.execute_batch("CHECKPOINT").expect("合入 WAL");
        drop(conn);

        let record = backup_database(
            &db,
            &backup_directory(&db),
            crate::STORE_SCHEMA_VERSION,
            1_700_000_000_000,
        )
        .expect("备份");
        // 先校验（Windows 上 DuckDB 打开备份后会独占文件），再打开做恢复演练。
        assert_eq!(
            verify_backup(Path::new(&record.path), &record.sha256).expect("校验"),
            record.bytes
        );
        let restored = Connection::open(&record.path).expect("备份必须可打开");
        assert_eq!(
            schema_version(&restored).expect("读版本"),
            crate::STORE_SCHEMA_VERSION,
            "备份必须保留 schema 版本"
        );
        let rows: i64 = restored
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(rows, 3, "备份必须保留行数");
    }

    /// 半截备份清理：目录里遗留的 `.partial` 在下次备份前被删除（不随崩溃次数累积）。
    #[test]
    fn stale_partial_files_are_cleaned_before_backup() {
        let dir = test_util::dir("backup-partial");
        let db = dir.join("whcode.duckdb");
        write_file(&db, b"seed");
        let backups = dir.join(BACKUP_DIR_NAME);
        fs::create_dir_all(&backups).expect("建备份目录");
        let stale = backups.join("whcode.duckdb.v0007-1700000000000.bak.partial");
        write_file(&stale, b"half");
        backup_database(&db, &backups, 8, 1_700_000_000_001).expect("备份");
        assert!(!stale.exists(), "遗留的半截备份必须被清理");
    }

    /// 失败路径：源缺失为 `not_found`；而非普通文件为 `invalid_state`。
    #[test]
    fn missing_or_directory_source_is_rejected() {
        let dir = test_util::dir("backup-missing");
        let missing = dir.join("nope.duckdb");
        let err =
            backup_database(&missing, &dir.join(BACKUP_DIR_NAME), 8, 1).expect_err("缺失必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);

        let err =
            backup_database(&dir, &dir.join(BACKUP_DIR_NAME), 8, 1).expect_err("目录必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err = verify_backup(&missing, "00").expect_err("缺失备份必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
    }
}
