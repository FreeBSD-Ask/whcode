//! # 五协议适配器（FR-206／FR-207、ADR-031）
//!
//! [`WireProtocol`] 是线协议契约：请求路径/体构造、附加头、流帧格式、单帧解码
//! （正文增量/终止/用量/错误）与协议错误包提取。新增协议只新增实现与夹具，
//! **不修改领域 `Provider` 接口**（ADR-031）。终止原因与用量字段跨协议归一：
//! `finish_reason`/`stop_reason`/`finishReason`/`done_reason` → 归一 `reason`；
//! 各协议 usage 字段 → [`Usage`]（`cost_is_estimate = false`）。

use serde_json::{Value, json};

use whcode_domain::WhError;

use crate::http::frames::Frame;
use crate::http::profile::{FrameFormat, Protocol, ProviderProfile};
use crate::metering::Usage;
use crate::provider::{LlmRequest, Role};

/// 单帧解码结果（协议层 → chunk 语义；FR-206／FR-208）。
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeOutcome {
    /// 空帧（如 SSE `message_start`），无正文。
    Empty,
    /// 正文增量（可携带流式工具调用片段；ADR-088）。
    Delta {
        text: String,
        /// 本帧的工具调用片段（按供应商线格式归一；无则空）。
        tool_fragments: Vec<ToolCallFragment>,
    },
    /// 终止（含归一原因与供应商用量）。
    Finish {
        reason: String,
        usage: Option<Usage>,
    },
    /// 增量 + 终止（Ollama 最后一帧同帧携带正文与 done）。
    DeltaAndFinish {
        text: String,
        reason: String,
        usage: Option<Usage>,
        /// 本帧的工具调用片段（ADR-088）。
        tool_fragments: Vec<ToolCallFragment>,
    },
    /// 未知事件：跳过但**必须计数**并留完整性标记（FR-206／FR-208；经传输快照暴露）。
    Unknown,
    /// 协议错误事件（SSE `error` 事件等）：转为结构化错误，不得静默。
    Error { message: String },
}

/// 流式工具调用增量片段（ADR-088／FR-797）：OpenAI `delta.tool_calls` 按 index
/// 分片送达——id／name 通常随首片（后续片段缺省），arguments 跨片段拼接；
/// 累积归一在 [`crate::http::chunks::ChunkNormalizer`] 完成，终结 chunk 整体交付。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallFragment {
    /// 片段序号（OpenAI `index`；同一次响应内从 0 递增）。
    pub index: u64,
    /// 调用 ID（通常随首片；后续片段可能缺省）。
    pub id: Option<String>,
    /// 工具名（通常随首片）。
    pub name: Option<String>,
    /// 参数 JSON 增量（跨片段拼接）。
    pub arguments_delta: String,
}

/// 线协议契约（ADR-031；实现为无状态对象，注册表按协议枚举分派）。
pub trait WireProtocol: Send + Sync {
    /// 协议名（与 [`Protocol::as_str`] 一致）。
    fn name(&self) -> &'static str;

    /// 流帧格式（FR-206／FR-208）。
    fn frame_format(&self) -> FrameFormat;

    /// 完整请求路径（含查询；不含 base_url）。
    fn request_path(&self, profile: &ProviderProfile) -> String;

    /// 请求体 JSON（剖面 extra_body 与协议默认体合并，同键以剖面为准）。
    ///
    /// # Errors
    ///
    /// extra_body 非对象时返回 [`whcode_domain::ErrorCode::InvalidState`]。
    fn request_body(
        &self,
        request: &LlmRequest,
        profile: &ProviderProfile,
    ) -> Result<String, WhError>;

    /// 协议必需头（如 Anthropic 的 `anthropic-version`）。
    fn extra_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// 单帧解码（FR-206／FR-208：正文增量、终止、用量、未知事件、协议错误）。
    fn decode(&self, frame: &Frame) -> DecodeOutcome;

    /// 非 2xx 响应正文的错误消息提取（FR-201／FR-616：协议错误包字段归一）。
    fn error_message(&self, body: &str) -> String;
}

/// 按协议枚举取实现（注册表；ADR-031）。
#[must_use]
pub fn registry(protocol: Protocol) -> &'static dyn WireProtocol {
    match protocol {
        Protocol::OpenAi => &OpenAi,
        Protocol::Anthropic => &Anthropic,
        Protocol::Gemini => &Gemini,
        Protocol::Ollama => &Ollama,
        Protocol::Cohere => &Cohere,
    }
}

/// 供应商用量（`cost_is_estimate = false`：供应商返回值，FR-207）。
fn usage(prompt_tokens: Value, completion_tokens: Value) -> Option<Usage> {
    let prompt = prompt_tokens.as_u64()?;
    let completion = completion_tokens.as_u64()?;
    Some(Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        latency_ms: 0,
        cost: None,
        cost_is_estimate: false,
    })
}

/// 把剖面 extra_body（对象）合并进默认体（同键以剖面为准，ADR-031）。
fn merge_extra(mut base: Value, extra: &Value) -> Result<Value, WhError> {
    if extra.is_null() {
        return Ok(base);
    }
    let Some(extra_obj) = extra.as_object() else {
        return Err(whcode_domain::WhError::invalid_state(
            "剖面的 extra_body 必须是 JSON 对象（与协议请求体合并）",
        ));
    };
    let obj = base.as_object_mut().ok_or_else(|| {
        whcode_domain::WhError::invalid_state("协议请求体不是对象，无法合并 extra_body")
    })?;
    for (key, value) in extra_obj {
        obj.insert(key.clone(), value.clone());
    }
    let _ = &mut base;
    Ok(base)
}

fn body_string(value: Value) -> Result<String, WhError> {
    serde_json::to_string(&value)
        .map_err(|err| whcode_domain::WhError::invalid_state(format!("请求体序列化失败：{err}")))
}

/// OpenAI 兼容扁平消息数组（FR-720：openai/ollama/cohere 直映，role 稳定名）。
fn messages_flat(request: &LlmRequest) -> Value {
    Value::Array(
        request
            .messages
            .iter()
            .map(|m| match m.role {
                Role::Assistant if !m.tool_calls.is_empty() => json!({
                    "role": "assistant",
                    "content": m.content,
                    "tool_calls": m.tool_calls.iter().map(|call| json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments },
                    })).collect::<Vec<_>>(),
                }),
                Role::Tool => json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.as_deref().unwrap_or_default(),
                    "content": m.content,
                }),
                _ => json!({ "role": m.role.as_str(), "content": m.content }),
            })
            .collect(),
    )
}

/// Anthropic 消息映射（FR-720；ADR-088 结构化升级）：system 抽为顶层 `system`
/// 段（换行连接）；user/assistant 交替进 `messages`；相邻同角色合并（Anthropic
/// 要求严格交替）；assistant 的模型工具调用序列化为 `tool_use` 块，tool 结果
/// 序列化为 user 消息的 `tool_result` 块（按 `tool_use_id` 关联）。
fn anthropic_messages(request: &LlmRequest) -> (String, Value) {
    let system = request
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let mut messages: Vec<Value> = Vec::new();
    for m in request.messages.iter().filter(|m| m.role != Role::System) {
        let role = if m.role == Role::Assistant {
            "assistant"
        } else {
            "user"
        };
        match m.role {
            Role::Tool => {
                // tool_result 块必须进 user 消息；连续多条工具结果合并进同一 user。
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": m.tool_call_id.as_deref().unwrap_or_default(),
                    "content": m.content,
                });
                match messages.last_mut() {
                    Some(last) if last["role"] == json!("user") && last["content"].is_array() => {
                        last["content"]
                            .as_array_mut()
                            .expect("数组内容")
                            .push(block);
                    }
                    _ => messages.push(json!({ "role": "user", "content": [block] })),
                }
            }
            Role::Assistant if !m.tool_calls.is_empty() => {
                let mut content = Vec::new();
                if !m.content.is_empty() {
                    content.push(json!({ "type": "text", "text": m.content }));
                }
                for call in &m.tool_calls {
                    let input = serde_json::from_str::<Value>(&call.arguments)
                        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
                    content.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": input,
                    }));
                }
                messages.push(json!({ "role": "assistant", "content": content }));
            }
            _ => {
                // 纯文本消息保持字符串承载（既有测试与可读性口径不变）。
                messages.push(json!({ "role": role, "content": m.content }));
            }
        }
    }
    (system, Value::Array(messages))
}

/// Gemini 内容映射（FR-720；ADR-088 结构化升级）：system 抽为
/// `systemInstruction`；user→`user`，assistant/tool→`model`（Gemini 角色词）；
/// assistant 的模型工具调用序列化为 `functionCall` parts，tool 结果序列化为
/// `functionResponse` parts（Gemini 无调用 ID，`tool_call_id` 承载函数名）；
/// 相邻同角色合并。
fn gemini_contents(request: &LlmRequest) -> (Option<Value>, Value) {
    let system = request
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| json!({ "text": m.content }))
        .collect::<Vec<_>>();
    let system_instruction = (!system.is_empty()).then(|| json!({ "parts": system }));
    let mut contents: Vec<Value> = Vec::new();
    for m in request.messages.iter().filter(|m| m.role != Role::System) {
        let role = if m.role == Role::User {
            "user"
        } else {
            "model"
        };
        let mut parts: Vec<Value> = Vec::new();
        match m.role {
            Role::Tool => {
                parts.push(json!({
                    "functionResponse": {
                        "name": m.tool_call_id.as_deref().unwrap_or_default(),
                        "response": { "result": m.content },
                    }
                }));
            }
            Role::Assistant if !m.tool_calls.is_empty() => {
                if !m.content.is_empty() {
                    parts.push(json!({ "text": m.content }));
                }
                for call in &m.tool_calls {
                    let args = serde_json::from_str::<Value>(&call.arguments)
                        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
                    parts.push(json!({ "functionCall": { "name": call.name, "args": args } }));
                }
            }
            _ => parts.push(json!({ "text": m.content })),
        }
        match contents.last_mut() {
            Some(last) if last["role"] == json!(role) => {
                last["parts"]
                    .as_array_mut()
                    .expect("parts 数组")
                    .extend(parts);
            }
            _ => contents.push(json!({ "role": role, "parts": parts })),
        }
    }
    (system_instruction, Value::Array(contents))
}

fn extract_usage(
    value: &Value,
    prompt_keys: [&str; 1],
    completion_keys: [&str; 1],
) -> Option<Usage> {
    let prompt = prompt_keys.iter().find_map(|k| value.get(*k)).cloned()?;
    let completion = completion_keys
        .iter()
        .find_map(|k| value.get(*k))
        .cloned()?;
    usage(prompt, completion)
}

/// OpenAI Chat Completions 及兼容端点（SenseNova/DeepSeek/Qwen/Moonshot/…、Azure）。
pub struct OpenAi;

impl WireProtocol for OpenAi {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn frame_format(&self) -> FrameFormat {
        FrameFormat::Sse
    }

    fn request_path(&self, profile: &ProviderProfile) -> String {
        let base = profile.path.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    fn request_body(
        &self,
        request: &LlmRequest,
        profile: &ProviderProfile,
    ) -> Result<String, WhError> {
        let mut body = json!({
            "model": profile.model,
            "messages": messages_flat(request),
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": profile.max_tokens,
        });
        if let Some(tools) = &request.tools {
            body["tools"] = tools.clone();
        }
        body = merge_extra(body, &profile.extra_body)?;
        body_string(body)
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        if frame.data == "[DONE]" {
            return DecodeOutcome::Finish {
                reason: "stop".to_string(),
                usage: None,
            };
        }
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("openai 协议错误（未提供 message）");
            return DecodeOutcome::Error {
                message: message.to_string(),
            };
        }
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
        else {
            // usage-only 终帧（include_usage 的最后一块没有 choices）。
            if let Some(u) = value.get("usage") {
                return match extract_usage(u, ["prompt_tokens"], ["completion_tokens"]) {
                    Some(usage) => DecodeOutcome::Finish {
                        reason: "stop".to_string(),
                        usage: Some(usage),
                    },
                    None => DecodeOutcome::Empty,
                };
            }
            return DecodeOutcome::Empty;
        };
        let text = choice
            .get("delta")
            .and_then(|d| d.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // ADR-088：`delta.tool_calls` 片段提取——id／name 随首片，arguments 跨帧拼接；
        // 累积归一交给 ChunkNormalizer，终结 chunk 整体交付给 turn 循环。
        let tool_fragments = choice
            .get("delta")
            .and_then(|d| d.get("tool_calls"))
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .map(|call| {
                        let function = call.get("function");
                        ToolCallFragment {
                            index: call.get("index").and_then(Value::as_u64).unwrap_or(0),
                            id: call
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                                .map(str::to_string),
                            name: function
                                .and_then(|f| f.get("name"))
                                .and_then(Value::as_str)
                                .filter(|value| !value.is_empty())
                                .map(str::to_string),
                            arguments_delta: function
                                .and_then(|f| f.get("arguments"))
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            let usage = value
                .get("usage")
                .and_then(|u| extract_usage(u, ["prompt_tokens"], ["completion_tokens"]));
            return DecodeOutcome::DeltaAndFinish {
                text,
                reason: reason.to_string(),
                usage,
                tool_fragments,
            };
        }
        if text.is_empty() && tool_fragments.is_empty() {
            return DecodeOutcome::Empty;
        }
        DecodeOutcome::Delta {
            text,
            tool_fragments,
        }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.get("message")).cloned())
            .and_then(|m| m.as_str().map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

/// Anthropic Messages API（SSE 类型化事件；）。
pub struct Anthropic;

impl WireProtocol for Anthropic {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn frame_format(&self) -> FrameFormat {
        FrameFormat::Sse
    }

    fn request_path(&self, profile: &ProviderProfile) -> String {
        let base = profile.path.trim_end_matches('/');
        if base.ends_with("/messages") {
            base.to_string()
        } else {
            format!("{base}/v1/messages")
        }
    }

    fn request_body(
        &self,
        request: &LlmRequest,
        profile: &ProviderProfile,
    ) -> Result<String, WhError> {
        let (system, messages) = anthropic_messages(request);
        let mut body = json!({
            "model": profile.model,
            "max_tokens": profile.max_tokens,
            "messages": messages,
            "stream": true,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        body_string(merge_extra(body, &profile.extra_body)?)
    }

    fn extra_headers(&self) -> Vec<(String, String)> {
        vec![("anthropic-version".to_string(), "2023-06-01".to_string())]
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        let event = frame.event_name.as_deref().unwrap_or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        });
        match event {
            // ADR-088：tool_use 块开始——id／name 随首事件到达（index 为块序号）。
            "content_block_start" => {
                let block_type = value
                    .pointer("/content_block/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if block_type != "tool_use" {
                    return DecodeOutcome::Empty;
                }
                let index = value.get("index").and_then(Value::as_u64).unwrap_or(0);
                let id = value
                    .pointer("/content_block/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = value
                    .pointer("/content_block/name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                DecodeOutcome::Delta {
                    text: String::new(),
                    tool_fragments: vec![ToolCallFragment {
                        index,
                        id: (!id.is_empty()).then_some(id),
                        name: (!name.is_empty()).then_some(name),
                        arguments_delta: String::new(),
                    }],
                }
            }
            "content_block_delta" => {
                let text = value
                    .pointer("/delta/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // ADR-088：input_json_delta——tool_use 参数跨事件拼接。
                let mut tool_fragments = Vec::new();
                if value.pointer("/delta/type").and_then(Value::as_str) == Some("input_json_delta")
                {
                    let partial = value
                        .pointer("/delta/partial_json")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if !partial.is_empty() {
                        tool_fragments.push(ToolCallFragment {
                            index: value.get("index").and_then(Value::as_u64).unwrap_or(0),
                            id: None,
                            name: None,
                            arguments_delta: partial,
                        });
                    }
                }
                if text.is_empty() && tool_fragments.is_empty() {
                    DecodeOutcome::Empty
                } else {
                    DecodeOutcome::Delta {
                        tool_fragments,
                        text,
                    }
                }
            }
            "message_delta" => {
                let reason = value
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("stop")
                    .to_string();
                let usage = value
                    .get("usage")
                    .and_then(|u| extract_usage(u, ["input_tokens"], ["output_tokens"]));
                DecodeOutcome::Finish { reason, usage }
            }
            "message_stop" => DecodeOutcome::Finish {
                reason: "stop".to_string(),
                usage: None,
            },
            "error" => DecodeOutcome::Error {
                message: value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("anthropic 协议错误")
                    .to_string(),
            },
            _ => DecodeOutcome::Empty,
        }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.pointer("/error/message").cloned())
            .and_then(|m| m.as_str().map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

/// Google Gemini（`alt=sse`；）。
pub struct Gemini;

impl WireProtocol for Gemini {
    fn name(&self) -> &'static str {
        "gemini"
    }

    fn frame_format(&self) -> FrameFormat {
        FrameFormat::Sse
    }

    fn request_path(&self, profile: &ProviderProfile) -> String {
        let base = profile.path.trim_end_matches('/');
        let mut path = if base.contains(":streamGenerateContent") {
            base.to_string()
        } else {
            format!(
                "{base}/v1beta/models/{}:streamGenerateContent",
                profile.model
            )
        };
        if !path.contains("alt=sse") {
            path.push(if path.contains('?') { '&' } else { '?' });
            path.push_str("alt=sse");
        }
        let _ = &mut path;
        path
    }

    fn request_body(
        &self,
        request: &LlmRequest,
        profile: &ProviderProfile,
    ) -> Result<String, WhError> {
        let (system_instruction, contents) = gemini_contents(request);
        let mut body = json!({
            "contents": contents,
            "generationConfig": { "maxOutputTokens": profile.max_tokens },
        });
        if let Some(instruction) = system_instruction {
            body["systemInstruction"] = instruction;
        }
        body_string(merge_extra(body, &profile.extra_body)?)
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        if let Some(error) = value.get("error") {
            return DecodeOutcome::Error {
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("gemini 协议错误")
                    .to_string(),
            };
        }
        let candidate = value
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|c| c.first());
        let parts = candidate
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let text = parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<String>();
        // ADR-088：functionCall part 整体到达（无分片）；Gemini 无调用 ID——
        // 以函数名合成（回传侧 `tool_call_id` 同样承载函数名，见 gemini_contents）。
        let tool_fragments = parts
            .iter()
            .enumerate()
            .filter_map(|(position, part)| {
                let call = part.get("functionCall")?;
                let name = call.get("name").and_then(Value::as_str)?;
                let args = call
                    .get("args")
                    .cloned()
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                Some(ToolCallFragment {
                    index: u64::try_from(position).unwrap_or(0),
                    id: Some(name.to_string()),
                    name: Some(name.to_string()),
                    arguments_delta: args.to_string(),
                })
            })
            .collect::<Vec<_>>();
        let reason = candidate
            .and_then(|c| c.get("finishReason"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let usage = value
            .get("usageMetadata")
            .and_then(|u| extract_usage(u, ["promptTokenCount"], ["candidatesTokenCount"]));
        match (text.is_empty(), reason) {
            (false, Some(reason)) => DecodeOutcome::DeltaAndFinish {
                tool_fragments,
                text,
                reason,
                usage,
            },
            (false, None) => DecodeOutcome::Delta {
                tool_fragments,
                text,
            },
            // 文本空但带 functionCall 片段（函数调用常无正文）。
            (true, Some(reason)) if !tool_fragments.is_empty() => DecodeOutcome::DeltaAndFinish {
                tool_fragments,
                text: String::new(),
                reason,
                usage,
            },
            (true, Some(reason)) => DecodeOutcome::Finish { reason, usage },
            (true, None) if !tool_fragments.is_empty() => DecodeOutcome::DeltaAndFinish {
                tool_fragments,
                text: String::new(),
                reason: "stop".to_string(),
                usage,
            },
            (true, None) => {
                if usage.is_some() {
                    DecodeOutcome::Finish {
                        reason: "stop".to_string(),
                        usage,
                    }
                } else {
                    DecodeOutcome::Empty
                }
            }
        }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.pointer("/error/message").cloned())
            .and_then(|m| m.as_str().map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

/// Ollama 原生 `/api/chat`（NDJSON；）。
pub struct Ollama;

impl WireProtocol for Ollama {
    fn name(&self) -> &'static str {
        "ollama"
    }

    fn frame_format(&self) -> FrameFormat {
        FrameFormat::Ndjson
    }

    fn request_path(&self, profile: &ProviderProfile) -> String {
        let base = profile.path.trim_end_matches('/');
        if base.ends_with("/api/chat") {
            base.to_string()
        } else {
            format!("{base}/api/chat")
        }
    }

    fn request_body(
        &self,
        request: &LlmRequest,
        profile: &ProviderProfile,
    ) -> Result<String, WhError> {
        let body = json!({
            "model": profile.model,
            "messages": messages_flat(request),
            "stream": true,
        });
        body_string(merge_extra(body, &profile.extra_body)?)
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        if let Some(error) = value.get("error") {
            return DecodeOutcome::Error {
                message: error.as_str().unwrap_or("ollama 协议错误").to_string(),
            };
        }
        let text = value
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // ADR-088：Ollama 工具调用随 message.tool_calls 整体到达（OpenAI 形状、
        // 无 id——以函数名合成，回传侧承载同名）。
        let tool_fragments = value
            .pointer("/message/tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .enumerate()
                    .filter_map(|(position, call)| {
                        let name = call
                            .pointer("/function/name")
                            .and_then(Value::as_str)?
                            .to_string();
                        let arguments = call
                            .pointer("/function/arguments")
                            .cloned()
                            .unwrap_or(Value::Object(serde_json::Map::new()));
                        let arguments_delta = match arguments {
                            Value::String(raw) => raw,
                            other => other.to_string(),
                        };
                        Some(ToolCallFragment {
                            index: u64::try_from(position).unwrap_or(0),
                            id: Some(name.clone()),
                            name: Some(name),
                            arguments_delta,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let done = value.get("done").and_then(Value::as_bool).unwrap_or(false);
        if !done {
            if text.is_empty() && tool_fragments.is_empty() {
                return DecodeOutcome::Empty;
            }
            return DecodeOutcome::Delta {
                tool_fragments,
                text,
            };
        }
        let reason = value
            .get("done_reason")
            .and_then(Value::as_str)
            .unwrap_or("stop")
            .to_string();
        let usage = value
            .get("prompt_eval_count")
            .zip(value.get("eval_count"))
            .and_then(|(p, c)| usage(p.clone(), c.clone()));
        DecodeOutcome::DeltaAndFinish {
            tool_fragments,
            text,
            reason,
            usage,
        }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

/// Cohere Chat v2（SSE 类型化事件；）。
pub struct Cohere;

impl WireProtocol for Cohere {
    fn name(&self) -> &'static str {
        "cohere"
    }

    fn frame_format(&self) -> FrameFormat {
        FrameFormat::Sse
    }

    fn request_path(&self, profile: &ProviderProfile) -> String {
        let base = profile.path.trim_end_matches('/');
        if base.ends_with("/chat") {
            base.to_string()
        } else {
            format!("{base}/v2/chat")
        }
    }

    fn request_body(
        &self,
        request: &LlmRequest,
        profile: &ProviderProfile,
    ) -> Result<String, WhError> {
        let body = json!({
            "model": profile.model,
            "messages": messages_flat(request),
            "stream": true,
        });
        body_string(merge_extra(body, &profile.extra_body)?)
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        let event = frame.event_name.as_deref().unwrap_or_else(|| {
            value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        });
        match event {
            "content-delta" => {
                let text = value
                    .pointer("/delta/message/content/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if text.is_empty() {
                    DecodeOutcome::Empty
                } else {
                    DecodeOutcome::Delta {
                        tool_fragments: Vec::new(),
                        text,
                    }
                }
            }
            "message-end" => {
                let reason = value
                    .pointer("/delta/finish_reason")
                    .and_then(Value::as_str)
                    .unwrap_or("stop")
                    .to_string();
                let usage = value.get("usage").and_then(|u| {
                    let tokens = u.get("tokens")?;
                    extract_usage(tokens, ["input_tokens"], ["output_tokens"])
                });
                DecodeOutcome::Finish { reason, usage }
            }
            _ => DecodeOutcome::Empty,
        }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(protocol: Protocol) -> ProviderProfile {
        ProviderProfile {
            name: "p".to_string(),
            protocol,
            base_url: "http://127.0.0.1:1".to_string(),
            path: "/v1".to_string(),
            query: Vec::new(),
            model: "m".to_string(),
            models: Vec::new(),
            auth: crate::http::profile::AuthScheme::None,
            extra_headers: Vec::new(),
            extra_body: Value::Null,
            connect_timeout_ms: 1_000,
            read_timeout_ms: 1_000,
            total_timeout_ms: 1_000,
            max_tokens: 64,
        }
    }

    fn request() -> LlmRequest {
        LlmRequest {
            request_id: "R".to_string(),
            session_id: "S".to_string(),
            task_id: None,
            model: "m".to_string(),
            messages: vec![crate::provider::LlmMessage::user("hi")],
            max_tokens: 64,
            timeout: None,
            tools: None,
        }
    }

    fn multi_message_request() -> LlmRequest {
        use crate::provider::{LlmMessage, Role};
        LlmRequest {
            request_id: "R".to_string(),
            session_id: "S".to_string(),
            task_id: None,
            model: "m".to_string(),
            messages: vec![
                LlmMessage {
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    role: Role::System,
                    content: "你是测试助手".to_string(),
                },
                LlmMessage::user("第一轮"),
                LlmMessage {
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                    role: Role::Assistant,
                    content: "第一轮回复".to_string(),
                },
                LlmMessage::user("第二轮"),
            ],
            max_tokens: 64,
            timeout: None,
            tools: None,
        }
    }

    fn frame(data: &str) -> Frame {
        Frame {
            event_name: None,
            data: data.to_string(),
        }
    }

    #[test]
    fn openai_request_body_streams_and_includes_usage() {
        let body = OpenAi
            .request_body(&request(), &profile(Protocol::OpenAi))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value["stream"], true);
        assert_eq!(value["stream_options"]["include_usage"], true);
        assert_eq!(value["max_tokens"], 64);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(
            OpenAi.request_path(&profile(Protocol::OpenAi)),
            "/v1/chat/completions"
        );
    }

    #[test]
    fn flat_protocols_map_multi_message_roles_in_order() {
        // FR-720：openai/ollama/cohere 直映四角色数组，顺序与角色稳定名一致。
        for (name, protocol) in [
            ("openai", Protocol::OpenAi),
            ("ollama", Protocol::Ollama),
            ("cohere", Protocol::Cohere),
        ] {
            let body = registry(protocol)
                .request_body(&multi_message_request(), &profile(protocol))
                .expect("body");
            let value: Value = serde_json::from_str(&body).expect("JSON");
            let roles: Vec<&str> = value["messages"]
                .as_array()
                .expect("messages 数组")
                .iter()
                .map(|m| m["role"].as_str().expect("role"))
                .collect();
            assert_eq!(
                roles,
                ["system", "user", "assistant", "user"],
                "{name} 角色顺序"
            );
            assert_eq!(value["messages"][0]["content"], "你是测试助手");
        }
    }

    #[test]
    fn anthropic_extracts_system_and_maps_turns() {
        // FR-720：system 抽顶层段，user/assistant 交替；tool 按 user 承载（P0-2 结构化）。
        let mut multi = multi_message_request();
        multi.messages.push(crate::provider::LlmMessage {
            tool_calls: Vec::new(),
            tool_call_id: None,
            role: crate::provider::Role::Tool,
            content: "工具结果".to_string(),
        });
        let body = Anthropic
            .request_body(&multi, &profile(Protocol::Anthropic))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value["system"], "你是测试助手", "system 抽为顶层段");
        let roles: Vec<&str> = value["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| m["role"].as_str().expect("role"))
            .collect();
        assert_eq!(roles, ["user", "assistant", "user", "user"], "交替轮次");
        assert!(
            !value["messages"][0]["content"].as_str().unwrap().is_empty(),
            "正文保留"
        );
    }

    /// ADR-088／FR-797：Anthropic tool_use 解码——块开始给 id／name，
    /// input_json_delta 跨事件拼接，累积后随终结 chunk 交付。
    #[test]
    fn anthropic_tool_use_fragments_accumulate_into_final_chunk() {
        let mut normalizer = crate::http::chunks::ChunkNormalizer::new();
        let _ = normalizer.push(Anthropic.decode(&Frame {
            event_name: Some("content_block_start".to_string()),
            data: r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file"}}"#
                .to_string(),
        }));
        let _ = normalizer.push(
            Anthropic.decode(&Frame {
                event_name: Some("content_block_delta".to_string()),
                data:
                    r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#
                        .to_string(),
            }),
        );
        let _ = normalizer.push(
            Anthropic.decode(&Frame {
                event_name: Some("content_block_delta".to_string()),
                data:
                    r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"\"a.txt\"}"}}"#
                        .to_string(),
            }),
        );
        let final_chunk = match normalizer.push(Anthropic.decode(&Frame {
            event_name: Some("message_delta".to_string()),
            data: r#"{"delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":9,"output_tokens":4}}"#
                .to_string(),
        })) {
            crate::http::chunks::PushOutcome::Chunk(chunk) => chunk,
            other => panic!("tool_use 终止必须产出终结 chunk：{other:?}"),
        };
        assert_eq!(final_chunk.finish_reason.as_deref(), Some("tool_use"));
        let calls = &final_chunk.tool_calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments, r#"{"path":"a.txt"}"#);
    }

    /// ADR-088／FR-797：Anthropic 请求侧——assistant 的 `tool_use` 块与 tool 结果
    /// 的 `tool_result` 块（按 tool_use_id 关联）；相邻 tool 结果合并进同一 user。
    #[test]
    fn anthropic_request_serializes_tool_use_and_result_blocks() {
        let mut multi = multi_message_request();
        multi
            .messages
            .push(crate::provider::LlmMessage::assistant_with_tool_calls(
                "",
                vec![crate::provider::LlmToolCall {
                    id: "toolu_9".to_string(),
                    name: "read_file".to_string(),
                    arguments: r#"{"path":"a.txt"}"#.to_string(),
                }],
            ));
        multi
            .messages
            .push(crate::provider::LlmMessage::tool_result(
                "toolu_9",
                "文件内容",
            ));
        let body = Anthropic
            .request_body(&multi, &profile(Protocol::Anthropic))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        let assistant = value["messages"]
            .as_array()
            .expect("数组")
            .iter()
            .find(|m| m["role"] == "assistant" && m["content"].is_array())
            .expect("assistant 工具消息");
        assert_eq!(assistant["content"][0]["type"], "tool_use");
        assert_eq!(assistant["content"][0]["id"], "toolu_9");
        assert_eq!(assistant["content"][0]["input"]["path"], "a.txt");
        let tool = value["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["content"].is_array() && m["content"][0]["type"] == "tool_result")
            .expect("tool_result 消息");
        assert_eq!(tool["content"][0]["tool_use_id"], "toolu_9");
        assert_eq!(tool["content"][0]["content"], "文件内容");
    }

    /// ADR-088／FR-797：Gemini functionCall 解码（无调用 ID——以函数名合成）
    /// 与请求侧 functionResponse（tool_call_id 承载函数名）。
    #[test]
    fn gemini_function_call_decode_and_function_response_request() {
        let frame = Gemini.decode(&frame(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_file","args":{"path":"a.txt"}}}]}}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":3}}"#,
        ));
        match frame {
            DecodeOutcome::DeltaAndFinish { tool_fragments, .. } => {
                assert_eq!(tool_fragments.len(), 1, "functionCall 必须产出片段");
                assert_eq!(tool_fragments[0].id.as_deref(), Some("read_file"));
                assert_eq!(tool_fragments[0].name.as_deref(), Some("read_file"));
            }
            other => panic!("functionCall 帧不得为空：{other:?}"),
        }
        let mut multi = multi_message_request();
        multi
            .messages
            .push(crate::provider::LlmMessage::tool_result(
                "read_file",
                "文件内容",
            ));
        let body = Gemini
            .request_body(&multi, &profile(Protocol::Gemini))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        let response = value["contents"]
            .as_array()
            .expect("contents")
            .iter()
            .find(|c| {
                c["parts"]
                    .as_array()
                    .is_some_and(|parts| parts[0].get("functionResponse").is_some())
            })
            .expect("functionResponse 内容");
        assert_eq!(response["role"], "model");
        assert_eq!(
            response["parts"][0]["functionResponse"]["name"],
            "read_file"
        );
        assert_eq!(
            response["parts"][0]["functionResponse"]["response"]["result"],
            "文件内容"
        );
    }

    #[test]
    fn gemini_extracts_system_instruction_and_maps_roles() {
        // FR-720：system→systemInstruction；user→user，assistant→model。
        let body = Gemini
            .request_body(&multi_message_request(), &profile(Protocol::Gemini))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(
            value["systemInstruction"]["parts"][0]["text"],
            "你是测试助手"
        );
        let roles: Vec<&str> = value["contents"]
            .as_array()
            .expect("contents")
            .iter()
            .map(|c| c["role"].as_str().expect("role"))
            .collect();
        assert_eq!(roles, ["user", "model", "user"], "Gemini 角色词映射");
        assert_eq!(value["contents"][0]["parts"][0]["text"], "第一轮");
    }

    #[test]
    fn single_user_message_is_equivalent_across_protocols() {
        // FR-720：单条 user 消息与既有单发行为等价（各协议首条均为 user/hi）。
        for protocol in [
            Protocol::OpenAi,
            Protocol::Anthropic,
            Protocol::Gemini,
            Protocol::Ollama,
            Protocol::Cohere,
        ] {
            let body = registry(protocol)
                .request_body(&request(), &profile(protocol))
                .expect("body");
            let value: Value = serde_json::from_str(&body).expect("JSON");
            let first_content = match protocol {
                Protocol::Gemini => value["contents"][0]["parts"][0]["text"].clone(),
                _ => value["messages"][0]["content"].clone(),
            };
            assert_eq!(first_content, "hi", "{protocol:?} 单消息等价");
        }
    }

    #[test]
    fn openai_decode_maps_delta_finish_and_done() {
        let delta = OpenAi.decode(&frame(r#"{"choices":[{"delta":{"content":"你好"}}]}"#));
        assert_eq!(
            delta,
            DecodeOutcome::Delta {
                tool_fragments: Vec::new(),
                text: "你好".to_string()
            }
        );

        let finish = OpenAi.decode(&frame(
            r#"{"choices":[{"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        ));
        match finish {
            DecodeOutcome::DeltaAndFinish {
                tool_fragments: _,
                text,
                reason,
                usage,
            } => {
                assert_eq!(reason, "length");
                let usage = usage.expect("供应商用量");
                assert!(!usage.cost_is_estimate);
                assert_eq!((usage.prompt_tokens, usage.completion_tokens), (10, 5));
                let _ = text;
            }
            other => panic!("应为 DeltaAndFinish：{other:?}"),
        }

        let done = OpenAi.decode(&frame("[DONE]"));
        assert!(matches!(done, DecodeOutcome::Finish { .. }));
    }

    #[test]
    fn openai_error_event_becomes_structured_error() {
        let outcome = OpenAi.decode(&frame(r#"{"error":{"message":"配额超限"}}"#));
        assert_eq!(
            outcome,
            DecodeOutcome::Error {
                message: "配额超限".to_string()
            },
            "协议错误不得静默（FR-210）"
        );
        assert_eq!(
            OpenAi.error_message(r#"{"error":{"message":"bad key"}}"#),
            "bad key"
        );
    }

    #[test]
    fn anthropic_maps_typed_sse_events() {
        let delta = Anthropic.decode(&Frame {
            event_name: Some("content_block_delta".to_string()),
            data: r#"{"delta":{"text":"段落"}}"#.to_string(),
        });
        assert_eq!(
            delta,
            DecodeOutcome::Delta {
                tool_fragments: Vec::new(),
                text: "段落".to_string()
            }
        );

        let finish = Anthropic.decode(&Frame {
            event_name: Some("message_delta".to_string()),
            data: r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#.to_string(),
        });
        assert!(matches!(finish, DecodeOutcome::Finish { .. }));
        let mut base = profile(Protocol::Anthropic);
        base.path = String::new();
        assert_eq!(
            Anthropic.request_path(&base),
            "/v1/messages",
            "默认 path 补全"
        );
        base.path = "/gateway/anthropic".to_string();
        assert_eq!(
            Anthropic.request_path(&base),
            "/gateway/anthropic/v1/messages",
            "网关变体经路径剖面覆盖（FR-209）"
        );
        assert_eq!(Anthropic.extra_headers()[0].0, "anthropic-version");
    }

    #[test]
    fn gemini_maps_parts_and_finish_reason() {
        let delta = Gemini.decode(&frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"G"}]}}]}"#,
        ));
        assert_eq!(
            delta,
            DecodeOutcome::Delta {
                tool_fragments: Vec::new(),
                text: "G".to_string()
            }
        );

        let finish = Gemini.decode(&frame(
            r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4}}"#,
        ));
        assert!(matches!(finish, DecodeOutcome::Finish { .. }));
        let path = Gemini.request_path(&profile(Protocol::Gemini));
        assert!(
            path.contains(":streamGenerateContent") && path.contains("alt=sse"),
            "路径：{path}"
        );
    }

    #[test]
    fn ollama_ndjson_final_frame_carries_delta_and_usage() {
        let delta = Ollama.decode(&frame(r#"{"message":{"content":"O"},"done":false}"#));
        assert_eq!(
            delta,
            DecodeOutcome::Delta {
                tool_fragments: Vec::new(),
                text: "O".to_string()
            }
        );

        let finish = Ollama.decode(&frame(
            r#"{"message":{"content":""},"done":true,"done_reason":"stop","prompt_eval_count":9,"eval_count":11}"#,
        ));
        match finish {
            DecodeOutcome::DeltaAndFinish {
                tool_fragments: _,
                text,
                reason,
                usage,
            } => {
                assert_eq!(reason, "stop");
                let usage = usage.expect("用量");
                assert_eq!((usage.prompt_tokens, usage.completion_tokens), (9, 11));
                let _ = text;
            }
            other => panic!("应为 DeltaAndFinish：{other:?}"),
        }
        assert!(
            Ollama
                .error_message(r#"{"error":"model not found"}"#)
                .contains("model not found")
        );
    }

    #[test]
    fn cohere_maps_typed_deltas() {
        let delta = Cohere.decode(&Frame {
            event_name: Some("content-delta".to_string()),
            data: r#"{"delta":{"message":{"content":{"text":"C"}}}}"#.to_string(),
        });
        assert_eq!(
            delta,
            DecodeOutcome::Delta {
                tool_fragments: Vec::new(),
                text: "C".to_string()
            }
        );

        let finish = Cohere.decode(&Frame {
            event_name: Some("message-end".to_string()),
            data: r#"{"delta":{"finish_reason":"COMPLETE"},"usage":{"tokens":{"input_tokens":2,"output_tokens":3}}}"#.to_string(),
        });
        assert!(matches!(finish, DecodeOutcome::Finish { .. }));
    }

    #[test]
    fn openai_request_body_includes_tools_when_present() {
        // FR-734：tools 数组随请求下发（openai 兼容族）。
        let mut req = request();
        req.tools = Some(serde_json::json!([
            { "type": "function", "function": { "name": "read_file", "parameters": {} } }
        ]));
        let body = OpenAi
            .request_body(&req, &profile(Protocol::OpenAi))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value["tools"][0]["type"], "function");
        assert_eq!(value["tools"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn no_tools_field_when_none() {
        // tools: None 时不产生 tools 键。
        let body = OpenAi
            .request_body(&request(), &profile(Protocol::OpenAi))
            .expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert!(
            value.get("tools").is_none(),
            "tools: None 时不得产生 tools 键"
        );
    }

    #[test]
    fn extra_body_overrides_default_keys() {
        let mut profile = profile(Protocol::OpenAi);
        profile.extra_body = serde_json::json!({ "temperature": 0.2, "max_tokens": 8 });
        let body = OpenAi.request_body(&request(), &profile).expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value["temperature"], 0.2);
        assert_eq!(value["max_tokens"], 8, "同键以剖面为准（FR-209）");
    }

    #[test]
    fn extra_body_must_be_object() {
        let mut profile = profile(Protocol::OpenAi);
        profile.extra_body = serde_json::json!("not-object");
        let err = OpenAi
            .request_body(&request(), &profile)
            .expect_err("必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    }

    #[test]
    fn unknown_frames_map_to_unknown_not_panic() {
        assert_eq!(OpenAi.decode(&frame("not json")), DecodeOutcome::Unknown);
        assert_eq!(Ollama.decode(&frame("[")), DecodeOutcome::Unknown);
    }
}
