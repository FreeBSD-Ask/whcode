//! # LSP 基础协议帧（ADR-078 决策 2）
//!
//! 规范事实（LSP 3.18 官方页，2026-11-15 取读）：头与体以 `\r\n` 分隔，头块以空行
//! （`\r\n\r\n`）结束；必填头 `Content-Length` 为**字节数**；可选头 `Content-Type`
//! 默认 `application/vscode-jsonrpc; charset=utf-8`（当前唯一支持的编码是 UTF-8）。
//!
//! 有界性：单条消息上限 [`MAX_MESSAGE_BYTES`]，超限**显式报错**而不是截断——截断会产生
//! 伪 JSON，让调用方拿到“看起来解析失败”的错误而不是“消息过大”的事实。
//!
//! 本模块是纯函数：不碰进程与套接字，帧编解码的失败路径全部可测。

use serde_json::Value;
use whcode_domain::{ErrorCode, WhError};

/// 单条消息上限（字节）。LSP 服务器多在 64 KiB 量级，1 MiB 留足余量又挡住无界累积。
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// 默认 `Content-Type`（规范缺省值；本仓只支持 UTF-8）。
pub const CONTENT_TYPE: &str = "application/vscode-jsonrpc; charset=utf-8";

/// 头块分隔符（规范：两个 `\r\n`）。
const HEADER_END: &[u8; 4] = b"\r\n\r\n";

fn frame_err(message: String) -> WhError {
    WhError::new(ErrorCode::ParseError, message)
}

/// 编码一条消息为完整帧（`Content-Length` ＋ `Content-Type` ＋ 空行 ＋ JSON 体）。
///
/// # Errors
///
/// 序列化失败或体超过 [`MAX_MESSAGE_BYTES`] 时返回 [`ErrorCode::ParseError`]／
/// [`ErrorCode::InvalidState`]。
pub fn encode_frame(message: &Value) -> Result<Vec<u8>, WhError> {
    let body = serde_json::to_vec(message)
        .map_err(|err| frame_err(format!("LSP 消息序列化失败：{err}")))?;
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(WhError::invalid_state(format!(
            "LSP 消息体 {} 字节超过上限 {MAX_MESSAGE_BYTES}：不接受超限消息",
            body.len()
        )));
    }
    let mut out = format!(
        "Content-Length: {}\r\nContent-Type: {CONTENT_TYPE}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(&body);
    Ok(out)
}

/// 从缓冲区头部解析一帧：`Ok(None)` 表示数据还不够一条完整消息（继续读）。
///
/// 返回 `(消息, 已消费字节数)`——调用方据消费长度推进缓冲区。
///
/// # Errors
///
/// 头格式非法（无冒号／`Content-Length` 缺失或非数字）、体超过上限、体不是合法 JSON 时
/// 返回 [`ErrorCode::ParseError`]／[`ErrorCode::InvalidState`]。错误消息只给字节数与
/// 结构事实，不回显消息体（体里可能是用户代码）。
pub fn parse_frame(buffer: &[u8]) -> Result<Option<(Value, usize)>, WhError> {
    let Some(header_end) = find_header_end(buffer) else {
        return Ok(None);
    };
    let mut content_length: Option<usize> = None;
    for line in buffer[..header_end].split(|byte| *byte == b'\n') {
        let line = trim_ascii(line);
        if line.is_empty() {
            continue;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(frame_err(
                "LSP 头行缺少冒号（应为 `Name: value`），无法解析该帧".to_string(),
            ));
        };
        let name = trim_ascii(&line[..colon]);
        let value = trim_ascii(&line[colon + 1..]);
        if name.eq_ignore_ascii_case(b"content-length") {
            let text = std::str::from_utf8(value)
                .map_err(|_| frame_err("LSP 的 Content-Length 不是 ASCII 数字".to_string()))?;
            let parsed = text.parse::<usize>().map_err(|_| {
                frame_err(format!(
                    "LSP 的 Content-Length 不是合法字节数（{text}），无法解析该帧"
                ))
            })?;
            content_length = Some(parsed);
        }
    }
    let Some(length) = content_length else {
        return Err(frame_err(
            "LSP 头缺少 Content-Length：规范要求必填，无法确定消息边界".to_string(),
        ));
    };
    if length > MAX_MESSAGE_BYTES {
        return Err(WhError::invalid_state(format!(
            "LSP 消息体声明 {length} 字节，超过上限 {MAX_MESSAGE_BYTES}：拒绝接收"
        )));
    }
    let start = header_end + HEADER_END.len();
    let Some(end) = start.checked_add(length) else {
        return Err(frame_err(
            "LSP 的 Content-Length 溢出（与缓冲区长度相加越界）".to_string(),
        ));
    };
    if buffer.len() < end {
        return Ok(None);
    }
    let body = &buffer[start..end];
    let value: Value = serde_json::from_slice(body).map_err(|err| {
        frame_err(format!(
            "LSP 消息体不是合法 JSON（声明 {length} 字节）：{err}"
        ))
    })?;
    Ok(Some((value, end)))
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(HEADER_END.len())
        .position(|window| window == HEADER_END)
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 往返：编码后再解析，消息内容一致且消费长度等于帧长。
    #[test]
    fn encode_then_parse_roundtrips() {
        let message = json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": true}});
        let frame = encode_frame(&message).expect("编码");
        let (parsed, consumed) = parse_frame(&frame).expect("解析").expect("完整帧");
        assert_eq!(parsed, message);
        assert_eq!(consumed, frame.len());
    }

    /// 数据不完整时必须返回 `None`（不是错误）——调用方继续读。
    #[test]
    fn partial_frame_is_incomplete_not_error() {
        let frame = encode_frame(&json!({"id": 1})).expect("编码");
        for cut in [0, 4, frame.len() - 1] {
            assert!(
                parse_frame(&frame[..cut])
                    .expect("不完整不算错误")
                    .is_none(),
                "截断到 {cut} 字节时应判定为不完整"
            );
        }
    }

    /// 缺 `Content-Length`、非数字、超上限、体非 JSON：全部显式失败。
    #[test]
    fn malformed_frames_fail_loudly() {
        let missing = b"Content-Type: application/json\r\n\r\n{}";
        let err = parse_frame(missing).expect_err("缺 Content-Length 必须失败");
        assert_eq!(err.code, ErrorCode::ParseError);

        let not_number = b"Content-Length: abc\r\n\r\n{}";
        let err = parse_frame(not_number).expect_err("非数字必须失败");
        assert_eq!(err.code, ErrorCode::ParseError);

        let oversize = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 1);
        let err = parse_frame(oversize.as_bytes()).expect_err("超上限必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.message.contains("超过上限"),
            "错误须说明超限事实：{}",
            err.message
        );

        let bad_json = b"Content-Length: 3\r\n\r\n{ x";
        let err = parse_frame(bad_json).expect_err("坏 JSON 必须失败");
        assert_eq!(err.code, ErrorCode::ParseError);

        let no_colon = b"ContentLength 12\r\n\r\n{}";
        let err = parse_frame(no_colon).expect_err("无冒号必须失败");
        assert_eq!(err.code, ErrorCode::ParseError);
    }

    /// 头名大小写不敏感、未知头忽略（规范对头名 ASCII 大小写不作限定）。
    #[test]
    fn header_name_is_case_insensitive_and_unknown_headers_ignored() {
        let body = b"{\"id\":1}";
        let frame = format!(
            "content-length: {}\r\nX-Unknown: yes\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        let (parsed, _) = parse_frame(frame.as_bytes())
            .expect("解析")
            .expect("完整帧");
        assert_eq!(parsed["id"], json!(1));
    }

    /// 两帧连续：第一帧的消费长度必须精确落在第二帧起点。
    #[test]
    fn back_to_back_frames_advance_exactly() {
        let first = encode_frame(&json!({"id": 1})).expect("编码");
        let second = encode_frame(&json!({"id": 2})).expect("编码");
        let mut buffer = first.clone();
        buffer.extend_from_slice(&second);
        let (parsed, consumed) = parse_frame(&buffer).expect("解析").expect("首帧");
        assert_eq!(parsed["id"], json!(1));
        let (next, _) = parse_frame(&buffer[consumed..])
            .expect("解析")
            .expect("次帧");
        assert_eq!(next["id"], json!(2));
    }
}
