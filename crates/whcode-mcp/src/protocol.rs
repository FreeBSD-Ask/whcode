//! # MCP JSON-RPC 2.0 协议层（FR-736、ADR-054 §2.2）
//!
//! newline 分帧的 JSON-RPC 2.0 编解码与 MCP 消息解析（initialize / tools/list /
//! tools/call）。本层是纯函数层（无 IO），正反路径测试在此收口。
//!
//! 客户端只发 `u64` id（`0` 起自增）；入站响应 id 非本会话 `u64` 形态（字符串 id、
//! 缺 id 的请求→通知判别失败）一律按解析错误或"外来消息"处理，不猜测。

use serde_json::Value;
use whcode_domain::{ErrorCode, WhError};

/// 客户端请求的协议版本（ADR-054 §2.4；复审触发见 ADR §5）。
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// 客户端可接受的协议版本集合（initialize 协商：服务器回显必须落在其中）。
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

/// JSON-RPC 方法未找到（对未声明能力的服务器请求统一回此错误）。
pub const METHOD_NOT_FOUND: i64 = -32601;

/// 客户端信息（initialize 的 clientInfo）。
pub const CLIENT_NAME: &str = "whcode-mcp";

/// 编码请求：`{"jsonrpc":"2.0","id":N,"method":…,"params":…}`（单行、无内嵌换行——
/// serde_json 紧凑序列化保证）。
#[must_use]
pub fn encode_request(id: u64, method: &str, params: &Value) -> String {
    let mut line = String::with_capacity(64 + method.len());
    line.push_str(r#"{"jsonrpc":"2.0","id":"#);
    line.push_str(&id.to_string());
    line.push_str(r#","method":"#);
    line.push_str(&serde_json::to_string(method).unwrap_or_else(|_| r#""""#.to_string()));
    line.push_str(r#","params":"#);
    line.push_str(&params.to_string());
    line.push('}');
    line
}

/// 编码通知（无 id）。
#[must_use]
pub fn encode_notification(method: &str, params: &Value) -> String {
    let mut line = String::with_capacity(48 + method.len());
    line.push_str(r#"{"jsonrpc":"2.0","method":"#);
    line.push_str(&serde_json::to_string(method).unwrap_or_else(|_| r#""""#.to_string()));
    line.push_str(r#","params":"#);
    line.push_str(&params.to_string());
    line.push('}');
    line
}

/// 编码错误响应（对服务器请求的拒绝；`-32601` 见 [`METHOD_NOT_FOUND`]）。
#[must_use]
pub fn encode_error_response(id: u64, code: i64, message: &str) -> String {
    let mut line = String::with_capacity(64 + message.len());
    line.push_str(r#"{"jsonrpc":"2.0","id":"#);
    line.push_str(&id.to_string());
    line.push_str(r#","error":{"code":"#);
    line.push_str(&code.to_string());
    line.push_str(r#","message":"#);
    line.push_str(&serde_json::to_string(message).unwrap_or_else(|_| r#""""#.to_string()));
    line.push_str("}}");
    line
}

/// JSON-RPC 错误对象。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    /// 错误码。
    pub code: i64,
    /// 错误消息。
    pub message: String,
}

/// 入站消息三分类（ADR-054 §2.5：响应配对、通知计数、服务器请求拒绝）。
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// 响应（result 与 error 至少一者存在；MCP 禁止二者并存，解析层强制）。
    Response {
        /// 响应 id（客户端只接受 `u64`）。
        id: u64,
        /// 成功载荷。
        result: Option<Value>,
        /// 错误对象。
        error: Option<RpcError>,
    },
    /// 通知（无 id；计数丢弃）。
    Notification {
        /// 通知方法名。
        method: String,
    },
    /// 服务器请求（有 id 有 method；未声明能力，统一回 `-32601`）。
    ServerRequest {
        /// 请求 id。
        id: u64,
        /// 请求方法名。
        method: String,
    },
}

/// 解析单行入站消息。
///
/// # Errors
///
/// 非 JSON、非对象形态、jsonrpc 字段缺失、id 形态非法（非 u64 数值）或响应同时带
/// result 与 error 时返回 [`ErrorCode::ParseError`]。
pub fn parse_line(line: &str) -> Result<Incoming, WhError> {
    let fail = |msg: String| {
        Err(WhError::new(
            ErrorCode::ParseError,
            format!("MCP 入站消息解析失败：{msg}"),
        ))
    };
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => return fail(format!("{err}")),
    };
    let Some(object) = value.as_object() else {
        return fail("非 JSON 对象".to_string());
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return fail("jsonrpc 字段必须是 \"2.0\"".to_string());
    }
    let id_u64 = || -> Result<Option<u64>, WhError> {
        match object.get("id") {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => n.as_u64().map(Some).ok_or_else(|| {
                WhError::new(ErrorCode::ParseError, "id 必须是非负整数".to_string())
            }),
            Some(_) => Err(WhError::new(
                ErrorCode::ParseError,
                "id 形态非法（本客户端只发 u64 数字 id）".to_string(),
            )),
        }
    };
    let id = id_u64()?;
    let has_method = object.get("method").is_some();
    match (id, has_method) {
        (Some(id), true) => {
            let method = object
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(Incoming::ServerRequest { id, method })
        }
        (Some(id), false) => {
            let result = object.get("result");
            let error = object.get("error");
            match (result.is_some(), error.is_some()) {
                (true, false) => Ok(Incoming::Response {
                    id,
                    result: result.cloned(),
                    error: None,
                }),
                (false, true) => {
                    let code = error
                        .and_then(|e| e.get("code"))
                        .and_then(Value::as_i64)
                        .unwrap_or_default();
                    let message = error
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Ok(Incoming::Response {
                        id,
                        result: None,
                        error: Some(RpcError { code, message }),
                    })
                }
                (true, true) => fail("响应不得同时携带 result 与 error".to_string()),
                (false, false) => fail("响应既无 result 也无 error".to_string()),
            }
        }
        (None, true) => Ok(Incoming::Notification {
            method: object
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        (None, false) => fail("消息既无 id 也无 method".to_string()),
    }
}

/// initialize 结果（协议版本 + 服务器信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitializeResult {
    /// 服务器声明/回显的协议版本。
    pub protocol_version: String,
    /// 服务器名。
    pub server_name: String,
    /// 服务器版本。
    pub server_version: String,
    /// 服务器提供的使用说明（MCP `instructions`，P6.2／FR-785）：已按
    /// [`MAX_INSTRUCTIONS_CHARS`] 截断；`None`＝未提供（正确的空，不是空串）。
    /// **内容不可信**：它是外部服务器给的文本，注入上下文时必须显式标注来源。
    pub instructions: Option<String>,
    /// 是否声明 `capabilities.resources`（决定是否投影 resource 读取工具）。
    pub resources: bool,
}

/// 解析 initialize 结果并校验协议版本协商（ADR-054 §2.4：未知版本中止）。
///
/// # Errors
///
/// 结果形态非法或版本不在 [`SUPPORTED_VERSIONS`] 时返回 [`WhError`]。
pub fn parse_initialize_result(result: &Value) -> Result<InitializeResult, WhError> {
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !SUPPORTED_VERSIONS.contains(&version) {
        return Err(WhError::new(
            ErrorCode::RuntimeUnavailable,
            format!("MCP 服务器声明不支持的协议版本 `{version}`"),
        ));
    }
    let info = result.get("serverInfo");
    let instructions = result
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| text.chars().take(MAX_INSTRUCTIONS_CHARS).collect());
    let resources = result
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("resources"))
        .is_some_and(|resources| !resources.is_null());
    Ok(InitializeResult {
        protocol_version: version.to_string(),
        server_name: info
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        server_version: info
            .and_then(|i| i.get("version"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        instructions,
        resources,
    })
}

/// 投影用的工具描述（服务器 tools/list 条目；名称字符集校验在投影层）。
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    /// 工具名（服务器声明）。
    pub name: String,
    /// 面向模型的说明。
    pub description: String,
    /// 参数 JSON Schema（透传给 provider 清单）。
    pub input_schema: Value,
}

/// tools/list 解析结果（cursor 分页）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolListing {
    /// 本页工具。
    pub tools: Vec<McpTool>,
    /// 下一页 cursor（None = 末页）。
    pub next_cursor: Option<String>,
}

/// 解析 tools/list 结果。
///
/// # Errors
///
/// tools 非数组或条目形态非法时返回 [`ErrorCode::ParseError`]。
pub fn parse_tools_list(result: &Value) -> Result<ToolListing, WhError> {
    let fail = |msg: &str| {
        Err(WhError::new(
            ErrorCode::ParseError,
            format!("MCP tools/list 结果非法：{msg}"),
        ))
    };
    let Some(entries) = result.get("tools").and_then(Value::as_array) else {
        return fail("缺少 tools 数组");
    };
    let mut tools = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            return fail("工具缺少 name");
        };
        tools.push(McpTool {
            name: name.to_string(),
            description: entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            input_schema: entry
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "type": "object", "properties": {} })),
        });
    }
    Ok(ToolListing {
        tools,
        next_cursor: result
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// tools/call 结果（content[].text 拼接 + 截断/错误语义，ADR-054 §2.10）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallResult {
    /// 文本内容拼接（已在会话层按 max_output_bytes 截断）。
    pub text: String,
    /// 非文本 content 块数（只计数，不伪造文本）。
    pub non_text_count: usize,
    /// 服务器声明的执行失败（isError=true 按失败回填，不伪造成功）。
    pub is_error: bool,
    /// 是否发生截断。
    pub truncated: bool,
}

/// 解析 tools/call 结果。
///
/// # Errors
///
/// 缺少 content 数组时返回 [`ErrorCode::ParseError`]。
pub fn parse_tools_call(result: &Value) -> Result<ToolCallResult, WhError> {
    let Some(blocks) = result.get("content").and_then(Value::as_array) else {
        return Err(WhError::new(
            ErrorCode::ParseError,
            "MCP tools/call 结果缺少 content 数组",
        ));
    };
    let mut text = String::new();
    let mut non_text_count = 0usize;
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
            }
            _ => non_text_count += 1,
        }
    }
    Ok(ToolCallResult {
        text,
        non_text_count,
        is_error: result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        truncated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encode_request_notification_error_shapes() {
        let request = encode_request(7, "tools/call", &json!({"name":"a","arguments":{}}));
        assert!(request.starts_with(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":"#));
        assert!(!request.contains('\n'), "单行分帧：{request}");
        let value: Value = serde_json::from_str(&request).expect("可回解析");
        assert_eq!(value["id"], 7);
        assert_eq!(value["params"]["name"], "a");

        let notification = encode_notification("notifications/initialized", &json!({}));
        let value: Value = serde_json::from_str(&notification).expect("可回解析");
        assert!(value.get("id").is_none(), "通知无 id");
        assert_eq!(value["method"], "notifications/initialized");

        let error = encode_error_response(3, METHOD_NOT_FOUND, "未声明能力");
        let value: Value = serde_json::from_str(&error).expect("可回解析");
        assert_eq!(value["error"]["code"], -32601);
        assert_eq!(value["error"]["message"], "未声明能力");
    }

    #[test]
    fn parse_line_three_classes() {
        let incoming =
            parse_line(r#"{"jsonrpc":"2.0","id":9,"result":{"ok":true}}"#).expect("合法响应");
        assert_eq!(
            incoming,
            Incoming::Response {
                id: 9,
                result: Some(json!({"ok": true})),
                error: None
            }
        );

        let incoming =
            parse_line(r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"x"}}"#)
                .expect("错误响应");
        assert_eq!(
            incoming,
            Incoming::Response {
                id: 2,
                result: None,
                error: Some(RpcError {
                    code: -32000,
                    message: "x".to_string()
                })
            }
        );

        let incoming =
            parse_line(r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#)
                .expect("合法通知");
        assert_eq!(
            incoming,
            Incoming::Notification {
                method: "notifications/message".to_string()
            }
        );

        let incoming = parse_line(r#"{"jsonrpc":"2.0","id":4,"method":"sampling/createMessage"}"#)
            .expect("服务器请求");
        assert_eq!(
            incoming,
            Incoming::ServerRequest {
                id: 4,
                method: "sampling/createMessage".to_string()
            }
        );
    }

    #[test]
    fn parse_line_rejects_malformed_and_ambiguous() {
        for bad in [
            "not json",
            "[1,2,3]",
            r#"{"id":1,"result":{}}"#,
            r#"{"jsonrpc":"1.0","id":1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":"abc","result":{}}"#,
            r#"{"jsonrpc":"2.0","id":-1,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}"#,
            r#"{"jsonrpc":"2.0","id":1}"#,
            r#"{"jsonrpc":"2.0"}"#,
        ] {
            let err = parse_line(bad).expect_err(bad);
            assert_eq!(err.code, ErrorCode::ParseError, "{bad}");
        }
    }

    #[test]
    fn initialize_version_negotiation() {
        let ok = parse_initialize_result(&json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "serverInfo": {"name": "fs", "version": "1.2.3"}
        }))
        .expect("受支持版本");
        assert_eq!(ok.server_name, "fs");
        assert_eq!(ok.protocol_version, "2025-06-18");

        let err = parse_initialize_result(&json!({
            "protocolVersion": "1999-01-01",
            "serverInfo": {"name": "x", "version": "0"}
        }))
        .expect_err("未知版本必须中止");
        assert_eq!(err.code, ErrorCode::RuntimeUnavailable);
        assert!(err.message.contains("1999-01-01"));
    }

    #[test]
    fn tools_list_and_call_parsing() {
        let listing = parse_tools_list(&json!({
            "tools": [
                {"name": "read", "description": "读", "inputSchema": {"type": "object"}},
                {"name": "ping"}
            ],
            "nextCursor": "c2"
        }))
        .expect("合法列表");
        assert_eq!(listing.tools.len(), 2);
        assert_eq!(listing.next_cursor.as_deref(), Some("c2"));
        assert_eq!(
            listing.tools[1].input_schema,
            json!({"type": "object", "properties": {}}),
            "缺 inputSchema 用空对象兜底"
        );

        let call = parse_tools_call(&json!({
            "content": [
                {"type": "text", "text": "第一段"},
                {"type": "image", "data": "..."},
                {"type": "text", "text": "第二段"}
            ],
            "isError": true
        }))
        .expect("合法调用结果");
        assert_eq!(call.text, "第一段\n第二段");
        assert_eq!(call.non_text_count, 1, "非文本块只计数");
        assert!(call.is_error);
        assert!(!call.truncated);

        let err = parse_tools_call(&json!({"content": "not-array"})).expect_err("content 非数组");
        assert_eq!(err.code, ErrorCode::ParseError);
    }
}

// ── resources 与 instructions（P6.2／FR-785／ADR-054 修订 2026-11-15） ─────────────

/// 每服务器 resources 条数上限。
pub const MAX_RESOURCES_PER_SERVER: usize = 64;

/// resources/list 的分页上限（防服务器返回无终止的 cursor 链）。
pub const MAX_RESOURCE_PAGES: usize = 4;

/// 单个 resource URI 字符上限。
pub const MAX_RESOURCE_URI_CHARS: usize = 1024;

/// resource 名称字符上限。
pub const MAX_RESOURCE_NAME_CHARS: usize = 256;

/// resource 描述字符上限。
pub const MAX_RESOURCE_DESC_CHARS: usize = 500;

/// 单次 resources/read 的文本字节上限（128 KiB）。
pub const MAX_RESOURCE_BYTES: usize = 128 * 1024;

/// 单服务器 `instructions` 字符上限（4 KiB）。
pub const MAX_INSTRUCTIONS_CHARS: usize = 4096;

/// 单个 resource 条目（只保留展示与调用需要的字段，全部有界）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResource {
    /// URI（`resources/read` 的参数）。
    pub uri: String,
    /// 名称。
    pub name: String,
    /// 描述（可空，已截断）。
    pub description: String,
    /// MIME 类型（可缺省）。
    pub mime_type: Option<String>,
}

/// resources/list 结果（已按上限截断）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceList {
    /// resource 条目。
    pub resources: Vec<McpResource>,
    /// 是否因上限被截断或跳过超限条目。
    pub truncated: bool,
    /// 下一页游标（服务器提供时；会话层据此分页，页数另有上限）。
    pub next_cursor: Option<String>,
}

/// resources/read 结果（文本拼接，有界）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpResourceContent {
    /// 文本内容（多个 text 块以换行拼接）。
    pub text: String,
    /// 非文本块数（blob 等；只计数，不伪造文本）。
    pub omitted_blocks: usize,
    /// 是否发生截断（字节上限或非文本块）。
    pub truncated: bool,
}

/// 解析 `resources/list` 结果。
///
/// # Errors
///
/// 缺少 `resources` 数组时返回 [`ErrorCode::ParseError`]；单条非法（缺 uri/name）跳过，
/// 不因一条坏条目丢掉整份清单。
pub fn parse_resources_list(result: &Value) -> Result<McpResourceList, WhError> {
    let entries = result
        .get("resources")
        .and_then(Value::as_array)
        .ok_or_else(|| WhError::parse_error("MCP resources/list 结果缺少 resources 数组"))?;
    let mut resources = Vec::new();
    let mut truncated = false;
    for entry in entries {
        let (Some(uri), Some(name)) = (
            entry.get("uri").and_then(Value::as_str),
            entry.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        if uri.chars().count() > MAX_RESOURCE_URI_CHARS
            || name.chars().count() > MAX_RESOURCE_NAME_CHARS
        {
            truncated = true;
            continue;
        }
        if resources.len() >= MAX_RESOURCES_PER_SERVER {
            truncated = true;
            break;
        }
        resources.push(McpResource {
            uri: uri.to_string(),
            name: name.to_string(),
            description: entry
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(MAX_RESOURCE_DESC_CHARS)
                .collect(),
            mime_type: entry
                .get("mimeType")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    Ok(McpResourceList {
        resources,
        truncated,
        next_cursor: result
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string),
    })
}

/// 解析 `resources/read` 结果：`contents[]` 中的 text 块拼接，blob 只计数。
///
/// # Errors
///
/// 缺少 `contents` 数组时返回 [`ErrorCode::ParseError`]。
pub fn parse_resource_read(result: &Value) -> Result<McpResourceContent, WhError> {
    let contents = result
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| WhError::parse_error("MCP resources/read 结果缺少 contents 数组"))?;
    let mut text = String::new();
    let mut omitted_blocks = 0usize;
    let mut truncated = false;
    for block in contents {
        let Some(chunk) = block.get("text").and_then(Value::as_str) else {
            omitted_blocks += 1;
            continue;
        };
        if !text.is_empty() {
            text.push('\n');
        }
        let remaining = MAX_RESOURCE_BYTES.saturating_sub(text.len());
        if chunk.len() > remaining {
            // 按字符边界截断：字节上限不能切断多字节字符。
            let mut taken = String::new();
            for ch in chunk.chars() {
                if taken.len() + ch.len_utf8() > remaining {
                    break;
                }
                taken.push(ch);
            }
            text.push_str(&taken);
            truncated = true;
            break;
        }
        text.push_str(chunk);
    }
    if omitted_blocks > 0 {
        truncated = true;
    }
    Ok(McpResourceContent {
        text,
        omitted_blocks,
        truncated,
    })
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use serde_json::json;

    /// 清单解析：缺字段条目跳过、超限条目计入截断、描述截断。
    #[test]
    fn resources_list_is_bounded_and_skips_invalid_entries() {
        let long_desc = "描".repeat(MAX_RESOURCE_DESC_CHARS + 10);
        let result = json!({"resources": [
            {"uri": "file:///a.md", "name": "A", "description": long_desc, "mimeType": "text/markdown"},
            {"uri": "file:///b.md"},
            {"name": "no-uri"},
        ]});
        let list = parse_resources_list(&result).expect("解析");
        assert_eq!(list.resources.len(), 1, "缺 uri/name 的条目跳过");
        assert_eq!(
            list.resources[0].description.chars().count(),
            MAX_RESOURCE_DESC_CHARS
        );
        assert_eq!(
            list.resources[0].mime_type.as_deref(),
            Some("text/markdown")
        );
        assert!(!list.truncated);

        let mut overflow = Vec::new();
        for index in 0..MAX_RESOURCES_PER_SERVER + 2 {
            overflow.push(json!({"uri": format!("file:///{index}"), "name": format!("r{index}")}));
        }
        let list = parse_resources_list(&json!({"resources": overflow})).expect("解析");
        assert_eq!(list.resources.len(), MAX_RESOURCES_PER_SERVER);
        assert!(list.truncated, "超限必须标记截断");

        let bad_uri =
            json!({"resources": [{"uri": "x".repeat(MAX_RESOURCE_URI_CHARS + 1), "name": "n"}]});
        let list = parse_resources_list(&bad_uri).expect("解析");
        assert!(list.resources.is_empty() && list.truncated);
    }

    /// 读取解析：多 text 块拼接、blob 只计数、字节上限按字符边界截断。
    #[test]
    fn resource_read_concatenates_text_and_counts_blobs() {
        let result = json!({"contents": [
            {"uri": "file:///a", "mimeType": "text/plain", "text": "第一段"},
            {"uri": "file:///a", "text": "第二段"},
            {"uri": "file:///a", "blob": "AAAA"},
        ]});
        let content = parse_resource_read(&result).expect("解析");
        assert_eq!(content.text, "第一段\n第二段");
        assert_eq!(content.omitted_blocks, 1);
        assert!(content.truncated, "非文本块未内联必须标记截断");

        let huge = "中".repeat(MAX_RESOURCE_BYTES / 3 + 100);
        let content =
            parse_resource_read(&json!({"contents": [{"uri": "u", "text": huge}]})).expect("解析");
        assert!(content.truncated);
        assert!(content.text.len() <= MAX_RESOURCE_BYTES);
        assert!(
            content.text.chars().all(|ch| ch == '中'),
            "不得切断多字节字符"
        );

        assert!(
            parse_resource_read(&json!({})).is_err(),
            "缺 contents 必须报错"
        );
    }

    /// initialize 结果带 instructions 与 resources 能力标记；空 instructions 视为缺失。
    #[test]
    fn initialize_result_carries_instructions_and_resources_flag() {
        let result = parse_initialize_result(&json!({
            "protocolVersion": SUPPORTED_VERSIONS[0],
            "capabilities": {"resources": {"subscribe": false}},
            "serverInfo": {"name": "s", "version": "1"},
            "instructions": "  使用前先读 README  ",
        }))
        .expect("解析");
        assert_eq!(result.instructions.as_deref(), Some("使用前先读 README"));
        assert!(result.resources);

        let blank = parse_initialize_result(&json!({
            "protocolVersion": SUPPORTED_VERSIONS[0],
            "capabilities": {},
            "serverInfo": {"name": "s", "version": "1"},
            "instructions": "   ",
        }))
        .expect("解析");
        assert!(blank.instructions.is_none(), "空白 instructions 视为缺失");
        assert!(!blank.resources);
    }
}
