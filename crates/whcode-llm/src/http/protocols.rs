//! # 五协议适配器（FR-209/210、162）
//!
//! [`WireProtocol`] 是线协议契约：请求路径/体构造、附加头、流帧格式、单帧解码
//! （正文增量/终止/用量/错误）与协议错误包提取。新增协议只新增实现与夹具，
//! **不修改领域 `Provider` 接口**（FR-209）。终止原因与用量字段跨协议归一：
//! `finish_reason`/`stop_reason`/`finishReason`/`done_reason` → 归一 `reason`；
//! 各协议 usage 字段 → [`Usage`]（`cost_is_estimate = false`）。

use serde_json::{Value, json};

use whcode_domain::WhError;

use crate::http::frames::Frame;
use crate::http::profile::{FrameFormat, Protocol, ProviderProfile};
use crate::metering::Usage;
use crate::provider::LlmRequest;

/// 单帧解码结果（协议层 → chunk 语义；FR-210）。
#[derive(Debug, Clone, PartialEq)]
pub enum DecodeOutcome {
    /// 空帧（如 SSE `message_start`），无正文。
    Empty,
    /// 正文增量。
    Delta { text: String },
    /// 终止（含归一原因与供应商用量）。
    Finish { reason: String, usage: Option<Usage> },
    /// 增量 + 终止（Ollama 最后一帧同帧携带正文与 done）。
    DeltaAndFinish { text: String, reason: String, usage: Option<Usage> },
    /// 未知事件：跳过但**必须计数**并留完整性标记（FR-210；经传输快照暴露）。
    Unknown,
    /// 协议错误事件（SSE `error` 事件等）：转为结构化错误，不得静默。
    Error { message: String },
}

/// 线协议契约（FR-209；实现为无状态对象，注册表按协议枚举分派）。
pub trait WireProtocol: Send + Sync {
    /// 协议名（与 [`Protocol::as_str`] 一致）。
    fn name(&self) -> &'static str;

    /// 流帧格式（FR-210）。
    fn frame_format(&self) -> FrameFormat;

    /// 完整请求路径（含查询；不含 base_url）。
    fn request_path(&self, profile: &ProviderProfile) -> String;

    /// 请求体 JSON（剖面 extra_body 与协议默认体合并，同键以剖面为准）。
    ///
    /// # Errors
    ///
    /// extra_body 非对象时返回 [`whcode_domain::ErrorCode::InvalidState`]。
    fn request_body(&self, request: &LlmRequest, profile: &ProviderProfile) -> Result<String, WhError>;

    /// 协议必需头（如 Anthropic 的 `anthropic-version`）。
    fn extra_headers(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// 单帧解码（FR-210：正文增量、终止、用量、未知事件、协议错误）。
    fn decode(&self, frame: &Frame) -> DecodeOutcome;

    /// 非 2xx 响应正文的错误消息提取（FR-212：协议错误包字段归一）。
    fn error_message(&self, body: &str) -> String;
}

/// 按协议枚举取实现（注册表；FR-209）。
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

/// 把剖面 extra_body（对象）合并进默认体（同键以剖面为准，FR-209）。
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

fn messages_role_user(request: &LlmRequest) -> Value {
    json!([{ "role": "user", "content": request.prompt }])
}

fn extract_usage(value: &Value, prompt_keys: [&str; 1], completion_keys: [&str; 1]) -> Option<Usage> {
    let prompt = prompt_keys.iter().find_map(|k| value.get(*k)).cloned()?;
    let completion = completion_keys.iter().find_map(|k| value.get(*k)).cloned()?;
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

    fn request_body(&self, request: &LlmRequest, profile: &ProviderProfile) -> Result<String, WhError> {
        let mut body = json!({
            "model": profile.model,
            "messages": messages_role_user(request),
            "stream": true,
            "stream_options": { "include_usage": true },
            "max_tokens": profile.max_tokens,
        });
        body = merge_extra(body, &profile.extra_body)?;
        body_string(body)
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        if frame.data == "[DONE]" {
            return DecodeOutcome::Finish { reason: "stop".to_string(), usage: None };
        }
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("openai 协议错误（未提供 message）");
            return DecodeOutcome::Error { message: message.to_string() };
        }
        let Some(choice) = value.get("choices").and_then(Value::as_array).and_then(|c| c.first()) else {
            // usage-only 终帧（include_usage 的最后一块没有 choices）。
            if let Some(u) = value.get("usage") {
                return match extract_usage(u, ["prompt_tokens"], ["completion_tokens"]) {
                    Some(usage) => DecodeOutcome::Finish { reason: "stop".to_string(), usage: Some(usage) },
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
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            let usage = value.get("usage").and_then(|u| extract_usage(u, ["prompt_tokens"], ["completion_tokens"]));
            return DecodeOutcome::DeltaAndFinish { text, reason: reason.to_string(), usage };
        }
        if text.is_empty() {
            return DecodeOutcome::Empty;
        }
        DecodeOutcome::Delta { text }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.get("message")).cloned())
            .and_then(|m| m.as_str().map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

/// Anthropic Messages API（SSE 类型化事件；162.3）。
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

    fn request_body(&self, request: &LlmRequest, profile: &ProviderProfile) -> Result<String, WhError> {
        let body = json!({
            "model": profile.model,
            "max_tokens": profile.max_tokens,
            "messages": messages_role_user(request),
            "stream": true,
        });
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
            value.get("type").and_then(Value::as_str).unwrap_or_default()
        });
        match event {
            "content_block_delta" => {
                let text = value
                    .pointer("/delta/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if text.is_empty() {
                    DecodeOutcome::Empty
                } else {
                    DecodeOutcome::Delta { text }
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
            "message_stop" => DecodeOutcome::Finish { reason: "stop".to_string(), usage: None },
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

/// Google Gemini（`alt=sse`；162.4）。
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
            format!("{base}/v1beta/models/{}:streamGenerateContent", profile.model)
        };
        if !path.contains("alt=sse") {
            path.push(if path.contains('?') { "&" } else { "?" });
            path.push_str("alt=sse");
        }
        let _ = &mut path;
        path
    }

    fn request_body(&self, request: &LlmRequest, profile: &ProviderProfile) -> Result<String, WhError> {
        let body = json!({
            "contents": [{ "parts": [{ "text": request.prompt }] }],
            "generationConfig": { "maxOutputTokens": profile.max_tokens },
        });
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
        let text = candidate
            .and_then(|c| c.get("content"))
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<String>()
            })
            .unwrap_or_default();
        let reason = candidate
            .and_then(|c| c.get("finishReason"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let usage = value
            .get("usageMetadata")
            .and_then(|u| extract_usage(u, ["promptTokenCount"], ["candidatesTokenCount"]));
        match (text.is_empty(), reason) {
            (false, Some(reason)) => DecodeOutcome::DeltaAndFinish { text, reason, usage },
            (false, None) => DecodeOutcome::Delta { text },
            (true, Some(reason)) => DecodeOutcome::Finish { reason, usage },
            (true, None) => {
                if usage.is_some() {
                    DecodeOutcome::Finish { reason: "stop".to_string(), usage }
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

/// Ollama 原生 `/api/chat`（NDJSON；162.5）。
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

    fn request_body(&self, request: &LlmRequest, profile: &ProviderProfile) -> Result<String, WhError> {
        let body = json!({
            "model": profile.model,
            "messages": messages_role_user(request),
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
                message: error
                    .as_str()
                    .unwrap_or("ollama 协议错误")
                    .to_string(),
            };
        }
        let text = value
            .pointer("/message/content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let done = value.get("done").and_then(Value::as_bool).unwrap_or(false);
        if !done {
            if text.is_empty() {
                return DecodeOutcome::Empty;
            }
            return DecodeOutcome::Delta { text };
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
        DecodeOutcome::DeltaAndFinish { text, reason, usage }
    }

    fn error_message(&self, body: &str) -> String {
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| body.chars().take(200).collect())
    }
}

/// Cohere Chat v2（SSE 类型化事件；162.6）。
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

    fn request_body(&self, request: &LlmRequest, profile: &ProviderProfile) -> Result<String, WhError> {
        let body = json!({
            "model": profile.model,
            "messages": messages_role_user(request),
            "stream": true,
        });
        body_string(merge_extra(body, &profile.extra_body)?)
    }

    fn decode(&self, frame: &Frame) -> DecodeOutcome {
        let Ok(value) = frame.json() else {
            return DecodeOutcome::Unknown;
        };
        let event = frame.event_name.as_deref().unwrap_or_else(|| {
            value.get("type").and_then(Value::as_str).unwrap_or_default()
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
                    DecodeOutcome::Delta { text }
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
            prompt: "hi".to_string(),
            max_tokens: 64,
            timeout: None,
        }
    }

    fn frame(data: &str) -> Frame {
        Frame { event_name: None, data: data.to_string() }
    }

    #[test]
    fn openai_request_body_streams_and_includes_usage() {
        let body = OpenAi.request_body(&request(), &profile(Protocol::OpenAi)).expect("body");
        let value: Value = serde_json::from_str(&body).expect("JSON");
        assert_eq!(value["stream"], true);
        assert_eq!(value["stream_options"]["include_usage"], true);
        assert_eq!(value["max_tokens"], 64);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(OpenAi.request_path(&profile(Protocol::OpenAi)), "/v1/chat/completions");
    }

    #[test]
    fn openai_decode_maps_delta_finish_and_done() {
        let delta = OpenAi.decode(&frame(
            r#"{"choices":[{"delta":{"content":"你好"}}]}"#,
        ));
        assert_eq!(delta, DecodeOutcome::Delta { text: "你好".to_string() });

        let finish = OpenAi.decode(&frame(
            r#"{"choices":[{"delta":{},"finish_reason":"length"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        ));
        match finish {
            DecodeOutcome::DeltaAndFinish { text, reason, usage } => {
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
            DecodeOutcome::Error { message: "配额超限".to_string() },
            "协议错误不得静默（FR-210）"
        );
        assert_eq!(OpenAi.error_message(r#"{"error":{"message":"bad key"}}"#), "bad key");
    }

    #[test]
    fn anthropic_maps_typed_sse_events() {
        let delta = Anthropic.decode(&Frame {
            event_name: Some("content_block_delta".to_string()),
            data: r#"{"delta":{"text":"段落"}}"#.to_string(),
        });
        assert_eq!(delta, DecodeOutcome::Delta { text: "段落".to_string() });

        let finish = Anthropic.decode(&Frame {
            event_name: Some("message_delta".to_string()),
            data: r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}"#.to_string(),
        });
        assert!(matches!(finish, DecodeOutcome::Finish { .. }));
        let mut base = profile(Protocol::Anthropic);
        base.path = String::new();
        assert_eq!(Anthropic.request_path(&base), "/v1/messages", "默认 path 补全");
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
        assert_eq!(delta, DecodeOutcome::Delta { text: "G".to_string() });

        let finish = Gemini.decode(&frame(
            r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":4}}"#,
        ));
        assert!(matches!(finish, DecodeOutcome::Finish { .. }));
        let path = Gemini.request_path(&profile(Protocol::Gemini));
        assert!(path.contains(":streamGenerateContent") && path.contains("alt=sse"), "路径：{path}");
    }

    #[test]
    fn ollama_ndjson_final_frame_carries_delta_and_usage() {
        let delta = Ollama.decode(&frame(r#"{"message":{"content":"O"},"done":false}"#));
        assert_eq!(delta, DecodeOutcome::Delta { text: "O".to_string() });

        let finish = Ollama.decode(&frame(
            r#"{"message":{"content":""},"done":true,"done_reason":"stop","prompt_eval_count":9,"eval_count":11}"#,
        ));
        match finish {
            DecodeOutcome::DeltaAndFinish { text, reason, usage } => {
                assert_eq!(reason, "stop");
                let usage = usage.expect("用量");
                assert_eq!((usage.prompt_tokens, usage.completion_tokens), (9, 11));
                let _ = text;
            }
            other => panic!("应为 DeltaAndFinish：{other:?}"),
        }
        assert!(Ollama.error_message(r#"{"error":"model not found"}"#).contains("model not found"));
    }

    #[test]
    fn cohere_maps_typed_deltas() {
        let delta = Cohere.decode(&Frame {
            event_name: Some("content-delta".to_string()),
            data: r#"{"delta":{"message":{"content":{"text":"C"}}}}"#.to_string(),
        });
        assert_eq!(delta, DecodeOutcome::Delta { text: "C".to_string() });

        let finish = Cohere.decode(&Frame {
            event_name: Some("message-end".to_string()),
            data: r#"{"delta":{"finish_reason":"COMPLETE"},"usage":{"tokens":{"input_tokens":2,"output_tokens":3}}}"#.to_string(),
        });
        assert!(matches!(finish, DecodeOutcome::Finish { .. }));
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
        let err = OpenAi.request_body(&request(), &profile).expect_err("必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    }

    #[test]
    fn unknown_frames_map_to_unknown_not_panic() {
        assert_eq!(OpenAi.decode(&frame("not json")), DecodeOutcome::Unknown);
        assert_eq!(Ollama.decode(&frame("[")), DecodeOutcome::Unknown);
    }
}
