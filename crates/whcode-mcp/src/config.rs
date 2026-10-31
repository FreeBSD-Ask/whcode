//! # MCP 服务器配置（FR-736、ADR-054 §2.3）
//!
//! 配置文件 `<workspace>/.zcode/mcp.json`，显式 opt-in、无自动发现：
//! 无配置文件 = 功能关闭（正确的空）；配置存在但解析失败 = 显式报错（错误的空）。
//!
//! 上限校验在加载层强制：服务器数、env 键数、名称字符集、超时范围——越界即拒绝，
//! 不静默截断（截断类上限在投影层另有显式标记，见 [`crate::manager`]）。

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use whcode_domain::{ErrorCode, WhError};

/// 配置文件相对工作区根的路径（与技能目录 `.zcode/skills/` 同一约定前缀）。
pub const CONFIG_PATH: &str = ".zcode/mcp.json";

/// 服务器数量上限（ADR-054 §2.3）。
pub const MAX_SERVERS: usize = 8;

/// 单服务器显式 env 键数上限（ADR-054 §2.9：授权通道收口，防配置膨胀）。
pub const MAX_ENV_KEYS: usize = 16;

/// 默认请求超时（毫秒；与 ZCode `MCP_TOOL_TIMEOUT_MS` 同口径，ADR-054 §2.5）。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// 请求超时上限（毫秒）。
pub const MAX_TIMEOUT_MS: u64 = 600_000;

/// 默认单次工具输出上限（字节；与内置工具 64 KiB 口径一致）。
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// 单次工具输出上限（字节）。
pub const MAX_OUTPUT_LIMIT: usize = 1024 * 1024;

/// 单服务器配置。
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    /// 服务器名（`[a-z0-9_-]{1,32}`；命名空间 `mcp__<name>__<tool>` 的组成部分）。
    pub name: String,
    /// 启动命令（可执行文件名或路径；PATH 解析交给父环境最小白名单）。
    pub command: String,
    /// 启动参数（≤ 32 条）。
    pub args: Vec<String>,
    /// 显式 env（≤ [`MAX_ENV_KEYS`] 键；用户为该服务器声明的授权通道，ADR-054 §2.9）。
    pub env: BTreeMap<String, String>,
    /// 请求超时（毫秒）。
    pub timeout_ms: u64,
    /// 单次工具输出上限（字节）。
    pub max_output_bytes: usize,
    /// 是否启用（关闭的服务器不 spawn、不投影）。
    pub enabled: bool,
}

/// 全量配置：服务器列表（≤ [`MAX_SERVERS`]，重名拒绝）。
#[derive(Debug, Clone, Default)]
pub struct McpConfig {
    /// 启用的服务器（加载时校验与排序后的稳定列表）。
    pub servers: Vec<McpServerConfig>,
}

/// serde 中间形态（`deny_unknown_fields` 拒绝未知字段；缺省值显式）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawServer {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<usize>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

/// 配置文件顶层形态（`deny_unknown_fields`）。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    servers: Vec<RawServer>,
}

impl McpConfig {
    /// 从工作区加载配置：文件缺失返回 `Ok(None)`（功能关闭），存在则解析并校验。
    ///
    /// # Errors
    ///
    /// 读取失败（IO）或解析/校验失败（[`ErrorCode::ParseError`]）时返回 [`WhError`]。
    pub fn load(workspace: &Path) -> Result<Option<Self>, WhError> {
        let path = workspace.join(CONFIG_PATH);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(WhError::new(
                    ErrorCode::IoError,
                    format!("读取 MCP 配置失败 {}: {err}", path.display()),
                ));
            }
        };
        Self::parse(&bytes).map(Some)
    }

    /// 从字节解析并校验配置（测试与 [`Self::load`] 共用）。
    ///
    /// # Errors
    ///
    /// JSON 非法、未知字段、服务器重名、名称/命令/env/超时越界时返回
    /// [`ErrorCode::ParseError`]。
    pub fn parse(bytes: &[u8]) -> Result<Self, WhError> {
        let raw: RawConfig = serde_json::from_slice(bytes).map_err(|err| {
            WhError::new(
                ErrorCode::ParseError,
                format!("MCP 配置解析失败（拒绝未知字段）：{err}"),
            )
        })?;
        if raw.servers.len() > MAX_SERVERS {
            return Err(WhError::new(
                ErrorCode::ParseError,
                format!("MCP 服务器数 {} 超上限 {MAX_SERVERS}", raw.servers.len()),
            ));
        }
        let mut servers = Vec::with_capacity(raw.servers.len());
        let mut seen = std::collections::BTreeSet::new();
        for server in raw.servers {
            validate_server(&server)?;
            if !seen.insert(server.name.clone()) {
                return Err(WhError::new(
                    ErrorCode::ParseError,
                    format!("MCP 服务器重名：{}", server.name),
                ));
            }
            servers.push(McpServerConfig {
                name: server.name,
                command: server.command,
                args: server.args,
                env: server.env,
                timeout_ms: server.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
                max_output_bytes: server.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES),
                enabled: server.enabled,
            });
        }
        Ok(Self { servers })
    }

    /// 启用的服务器配置（投影与连接只看这些）。
    #[must_use]
    pub fn enabled_servers(&self) -> Vec<&McpServerConfig> {
        self.servers.iter().filter(|s| s.enabled).collect()
    }
}

/// 名称字符集：`[a-z0-9_-]`、长度 1—32，且**禁止连续下划线**（`mcp__<server>__<tool>`
/// 命名空间按首个 `__` 切分，服务器名含 `__` 会与工具名歧义，ADR-054 §2.7）。
pub(crate) fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !name.contains("__")
}

/// env 键字符集：`[A-Za-z0-9_]`、长度 1—64。
fn valid_env_key(key: &str) -> bool {
    (1..=64).contains(&key.len()) && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn validate_server(server: &RawServer) -> Result<(), WhError> {
    if !valid_name(&server.name) {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!(
                "MCP 服务器名 `{}` 非法（要求 [a-z0-9_-]，长度 1—32）",
                server.name
            ),
        ));
    }
    if server.command.trim().is_empty() {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!("MCP 服务器 {} 的 command 不能为空", server.name),
        ));
    }
    if server.args.len() > 32 {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!(
                "MCP 服务器 {} 启动参数数 {} 超上限 32",
                server.name,
                server.args.len()
            ),
        ));
    }
    if server.env.len() > MAX_ENV_KEYS {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!(
                "MCP 服务器 {} env 键数 {} 超上限 {MAX_ENV_KEYS}",
                server.name,
                server.env.len()
            ),
        ));
    }
    for key in server.env.keys() {
        if !valid_env_key(key) {
            return Err(WhError::new(
                ErrorCode::ParseError,
                format!(
                    "MCP 服务器 {} 的 env 键 `{key}` 非法（要求 [A-Za-z0-9_]，长度 1—64）",
                    server.name
                ),
            ));
        }
    }
    if let Some(timeout) = server.timeout_ms
        && (timeout == 0 || timeout > MAX_TIMEOUT_MS)
    {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!(
                "MCP 服务器 {} timeout_ms {timeout} 越界（1—{MAX_TIMEOUT_MS}）",
                server.name
            ),
        ));
    }
    if let Some(limit) = server.max_output_bytes
        && (limit == 0 || limit > MAX_OUTPUT_LIMIT)
    {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!(
                "MCP 服务器 {} max_output_bytes {limit} 越界（1—{MAX_OUTPUT_LIMIT}）",
                server.name
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    #[test]
    fn parse_valid_config_with_defaults() {
        let config = McpConfig::parse(
            br#"{"servers":[{"name":"fs","command":"npx","args":["-y","server-fs"]}]}"#,
        )
        .expect("合法配置");
        assert_eq!(config.servers.len(), 1);
        let server = &config.servers[0];
        assert_eq!(server.timeout_ms, DEFAULT_TIMEOUT_MS, "缺省超时 30s");
        assert_eq!(server.max_output_bytes, DEFAULT_MAX_OUTPUT_BYTES);
        assert!(server.enabled, "缺省启用");
        assert_eq!(config.enabled_servers().len(), 1);
    }

    #[test]
    fn missing_file_is_correct_empty() {
        let dir = std::env::temp_dir().join(format!("whcode-mcp-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let loaded = McpConfig::load(&dir).expect("缺文件=Ok(None)");
        assert!(loaded.is_none(), "无配置文件=功能关闭（正确的空）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_present_but_malformed_is_explicit_error() {
        let dir = std::env::temp_dir().join(format!("whcode-mcp-bad-{}", std::process::id()));
        let zcode = dir.join(".zcode");
        std::fs::create_dir_all(&zcode).expect("建目录");
        std::fs::write(zcode.join("mcp.json"), b"{ not json }").expect("写坏配置");
        let err = McpConfig::load(&dir).expect_err("坏配置必须显式报错（错误的空）");
        assert_eq!(err.code, ErrorCode::ParseError);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_unknown_fields_duplicate_names_and_bounds() {
        let err = McpConfig::parse(br#"{"servers":[{"name":"a","command":"c","bogus":1}]}"#)
            .expect_err("未知字段拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);

        let dup = br#"{"servers":[{"name":"a","command":"c"},{"name":"a","command":"c"}]}"#;
        let err = McpConfig::parse(dup).expect_err("重名拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);

        let err = McpConfig::parse(br#"{"servers":[{"name":"BadName","command":"c"}]}"#)
            .expect_err("大写名拒绝");
        assert!(err.message.contains("非法"), "指名非法原因：{err}");

        let err =
            McpConfig::parse(br#"{"servers":[{"name":"a","command":"c","timeout_ms":900000}]}"#)
                .expect_err("超时越上限拒绝");
        assert!(err.message.contains("越界"), "指名越界：{err}");

        let err =
            McpConfig::parse(br#"{"servers":[{"name":"a","command":"c","env":{"BAD KEY":"v"}}]}"#)
                .expect_err("env 键非法拒绝");
        assert!(err.message.contains("env 键"), "指名 env 键：{err}");
    }

    #[test]
    fn server_count_and_disabled_filter() {
        let mut json = String::from(r#"{"servers":["#);
        for i in 0..MAX_SERVERS {
            if i > 0 {
                json.push(',');
            }
            json.push_str(&format!(r#"{{"name":"s{i}","command":"c"}}"#));
        }
        json.push_str(r#"]}"#);
        let config = McpConfig::parse(json.as_bytes()).expect("恰好 8 台");
        assert_eq!(config.servers.len(), MAX_SERVERS);

        let err = McpConfig::parse(
            br#"{"servers":[{"name":"s0","command":"c"},{"name":"s1","command":"c"},{"name":"s2","command":"c"},{"name":"s3","command":"c"},{"name":"s4","command":"c"},{"name":"s5","command":"c"},{"name":"s6","command":"c"},{"name":"s7","command":"c"},{"name":"s8","command":"c"}]}"#,
        )
        .expect_err("第 9 台拒绝");
        assert!(err.message.contains("超上限"), "指名超上限：{err}");

        let config = McpConfig::parse(
            br#"{"servers":[{"name":"on","command":"c"},{"name":"off","command":"c","enabled":false}]}"#,
        )
        .expect("合法配置");
        assert_eq!(
            config.enabled_servers().len(),
            1,
            "enabled=false 不参与连接"
        );
        assert_eq!(config.enabled_servers()[0].name, "on");
    }
}
