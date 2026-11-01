//! # MCP 工具投影与连接管理（FR-737、ADR-054 §2.7—2.10）
//!
//! [`McpManager`] 持有 per-turn 的服务器会话与工具清单：连接 = spawn → initialize →
//! tools/list；连接失败逐服务器记入 [`McpManager::connect_errors`]（显式告示，不静默）。
//! 投影把服务器工具变为 `mcp__<server>__<tool>` 命名空间下的 provider 清单条目
//! （描述截断、schema 上限、总预算三重有界；越界跳过/丢尾均有注记）。
//!
//! 动态名**不进** `whcode-tools` 内置注册表（其 `ToolDescriptor.name: &'static str`
//! 是静态名契约）：清单在组装层合并、分派在 `execute_tool_call` 走
//! [`parse_tool_name`] 前缀分支（ADR-054 §2.7）。

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use whcode_domain::{ErrorCode, WhError};

use crate::config::McpConfig;
use crate::protocol::{McpTool, ToolCallResult};
use crate::session::{CancelFlag, McpSession};

/// MCP 工具命名空间前缀（`mcp__<server>__<tool>`）。
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// 每服务器工具数上限（tools/list 累计；超出部分丢弃并 `truncated` 标记）。
pub const MAX_TOOLS_PER_SERVER: usize = 64;

/// 工具描述字符上限（投影截断）。
pub const MAX_DESC_CHARS: usize = 500;

/// 单工具 inputSchema 序列化字节上限（超限跳过该工具并注记）。
pub const MAX_SCHEMA_BYTES: usize = 4 * 1024;

/// MCP 清单总预算（字节；超出按序丢尾并 `truncated` 标记）。
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;

/// 服务器 tools/list 的聚合结果。
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolSet {
    /// 全部工具（已按每服务器上限截断）。
    pub tools: Vec<McpTool>,
    /// 是否因上限截断（显式标记，R-05）。
    pub truncated: bool,
}

/// 把展示名拆回 `(server, tool)`；非 `mcp__` 前缀或任一侧名称非法返回 `None`
/// （内置工具走原路径）。
#[must_use]
pub fn parse_tool_name(full: &str) -> Option<(String, String)> {
    let rest = full.strip_prefix(MCP_TOOL_PREFIX)?;
    let (server, tool) = rest.split_once("__")?;
    if !crate::config::valid_name(server) || !valid_tool_name(tool) {
        return None;
    }
    Some((server.to_string(), tool.to_string()))
}

/// 命名空间展示名。
#[must_use]
pub fn tool_display_name(server: &str, tool: &str) -> String {
    format!("{MCP_TOOL_PREFIX}{server}__{tool}")
}

/// 工具名字符集：`[A-Za-z0-9_-]`、长度 1—64（ADR-054 §2.7）。
fn valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 单工具投影：描述截 500 字符、schema ≤ 4 KiB；名称越界返回 `None`（跳过）。
#[must_use]
pub fn manifest_entry(server: &str, tool: &McpTool) -> Option<Value> {
    if !valid_tool_name(&tool.name) {
        return None;
    }
    let schema = if tool.input_schema.to_string().len() > MAX_SCHEMA_BYTES {
        // Schema 超限：以空对象占位会误导模型——整个跳过更诚实（调用方注记）。
        return None;
    } else {
        tool.input_schema.clone()
    };
    let description: String = tool.description.chars().take(MAX_DESC_CHARS).collect();
    Some(serde_json::json!({
        "type": "function",
        "function": {
            "name": tool_display_name(server, &tool.name),
            "description": description,
            "parameters": schema,
        },
    }))
}

/// 已连接服务器（会话 + 工具集 + 调用参数快照）。
pub struct ConnectedServer {
    /// stdio 会话。
    pub session: McpSession,
    /// tools/list 结果（已按上限截断）。
    pub tools: Vec<McpTool>,
    /// tools/list 是否被上限截断。
    pub tools_truncated: bool,
    /// 请求超时（配置快照）。
    pub timeout: Duration,
    /// 输出上限（配置快照）。
    pub max_output_bytes: usize,
}

/// 清单投影结果（条目 + 截断标记 + 投影注记）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpManifest {
    /// provider 清单条目（`type:"function"` 形态，与内置注册表清单同构）。
    pub entries: Vec<Value>,
    /// 是否因总预算丢尾。
    pub truncated: bool,
    /// 投影注记（名称越界/schema 超限跳过的工具，逐条可定位）。
    pub notes: Vec<String>,
}

/// per-turn MCP 连接管理器（`Drop` 级联回收全部会话）。
#[derive(Default)]
pub struct McpManager {
    servers: BTreeMap<String, ConnectedServer>,
    connect_errors: Vec<(String, String)>,
}

impl McpManager {
    /// 从工作区连接全部 enabled 服务器；无工作区/无配置 = 空管理器（正确的空）。
    ///
    /// # Errors
    ///
    /// 配置加载失败（坏配置显式报错）时返回 [`WhError`]；单服务器连接失败**不**使
    /// 本函数失败——记入 [`Self::connect_errors`] 由调用方告示。
    pub fn connect_workspace(workspace: Option<&Path>) -> Result<Self, WhError> {
        let Some(workspace) = workspace else {
            return Ok(Self::default());
        };
        let Some(config) = McpConfig::load(workspace)? else {
            return Ok(Self::default());
        };
        let mut manager = Self::default();
        for server in config.enabled_servers() {
            let timeout = Duration::from_millis(server.timeout_ms);
            match Self::connect_one(server, timeout) {
                Ok(connected) => {
                    manager.servers.insert(server.name.clone(), connected);
                }
                Err(err) => {
                    manager
                        .connect_errors
                        .push((server.name.clone(), err.to_string()));
                }
            }
        }
        Ok(manager)
    }

    fn connect_one(
        server: &crate::config::McpServerConfig,
        timeout: Duration,
    ) -> Result<ConnectedServer, WhError> {
        let mut session = McpSession::connect(server)?;
        session.initialize(timeout, None)?;
        let set = session.list_tools(timeout, None)?;
        Ok(ConnectedServer {
            session,
            tools: set.tools,
            tools_truncated: set.truncated,
            timeout,
            max_output_bytes: server.max_output_bytes,
        })
    }

    /// 是否没有连接任何服务器（无配置或全部失败）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    /// 连接失败清单（`server, message`；显式告示用，不静默）。
    #[must_use]
    pub fn connect_errors(&self) -> &[(String, String)] {
        &self.connect_errors
    }

    /// 投影合并清单（BTreeMap 服务器序 + 工具名字典序，确定性输出）。
    #[must_use]
    pub fn tool_manifest(&self) -> McpManifest {
        let mut manifest = McpManifest::default();
        let mut used = 0usize;
        'outer: for (server_name, server) in &self.servers {
            let mut tools: Vec<&McpTool> = server.tools.iter().collect();
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            for tool in tools {
                let Some(entry) = manifest_entry(server_name, tool) else {
                    manifest.notes.push(format!(
                        "服务器 {server_name} 的工具 {} 被跳过（名称越界或 inputSchema 超 {MAX_SCHEMA_BYTES} 字节）",
                        tool.name
                    ));
                    continue;
                };
                let entry_len = entry.to_string().len();
                if used + entry_len > MAX_MANIFEST_BYTES {
                    manifest.truncated = true;
                    break 'outer;
                }
                used += entry_len;
                manifest.entries.push(entry);
            }
            if server.tools_truncated {
                manifest.notes.push(format!(
                    "服务器 {server_name} 的工具清单超过 {MAX_TOOLS_PER_SERVER} 条上限，超出部分未投影"
                ));
            }
        }
        manifest
    }

    /// 调用 `mcp__<server>__<tool>` 中的工具（会话失效/超时/取消按 `Err` 归因）。
    ///
    /// # Errors
    ///
    /// 服务器未连接或会话失败时返回 [`WhError`]。
    pub fn call_tool(
        &mut self,
        server: &str,
        tool: &str,
        arguments: &Value,
        cancel: Option<CancelFlag>,
    ) -> Result<ToolCallResult, WhError> {
        let Some(connected) = self.servers.get_mut(server) else {
            return Err(WhError::new(
                ErrorCode::NotFound,
                format!("MCP 服务器 {server} 未连接"),
            ));
        };
        let timeout = connected.timeout;
        let max_output = connected.max_output_bytes;
        connected
            .session
            .call_tool(tool, arguments, timeout, cancel, max_output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_tool_name_splits_at_first_separator() {
        assert_eq!(
            parse_tool_name("mcp__fs__read"),
            Some(("fs".to_string(), "read".to_string()))
        );
        // 工具名可含连续下划线——在首个分隔符处切分，剩余归工具名。
        assert_eq!(
            parse_tool_name("mcp__fs__read__file"),
            Some(("fs".to_string(), "read__file".to_string()))
        );
        assert_eq!(parse_tool_name("read_file"), None, "非 mcp__ 前缀");
        assert_eq!(parse_tool_name("mcp__bad name__t"), None, "工具名非法字符");
        assert_eq!(parse_tool_name("mcp__fs__"), None, "空工具名");
    }

    #[test]
    fn manifest_entry_bounds_description_and_schema() {
        let long_desc = "x".repeat(2000);
        let tool = McpTool {
            name: "read".to_string(),
            description: long_desc,
            input_schema: json!({"type": "object"}),
        };
        let entry = manifest_entry("fs", &tool).expect("合法投影");
        let desc = entry["function"]["description"].as_str().expect("字符串");
        assert_eq!(desc.chars().count(), MAX_DESC_CHARS, "描述截断到上限");
        assert_eq!(entry["function"]["name"], "mcp__fs__read");
        assert_eq!(entry["type"], "function");

        let big_schema = json!({"type": "object", "x": "y".repeat(5 * 1024)});
        let tool = McpTool {
            name: "big".to_string(),
            description: "d".to_string(),
            input_schema: big_schema,
        };
        assert!(manifest_entry("fs", &tool).is_none(), "schema 超限跳过");

        let bad_name = McpTool {
            name: "bad name".to_string(),
            description: "d".to_string(),
            input_schema: json!({"type": "object"}),
        };
        assert!(manifest_entry("fs", &bad_name).is_none(), "名称越界跳过");
    }

    #[test]
    fn empty_manager_is_correct_empty() {
        let manager = McpManager::connect_workspace(None).expect("无工作区");
        assert!(manager.is_empty(), "无工作区=正确的空");
        assert!(manager.connect_errors().is_empty());
        let manifest = manager.tool_manifest();
        assert!(manifest.entries.is_empty() && !manifest.truncated && manifest.notes.is_empty());
    }

    #[test]
    fn missing_workspace_config_is_correct_empty() {
        let dir = std::env::temp_dir().join(format!("whcode-mcp-mgr-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let manager = McpManager::connect_workspace(Some(&dir)).expect("无配置");
        assert!(manager.is_empty(), "无 .zcode/mcp.json=功能关闭");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_failure_is_recorded_not_fatal() {
        let dir = std::env::temp_dir().join(format!("whcode-mcp-fail-{}", std::process::id()));
        let zcode = dir.join(".zcode");
        std::fs::create_dir_all(&zcode).expect("建目录");
        std::fs::write(
            zcode.join("mcp.json"),
            br#"{"servers":[{"name":"dead","command":"whcode-mcp-nonexistent-program-xyz"}]}"#,
        )
        .expect("写配置");
        let manager = McpManager::connect_workspace(Some(&dir)).expect("配置合法");
        assert!(manager.is_empty(), "连接失败不进服务器表");
        let errors = manager.connect_errors();
        assert_eq!(errors.len(), 1, "失败逐服务器记录");
        assert_eq!(errors[0].0, "dead");
        assert!(!errors[0].1.is_empty(), "失败原因可定位");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
