//! # Web 绑定与外部监听门禁（T091、FR-606、V-110）
//!
//! 默认 **仅监听回环地址**（`127.0.0.1`）；对外监听（非回环）必须**显式开启**
//! 认证与 Origin 检查，否则**拒绝启动**（FR-606、V-110）——门禁由程序实施，
//! 不依赖调用方自觉。
//!
//! 边界：本模块只校验绑定配置与生成监听器；HTTP 语义属 [`crate::api`]。

use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use whcode_domain::{ErrorCode, WhError};

/// 默认绑定地址（仅回环，FR-606）。
pub const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// Web 绑定配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindConfig {
    /// 绑定地址（`host:port`）。
    pub addr: String,
    /// 是否显式启用认证（对外监听必须为真）。
    pub auth_enabled: bool,
    /// 是否显式启用 Origin 检查（对外监听必须为真）。
    pub origin_check_enabled: bool,
}

impl BindConfig {
    /// 回环默认配置（认证关闭——仅回环时合法，V-110）。
    #[must_use]
    pub fn loopback() -> Self {
        Self {
            addr: DEFAULT_BIND.to_string(),
            auth_enabled: false,
            origin_check_enabled: false,
        }
    }
}

/// 解析绑定地址为 SocketAddr（仅支持 IPv4 字面量 + 端口）。
fn parse_addr(addr: &str) -> Result<SocketAddr, WhError> {
    let (host, port) = addr
        .rsplit_once(':')
        .ok_or_else(|| invalid(format!("绑定地址缺少端口：{addr}")))?;
    let port: u16 = port
        .parse()
        .map_err(|_| invalid(format!("端口非法：{port}")))?;
    let ip: Ipv4Addr = host
        .parse()
        .map_err(|_| invalid(format!("主机须为 IPv4 字面量：{host}")))?;
    Ok(SocketAddr::new(ip.into(), port))
}

fn invalid(message: String) -> WhError {
    WhError::new(ErrorCode::InvalidState, message)
}

/// 地址是否回环（V-110 门禁依据）。
#[must_use]
pub fn is_loopback(addr: &str) -> bool {
    parse_addr(addr).is_ok_and(|socket| socket.ip().is_loopback())
}

/// 地址是否 RFC1918 私网（局域网范围，ADR-023）。
///
/// 允许 `10/8`、`172.16/12`、`192.168/16`；通配（`0.0.0.0`）、公网与 IPv6 均不属于
/// 本范围（"Web 仅限局域网"）。
#[must_use]
pub fn is_lan(addr: &str) -> bool {
    parse_addr(addr).is_ok_and(|socket| {
        let ip = socket.ip();
        if let std::net::IpAddr::V4(ip) = ip {
            ip.is_private()
        } else {
            false
        }
    })
}

/// 校验绑定配置并生成监听器。
///
/// 门禁（FR-606、V-110）：**非回环**地址且未显式开启认证与 Origin 检查时
/// **拒绝启动**（`invalid_state`，`retryable = false`）。
///
/// # Errors
///
/// 地址非法、对外监听未开启认证/Origin 检查、或端口绑定失败时返回 [`WhError`]。
pub fn bind(config: &BindConfig) -> Result<TcpListener, WhError> {
    let socket = parse_addr(&config.addr)?;
    // 地址边界（ADR-023）：仅回环或 RFC1918 私网——公网/通配一律拒绝（"仅限局域网"）。
    if !socket.ip().is_loopback() && !is_lan(&config.addr) {
        return Err(WhError {
            code: ErrorCode::InvalidState,
            message: format!(
                "绑定 {} 不在局域网范围（仅回环或 RFC1918 私网，ADR-023）：拒绝启动",
                config.addr
            ),
            retryable: false,
            details: Some(serde_json::json!({ "addr": config.addr, "policy": "lan-only" })),
        });
    }
    if !socket.ip().is_loopback() && (!config.auth_enabled || !config.origin_check_enabled) {
        return Err(WhError {
            code: ErrorCode::InvalidState,
            message: format!(
                "对外监听 {} 必须显式开启认证与 Origin 检查（FR-606、V-110）：拒绝启动",
                config.addr
            ),
            retryable: false,
            details: Some(serde_json::json!({
                "addr": config.addr,
                "auth_enabled": config.auth_enabled,
                "origin_check_enabled": config.origin_check_enabled,
                "requirement": "external listeners require auth + origin check",
            })),
        });
    }
    TcpListener::bind(socket).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("绑定 {} 失败：{err}", config.addr),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-023：「仅限局域网」地址边界。
    #[test]
    fn lan_policy_allows_private_ranges_only() {
        for lan in ["10.0.0.5:8080", "172.16.8.9:1", "192.168.5.4:8737"] {
            assert!(is_lan(lan), "{lan} 属 RFC1918");
            assert!(!is_loopback(lan));
        }
        for other in ["8.8.8.8:80", "0.0.0.0:8080", "172.32.0.1:80", "1.1.1.1:1"] {
            assert!(!is_lan(other), "{other} 不属局域网范围");
        }
        // 公网/通配地址拒绝启动（即使开启了认证）。
        for addr in ["8.8.8.8:8080", "0.0.0.0:8080"] {
            let config = BindConfig {
                addr: addr.to_string(),
                auth_enabled: true,
                origin_check_enabled: true,
            };
            let err = bind(&config).expect_err("公网/通配必须拒绝（ADR-023）");
            assert_eq!(err.code, ErrorCode::InvalidState);
            assert!(err.message.contains("局域网"), "{}", err.message);
        }
    }

    #[test]
    fn external_listener_without_auth_is_refused() {
        // V-110：对外监听未开启认证/Origin → 拒绝启动（FR-606）。
        let config = BindConfig {
            addr: "192.168.1.10:8080".to_string(),
            auth_enabled: false,
            origin_check_enabled: false,
        };
        let err = bind(&config).expect_err("对外监听必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable);
        assert!(err.to_string().contains("V-110"), "{err}");

        // 显式开启后可绑定（本机未必有 192.168 地址——绑定失败属环境限制，
        // 用回环+显式开启验证门禁放行路径）。
        let config = BindConfig {
            addr: "127.0.0.1:0".to_string(),
            auth_enabled: true,
            origin_check_enabled: true,
        };
        assert!(bind(&config).is_ok(), "开启认证与 Origin 后监听放行");
    }

    #[test]
    fn loopback_default_binds_without_auth() {
        let listener = bind(&BindConfig::loopback()).expect("回环默认可绑定");
        assert!(listener.local_addr().is_ok());
        assert!(is_loopback(DEFAULT_BIND));
        assert!(is_loopback("127.0.0.1:1"));
        assert!(!is_loopback("10.0.0.1:1"));
    }

    #[test]
    fn malformed_addresses_are_rejected() {
        for addr in [
            "nonsense",
            "127.0.0.1",
            "999.1.1.1:80",
            "127.0.0.1:notaport",
        ] {
            let config = BindConfig {
                addr: addr.to_string(),
                ..BindConfig::loopback()
            };
            let err = bind(&config).expect_err("非法地址必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState, "地址 {addr}");
        }
    }
}
