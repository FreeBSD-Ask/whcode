//! 工作区装配（装配根共用）：数据库打开/初始化、临时目录与 `--workspace` 解析。
//!
//! `serve --web` 与交互式 TUI 共用同一工作区约定（ADR-001）：
//! - 数据库：`<workspace>/whcode.duckdb`（单文件 DuckDB）；
//! - 临时目录：`<workspace>/tmp`（`whcode-platform::paths` 约定），并设为 DuckDB
//!   `temp_directory`；
//! - 迁移：`whcode_store::apply_migrations`（幂等）。
//!
//! **已知限制（诚实标注）**：本机同一工作区同一时刻只允许**一个进程**打开数据库
//! （DuckDB 跨进程写锁；单写者服务属 `crates/whcode-store/src/service.rs`，尚未作为
//! 唯一所有权进程接入）。因此 TUI 与 `serve --web` 不应同时对同一工作区运行；
//! 打开失败时调用方须显式回退（TUI 用空数据源）或报错（serve 退出）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

use crate::config::Config;

/// 工作区数据库文件名。
pub const WORKSPACE_DB_FILE: &str = "whcode.duckdb";

/// 解析有效工作区：`--workspace DIR` 优先，否则配置工作区（`--config` 或 cwd）。
///
/// # Errors
///
/// `--workspace` 缺值返回 `parse_error`。
pub fn resolve_workspace(args: &[String], config: &Config) -> Result<PathBuf, WhError> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--workspace" {
            let value = iter
                .next()
                .ok_or_else(|| WhError::parse_error("--workspace 缺少目录参数"))?;
            return Ok(PathBuf::from(value));
        }
    }
    Ok(config.workspace_path.clone())
}

/// 工作区数据库路径（`<workspace>/whcode.duckdb`）。
#[must_use]
pub fn workspace_db_path(workspace: &Path) -> PathBuf {
    workspace.join(WORKSPACE_DB_FILE)
}

/// 打开并初始化工作区数据库（幂等迁移；库参数取自配置；临时目录属工作区）。
///
/// # Errors
///
/// 目录创建、打开（含跨进程锁冲突）或迁移失败时返回 [`WhError`]。
pub fn open_workspace_db(workspace: &Path, config: &Config) -> Result<Connection, WhError> {
    let db_path = workspace_db_path(workspace);
    let temp_directory = whcode_platform::paths::workspace_temp_dir(workspace);
    fs::create_dir_all(&temp_directory)
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("创建工作区目录失败：{err}")))?;
    // 迁移前备份（P3.4／FR-767／ADR-063）：库文件已存在且版本落后时先复制到
    // `<workspace>/backups/` 并核对 SHA-256；备份失败即返回错误，**不继续迁移**
    // （data-model §5、NFR-103：不得出现“无备份却已升版”的状态）。
    // 必须在主连接之前执行：Windows 上 DuckDB 独占持有库文件，连接期间无法做文件级复制。
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    if let Some(record) =
        whcode_store::pre_migration_backup(&db_path, whcode_store::STORE_SCHEMA_VERSION, now_ms)?
    {
        eprintln!("{}", record.notice());
    }
    let conn = Connection::open(&db_path).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("打开数据库失败（{}）：{err}", db_path.display()),
        )
    })?;
    // DuckDB 接受正斜杠路径；单引号按 SQL 字面量规则转义（路径含 `'` 的极端情形）。
    let temp_literal = temp_directory
        .display()
        .to_string()
        .replace('\\', "/")
        .replace('\'', "''");
    // async_threads 默认 64（DuckDB 2.0.0），是空闲线程膨胀主因（ADR-042）；与 threads 同预算收敛。
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB'; SET threads = {}; SET async_threads = {}; SET temp_directory = '{}'; \
         SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         SET allow_community_extensions = false; SET allow_extension_repositories = 'forbidden';",
        config.db.memory_limit_mb, config.db.threads, config.db.threads, temp_literal
    ))
    .map_err(|err| {
        WhError::new(
            ErrorCode::InvalidState,
            format!("配置数据库参数失败：{err}"),
        )
    })?;
    whcode_store::apply_migrations(&conn)?;
    // 恢复协议（FR-410、V-105/V-307）：上次进程可能在“意图—执行—登记”之间中断。
    // 只把不确定记录标为 unknown 并报告，不重放任何副作用（处置由使用者决定）。
    let mut conn = conn;
    let recovery = whcode_store::recover_uncertain(&mut conn)?;
    if !recovery.is_empty() {
        let ids: Vec<&str> = recovery
            .recovered
            .iter()
            .take(5)
            .map(|item| item.id.as_str())
            .collect();
        eprintln!(
            "警告：{} 个工具执行在上次中断时无法判定副作用是否已发生，已标记 unknown（{}）。{}",
            recovery.len(),
            ids.join(", "),
            recovery.next_action
        );
        if recovery.truncated {
            eprintln!("（本次扫描有界，剩余记录将在下次启动继续处理）");
        }
    }
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_resolution_prefers_flag() {
        let config = Config::for_workspace("D:/cfg-ws");
        let args: Vec<String> = ["--workspace", "D:/flag-ws"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            resolve_workspace(&args, &config).expect("解析"),
            PathBuf::from("D:/flag-ws")
        );
        assert_eq!(
            resolve_workspace(&[], &config).expect("缺省"),
            PathBuf::from("D:/cfg-ws")
        );
        let missing: Vec<String> = vec!["--workspace".to_string()];
        assert!(
            resolve_workspace(&missing, &config).is_err(),
            "缺值必须报错"
        );
    }

    #[test]
    fn db_path_uses_workspace_convention() {
        assert_eq!(
            workspace_db_path(Path::new("D:/ws")),
            PathBuf::from("D:/ws").join("whcode.duckdb")
        );
    }

    #[test]
    fn workspace_connection_denies_extension_installs_and_repositories() {
        let dir = std::env::temp_dir().join(format!("whcode-ws-ext-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let config = Config::for_workspace(dir.clone());
        let conn = open_workspace_db(&dir, &config).expect("打开工作区");
        let setting = |name: &str| -> String {
            conn.query_row(
                &format!("SELECT CAST(current_setting('{name}') AS VARCHAR)"),
                [],
                |row| row.get(0),
            )
            .expect("读取设置")
        };
        assert_eq!(setting("autoinstall_known_extensions"), "false");
        assert_eq!(setting("autoload_known_extensions"), "false");
        assert_eq!(setting("allow_community_extensions"), "false");
        assert_eq!(setting("allow_extension_repositories"), "forbidden");
        assert_eq!(
            setting("async_threads"),
            config.db.threads.to_string(),
            "async_threads 必须与 threads 同预算收敛（ADR-042）"
        );
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 打开工作区即执行恢复协议（FR-410）：上一次进程遗留的 running 记录必须变 `unknown`。
    #[test]
    fn opening_workspace_recovers_uncertain_executions() {
        let dir = std::env::temp_dir().join(format!("whcode-ws-recover-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let config = Config::for_workspace(dir.clone());
        let conn = open_workspace_db(&dir, &config).expect("首次打开");
        whcode_store::record_intent(&conn, "X-1", "C-1", "hash").expect("登记意图");
        whcode_store::mark_running(&conn, "X-1", 1_700_000_000_000).expect("进入执行");
        drop(conn);

        let conn = open_workspace_db(&dir, &config).expect("再次打开");
        let row = whcode_store::load(&conn, "X-1")
            .expect("查询")
            .expect("记录存在");
        assert_eq!(
            row.state,
            whcode_domain::RuntimeExecutionStatus::Unknown,
            "打开工作区必须把中断遗留的执行标为 unknown（不重放副作用）"
        );
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    /// 迁移前自动备份（P3.4／FR-767／ADR-063）：旧库打开时先落一份备份，再完成迁移；
    /// 同版本重开不再新增副本（不随启动次数增长）。
    #[test]
    fn opening_workspace_backs_up_before_migration_and_only_once() {
        let dir = std::env::temp_dir().join(format!("whcode-ws-backup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let config = Config::for_workspace(dir.clone());

        let conn = open_workspace_db(&dir, &config).expect("首次打开");
        // 模拟待迁移旧库：只回退最后一版版本记录（v9 迁移是 IF NOT EXISTS，可重复应用）。
        conn.execute("DELETE FROM schema_migrations WHERE version = 9", [])
            .expect("回退版本记录");
        drop(conn);

        let conn = open_workspace_db(&dir, &config).expect("再次打开");
        let version = whcode_store::schema_version(&conn).expect("读版本");
        drop(conn);
        assert_eq!(
            version,
            whcode_store::STORE_SCHEMA_VERSION,
            "备份后迁移必须完成"
        );

        let backups = dir.join(whcode_store::BACKUP_DIR_NAME);
        let names: Vec<String> = fs::read_dir(&backups)
            .expect("备份目录必须存在")
            .filter_map(|entry| {
                let entry = entry.ok()?;
                Some(entry.file_name().to_string_lossy().to_string())
            })
            .collect();
        assert_eq!(
            names.len(),
            1,
            "落后版本必须产生且只产生一份备份：{names:?}"
        );
        assert!(
            names[0].contains(".v0008-"),
            "备份文件名须含迁移前版本：{names:?}"
        );

        let conn = open_workspace_db(&dir, &config).expect("第三次打开");
        drop(conn);
        let count = fs::read_dir(&backups).expect("备份目录").count();
        assert_eq!(count, 1, "同版本重开不得新增备份");
        let _ = fs::remove_dir_all(&dir);
    }
}
