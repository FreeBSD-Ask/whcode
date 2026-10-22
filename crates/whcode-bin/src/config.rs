//! 配置加载与数据库/追踪基线默认值（tasks.md T016）。
//!
//! 默认值来源：任务书 §5.4（数据库配置基线）与 D-05（追踪默认 `redacted`）。这些数值是
//! **可配置基线**，须由 M0/M1 基准校准，不得写死为架构定理（§0.3-6）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use whcode_domain::{TraceLevel, WhError};
use whcode_platform::{paths, resources};

/// DuckDB 缓冲管理器默认内存上限（MiB），§5.4。
pub const DEFAULT_MEMORY_LIMIT_MB: u32 = 192;
/// DuckDB 缓冲管理器候选最大内存上限（MiB），§5.4。
pub const CANDIDATE_MAX_MEMORY_LIMIT_MB: u32 = 384;

/// 数据库配置基线（§5.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbConfig {
    /// `memory_limit`（MiB）。仅约束 DuckDB 缓冲管理器，**不**代表进程 RSS 上限。
    pub memory_limit_mb: u32,
    /// `memory_limit` 的候选上限（MiB），由用户配置决定是否使用。
    pub max_memory_limit_mb: u32,
    /// `threads`：初始不高于 `min(4, 可用物理核数)`。
    pub threads: u32,
    /// `temp_directory`：工作区专用临时目录，启动时须检查可写性与磁盘余量。
    pub temp_directory: PathBuf,
    /// `max_temp_directory_size`（MiB）：显式上限；超限须返回明确错误并保护一致性。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_temp_directory_size_mb: Option<u64>,
}

/// 追踪配置（FR-203、D-05）。
///
/// 默认值由 [`TraceLevel`] 的 `#[default]` 变体提供（`redacted`），因此本结构可派生 `Default`。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TraceConfig {
    /// 默认追踪级别；默认 `redacted`。
    pub default_level: TraceLevel,
}

/// 顶层配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// 工作区根路径（数据库、产物与运行时状态的持久化边界，ADR-001）。
    pub workspace_path: PathBuf,
    /// 数据库基线。
    pub db: DbConfig,
    /// 追踪基线。
    pub trace: TraceConfig,
}

impl Config {
    /// 按基线默认值构造某工作区的配置。
    #[must_use]
    pub fn for_workspace(workspace_path: impl Into<PathBuf>) -> Self {
        let workspace_path = workspace_path.into();
        let cores = resources::available_physical_cores();
        Self {
            db: DbConfig {
                memory_limit_mb: DEFAULT_MEMORY_LIMIT_MB,
                max_memory_limit_mb: CANDIDATE_MAX_MEMORY_LIMIT_MB,
                threads: resources::thread_budget(cores),
                temp_directory: paths::workspace_temp_dir(&workspace_path),
                max_temp_directory_size_mb: None,
            },
            trace: TraceConfig::default(),
            workspace_path,
        }
    }

    /// 从 JSON 加载配置。
    ///
    /// # Errors
    ///
    /// JSON 解析失败时返回 `parse_error`（`retryable = false`）：配置错误必须由人修正，
    /// 自动重试没有意义且会掩盖问题。
    pub fn from_json(json: &str) -> Result<Self, WhError> {
        serde_json::from_str(json)
            .map_err(|err| WhError::parse_error(format!("配置解析失败：{err}")))
    }

    /// 启动期检查：临时目录必须存在且可写（§5.4）。
    ///
    /// # Errors
    ///
    /// 目录不可写时返回 `io_error`（`retryable = true`，修复权限后重试有效）。
    pub fn check_temp_directory(&self) -> Result<(), WhError> {
        // 骨架阶段：目录自动创建属 M1 工作区初始化任务（T032/T047）；此处只做可写性判定。
        if !self.db.temp_directory.is_dir() {
            return Err(WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!("临时目录不存在：{}", self.db.temp_directory.display()),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_task_book_baseline() {
        let config = Config::for_workspace("D:/ws");
        assert_eq!(config.db.memory_limit_mb, 192);
        assert_eq!(config.db.max_memory_limit_mb, 384);
        let cores = resources::available_physical_cores();
        // 断言精确值而非上界：`threads` 必须是 min(4, 可用并行度)，硬编码为 1 也应被抓住。
        assert_eq!(config.db.threads, resources::thread_budget(cores));
        assert!(config.db.threads <= 4);
        assert_eq!(config.trace.default_level, TraceLevel::Redacted);
        assert_eq!(config.db.temp_directory, PathBuf::from("D:/ws").join("tmp"));
        assert_eq!(config.db.max_temp_directory_size_mb, None);
    }

    #[test]
    fn json_roundtrip_preserves_values() {
        let config = Config::for_workspace("/tmp/ws");
        let json = serde_json::to_string_pretty(&config).expect("序列化应成功");
        let restored = Config::from_json(&json).expect("反序列化应成功");
        assert_eq!(config, restored);
    }

    #[test]
    fn invalid_json_yields_parse_error() {
        let err = Config::from_json("{ not json").expect_err("无效 JSON 必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(!err.retryable, "配置错误不应标记为可重试");
    }

    #[test]
    fn json_allows_explicit_overrides() {
        let json = r#"{
            "workspace_path": "/tmp/ws",
            "db": {
                "memory_limit_mb": 96,
                "max_memory_limit_mb": 384,
                "threads": 2,
                "temp_directory": "/tmp/ws/tmp",
                "max_temp_directory_size_mb": 512
            },
            "trace": { "default_level": "metadata" }
        }"#;
        let config = Config::from_json(json).expect("显式覆盖应被接受");
        assert_eq!(config.db.memory_limit_mb, 96);
        assert_eq!(config.db.max_temp_directory_size_mb, Some(512));
        assert_eq!(config.trace.default_level, TraceLevel::Metadata);
    }

    #[test]
    fn missing_temp_directory_is_reported() {
        let mut config = Config::for_workspace("D:/nonexistent-workspace-for-test");
        config.db.temp_directory = PathBuf::from("D:/nonexistent-workspace-for-test/tmp");
        let err = config
            .check_temp_directory()
            .expect_err("不存在的临时目录必须报错");
        assert_eq!(err.code, whcode_domain::ErrorCode::IoError);
    }
}
