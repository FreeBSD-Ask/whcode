//! # LSP 服务器配置（ADR-078 决策 1）
//!
//! 配置文件＝`<workspace>/.zcode/lsp.json`，与 `.zcode/mcp.json`／`.zcode/shell.json` 同
//! 模式：**显式 opt-in、无自动发现**，文件不存在即“未配置”（正确的空）。写错（坏 JSON、
//! 超限、字段缺失）**显式失败**——不静默降级为“未配置”，否则用户会以为自己的服务器在跑。
//!
//! 有界性：服务器 ≤ [`MAX_SERVERS`] 台、单台参数 ≤ [`MAX_ARGS`] 条（每条 ≤
//! [`MAX_ARG_BYTES`] 字节）、扩展名 ≤ [`MAX_EXTENSIONS`] 条，文件 ≤ [`MAX_CONFIG_BYTES`]。
//!
//! 命令与参数由用户配置给出，**不经模型**（模型工具面属 P5.3b，届时按 ADR-070 的审批口径
//! 接线）。

use std::path::Path;

use serde::Deserialize;
use whcode_domain::WhError;

/// 配置文件路径（相对工作区根）。
pub const CONFIG_PATH: &str = ".zcode/lsp.json";

/// 配置文件字节上限。
pub const MAX_CONFIG_BYTES: usize = 16 * 1024;

/// 服务器条数上限（每台服务器都要起一个子进程，不放开累积）。
pub const MAX_SERVERS: usize = 4;

/// 单台服务器参数条数上限。
pub const MAX_ARGS: usize = 16;

/// 单个参数／命令的字节上限。
pub const MAX_ARG_BYTES: usize = 256;

/// 单台服务器扩展名条数上限。
pub const MAX_EXTENSIONS: usize = 16;

/// 单个扩展名字节上限（含点号）。
pub const MAX_EXTENSION_BYTES: usize = 16;

/// 服务器标识字节上限。
pub const MAX_ID_BYTES: usize = 32;

/// 请求超时缺省值（毫秒）。
pub const DEFAULT_TIMEOUT_MS: u64 = 5_000;

/// 请求超时下限（毫秒）：低于此值等于必然超时，属配置错误。
pub const MIN_TIMEOUT_MS: u64 = 100;

/// 请求超时上限（毫秒）。
pub const MAX_TIMEOUT_MS: u64 = 30_000;

/// 一台语言服务器的配置（已校验）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// 稳定标识（`a-z0-9-`，1..=[`MAX_ID_BYTES`] 字节）——错误消息与缓存键用它，不用可执行路径。
    pub id: String,
    /// 可执行文件（不解析 shell；直传 argv，ADR-070 T7 口径）。
    pub command: String,
    /// 参数（逐条直传）。
    pub args: Vec<String>,
    /// 负责的扩展名（含点号，小写比较）。
    pub extensions: Vec<String>,
    /// 单请求超时（毫秒，已夹取到 `MIN..=MAX`）。
    pub timeout_ms: u64,
}

impl ServerConfig {
    /// 该服务器是否负责此扩展名（大小写不敏感）。
    #[must_use]
    pub fn supports_extension(&self, extension: &str) -> bool {
        self.extensions
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
    }
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    servers: Vec<ServerEntry>,
}

#[derive(Debug, Deserialize)]
struct ServerEntry {
    id: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// 读取并校验配置；文件不存在返回空列表（未 opt-in）。
///
/// # Errors
///
/// 文件超限、JSON 非法、任一条目违反上限或取值非法时返回 [`ErrorCode::ParseError`]／
/// [`ErrorCode::InvalidState`]，消息含路径与补救方向。
pub fn load(workspace: &Path) -> Result<Vec<ServerConfig>, WhError> {
    let path = workspace.join(CONFIG_PATH);
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(Vec::new());
    };
    let display = path.display();
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(WhError::invalid_state(format!(
            "LSP 配置 {} 字节超上限 {MAX_CONFIG_BYTES}：{display}",
            bytes.len()
        )));
    }
    let parsed: ConfigFile = serde_json::from_slice(&bytes)
        .map_err(|err| WhError::parse_error(format!("LSP 配置解析失败（{display}）：{err}")))?;
    if parsed.servers.len() > MAX_SERVERS {
        return Err(WhError::invalid_state(format!(
            "LSP 服务器 {} 台超过上限 {MAX_SERVERS}：{display}",
            parsed.servers.len()
        )));
    }
    let mut servers = Vec::with_capacity(parsed.servers.len());
    for entry in parsed.servers {
        servers.push(validate(entry, &display)?);
    }
    Ok(servers)
}

fn validate(entry: ServerEntry, display: &std::path::Display<'_>) -> Result<ServerConfig, WhError> {
    let id = entry.id.trim().to_string();
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return Err(WhError::invalid_state(format!(
            "LSP 服务器 id 长度非法（1..={MAX_ID_BYTES} 字节）：{display}"
        )));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(WhError::invalid_state(format!(
            "LSP 服务器 id 只允许 a-z、0-9 与 `-`：{id}（{display}）"
        )));
    }
    let command = entry.command.trim().to_string();
    if command.is_empty() || command.len() > MAX_ARG_BYTES {
        return Err(WhError::invalid_state(format!(
            "LSP 服务器 {id} 的 command 长度非法（1..={MAX_ARG_BYTES} 字节）"
        )));
    }
    if entry.args.len() > MAX_ARGS {
        return Err(WhError::invalid_state(format!(
            "LSP 服务器 {id} 参数 {} 条超过上限 {MAX_ARGS}",
            entry.args.len()
        )));
    }
    for arg in &entry.args {
        if arg.len() > MAX_ARG_BYTES {
            return Err(WhError::invalid_state(format!(
                "LSP 服务器 {id} 的参数超过 {MAX_ARG_BYTES} 字节"
            )));
        }
    }
    if entry.extensions.len() > MAX_EXTENSIONS {
        return Err(WhError::invalid_state(format!(
            "LSP 服务器 {id} 扩展名 {} 条超过上限 {MAX_EXTENSIONS}",
            entry.extensions.len()
        )));
    }
    for extension in &entry.extensions {
        let valid = extension.starts_with('.')
            && extension.len() >= 2
            && extension.len() <= MAX_EXTENSION_BYTES
            && extension
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.');
        if !valid {
            return Err(WhError::invalid_state(format!(
                "LSP 服务器 {id} 的扩展名非法（须形如 `.rs`，≤{MAX_EXTENSION_BYTES} 字节）：{extension}"
            )));
        }
    }
    let timeout_ms = entry
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
    Ok(ServerConfig {
        id,
        command,
        args: entry.args,
        extensions: entry.extensions,
        timeout_ms,
    })
}

/// 按扩展名挑选服务器（首个命中的服务器；未命中返回 `None`）。
#[must_use]
pub fn for_extension<'a>(servers: &'a [ServerConfig], extension: &str) -> Option<&'a ServerConfig> {
    servers
        .iter()
        .find(|server| server.supports_extension(extension))
}

/// 小写扩展名（含点号）；无扩展名时返回空串。
#[must_use]
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default()
}

/// 扩展名 → LSP `languageId`（封闭映射；未知扩展名给 `plaintext`，不猜语言）。
#[must_use]
pub fn language_id(path: &Path) -> &'static str {
    match extension_of(path).as_str() {
        ".rs" => "rust",
        ".ts" | ".mts" | ".cts" => "typescript",
        ".tsx" => "typescriptreact",
        ".js" | ".mjs" | ".cjs" => "javascript",
        ".jsx" => "javascriptreact",
        ".py" => "python",
        ".go" => "go",
        ".java" => "java",
        ".c" | ".h" => "c",
        ".cc" | ".cpp" | ".hpp" | ".cxx" => "cpp",
        ".json" => "json",
        ".toml" => "toml",
        ".yaml" | ".yml" => "yaml",
        ".md" => "markdown",
        ".sh" => "shellscript",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;

    fn workspace(tag: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "whcode-lsp-config-{tag}-{}-{stamp}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".zcode")).expect("建夹具");
        dir
    }

    /// 无配置＝未 opt-in：空列表、不报错。
    #[test]
    fn absent_config_means_not_configured() {
        let dir = workspace("absent");
        assert!(load(&dir).expect("读取").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 合法配置解析出全部字段，超时缺省并对越界值夹取。
    #[test]
    fn valid_config_loads_and_clamps_timeout() {
        let dir = workspace("valid");
        std::fs::write(
            dir.join(CONFIG_PATH),
            r#"{"servers":[
                {"id":"ra","command":"rust-analyzer","extensions":[".rs"],"timeout_ms":999999},
                {"id":"ts","command":"typescript-language-server","args":["--stdio"],"extensions":[".ts",".tsx"],"timeout_ms":1}
            ]}"#,
        )
        .expect("写配置");
        let servers = load(&dir).expect("读取");
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].timeout_ms, MAX_TIMEOUT_MS, "越界必须夹取");
        assert_eq!(servers[1].timeout_ms, MIN_TIMEOUT_MS);
        assert_eq!(servers[1].args, vec!["--stdio".to_string()]);
        assert!(
            servers[0].supports_extension(".RS"),
            "扩展名比较不区分大小写"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 坏 JSON／非法 id／非法扩展名／超限条数：全部显式失败。
    #[test]
    fn invalid_config_fails_loudly() {
        let dir = workspace("bad");
        let cases = [
            (r#"{ not json }"#, ErrorCode::ParseError),
            (
                r#"{"servers":[{"id":"Bad Id","command":"x","extensions":[".rs"]}]}"#,
                ErrorCode::InvalidState,
            ),
            (
                r#"{"servers":[{"id":"ra","command":"x","extensions":["rs"]}]}"#,
                ErrorCode::InvalidState,
            ),
            (
                r#"{"servers":[{"id":"ra","command":"","extensions":[".rs"]}]}"#,
                ErrorCode::InvalidState,
            ),
        ];
        for (raw, expected) in cases {
            std::fs::write(dir.join(CONFIG_PATH), raw).expect("写配置");
            let err = load(&dir).expect_err("必须显式失败");
            assert_eq!(err.code, expected, "配置：{raw}");
        }

        let too_many: String = (0..=MAX_SERVERS)
            .map(|index| format!(r#"{{"id":"s{index}","command":"x","extensions":[".rs"]}}"#))
            .collect::<Vec<_>>()
            .join(",");
        std::fs::write(
            dir.join(CONFIG_PATH),
            format!(r#"{{"servers":[{too_many}]}}"#),
        )
        .expect("写配置");
        let err = load(&dir).expect_err("超限服务器数必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 扩展名映射与 `for_extension` 的选择顺序（首个命中）。
    #[test]
    fn extension_selection_and_language_ids() {
        let servers = vec![
            ServerConfig {
                id: "a".to_string(),
                command: "a".to_string(),
                args: Vec::new(),
                extensions: vec![".rs".to_string()],
                timeout_ms: DEFAULT_TIMEOUT_MS,
            },
            ServerConfig {
                id: "b".to_string(),
                command: "b".to_string(),
                args: Vec::new(),
                extensions: vec![".rs".to_string(), ".ts".to_string()],
                timeout_ms: DEFAULT_TIMEOUT_MS,
            },
        ];
        assert_eq!(
            for_extension(&servers, ".rs").map(|server| server.id.as_str()),
            Some("a"),
            "首个命中的服务器优先"
        );
        assert_eq!(
            for_extension(&servers, ".ts").map(|server| server.id.as_str()),
            Some("b")
        );
        assert!(for_extension(&servers, ".md").is_none());

        assert_eq!(language_id(Path::new("x.rs")), "rust");
        assert_eq!(language_id(Path::new("x.TSX")), "typescriptreact");
        assert_eq!(language_id(Path::new("x.unknown")), "plaintext");
        assert_eq!(extension_of(Path::new("x.TS")), ".ts");
        assert_eq!(extension_of(Path::new("Makefile")), "");
    }
}
