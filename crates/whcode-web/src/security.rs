//! # 认证、Origin 校验与消息大小限制（T092、§10.2、V-110）
//!
//! - **认证**：`Authorization: Bearer <token>` 头（**token 不经 URL query 传递**，
//!   §10.2）——恒定时间比较防时序侧信道；
//! - **Origin 校验**：WebSocket 升级与跨域请求的 `Origin` 头必须命中允许集
//!   （默认回环来源）；
//! - **消息大小限制**：请求体与 WebSocket 帧的上限（§10.2）。
//!
//! 安全默认值（V-110）：认证未启用时**只允许回环来源**；token 配置不落入日志
//! （§10.1，调用方负责）。
//!
//! 边界：本模块只做判定（纯函数 + 请求检查），不做连接管理（属 [`crate::api`]）。

use whcode_domain::{ErrorCode, WhError};

/// 默认请求体/帧大小上限（字节）。
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// 默认允许的 Origin（回环来源）。
pub const ALLOWED_ORIGINS: [&str; 2] = ["http://127.0.0.1", "http://localhost"];

fn forbidden(message: String) -> WhError {
    WhError::new(ErrorCode::PermissionDenied, message)
}

/// 恒定时间字符串比较（防时序侧信道；长度不同立即不等）。
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = 0u8;
    for index in 0..a.len() {
        diff |= a[index] ^ b[index];
    }
    diff == 0
}

/// 认证检查：`Authorization: Bearer <token>` 头必须与配置 token 一致
/// （恒定时间比较）；**token 不经 URL query**（§10.2——调用方传入的 query 已剥离）。
///
/// # Errors
///
/// 认证启用但头缺失或不匹配时返回 `permission_denied`（不可重试）。
pub fn check_auth(
    auth_enabled: bool,
    expected_token: Option<&str>,
    authorization_header: Option<&str>,
) -> Result<(), WhError> {
    if !auth_enabled {
        return Ok(()); // 仅回环监听时合法（bind 门禁保证，V-110）。
    }
    let Some(token) = expected_token.filter(|token| !token.trim().is_empty()) else {
        return Err(forbidden(
            "认证已启用但未配置 token：拒绝请求（§10.2）".to_string(),
        ));
    };
    let Some(header) = authorization_header else {
        return Err(forbidden(
            "缺少 Authorization 头（Bearer token，§10.2）".to_string(),
        ));
    };
    let Some(bearer) = header.strip_prefix("Bearer ") else {
        return Err(forbidden(
            "Authorization 头须为 Bearer scheme（§10.2）".to_string(),
        ));
    };
    if constant_time_eq(bearer, token) {
        Ok(())
    } else {
        Err(forbidden("token 不匹配（恒定时间比较，§10.2）".to_string()))
    }
}

/// Origin 校验：认证启用（对外监听）时，Origin 必须命中允许集；未启用时仅回环
/// 来源放行（V-110 默认）。
///
/// # Errors
///
/// Origin 缺失或不在允许集时返回 `permission_denied`（不可重试）。
pub fn check_origin(
    auth_enabled: bool,
    allowed_origins: &[&str],
    origin: Option<&str>,
) -> Result<(), WhError> {
    let Some(origin) = origin.filter(|origin| !origin.trim().is_empty()) else {
        return Err(forbidden("缺少 Origin 头（§10.2）".to_string()));
    };
    let allowed: &[&str] = if auth_enabled {
        allowed_origins
    } else {
        &ALLOWED_ORIGINS
    };
    if allowed
        .iter()
        .any(|candidate| constant_time_eq(origin, candidate))
    {
        Ok(())
    } else {
        Err(forbidden(format!(
            "Origin {origin} 不在允许集（§10.2、V-110）"
        )))
    }
}

/// 消息大小限制（§10.2）。
///
/// # Errors
///
/// 超过 [`MAX_MESSAGE_BYTES`] 时返回 `invalid_state`（不可重试）。
pub fn check_message_size(size: usize) -> Result<(), WhError> {
    if size > MAX_MESSAGE_BYTES {
        Err(WhError::new(
            ErrorCode::InvalidState,
            format!("消息 {size} 字节超过上限 {MAX_MESSAGE_BYTES}（§10.2）"),
        ))
    } else {
        Ok(())
    }
}

/// token 是否出现在 URL query 中（§10.2 禁止——调用方校验用）。
#[must_use]
pub fn token_in_query(url: &str, token: &str) -> bool {
    let Some((_, query)) = url.split_once('?') else {
        return false;
    };
    query.split('&').any(|pair| {
        let Some((_, value)) = pair.split_once('=') else {
            return false;
        };
        value == token
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_requires_bearer_header_when_enabled() {
        // 认证启用：缺头/无 scheme/不匹配均拒绝（§10.2）。
        for header in [None, Some("Basic abc"), Some("Bearer wrong")] {
            let err = check_auth(true, Some("secret"), header).expect_err("必须拒绝");
            assert_eq!(err.code, ErrorCode::PermissionDenied);
            assert!(!err.retryable);
        }
        check_auth(true, Some("secret"), Some("Bearer secret")).expect("匹配放行");
        // 认证未启用：回环监听合法（bind 门禁保证）。
        check_auth(false, None, None).expect("回环放行");
        // 空配置 token：拒绝（认证启用但无 token）。
        check_auth(true, Some("  "), Some("Bearer anything")).expect_err("空 token 拒绝");
    }

    #[test]
    fn origin_must_be_in_allowlist() {
        for origin in [Some("http://evil.example"), Some(""), None] {
            let err = check_origin(false, &[], origin).expect_err("必须拒绝");
            assert_eq!(err.code, ErrorCode::PermissionDenied);
        }
        check_origin(false, &[], Some("http://127.0.0.1")).expect("回环来源放行");
        check_origin(
            true,
            &["http://10.0.0.5:8080"],
            Some("http://10.0.0.5:8080"),
        )
        .expect("对外监听的显式允许集");
    }

    #[test]
    fn message_size_is_bounded() {
        assert!(check_message_size(0).is_ok());
        assert!(check_message_size(MAX_MESSAGE_BYTES).is_ok());
        let err = check_message_size(MAX_MESSAGE_BYTES + 1).expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable);
    }

    #[test]
    fn token_must_not_pass_through_url_query() {
        assert!(
            token_in_query("/api/status?token=secret", "secret"),
            "query 中的 token 必须检出"
        );
        assert!(!token_in_query("/api/status", "secret"));
        assert!(!token_in_query("/api/status?other=1", "secret"));
        // §10.2：正确方式是 Authorization 头。
        check_auth(true, Some("secret"), Some("Bearer secret")).expect("头传递合法");
    }
}
