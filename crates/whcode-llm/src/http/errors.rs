//! # HTTP 状态与错误归一化（FR-212、161.6）
//!
//! HTTP 状态、连接/TLS 失败、读取超时与协议错误包统一映射为既有
//! [`ErrorCode`](whcode_domain::ErrorCode)，并附 `provider`/`protocol`/
//! `http_status`/`retry_after_ms`/`request_id` details。供应商响应正文只用于
//! 提取错误消息，**不得作为控制指令执行**（§10.1）。默认不自动重试
//! （避免重复计费），可重试性经 `retryable` 暴露，由调用方决定（FR-212）。

use std::io::Read;

use whcode_domain::{ErrorCode, WhError};

/// 非 2xx 错误正文的读取上限（错误信息足够，不吞整个响应）。
pub const MAX_ERROR_BODY_BYTES: usize = 64 << 10;

/// 把 HTTP 状态映射为既有错误码并附加 details（FR-212）。
#[must_use]
pub fn normalize_status(
    status: u16,
    protocol_message: &str,
    provider: &str,
    protocol: &str,
    request_id: &str,
    retry_after_ms: Option<u64>,
) -> WhError {
    let (code, retryable, message) = match status {
        400 | 405 | 422 => (ErrorCode::InvalidState, false, "请求形态被供应商拒绝"),
        401 | 403 => (ErrorCode::PermissionDenied, false, "鉴权失败（凭据缺失/无效/无权限）"),
        404 => (ErrorCode::NotFound, false, "端点或模型不存在"),
        429 => (ErrorCode::Backpressure, true, "供应商限流（429）"),
        500..=599 => (ErrorCode::Internal, true, "供应商服务端错误（5xx）"),
        _ => (ErrorCode::Internal, false, "供应商返回未预期状态"),
    };
    let mut details = serde_json::json!({
        "provider": provider,
        "protocol": protocol,
        "http_status": status,
        "request_id": request_id,
    });
    if let Some(ms) = retry_after_ms {
        details["retry_after_ms"] = serde_json::json!(ms);
    }
    WhError {
        code,
        message: format!("{message}：{protocol_message}（HTTP {status}）"),
        retryable,
        details: Some(details),
    }
}

/// 从错误响应流读取有界正文（不超过 [`MAX_ERROR_BODY_BYTES`]）。
///
/// # Errors
///
/// IO 错误原样返回 [`ErrorCode::IoError`]。
pub fn read_error_body(reader: &mut dyn Read) -> Result<String, WhError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = reader.read(&mut chunk).map_err(|err| {
            WhError::new(ErrorCode::IoError, format!("错误响应读取失败：{err}"))
        })?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
        if buf.len() >= MAX_ERROR_BODY_BYTES {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// 解析 `Retry-After` 头（秒或 HTTP 日期；日期形态不支持，返回 `None` 不伪装）。
#[must_use]
pub fn retry_after_ms(header: Option<&str>) -> Option<u64> {
    header.and_then(|v| v.trim().parse::<u64>().ok()).map(|s| s * 1_000)
}

/// 连接/TLS/IO 失败的归一化（FR-212：timeout/连接失败与协议错误区分）。
#[must_use]
pub fn normalize_transport(error: &std::io::Error, provider: &str, protocol: &str, request_id: &str) -> WhError {
    let kind = error.kind();
    let (code, retryable) = if matches!(
        kind,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        (ErrorCode::Timeout, true)
    } else {
        (ErrorCode::IoError, true)
    };
    WhError {
        code,
        message: format!(
            "供应商连接失败（{code:?}）：{error}——连接/TLS/读取错误已归一化，供应商正文不作为控制指令"
        ),
        retryable,
        details: Some(serde_json::json!({
            "provider": provider,
            "protocol": protocol,
            "request_id": request_id,
            "io_kind": format!("{kind:?}"),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_map_matches_fr212() {
        let err = normalize_status(429, "rate limited", "p", "openai", "R-1", Some(2_000));
        assert_eq!(err.code, ErrorCode::Backpressure);
        assert!(err.retryable);
        assert_eq!(err.details.as_ref().expect("details")["retry_after_ms"], 2_000);

        let err = normalize_status(401, "bad key", "p", "openai", "R-1", None);
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);

        let err = normalize_status(404, "no model", "p", "openai", "R-1", None);
        assert_eq!(err.code, ErrorCode::NotFound);

        let err = normalize_status(500, "boom", "p", "openai", "R-1", None);
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.retryable);
    }

    #[test]
    fn details_carry_provider_protocol_status_without_secret_surface() {
        let err = normalize_status(400, "bad", "sensenova", "openai", "R-9", None);
        let details = err.details.expect("details");
        assert_eq!(details["provider"], "sensenova");
        assert_eq!(details["protocol"], "openai");
        assert_eq!(details["http_status"], 400);
        assert_eq!(details["request_id"], "R-9");
    }

    #[test]
    fn transport_errors_distinguish_timeout_from_io() {
        let timeout = std::io::Error::new(std::io::ErrorKind::TimedOut, "read timeout");
        let err = normalize_transport(&timeout, "p", "openai", "R");
        assert_eq!(err.code, ErrorCode::Timeout);
        assert!(err.retryable);

        let refused = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let err = normalize_transport(&refused, "p", "openai", "R");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(err.retryable, "连接失败可重试（调用方决定，FR-212）");
    }

    #[test]
    fn retry_after_parses_seconds_only() {
        assert_eq!(retry_after_ms(Some("2")), Some(2_000));
        assert_eq!(retry_after_ms(Some("Tue, 01 Jan 2030 00:00:00 GMT")), None, "日期形态不伪装");
        assert_eq!(retry_after_ms(None), None);
    }
}
