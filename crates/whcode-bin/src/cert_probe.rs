//! # 出站连通性探针（T190、ADR-040 决策 4）
//!
//! 把 `whcode-app` 的 [`CertProbe`] 抽象落到装配根：一次只握手校验走
//! `whcode_llm::http::tls::probe`（与出站 Provider 共用同一套信任装配与校验口径），
//! 叶子证书主题／SAN／到期／指纹由 `whcode_app::cert::describe_cert_der` 解析。
//!
//! 边界（ADR-031／ADR-040“代码归属”）：
//!
//! - **网络依赖只在装配根与 `whcode-llm`**：`whcode-web` 只见 trait，不引入 `rustls`；
//! - **不发请求、不带凭据**：只完成 TLS 握手，不发送 HTTP 请求；
//! - **信任材料由调用方逐次读取**（`POST /api/certs/verify` 每次经
//!   `whcode_app::cert::trust_material` 读取当前文件），出站 CA 变更立即生效，
//!   不缓存、不种入、不写文件；
//! - **有界**：握手预算由 `whcode_llm::http::tls::PROBE_TIMEOUT`（2 秒）约束；
//! - **不泄漏秘密**：错误消息只含主机、端口与原因，不含私钥或凭据。

use whcode_app::cert::{CertProbe, ProbeReport, TrustMaterial, describe_cert_der};
use whcode_domain::WhError;

/// 以出站信任做一次只握手校验的探针（零状态：信任材料由调用方逐次读取）。
#[derive(Debug, Clone, Copy, Default)]
pub struct LlmCertProbe;

impl CertProbe for LlmCertProbe {
    fn probe(
        &self,
        host: &str,
        port: u16,
        material: &TrustMaterial,
    ) -> Result<ProbeReport, WhError> {
        let outcome = whcode_llm::http::tls::probe(
            host,
            port,
            material.mode,
            &material.builtin_pem,
            &material.extra_pem,
        )?;
        let leaf = outcome.leaf_chain_der.first().ok_or_else(|| {
            WhError::invalid_state(format!(
                "与 {host}:{port} 的 TLS 握手完成但未取得叶子证书：无法给出主题与指纹；\
                 请确认对端为标准 TLS 服务"
            ))
        })?;
        let meta = describe_cert_der(leaf)?;
        Ok(ProbeReport {
            host: host.to_string(),
            port,
            subject: Some(meta.subject),
            sans: meta.sans,
            not_after: meta.not_after,
            sha256: Some(meta.sha256),
            tls_version: outcome.tls_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::{ErrorCode, TrustMode};

    /// 非法主机名在建立连接前即被拒绝：错误可操作、不含私钥，且说明改用什么取值。
    #[test]
    fn invalid_host_fails_before_connecting() {
        let material = TrustMaterial {
            mode: TrustMode::Builtin,
            builtin_pem: whcode_app::cert::BUILTIN_CA_BUNDLE.to_string(),
            extra_pem: String::new(),
        };
        let err = LlmCertProbe
            .probe("not a host name", 443, &material)
            .expect_err("非法主机名必须拒绝（不做网络访问）");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("主机"), "{}", err.message);
        assert!(
            !err.message.contains("PRIVATE KEY"),
            "错误消息不得含私钥：{}",
            err.message
        );
    }

    /// 空信任集拒绝（不关闭校验、不回落）：`builtin` 档位不得以空信任集握手。
    #[test]
    fn empty_builtin_trust_material_is_rejected() {
        let material = TrustMaterial {
            mode: TrustMode::Builtin,
            builtin_pem: String::new(),
            extra_pem: String::new(),
        };
        let err = LlmCertProbe
            .probe("127.0.0.1", 443, &material)
            .expect_err("空信任集必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("ca-builtin.pem"), "{}", err.message);
    }
}
