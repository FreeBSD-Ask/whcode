//! # 工作区 TLS 证书库：设置类型、状态报告与容量上限（ADR-040、T186）
//!
//! 本模块只承载**纯数据类型、校验与常量**：不读写文件、不发起网络、不引入任何加密依赖。
//! 证书库编排（原子写入、种子清单、自签生成、指纹与有效期解析）属 `whcode-app`，
//! 出站信任装配属 `whcode-llm`（`http` feature），入站握手属装配根 `whcode-bin`
//! （ADR-040“代码归属”——握手不迁入 `whcode-web`，以免该前端 crate 获得 `rustls`）。
//!
//! 两个不变量在此处落为类型约束，而非注释约定：
//!
//! 1. **永不关闭校验**（ADR-040 决策 2）：出站只有 `builtin`／`system` 两档，不存在
//!    “信任所有”档位；入站的 `off` 只关闭 TLS 本身（回到 ADR-023 明文），不是跳过证书校验。
//! 2. **未知档位不得静默回落**：[`TrustMode::parse`]／[`InboundMode::parse`] 严格匹配稳定
//!    字符串，未知输入返回 `parse_error`；serde 反序列化同样失败（无 `#[serde(other)]` 兜底）。
//!
//! 上限即拒绝阈值（ADR-040 决策 1）：解析前判定，超限报错，不截断。

use serde::{Deserialize, Serialize};

use crate::error::WhError;

/// 出站信任档位（ADR-040 决策 2）：两档都不关闭证书链与主机名校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustMode {
    /// 工作区内置根证书副本（`ca-builtin.pem`）为基线，`ca-extra.pem` 追加。
    Builtin,
    /// 操作系统证书库为基线，`ca-extra.pem` 追加。
    System,
}

impl Default for TrustMode {
    fn default() -> Self {
        Self::Builtin
    }
}

impl TrustMode {
    /// 稳定字符串（`settings.json` 取值、状态响应与审计字段共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::System => "system",
        }
    }

    /// 严格解析稳定字符串；未知取值报错，不回落默认档位（ADR-040 决策 2）。
    ///
    /// # Errors
    ///
    /// 未知档位名返回 [`crate::ErrorCode::ParseError`]，消息列出合法取值。
    pub fn parse(value: &str) -> Result<Self, WhError> {
        match value {
            "builtin" => Ok(Self::Builtin),
            "system" => Ok(Self::System),
            other => Err(WhError::parse_error(format!(
                "未知出站信任档位“{other}”；合法取值为 builtin 或 system（不得静默回落）"
            ))),
        }
    }
}

/// 入站 Web TLS 档位（ADR-040 决策 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InboundMode {
    /// 每工作区自签证书（首次运行生成，ECDSA P-256，397 天）。
    Builtin,
    /// 用户提供的 PEM 证书链与未加密私钥。
    External,
    /// 明文 HTTP（ADR-023 现状；显式回退，不被静默改回）。
    Off,
}

impl Default for InboundMode {
    fn default() -> Self {
        Self::Builtin
    }
}

impl InboundMode {
    /// 稳定字符串（`settings.json` 取值、`serve --tls` 参数与状态响应共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::External => "external",
            Self::Off => "off",
        }
    }

    /// 严格解析稳定字符串；未知取值报错，不回落默认档位。
    ///
    /// # Errors
    ///
    /// 未知档位名返回 [`crate::ErrorCode::ParseError`]，消息列出合法取值。
    pub fn parse(value: &str) -> Result<Self, WhError> {
        match value {
            "builtin" => Ok(Self::Builtin),
            "external" => Ok(Self::External),
            "off" => Ok(Self::Off),
            other => Err(WhError::parse_error(format!(
                "未知入站 TLS 档位“{other}”；合法取值为 builtin、external 或 off（不得静默回落）"
            ))),
        }
    }
}

// ── 证书库文件名（`<workspace>/.whcode/certs/`；ADR-040 决策 1） ──────────────
/// 证书库设置文件。
pub const SETTINGS_FILE: &str = "settings.json";
/// 可编辑的内置根证书副本（PEM 串接）。
pub const BUILTIN_CA_FILE: &str = "ca-builtin.pem";
/// 用户追加根证书（两档位都生效）。
pub const EXTRA_CA_FILE: &str = "ca-extra.pem";
/// 种子清单：来源版本、SHA-256、是否被修改。
pub const SEED_MANIFEST_FILE: &str = "ca-builtin.json";
/// 入站证书链（`external` 档）。
pub const SERVER_CERT_FILE: &str = "server-cert.pem";
/// 入站私钥（`external` 档，未加密；只进不出）。
pub const SERVER_KEY_FILE: &str = "server-key.pem";

// ── 容量上限（拒绝阈值，不是建议值；ADR-040 决策 1） ─────────────────────────
/// CA 单文件上限（内置副本与追加文件各自适用）。
pub const CA_BUNDLE_MAX_BYTES: usize = 512 * 1024;
/// 服务端证书链上限。
pub const SERVER_CERT_MAX_BYTES: usize = 64 * 1024;
/// 服务端私钥上限。
pub const SERVER_KEY_MAX_BYTES: usize = 16 * 1024;
/// 单次证书 API 请求体总长上限（解析前判定）。
pub const CERT_JSON_MAX_BYTES: usize = 640 * 1024;
/// 证书条目数上限（份数，不是字节数）。
pub const MAX_CERT_COUNT: usize = 1024;
/// 证书库设置 schema 版本（本模块 [`TlsSettings`] 唯一接受的版本）。
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
/// 内置自签证书有效期（天）。
pub const CERT_VALIDITY_DAYS: i64 = 397;
/// 到期告警门槛（剩余天数小于该值时告警；已过期由调用方判定）。
pub const CERT_RENEW_WARN_DAYS: i64 = 30;

/// 工作区证书库设置（`settings.json`；ADR-040 决策 1／2／3）。
///
/// 默认值两侧均启用（出站 `builtin`、入站 `builtin`），schema 版本为
/// [`SETTINGS_SCHEMA_VERSION`]。缺字段按默认值补齐（容器级 `#[serde(default)]`），
/// 但显式写错版本或档位名会在 [`TlsSettings::validate`] 或反序列化时报错。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsSettings {
    /// 设置 schema 版本（必须等于 [`SETTINGS_SCHEMA_VERSION`]）。
    pub schema_version: u32,
    /// 出站信任档位。
    pub outbound: TrustMode,
    /// 入站 Web TLS 档位。
    pub inbound: InboundMode,
}

impl Default for TlsSettings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            outbound: TrustMode::default(),
            inbound: InboundMode::default(),
        }
    }
}

impl TlsSettings {
    /// 校验 schema 版本；未知版本必须拒绝，不得按当前口径解释旧／新格式文件。
    ///
    /// # Errors
    ///
    /// `schema_version` 不等于 [`SETTINGS_SCHEMA_VERSION`] 时返回
    /// [`crate::ErrorCode::ParseError`]，消息给出当前值与期望值及可操作的补救方向。
    pub fn validate(&self) -> Result<(), WhError> {
        if self.schema_version != SETTINGS_SCHEMA_VERSION {
            return Err(WhError::parse_error(format!(
                "证书库设置 schema_version 不受支持（当前 {}，期望 {}）；\
                 请升级 WHCode 或执行证书库 reset 回到内置默认",
                self.schema_version, SETTINGS_SCHEMA_VERSION
            )));
        }
        Ok(())
    }
}

/// 单个证书文件的公共状态（份数、指纹与“已修改／有更新”标记）。
///
/// 不含 PEM 正文：状态报告只给摘要，避免把证书内容与私钥带入响应、日志或事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertFileStatus {
    /// 文件路径（供用户直接编辑与排障）。
    pub path: String,
    /// 可解析证书份数。
    pub cert_count: usize,
    /// 文件字节的 SHA-256（小写十六进制）。
    pub sha256: String,
    /// 副本自种入后是否被用户修改（`ca-builtin.pem` 用于保真刷新判定）。
    pub modified: bool,
    /// 种子清单记录的来源版本（仅内置副本）。
    pub bundled_version: Option<String>,
    /// 种子清单记录的种子 SHA-256（仅内置副本）。
    pub bundled_sha256: Option<String>,
    /// 内置资产比工作区副本更新（已修改时只提示，不覆盖）。
    pub newer_bundled_available: bool,
}

/// 出站信任状态（两档位共同字段 + 系统库数量）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundStatus {
    /// 当前档位。
    pub mode: TrustMode,
    /// 信任来源（`builtin`／`system`）。
    pub source: String,
    /// 内置根证书副本状态（`system` 档位下为 `None`）。
    pub builtin: Option<CertFileStatus>,
    /// 追加根证书状态（文件不存在时为 `None`）。
    pub extra: Option<CertFileStatus>,
    /// 操作系统证书库证书数（`builtin` 档位下为 `None`；读库失败不伪装成 0）。
    pub system_root_count: Option<usize>,
    /// 生效根证书总数（基线份数 + 追加份数）。
    pub effective_root_count: Option<usize>,
}

/// 入站服务端证书状态（ADR-040 决策 3；不含私钥内容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerCertStatus {
    /// 当前档位。
    pub mode: InboundMode,
    /// 证书链文件是否存在且可解析。
    pub cert_present: bool,
    /// 私钥文件是否存在（只报告存在性，永不返回内容）。
    pub key_present: bool,
    /// 证书主题（`external` 与 `builtin` 均适用）。
    pub subject: Option<String>,
    /// SAN 集合（`builtin` 应含 `localhost`、`127.0.0.1`、`::1`、本机名与绑定 IP）。
    pub sans: Vec<String>,
    /// 生效时间（ISO 8601）。
    pub not_before: Option<String>,
    /// 到期时间（ISO 8601）。
    pub not_after: Option<String>,
    /// 证书 SHA-256（小写十六进制）。
    pub sha256: Option<String>,
    /// 距到期的剩余天数（负值表示已过期；`builtin` 过期即重新生成，`external` 不自动替换）。
    pub days_remaining: Option<i64>,
}

/// 证书库整体状态（`GET /api/certs` 载荷；ADR-040 决策 4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsStatus {
    /// 设置 schema 版本。
    pub schema_version: u32,
    /// 出站信任状态。
    pub outbound: OutboundStatus,
    /// 入站服务端证书状态。
    pub inbound: ServerCertStatus,
    /// 档位或证书变更需重启 `serve --web` 才生效（不热切换监听）。
    pub restart_required: bool,
    /// 面向用户的提示（到期警告、“已修改且内置版本较新”、系统库为空等），可空。
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use serde_json::json;

    /// 默认值：两侧均启用（出站 `builtin`、入站 `builtin`），schema 版本为 1。
    #[test]
    fn defaults_enable_both_directions() {
        let settings = TlsSettings::default();
        assert_eq!(settings.schema_version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(settings.outbound, TrustMode::Builtin);
        assert_eq!(settings.inbound, InboundMode::Builtin);
        assert_eq!(TrustMode::default(), TrustMode::Builtin);
        assert_eq!(InboundMode::default(), InboundMode::Builtin);
        assert_eq!(SETTINGS_SCHEMA_VERSION, 1);
        settings.validate().expect("默认值合法");
    }

    /// 稳定字符串与 serde 往返（`lowercase`，与 `settings.json` 取值一致）。
    #[test]
    fn modes_serialize_and_roundtrip_stably() {
        for (mode, name) in [
            (TrustMode::Builtin, "builtin"),
            (TrustMode::System, "system"),
        ] {
            assert_eq!(mode.as_str(), name);
            assert_eq!(serde_json::to_value(mode).expect("序列化"), json!(name));
            let restored: TrustMode = serde_json::from_value(json!(name)).expect("反序列化");
            assert_eq!(restored, mode);
        }
        for (mode, name) in [
            (InboundMode::Builtin, "builtin"),
            (InboundMode::External, "external"),
            (InboundMode::Off, "off"),
        ] {
            assert_eq!(mode.as_str(), name);
            assert_eq!(serde_json::to_value(mode).expect("序列化"), json!(name));
            let restored: InboundMode = serde_json::from_value(json!(name)).expect("反序列化");
            assert_eq!(restored, mode);
        }

        let settings = TlsSettings {
            schema_version: SETTINGS_SCHEMA_VERSION,
            outbound: TrustMode::System,
            inbound: InboundMode::Off,
        };
        let text = serde_json::to_string(&settings).expect("序列化");
        let restored: TlsSettings = serde_json::from_str(&text).expect("反序列化");
        assert_eq!(settings, restored);

        // 缺字段按默认值补齐（旧文件不因新增字段而失效）。
        let partial: TlsSettings = serde_json::from_str(r#"{"outbound": "system"}"#)
            .expect("缺字段取默认值");
        assert_eq!(partial.outbound, TrustMode::System);
        assert_eq!(partial.inbound, InboundMode::Builtin);
        assert_eq!(partial.schema_version, SETTINGS_SCHEMA_VERSION);
    }

    /// 未知档位名解析失败，不静默回落（ADR-040 决策 2）。
    #[test]
    fn unknown_mode_names_are_rejected() {
        for value in ["", "Builtin", "BUILTIN", "insecure", "system "] {
            let err = TrustMode::parse(value).expect_err("未知出站档位必须报错");
            assert_eq!(err.code, ErrorCode::ParseError, "{value:?}");
            assert!(!err.retryable, "档位名错误属输入问题，不可重试");
            assert!(err.message.contains("builtin"), "{}", err.message);

            let err = InboundMode::parse(value).expect_err("未知入站档位必须报错");
            assert_eq!(err.code, ErrorCode::ParseError, "{value:?}");
            assert!(err.message.contains("external"), "{}", err.message);
        }
        assert_eq!(TrustMode::parse("builtin").expect("合法"), TrustMode::Builtin);
        assert_eq!(InboundMode::parse("off").expect("合法"), InboundMode::Off);
    }

    /// serde 侧同样严格：未知档位反序列化失败，不回落到默认值。
    #[test]
    fn unknown_mode_values_fail_deserialization() {
        assert!(serde_json::from_value::<TrustMode>(json!("trust-all")).is_err());
        assert!(serde_json::from_value::<TrustMode>(json!("Builtin")).is_err());
        assert!(serde_json::from_value::<InboundMode>(json!("disabled")).is_err());
        assert!(serde_json::from_str::<TlsSettings>(r#"{"inbound": "maybe"}"#).is_err());
    }

    /// 上限常量关系：证书 ⊆ CA 单文件上限，私钥 < 证书，JSON 总长 > CA 单文件。
    #[test]
    fn size_limits_are_consistent() {
        assert!(SERVER_CERT_MAX_BYTES <= CA_BUNDLE_MAX_BYTES);
        assert!(SERVER_KEY_MAX_BYTES < SERVER_CERT_MAX_BYTES);
        assert!(CERT_JSON_MAX_BYTES > CA_BUNDLE_MAX_BYTES);
        assert!(MAX_CERT_COUNT > 0);
        assert!(CERT_VALIDITY_DAYS > 0);
        assert!(CERT_RENEW_WARN_DAYS > 0 && CERT_RENEW_WARN_DAYS < CERT_VALIDITY_DAYS);
        // 文件名稳定：`settings.json` 与种子清单共用同一目录、不同文件。
        assert_ne!(SETTINGS_FILE, SEED_MANIFEST_FILE);
        assert!(BUILTIN_CA_FILE.ends_with(".pem"));
        assert!(EXTRA_CA_FILE.ends_with(".pem"));
        assert!(SERVER_CERT_FILE.ends_with(".pem"));
        assert!(SERVER_KEY_FILE.ends_with(".pem"));
    }

    /// `validate` 拒绝错误 schema 版本，错误可操作。
    #[test]
    fn validate_rejects_unsupported_schema_version() {
        for version in [0_u32, 2, u32::MAX] {
            let settings = TlsSettings {
                schema_version: version,
                ..TlsSettings::default()
            };
            let err = settings.validate().expect_err("未知 schema 版本必须拒绝");
            assert_eq!(err.code, ErrorCode::ParseError);
            assert!(err.message.contains("schema_version"), "{}", err.message);
            assert!(err.message.contains("reset"), "{}", err.message);
        }
    }

    /// 状态报告可序列化，且字段足以表达 spec 场景（份数、哈希、修改标记、系统库数量、
    /// SAN、有效期、重启提示）。
    #[test]
    fn status_reports_serialize_with_required_fields() {
        let status = TlsStatus {
            schema_version: SETTINGS_SCHEMA_VERSION,
            outbound: OutboundStatus {
                mode: TrustMode::Builtin,
                source: TrustMode::Builtin.as_str().to_string(),
                builtin: Some(CertFileStatus {
                    path: "certs/ca-builtin.pem".to_string(),
                    cert_count: 121,
                    sha256: "0".repeat(64),
                    modified: false,
                    bundled_version: Some("2026-10".to_string()),
                    bundled_sha256: Some("1".repeat(64)),
                    newer_bundled_available: false,
                }),
                extra: None,
                system_root_count: None,
                effective_root_count: Some(121),
            },
            inbound: ServerCertStatus {
                mode: InboundMode::Builtin,
                cert_present: true,
                key_present: true,
                subject: Some("CN=WHCode Local".to_string()),
                sans: vec!["localhost".to_string(), "127.0.0.1".to_string()],
                not_before: Some("2026-10-25T00:00:00Z".to_string()),
                not_after: Some("2027-11-26T00:00:00Z".to_string()),
                sha256: Some("2".repeat(64)),
                days_remaining: Some(CERT_VALIDITY_DAYS),
            },
            restart_required: true,
            notes: vec!["入站档位变更需重启 serve --web 后生效".to_string()],
        };
        let value = serde_json::to_value(&status).expect("序列化");
        assert_eq!(value["schema_version"], json!(SETTINGS_SCHEMA_VERSION));
        assert_eq!(value["outbound"]["mode"], json!("builtin"));
        assert_eq!(value["outbound"]["builtin"]["cert_count"], json!(121));
        assert_eq!(value["outbound"]["builtin"]["modified"], json!(false));
        assert_eq!(value["inbound"]["mode"], json!("builtin"));
        assert_eq!(value["inbound"]["sans"][0], json!("localhost"));
        assert_eq!(value["restart_required"], json!(true));
        assert_eq!(value["notes"].as_array().map(Vec::len), Some(1));

        let restored: TlsStatus = serde_json::from_value(value).expect("状态可反序列化");
        assert_eq!(restored, status);
    }
}
