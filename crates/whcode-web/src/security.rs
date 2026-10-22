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

/// Origin 校验：请求的 `Origin` 必须命中允许集（按 scheme + host 判定，**端口不参与**
/// ——浏览器携带实际端口，如 `http://127.0.0.1:8737`）。认证启用（对外监听）时，缺少
/// `Origin` 仍拒绝；**回环模式（认证关闭）放行缺失 Origin**——同源 GET 请求浏览器不发送
/// Origin，本机 CLI 客户端同样没有（ADR-022 修订，原全串精确比较会把真实浏览器一律拒绝）。
///
/// 威胁模型（不变）：拦截**非回环来源网页**对回环服务的跨站请求（DNS rebinding / 恶意
/// 页面 fetch）；跨站浏览器请求必带 `Origin: http://恶意站`，host 不在允许集即拒绝。
/// `Origin` 非秘密，按字节比较即可（不要求恒定时间——恒定时间仅用于 token 比较）。
///
/// # Errors
///
/// Origin 带非回环 host、格式非法，或认证模式下缺失时返回 `permission_denied`（不可重试）。
pub fn check_origin(
    auth_enabled: bool,
    allowed_origins: &[&str],
    origin: Option<&str>,
) -> Result<(), WhError> {
    let allowed: &[&str] = if auth_enabled {
        allowed_origins
    } else {
        &ALLOWED_ORIGINS
    };
    let Some(origin) = origin.filter(|origin| !origin.trim().is_empty()) else {
        return if auth_enabled {
            Err(forbidden(
                "认证模式缺少 Origin 头（§10.2；对外监听须显式来源）".to_string(),
            ))
        } else {
            Ok(()) // 回环模式：同源 GET / 本机 CLI 客户端不带 Origin（ADR-022）。
        };
    };
    let origin = origin.trim();
    let Some(origin_key) = origin_scheme_host(origin) else {
        return Err(forbidden(format!("Origin 格式非法：{origin}")));
    };
    if allowed
        .iter()
        .any(|candidate| origin_scheme_host(candidate).as_ref() == Some(&origin_key))
    {
        Ok(())
    } else {
        Err(forbidden(format!(
            "Origin {origin} 不在允许集（§10.2、V-110）"
        )))
    }
}

/// 提取 Origin 的 `(scheme, host)` 键（小写化；端口与路径不参与，ADR-022）。
fn origin_scheme_host(origin: &str) -> Option<(String, String)> {
    let (scheme, rest) = origin.split_once("://")?;
    let authority = rest.split('/').next()?;
    if authority.is_empty() {
        return None;
    }
    // IPv6 字面量：[::1]:8080 → [::1]
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split_once(']').map(|(host, _)| host)?
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) => host,
            _ => authority,
        }
    };
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
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
        let err = check_origin(false, &[], Some("http://evil.example")).expect_err("必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        check_origin(false, &[], Some("http://127.0.0.1")).expect("回环来源放行");
        check_origin(
            true,
            &["http://10.0.0.5:8080"],
            Some("http://10.0.0.5:8080"),
        )
        .expect("对外监听的显式允许集");
    }

    /// ADR-022：按 scheme + host 判定（端口不参与）；缺失 Origin 在回环模式放行、
    /// 认证模式拒绝；恶意 host 一律拒绝（威胁模型不变）。
    #[test]
    fn origin_matching_is_port_agnostic_and_mode_aware() {
        // 真实浏览器携带实际端口——必须放行（修订前全串比较会 403）。
        check_origin(false, &[], Some("http://127.0.0.1:8737")).expect("端口不参与命中");
        check_origin(false, &[], Some("http://localhost:3000")).expect("localhost 任意端口");
        check_origin(true, &ALLOWED_ORIGINS, Some("http://127.0.0.1:8080"))
            .expect("认证模式同样按 host 命中");

        // 缺失/空白 Origin：回环模式放行（同源 GET 与 CLI 客户端不带 Origin）。
        check_origin(false, &[], None).expect("回环模式缺 Origin 放行");
        check_origin(false, &[], Some("   ")).expect("空白 Origin 视为缺失");
        // 认证模式（对外监听）仍要求 Origin。
        let err = check_origin(true, &ALLOWED_ORIGINS, None).expect_err("认证模式缺 Origin 拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);

        // 威胁模型：非回环 host 拒绝——含相似前缀/任意端口/大小写伪装。
        for evil in [
            "http://evil.example",
            "http://evil.example:80",
            "http://127.0.0.1.evil.example",
            "http://localhost.evil.example",
            "https://127.0.0.1",
        ] {
            assert!(
                check_origin(false, &[], Some(evil)).is_err(),
                "{evil} 必须拒绝（非回环 host）"
            );
        }
        // 大小写与 IPv6 字面量的键提取（大小写不敏感；IPv6 去括号与端口）。
        check_origin(false, &[], Some("HTTP://LocalHost:8080")).expect("scheme/host 大小写不敏感");
        check_origin(true, &["http://[::1]:9000"], Some("http://[::1]")).expect("IPv6 字面量");
        assert!(
            check_origin(false, &[], Some("not-an-origin")).is_err(),
            "无 scheme 拒绝"
        );
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
