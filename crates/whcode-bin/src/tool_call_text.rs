//! 工具调用事件的本地化文本（Check 348／FR-743，CUI 与 TUI 共用）。
//!
//! 结构化投影在 [`whcode_app::tool_call`]（解析、配对与有界）；本模块只做一件事：
//! 把稳定数据值填进文本目录。缺字段一律呈现 `?`——区分“正确的空”与“缺前置数据”，
//! 不推断工具名、状态或耗时。
//!
//! 输出形如（zh-CN）：
//!
//! ```text
//! read_file · 调用开始 · exec=exec-9 · call=call-1 · 迭代 2
//! read_file · 调用结束 · state=completed · exec=exec-9 · call=call-1 · 42 毫秒 · 输出=…
//! ```
//!
//! `ToolCallFinished` 载荷通常不带工具名，调用方从同页 `ToolCallStarted` 事件按
//! `tool_call_id` 配对后作为 `tool_hint` 传入；配对缺失时显示 `?`，不猜。

use whcode_app::tool_call::{TOOL_STARTED, describe};
use whcode_domain::i18n::I18n;

/// 组装一条工具调用文本；非工具事件或载荷损坏返回 `None`（调用方回落通用摘要）。
#[must_use]
pub(crate) fn event_summary(
    texts: &I18n,
    kind: &str,
    payload: &str,
    tool_hint: Option<&str>,
) -> Option<String> {
    let event = describe(kind, payload)?;
    let tool = event.tool.as_deref().or(tool_hint).unwrap_or("?");
    let exec = event.exec.as_deref().unwrap_or("?");
    let call = event.call_id.as_str();
    let iteration = event
        .iteration
        .map(|value| value.to_string())
        .unwrap_or_else(|| "?".to_string());
    let mut line = if kind == TOOL_STARTED {
        texts.resolve_fmt(
            "tool.call.started",
            &[
                ("tool", tool),
                ("exec", exec),
                ("call", call),
                ("iteration", &iteration),
            ],
        )
    } else {
        let state = event.state.as_deref().unwrap_or("?");
        let duration = event
            .duration_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string());
        texts.resolve_fmt(
            "tool.call.finished",
            &[
                ("tool", tool),
                ("state", state),
                ("exec", exec),
                ("call", call),
                ("duration", &duration),
            ],
        )
    };
    if let Some(output) = event.output.as_deref() {
        line.push_str(" · ");
        line.push_str(&texts.resolve_fmt("tool.call.output", &[("output", output)]));
    }
    if let Some(error) = event.error.as_deref() {
        line.push_str(" · ");
        line.push_str(&texts.resolve_fmt("tool.call.error", &[("error", error)]));
    }
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::i18n::{I18n, LocaleSource};

    fn texts() -> I18n {
        I18n::load("zh-CN", LocaleSource::Default)
    }

    #[test]
    fn started_line_carries_tool_exec_call_and_iteration() {
        let payload = r#"{"tool_name":"read_file","tool_call_id":"call-1",
            "exec_id":"exec-9","iteration":2}"#;
        let line = event_summary(&texts(), TOOL_STARTED, payload, None).expect("结构化");
        assert!(line.contains("read_file") && line.contains("exec-9") && line.contains("call-1"));
        assert!(line.contains("迭代 2"), "轮次保留");
    }

    #[test]
    fn finished_line_uses_paired_tool_and_explicit_missing_markers() {
        let payload = r#"{"tool_call_id":"call-1","state":"failed",
            "error_code":"permission_denied","duration_ms":7}"#;
        let line = event_summary(&texts(), "ToolCallFinished", payload, Some("read_file"))
            .expect("结构化");
        assert!(line.contains("read_file"), "结束事件靠同页配对补工具名");
        assert!(line.contains("state=failed") && line.contains("7"));
        assert!(line.contains("permission_denied"));
        assert!(
            !line.contains("exec=") || line.contains("exec=?"),
            "缺执行 ID 显式呈现为 ?"
        );
    }

    #[test]
    fn non_tool_event_has_no_structured_line() {
        assert!(event_summary(&texts(), "StreamingChunk", "{}", None).is_none());
    }
}
