//! # 流帧解码（FR-210、161.5）：SSE／NDJSON／JSON 数组流的有界解码
//!
//! 三种线格式统一产出 [`Frame`]，供协议层映射为 chunk 语义。所有解码器**有界**：
//! 单行/单帧超过 [`MAX_FRAME_BYTES`]、累计超过 [`MAX_RESPONSE_BYTES`] 即返回
//! `parse_error`——不静默截断、不无界累积（NFR-215）。

use std::io::BufRead;

use whcode_domain::WhError;

use crate::http::profile::FrameFormat;

/// 单帧上限（字节；一行 SSE data、一条 NDJSON 或一个数组元素）。
pub const MAX_FRAME_BYTES: usize = 1 << 20; // 1 MiB

/// 单次响应累计上限（字节；含全部帧）。
pub const MAX_RESPONSE_BYTES: usize = 64 << 20; // 64 MiB

/// 解码后的一帧：事件名（SSE 可带）与 JSON 数据。
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// SSE 事件名（`event:` 行；NDJSON/JSON 数组流为 `None`）。
    pub event_name: Option<String>,
    /// 帧数据（SSE 的多行 `data:` 以 `\n` 拼接；NDJSON/数组流为原始 JSON 文本）。
    pub data: String,
}

impl Frame {
    /// 把 data 解析为 JSON；畸形 JSON 返回 `parse_error`（FR-210：异常流不静默）。
    ///
    /// # Errors
    ///
    /// JSON 解析失败时返回 [`ErrorCode::ParseError`]。
    pub fn json(&self) -> Result<serde_json::Value, WhError> {
        serde_json::from_str(&self.data).map_err(|err| {
            WhError::parse_error(format!(
                "流帧 JSON 解析失败（前 120 字节：{:.120}）：{err}",
                self.data
            ))
        })
    }
}

/// 有界累计器（总字节上限，跨帧共享）。
#[derive(Debug, Default)]
pub struct Budget {
    consumed: usize,
}

impl Budget {
    /// 记录一段字节；超限返回 `parse_error`（FR-210：不得截断后报告成功）。
    ///
    /// # Errors
    ///
    /// 累计超过 [`MAX_RESPONSE_BYTES`] 时返回 [`ErrorCode::ParseError`]。
    pub fn charge(&mut self, bytes: usize) -> Result<(), WhError> {
        self.consumed = self.consumed.saturating_add(bytes);
        if self.consumed > MAX_RESPONSE_BYTES {
            return Err(WhError::parse_error(format!(
                "响应累计超过上限 {MAX_RESPONSE_BYTES} 字节：已截断处理，不报告成功"
            )));
        }
        Ok(())
    }

    /// 已消费字节数（诊断）。
    #[must_use]
    pub const fn consumed(&self) -> usize {
        self.consumed
    }
}

/// 从有界 reader 逐帧解码（按 [`FrameFormat`]；返回 `None` 表示流结束）。
///
/// 阻塞式逐行读取（与 [`crate::provider::Provider`] 的同步迭代器契约一致）；
/// 取消由调用方在帧间检查（下一个读取边界语义，FR-212）。
///
/// # Errors
///
/// 行超限、累计超限或 JSON 解析失败时返回 [`ErrorCode::ParseError`]；IO 错误
/// 原样传播（由错误归一化层映射）。
pub fn next_frame(
    reader: &mut dyn BufRead,
    format: FrameFormat,
    budget: &mut Budget,
) -> Result<Option<Frame>, WhError> {
    match format {
        FrameFormat::Sse => next_sse_frame(reader, budget),
        FrameFormat::Ndjson => next_ndjson_frame(reader, budget),
        FrameFormat::JsonArray => next_json_array_frame(reader, budget),
    }
}

fn bounded_line(reader: &mut dyn BufRead, budget: &mut Budget) -> Result<Option<String>, WhError> {
    let mut line = String::new();
    let read = reader.read_line(&mut line).map_err(|err| {
        WhError::new(
            whcode_domain::ErrorCode::IoError,
            format!("流读取失败：{err}"),
        )
    })?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_FRAME_BYTES {
        return Err(WhError::parse_error(format!(
            "单行超过上限 {MAX_FRAME_BYTES} 字节：不静默截断"
        )));
    }
    budget.charge(line.len())?;
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(Some(line))
}

/// SSE：`event:`/`data:` 字段 + 空行分帧；`data: [DONE]` 也是一帧（协议层处理）。
fn next_sse_frame(reader: &mut dyn BufRead, budget: &mut Budget) -> Result<Option<Frame>, WhError> {
    let mut event_name: Option<String> = None;
    let mut data: Vec<String> = Vec::new();
    loop {
        let Some(line) = bounded_line(reader, budget)? else {
            if event_name.is_none() && data.is_empty() {
                return Ok(None);
            }
            break;
        };
        if line.is_empty() {
            if event_name.is_some() || !data.is_empty() {
                break;
            }
            continue;
        }
        if let Some(name) = line.strip_prefix("event:") {
            event_name = Some(name.trim_start().to_string());
        } else if let Some(payload) = line.strip_prefix("data:") {
            data.push(payload.strip_prefix(' ').unwrap_or(payload).to_string());
        }
        // 注释行（`:...`）与其他字段（`id:`/`retry:`）按 SSE 规范忽略。
    }
    Ok(Some(Frame {
        event_name,
        data: data.join("\n"),
    }))
}

/// NDJSON：每个非空行一个 JSON 值。
fn next_ndjson_frame(
    reader: &mut dyn BufRead,
    budget: &mut Budget,
) -> Result<Option<Frame>, WhError> {
    loop {
        let Some(line) = bounded_line(reader, budget)? else {
            return Ok(None);
        };
        if line.trim().is_empty() {
            continue;
        }
        return Ok(Some(Frame {
            event_name: None,
            data: line,
        }));
    }
}

/// JSON 数组流：顶层 `[` 后逐元素解码（括号深度 + 字符串状态机，161.5）。
fn next_json_array_frame(
    reader: &mut dyn BufRead,
    budget: &mut Budget,
) -> Result<Option<Frame>, WhError> {
    // 状态机：跳过空白/逗号 → 元素（深度计数）→ 顶层 `]` 结束。
    let mut element = String::new();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    loop {
        let mut byte_buf = [0u8; 1];
        let read = reader.read(&mut byte_buf).map_err(|err| {
            WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!("流读取失败：{err}"),
            )
        })?;
        if read == 0 {
            if element.is_empty() {
                return Ok(None);
            }
            return Err(WhError::parse_error("JSON 数组流在元素中途结束"));
        }
        let ch = byte_buf[0] as char;
        budget.charge(1)?;
        if element.is_empty() && depth == 0 {
            match ch {
                ' ' | '\n' | '\r' | '\t' | ',' => continue,
                ']' => return Ok(None),
                '[' => continue,
                _ => {}
            }
        }
        element.push(ch);
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                if depth == 0 {
                    // 顶层字符串标量元素（不用于 LLM 流，但状态机保持正确）。
                    return Ok(Some(Frame {
                        event_name: None,
                        data: element,
                    }));
                }
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && ch == '}' {
                    return Ok(Some(Frame {
                        event_name: None,
                        data: element,
                    }));
                }
            }
            _ => {}
        }
        if element.len() > MAX_FRAME_BYTES {
            return Err(WhError::parse_error(format!(
                "单个数组元素超过上限 {MAX_FRAME_BYTES} 字节：不静默截断"
            )));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    fn frames(format: FrameFormat, input: &str) -> Vec<Frame> {
        let mut reader = BufReader::new(input.as_bytes());
        let mut budget = Budget::default();
        let mut out = Vec::new();
        while let Some(frame) = next_frame(&mut reader, format, &mut budget).expect("解码") {
            out.push(frame);
        }
        out
    }

    #[test]
    fn sse_decodes_event_names_multiline_data_and_done() {
        let input = "event: content_block_delta\ndata: {\"a\":1}\n\ndata: {\"b\":2}\ndata: {\"c\":3}\n\ndata: [DONE]\n\n: keep-alive\n\n";
        let fs = frames(FrameFormat::Sse, input);
        assert_eq!(fs.len(), 3);
        assert_eq!(fs[0].event_name.as_deref(), Some("content_block_delta"));
        assert_eq!(fs[0].data, "{\"a\":1}");
        assert_eq!(fs[1].data, "{\"b\":2}\n{\"c\":3}", "多行 data 以 \\n 拼接");
        assert_eq!(fs[2].data, "[DONE]");
        assert!(fs[0].json().is_ok());
    }

    #[test]
    fn ndjson_skips_blank_lines() {
        let input = "{\"a\":1}\n\n{\"b\":2}\n";
        let fs = frames(FrameFormat::Ndjson, input);
        assert_eq!(fs.len(), 2);
        assert_eq!(fs[1].data, "{\"b\":2}");
    }

    #[test]
    fn json_array_stream_decodes_elements_incrementally() {
        let input = "[ {\"a\":1}, {\"b\":{\"c\":[2,3]}}, \"str\" ]";
        let fs = frames(FrameFormat::JsonArray, input);
        assert_eq!(fs.len(), 3);
        assert_eq!(fs[0].data, "{\"a\":1}");
        assert_eq!(
            fs[1].data, "{\"b\":{\"c\":[2,3]}}",
            "嵌套括号与字符串中的括号不混淆"
        );
        assert_eq!(fs[2].data, "\"str\"");
    }

    #[test]
    fn oversized_line_and_budget_are_parse_errors() {
        let long = "x".repeat(MAX_FRAME_BYTES + 1);
        let mut reader = BufReader::new(long.as_bytes());
        let mut budget = Budget::default();
        let err = next_ndjson_frame(&mut reader, &mut budget).expect_err("超长行必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);

        let mut budget = Budget::default();
        let err = budget
            .charge(MAX_RESPONSE_BYTES + 1)
            .expect_err("累计超限必须失败");
        assert!(err.message.contains("不报告成功"));
    }
}
