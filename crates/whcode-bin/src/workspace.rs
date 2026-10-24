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
    conn.execute_batch(&format!(
        "SET memory_limit = '{}MB'; SET threads = {}; SET temp_directory = '{}'; \
         SET autoinstall_known_extensions = false; SET autoload_known_extensions = false; \
         SET allow_community_extensions = false; SET allow_extension_repositories = 'forbidden';",
        config.db.memory_limit_mb, config.db.threads, temp_literal
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
}
