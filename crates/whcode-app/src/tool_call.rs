//! 工具调用事件的结构化投影（Check 348／FR-743）。
//!
//! ## 为什么需要它
//!
//! GUI 与 Web 已有工具调用卡，CUI 与 TUI 只能看到 `seq + kind`。四端语义一致要求
//! CUI／TUI 也能呈现“哪个工具、哪次执行、什么状态、耗时多少、输出或错误是什么”。
//! 本模块把 `ToolCallStarted`／`ToolCallFinished` 的载荷收敛为**展示用**结构
//! （`ToolCallEvent`），前端只负责按语言组装文本。
//!
//! ## 边界
//!
//! - **只读**：不触库、不写事件、不发起任何 I/O；输入是事件载荷字符串。
//! - **有界**：工具名与 ID 各 64 字符、输出摘要 200 字符、错误码 64 字符；超限按字符边界截断。
//! - **不伪造**：缺字段保留 `None`（前端以 `?` 显式呈现），不推断工具名或状态；
//!   载荷损坏或非工具事件返回 `None`，调用方回落通用摘要。
//! - **脱敏口径不变**：载荷在写入时已按追踪级别处理；本模块不复制参数，因为事件载荷
//!   本身不含工具参数（参数不入事件 payload 的既有纪律）。

use serde_json::Value;

/// 工具名与 ID 的展示上限（字符）。
pub const MAX_TOOL_FIELD_CHARS: usize = 64;
/// 输出摘要的展示上限（字符）。
pub const MAX_TOOL_OUTPUT_CHARS: usize = 200;
/// 错误码的展示上限（字符）。
pub const MAX_TOOL_ERROR_CHARS: usize = 64;

/// `ToolCallStarted` 的稳定事件名。
pub const TOOL_STARTED: &str = "ToolCallStarted";
/// `ToolCallFinished` 的稳定事件名。
pub const TOOL_FINISHED: &str = "ToolCallFinished";

/// 工具调用事件的展示投影（字段为稳定数据值，本地化由前端完成）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallEvent {
    /// 工具稳定名（`ToolCallFinished` 通常不带工具名，保持 `None`）。
    pub tool: Option<String>,
    /// 模型侧工具调用 ID。
    pub call_id: String,
    /// 执行 ID（审批与审计关联用；历史事件可能缺失）。
    pub exec: Option<String>,
    /// 终态稳定码（`completed`／`failed`／`timed_out`／`interrupted`／`unknown`）。
    pub state: Option<String>,
    /// 耗时（毫秒）。
    pub duration_ms: Option<u64>,
    /// 输出摘要（已折叠空白并截断）。
    pub output: Option<String>,
    /// 错误码（稳定码，不插值凭据）。
    pub error: Option<String>,
    /// turn 内轮次。
    pub iteration: Option<u64>,
}

/// 把工具调用事件载荷投影为展示结构。
///
/// 非工具事件、载荷不是 JSON 对象、或缺 `tool_call_id` 时返回 `None`——调用方
/// 回落通用摘要，避免把损坏事件伪装成结构化卡片。
#[must_use]
pub fn describe(kind: &str, payload_json: &str) -> Option<ToolCallEvent> {
    if kind != TOOL_STARTED && kind != TOOL_FINISHED {
        return None;
    }
    let value: Value = serde_json::from_str(payload_json).ok()?;
    let object = value.as_object()?;
    let call_id = text_field(object.get("tool_call_id"), MAX_TOOL_FIELD_CHARS)?;
    Some(ToolCallEvent {
        tool: text_field(object.get("tool_name"), MAX_TOOL_FIELD_CHARS),
        call_id,
        exec: text_field(object.get("exec_id"), MAX_TOOL_FIELD_CHARS),
        state: text_field(object.get("state"), MAX_TOOL_FIELD_CHARS),
        duration_ms: object.get("duration_ms").and_then(Value::as_u64),
        output: text_field(object.get("output_value"), MAX_TOOL_OUTPUT_CHARS)
            .map(|text| crate::panels::summarize(&text)),
        error: text_field(object.get("error_code"), MAX_TOOL_ERROR_CHARS),
        iteration: object.get("iteration").and_then(Value::as_u64),
    })
}

/// 取字符串字段并做字符边界截断；空白串视为缺失。
fn text_field(value: Option<&Value>, max_chars: usize) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(max_chars).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn started_event_projects_tool_exec_and_iteration() {
        let payload = r#"{"kind":"ToolCallStarted","tool_name":"read_file","tool_call_id":"call-1",
            "exec_id":"exec-9","iteration":2,"arguments":{"path":"secret"}}"#;
        let event = describe(TOOL_STARTED, payload).expect("结构化");
        assert_eq!(event.tool.as_deref(), Some("read_file"));
        assert_eq!(event.call_id, "call-1");
        assert_eq!(event.exec.as_deref(), Some("exec-9"));
        assert_eq!(event.iteration, Some(2));
        assert!(event.state.is_none());
        assert!(event.output.is_none(), "参数不得进入展示投影");
    }

    #[test]
    fn finished_event_projects_state_duration_output_and_error() {
        let payload = r#"{"kind":"ToolCallFinished","tool_call_id":"call-1","exec_id":"exec-9",
            "state":"completed","duration_ms":42,"output_value":"  line one\n line two  "}"#;
        let event = describe(TOOL_FINISHED, payload).expect("结构化");
        assert_eq!(event.state.as_deref(), Some("completed"));
        assert_eq!(event.duration_ms, Some(42));
        assert_eq!(event.output.as_deref(), Some("line one line two"));
        assert!(event.error.is_none());

        let failed = r#"{"kind":"ToolCallFinished","tool_call_id":"call-2","state":"failed",
            "error_code":"permission_denied","duration_ms":7}"#;
        let event = describe(TOOL_FINISHED, failed).expect("结构化");
        assert_eq!(event.error.as_deref(), Some("permission_denied"));
        assert!(event.output.is_none());
    }

    #[test]
    fn output_is_bounded_and_whitespace_is_collapsed() {
        let long = "x".repeat(MAX_TOOL_OUTPUT_CHARS + 50);
        let payload = format!(
            r#"{{"kind":"ToolCallFinished","tool_call_id":"call-1","state":"completed","output_value":"{long}"}}"#
        );
        let event = describe(TOOL_FINISHED, &payload).expect("结构化");
        let output = event.output.expect("输出");
        assert!(
            output.chars().count() <= MAX_TOOL_OUTPUT_CHARS + 1,
            "含省略号的有界摘要"
        );
    }

    #[test]
    fn malformed_or_non_tool_events_are_not_structured() {
        assert!(describe("StreamingChunk", r#"{"tool_call_id":"call-1"}"#).is_none());
        assert!(describe(TOOL_STARTED, "not json").is_none());
        assert!(
            describe(TOOL_STARTED, r#"{"tool_name":"read_file"}"#).is_none(),
            "缺 tool_call_id 不得伪造结构"
        );
        assert!(describe(TOOL_FINISHED, r#"{"tool_call_id":"   "}"#).is_none());
    }
}
