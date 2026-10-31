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
