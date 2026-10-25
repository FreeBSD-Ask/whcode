//! # 工作区证书库编排（T187、ADR-040 决策 1／2／3）
//!
//! 本模块把 `<workspace>/.whcode/certs/` 下的六个明文文件变成可见、可编辑、可回退的
//! 配置：初始化与保真刷新、CA 读取／写入／重置、入站自签证书生成与重新生成、外部
//! 证书对校验、状态投影与变更审计。
//!
//! 边界：
//!
//! - **不写 SQL**：审计经 [`CertAudit`] 注入，默认实现 [`StoreAudit`] 调用
//!   `whcode_store::insert_record`（ADR-018）；本模块只编排文件与解析。
//! - **不发网络**：`rcgen`／`x509-parser`／`sha2` 均为本地密码学与解析，
//!   握手与信任装配属 `whcode-llm`／`whcode-bin`（ADR-040）。
//! - **不泄漏秘密**：状态、审计与错误消息都不含 PEM 正文与私钥内容；私钥只进不出。
//! - **有界**：字节上限在解析前判定（[`CA_BUNDLE_MAX_BYTES`] 等），份数上限为
//!   [`MAX_CERT_COUNT`]，读取经 [`read_limited`] 截断到 `上限 + 1`，不无界累积。
//! - **原子**：同目录临时文件 + `sync_all` + `rename`；失败清理临时文件；审计失败或
//!   替换失败时恢复原字节，不产生半完成配置。

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use duckdb::Connection;
use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PublicKeyData,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::Duration;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use whcode_domain::{
    AuditAction, AuditRecord, BUILTIN_CA_FILE, CA_BUNDLE_MAX_BYTES, CERT_RENEW_WARN_DAYS,
    CERT_VALIDITY_DAYS, CertFileStatus, EXTRA_CA_FILE, InboundMode, MAX_CERT_COUNT, OutboundStatus,
    SEED_MANIFEST_FILE, SERVER_CERT_FILE, SERVER_CERT_MAX_BYTES, SERVER_KEY_FILE,
    SERVER_KEY_MAX_BYTES, SETTINGS_FILE, ServerCertStatus, TlsSettings, TlsStatus, TrustMode,
    WhError,
};

use crate::authz::{Actor, Capability, Policy, require};

/// 内置根证书资产（Task 2 入库；ADR-040 决策 2）。
pub const BUILTIN_CA_BUNDLE: &str = include_str!("../../../assets/certs/ca-bundle.pem");
/// 内置证书包版本（与 `assets/certs/SOURCES.md` 的上游版本日期一致）。
pub const BUILTIN_BUNDLE_VERSION: &str = "2026-10-20";
/// 种子清单里的来源描述（不含任何凭据）。
pub const BUILTIN_BUNDLE_SOURCE: &str =
    "https://curl.se/ca/cacert.pem（Mozilla 根证书库导出，ADR-040 决策 2）";
/// 种子清单 schema 版本。
pub const SEED_MANIFEST_SCHEMA_VERSION: u32 = 1;
/// `settings.json` 上限（ADR-040 决策 1 增补的设计上限）。
pub const SETTINGS_MAX_BYTES: usize = 16 * 1024;
/// `ca-builtin.json` 上限（ADR-040 决策 1 增补的设计上限）。
pub const SEED_MANIFEST_MAX_BYTES: usize = 16 * 1024;
/// 自签证书默认 SAN（ADR-040 决策 3；调用方未提供的项由 [`effective_sans`] 补齐）。
pub const DEFAULT_SANS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// 工作区证书库路径集合（唯一落点 `<workspace>/.whcode/certs/`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertPaths {
    /// 证书库目录。
    pub dir: PathBuf,
}

impl CertPaths {
    /// 由工作区根路径构造（`<workspace>/.whcode/certs`；ADR-040 决策 1）。
    #[must_use]
    pub fn for_workspace(workspace: &Path) -> Self {
        Self {
            dir: whcode_platform::workspace_certs_dir(workspace),
        }
    }

    /// `settings.json`。
    #[must_use]
    pub fn settings(&self) -> PathBuf {
        self.dir.join(SETTINGS_FILE)
    }

    /// `ca-builtin.pem`（可编辑的内置根证书副本）。
    #[must_use]
    pub fn builtin_ca(&self) -> PathBuf {
        self.dir.join(BUILTIN_CA_FILE)
    }

    /// `ca-extra.pem`（用户追加根证书）。
    #[must_use]
    pub fn extra_ca(&self) -> PathBuf {
        self.dir.join(EXTRA_CA_FILE)
    }

    /// `ca-builtin.json`（种子清单）。
    #[must_use]
    pub fn seed_manifest(&self) -> PathBuf {
        self.dir.join(SEED_MANIFEST_FILE)
    }

    /// `server-cert.pem`（入站证书链）。
    #[must_use]
    pub fn server_cert(&self) -> PathBuf {
        self.dir.join(SERVER_CERT_FILE)
    }

    /// `server-key.pem`（入站私钥，未加密，只进不出）。
    #[must_use]
    pub fn server_key(&self) -> PathBuf {
        self.dir.join(SERVER_KEY_FILE)
    }
}

/// 种子清单（`ca-builtin.json`）：记录来源版本与种入／当前哈希，用于保真刷新判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedManifest {
    /// 清单 schema 版本。
    pub schema_version: u32,
    /// 来源描述（不含凭据）。
    pub source: String,
    /// 内置资产版本。
    pub bundled_version: String,
    /// 种入时内置资产的 SHA-256。
    pub bundled_sha256: String,
    /// 种入时写入工作区副本的 SHA-256（保真刷新的比较基准）。
    pub seeded_sha256: String,
    /// 种入／刷新时间（Unix 毫秒）。
    pub seeded_at_ms: u64,
}

/// 初始化报告：本次创建的文件、是否刷新内置副本、是否重新生成自签证书与提示。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitReport {
    /// 本次创建的文件名（不含目录）。
    pub created: Vec<String>,
    /// 内置根证书副本是否随内置资产刷新。
    pub refreshed_builtin_ca: bool,
    /// 入站自签证书与私钥是否重新生成（过期、SAN 不覆盖或缺失时）。
    pub regenerated_server_cert: bool,
    /// 面向用户的提示（保真刷新、已修改保留、到期警告等）。
    pub notes: Vec<String>,
}

/// CA 文件种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaKind {
    /// `ca-builtin.pem`。
    Builtin,
    /// `ca-extra.pem`。
    Extra,
}

impl CaKind {
    /// 稳定字符串（审计与错误消息使用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Extra => "extra",
        }
    }

    /// 文件名。
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Builtin => BUILTIN_CA_FILE,
            Self::Extra => EXTRA_CA_FILE,
        }
    }
}

/// CA 文件摘要（份数、文件字节 SHA-256、重复证书数）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaSummary {
    /// 可解析证书份数。
    pub cert_count: usize,
    /// 文件字节的 SHA-256（小写十六进制）。
    pub sha256: String,
    /// 重复证书份数（仅告警，不拒绝）。
    pub duplicate_count: usize,
}

/// CA 文件读取结果（`GET /api/certs/ca` 的载荷来源；T190、ADR-040 决策 4）。
///
/// `pem` 是公开材料（根证书），可下载与编辑；`modified` 对追加文件恒为 `false`
/// （追加证书没有内置版本，不存在“被修改”判定）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaReport {
    /// 文件完整 PEM 文本。
    pub pem: String,
    /// 可解析证书份数。
    pub cert_count: usize,
    /// 文件字节的 SHA-256（小写十六进制）。
    pub sha256: String,
    /// 内置副本自种入后是否被修改（仅 `ca-builtin.pem` 有意义）。
    pub modified: bool,
}

/// 审计写入抽象：app 不写 SQL，默认实现经 `whcode-store`。
pub trait CertAudit {
    /// 写入一条审计；失败必须返回错误，调用方据此回滚文件。
    ///
    /// # Errors
    ///
    /// 存储不可用、主键冲突等由实现方映射为 [`WhError`]。
    fn record(&self, conn: &Connection, record: &AuditRecord) -> Result<(), WhError>;
}

/// 默认审计实现：调用 [`whcode_store::insert_record`]。
#[derive(Debug, Clone, Copy, Default)]
pub struct StoreAudit;

impl CertAudit for StoreAudit {
    fn record(&self, conn: &Connection, record: &AuditRecord) -> Result<(), WhError> {
        whcode_store::insert_record(conn, record)
    }
}

/// 确保证书库可用（幂等）：创建目录与缺失文件、保真刷新内置副本、按需生成自签证书。
///
/// 行为（ADR-040 决策 1／3）：
///
/// - `settings.json` 缺失写默认（出站 `builtin`、入站 `builtin`）；存在则解析并
///   [`TlsSettings::validate`]，损坏时返回可操作 `parse_error`，不以默认值覆盖用户文件。
/// - `ca-builtin.pem` 缺失时从 [`BUILTIN_CA_BUNDLE`] 种入并写清单；`ca-extra.pem` 缺失写空文件。
/// - 保真刷新：副本哈希等于清单 `seeded_sha256` 且与内置资产不同时原子刷新并记录旧／新
///   哈希；副本已被修改则保留并提示，绝不覆盖；清单缺失／损坏时不覆盖用户文件。
/// - 入站 `builtin` 时确保证书存在、未过期且 SAN 覆盖 [`effective_sans`]；否则重新生成。
///
/// # Errors
///
/// 目录不可写、设置文件损坏、文件超过上限或自签证书生成失败时返回 [`WhError`]；
/// 失败路径不留下半完成文件。
pub fn ensure_initialized(
    paths: &CertPaths,
    required_sans: &[String],
    now_ms: u64,
) -> Result<InitReport, WhError> {
    let mut report = InitReport::default();
    create_dir_all(&paths.dir)?;
    let settings = ensure_settings(paths, &mut report)?;
    ensure_builtin_ca(paths, now_ms, &mut report)?;
    ensure_extra_ca(paths, &mut report)?;
    ensure_server_cert(paths, settings.inbound, required_sans, now_ms, &mut report)?;
    Ok(report)
}

/// 仅出站初始化（TUI／GUI／CUI 装配根使用）：创建目录、默认设置、内置副本与追加证书，
/// **不生成入站证书**（ADR-040 决策 1／2、T191）。
///
/// 与 [`ensure_initialized`] 的差别只在最后一步：本函数不写 `server-cert.pem`／
/// `server-key.pem`，也不读取入站档位——因此不会因入站设置损坏而阻断出站信任，
/// 也不会在只读前端启动时凭空生成一份自签私钥（入站材料只由 `serve --web` 按档位准备）。
/// 内置副本的种入、保真刷新与“已修改保留”口径与 [`ensure_initialized`] 完全一致。
///
/// # Errors
///
/// 目录不可写、`settings.json` 损坏（schema 或档位非法）、内置副本超限或种子清单读写失败时
/// 返回 [`WhError`]；失败路径不留下半完成文件。
pub fn ensure_outbound_initialized(paths: &CertPaths, now_ms: u64) -> Result<InitReport, WhError> {
    let mut report = InitReport::default();
    create_dir_all(&paths.dir)?;
    // 设置文件仍需解析与校验：出站档位是后续信任装配的唯一来源，损坏时必须可操作报错，
    // 不得按默认档位静默继续。
    ensure_settings(paths, &mut report)?;
    ensure_builtin_ca(paths, now_ms, &mut report)?;
    ensure_extra_ca(paths, &mut report)?;
    Ok(report)
}

/// 读取证书库状态（只读，不创建文件）。
///
/// 缺失、为空与解析失败经 `notes` 区分（含文件路径与原因），不以空值冒充“正确空”；
/// `restart_required` 由 Web 会话状态另行设置，本函数恒为 `false`。
///
/// # Errors
///
/// 当前实现不返回错误；签名保留 `Result` 以便后续状态项需要显式失败时使用。
pub fn status(paths: &CertPaths, now_ms: u64) -> Result<TlsStatus, WhError> {
    let mut notes = Vec::new();
    let settings = load_settings_with_notes(paths, &mut notes);
    let manifest = match load_seed_manifest(&paths.seed_manifest()) {
        Ok(manifest) => Some(manifest),
        Err(err) => {
            if paths.seed_manifest().exists() {
                notes.push(format!("种子清单不可用：{}", err.message));
            }
            None
        }
    };

    let builtin = if settings.outbound == TrustMode::Builtin {
        ca_status(
            &paths.builtin_ca(),
            CaKind::Builtin,
            manifest.as_ref(),
            &mut notes,
        )
    } else {
        None
    };
    let extra = ca_status(&paths.extra_ca(), CaKind::Extra, None, &mut notes);
    let system_count = if settings.outbound == TrustMode::System {
        match system_root_count() {
            Ok(0) => {
                notes.push(
                    "系统证书库读取成功但为 0 份证书：出站连接将失败；请确认 OS 根证书存储可用，\
                     或把出站档位改回 builtin（system 档位不回落内置库，ADR-040 决策 2）"
                        .to_string(),
                );
                Some(0)
            }
            Ok(count) => Some(count),
            Err(err) => {
                notes.push(format!(
                    "系统证书库不可用（份数记为未知，未伪装成 0）：{}",
                    err.message
                ));
                None
            }
        }
    } else {
        None
    };
    let effective_root_count = if settings.outbound == TrustMode::Builtin {
        Some(
            builtin.as_ref().map_or(0, |status| status.cert_count)
                + extra.as_ref().map_or(0, |status| status.cert_count),
        )
    } else {
        system_count.map(|count| count + extra.as_ref().map_or(0, |status| status.cert_count))
    };
    let outbound = OutboundStatus {
        mode: settings.outbound,
        source: settings.outbound.as_str().to_string(),
        builtin,
        extra,
        system_root_count: system_count,
        effective_root_count,
    };
    let inbound = server_status(paths, settings.inbound, now_ms, &mut notes);

    Ok(TlsStatus {
        schema_version: settings.schema_version,
        outbound,
        inbound,
        restart_required: false,
        notes,
    })
}

/// 读取操作系统证书库的证书份数（只读，不发网络请求；ADR-040 决策 2、T188）。
///
/// 读取成功但份数为 0 时同样返回 `Ok(0)`：由调用方决定是否报错——`system` 档位的信任装配
/// （`whcode-llm`）把 0 视为可操作错误且不回落内置库，状态投影则标注原因而不伪装成未知。
///
/// # Errors
///
/// 读库过程中出现任何错误（含部分失败）时返回 [`WhError`]：部分成功的集合会掩盖缺失的根，
/// 属“错误的空”，不得当作可用。
pub fn system_root_count() -> Result<usize, WhError> {
    let result = rustls_native_certs::load_native_certs();
    if let Some(first) = result.errors.first() {
        return Err(WhError::invalid_state(format!(
            "读取操作系统证书库失败（{} 个错误，首个：{first}）；请确认 OS 根证书存储可访问，\
             或把出站档位改回 builtin（system 档位不回落内置库，ADR-040 决策 2）",
            result.errors.len()
        )));
    }
    Ok(result.certs.len())
}

/// 读取设置：缺失返回默认值；存在但损坏或 schema 不受支持时报错。
///
/// 状态投影需要“缺失可区分”的路径，见 [`status`]（本函数不携带 notes）。
///
/// # Errors
///
/// 文件超过 16 KiB、JSON 解析失败或 `schema_version` 不受支持时返回 [`WhError`]。
pub fn load_settings(paths: &CertPaths) -> Result<TlsSettings, WhError> {
    let path = paths.settings();
    if !path.exists() {
        return Ok(TlsSettings::default());
    }
    parse_settings(&path)
}

/// 读取 CA 文件并校验（内置不得为空，追加可为空）。
///
/// `ca-extra.pem` 不存在时返回空串（表示“无追加”，不是错误）；内置副本不存在时报错。
///
/// # Errors
///
/// 文件缺失（内置）、超限、含私钥块、不可解析或内置为空时返回 [`WhError`]。
pub fn read_ca(paths: &CertPaths, kind: CaKind) -> Result<String, WhError> {
    let path = match kind {
        CaKind::Builtin => paths.builtin_ca(),
        CaKind::Extra => paths.extra_ca(),
    };
    if kind == CaKind::Extra && !path.exists() {
        return Ok(String::new());
    }
    let bytes = read_limited(&path, CA_BUNDLE_MAX_BYTES, kind.file_name())?;
    let text = text_of(bytes, &path)?;
    validate_ca_pem(&text, kind, MAX_CERT_COUNT, &path.display().to_string())?;
    Ok(text)
}

/// 读取 CA 文件并给出 API 形状摘要：完整 PEM、份数、文件 SHA-256 与“已修改”标记。
///
/// “已修改”仅对内置副本判定（与种子清单的 `seeded_sha256` 比较；清单缺失或损坏时退化为
/// “与内置资产字节不同”），追加文件恒为 `false`（T190、ADR-040 决策 4）。
///
/// # Errors
///
/// 与 [`read_ca`] 相同：内置缺失、超限、含私钥、不可解析或内置为空时返回 [`WhError`]。
pub fn describe_ca(paths: &CertPaths, kind: CaKind) -> Result<CaReport, WhError> {
    let pem = read_ca(paths, kind)?;
    let path = match kind {
        CaKind::Builtin => paths.builtin_ca(),
        CaKind::Extra => paths.extra_ca(),
    };
    let summary = validate_ca_pem(&pem, kind, MAX_CERT_COUNT, &path.display().to_string())?;
    let modified = match kind {
        CaKind::Extra => false,
        CaKind::Builtin => match load_seed_manifest(&paths.seed_manifest()) {
            Ok(manifest) => manifest.seeded_sha256 != summary.sha256,
            Err(_) => summary.sha256 != sha256_hex(BUILTIN_CA_BUNDLE.as_bytes()),
        },
    };
    Ok(CaReport {
        pem,
        cert_count: summary.cert_count,
        sha256: summary.sha256,
        modified,
    })
}

/// 入站服务端证书材料（证书链 PEM + 未加密私钥 PEM）。
///
/// 私钥只进不出：本结构只供装配根（`whcode-bin`）构造 `rustls::ServerConfig`，
/// 不得序列化、写入日志、事件或响应（ADR-040 决策 1／3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerMaterial {
    /// `server-cert.pem` 的完整 PEM 文本（叶子证书在前）。
    pub cert_pem: String,
    /// `server-key.pem` 的完整 PEM 文本（未加密，PKCS#8／PKCS#1／SEC1）。
    pub key_pem: String,
}

/// 读取受管的入站证书链与私钥（`builtin` 与 `external` 档位共用）。
///
/// 上限与写入路径同一口径（[`SERVER_CERT_MAX_BYTES`]／[`SERVER_KEY_MAX_BYTES`]）：
/// 解析前判定，超限报错不截断。缺失时错误列出两个文件路径与三条补救路径
/// （上传入口、直接写文件、`--tls off`），不得静默降级为明文。
///
/// # Errors
///
/// 证书或私钥缺失、超限、非 UTF-8 文本时返回 [`WhError`]（中文、可操作）。
pub fn read_server_material(paths: &CertPaths) -> Result<ServerMaterial, WhError> {
    let cert_path = paths.server_cert();
    let key_path = paths.server_key();
    let missing: Vec<String> = [&cert_path, &key_path]
        .iter()
        .filter(|path| !path.is_file())
        .map(|path| path.display().to_string())
        .collect();
    if !missing.is_empty() {
        return Err(WhError::invalid_state(format!(
            "入站证书材料缺失：{}；请上传匹配的证书链与未加密私钥（Web 设置页 /settings/certs，\
             或直接写入上述文件），或以 --tls off 显式回到明文 HTTP（ADR-040 决策 3）",
            missing.join("、")
        )));
    }
    let cert_bytes = read_limited(&cert_path, SERVER_CERT_MAX_BYTES, SERVER_CERT_FILE)?;
    let key_bytes = read_limited(&key_path, SERVER_KEY_MAX_BYTES, SERVER_KEY_FILE)?;
    Ok(ServerMaterial {
        cert_pem: text_of(cert_bytes, &cert_path)?,
        key_pem: text_of(key_bytes, &key_path)?,
    })
}

/// 保存证书库设置（入站 `off` 需显式确认），成功写一条 `tls_config_change` 审计。
///
/// 入站档位变化时返回状态 `restart_required = true`（不热切换监听，ADR-040 决策 3）；
/// 审计失败恢复原设置字节并返回错误。
///
/// # Errors
///
/// 能力缺失（`settings.cert.write`）、设置非法、入站 `off` 未确认、写盘或审计失败时返回
/// [`WhError`]。
#[allow(clippy::too_many_arguments)]
pub fn update_settings(
    conn: &Connection,
    paths: &CertPaths,
    policy: &dyn Policy,
    actor: &Actor,
    audit: &dyn CertAudit,
    new: TlsSettings,
    confirm: bool,
    now_ms: u64,
) -> Result<TlsStatus, WhError> {
    require(policy, actor, Capability::SettingsCertWrite)?;
    new.validate()?;
    let previous = load_settings(paths)?;
    if new.inbound == InboundMode::Off && !confirm {
        return Err(WhError::invalid_state(
            "将入站 TLS 设为 off 会回到明文 HTTP，属降档操作：必须显式确认（confirm=true）；\
             当前设置未改变",
        ));
    }
    let path = paths.settings();
    let snapshot = snapshot_file(&path, SETTINGS_MAX_BYTES, SETTINGS_FILE)?;
    let bytes = json_bytes(&new)?;
    if let Err(err) = atomic_write(&path, &bytes) {
        return Err(with_rollback(err, std::slice::from_ref(&snapshot)));
    }
    let mut next = match status(paths, now_ms) {
        Ok(status) => status,
        Err(err) => return Err(with_rollback(err, std::slice::from_ref(&snapshot))),
    };
    let detail = serde_json::json!({
        "action": "update-settings",
        "outbound": new.outbound.as_str(),
        "inbound": new.inbound.as_str(),
        "previous_outbound": previous.outbound.as_str(),
        "previous_inbound": previous.inbound.as_str(),
        "builtin_cert_count": next.outbound.builtin.as_ref().map_or(0, |item| item.cert_count),
        "extra_cert_count": next.outbound.extra.as_ref().map_or(0, |item| item.cert_count),
        "builtin_sha256": next.outbound.builtin.as_ref().and_then(|item| {
            (!item.sha256.is_empty()).then(|| item.sha256.clone())
        }),
    });
    let record = audit_record(actor, "tls:settings", detail, now_ms);
    audit_or_rollback(audit, conn, &record, std::slice::from_ref(&snapshot))?;

    next.restart_required = previous.inbound != new.inbound;
    if next.restart_required {
        next.notes.push(format!(
            "入站档位已由 {} 改为 {}，需重启 serve --web 后生效",
            previous.inbound.as_str(),
            new.inbound.as_str()
        ));
    }
    if new.inbound == InboundMode::Builtin && !next.inbound.cert_present {
        next.notes.push(
            "入站档位为 builtin 但自签证书缺失或不可用；重启 serve --web 时将重新生成".to_string(),
        );
    }
    Ok(next)
}

/// 写入 CA 文件（仅 CERTIFICATE 块），成功写一条 `tls_config_change` 审计。
///
/// `builtin` 为空属降档：未带 `confirm` 时先拒绝；即使带确认，空内置库仍被拒绝
/// （关闭出站信任应改用 `system` 档位，ADR-040 决策 2）。`extra` 为空表示无追加。
///
/// # Errors
///
/// 能力缺失、内容超限／含私钥／不可解析／份数超限、空内置、写盘或审计失败时返回
/// [`WhError`]；失败后原文件字节不变。
#[allow(clippy::too_many_arguments)]
pub fn write_ca(
    conn: &Connection,
    paths: &CertPaths,
    policy: &dyn Policy,
    actor: &Actor,
    audit: &dyn CertAudit,
    kind: CaKind,
    pem: &str,
    confirm: bool,
    now_ms: u64,
) -> Result<TlsStatus, WhError> {
    require(policy, actor, Capability::SettingsCertWrite)?;
    if kind == CaKind::Builtin && pem.trim().is_empty() && !confirm {
        return Err(WhError::invalid_state(format!(
            "写入空的 {} 会清空出站信任，属降档操作：必须显式确认（confirm=true）；\
             当前文件未改变",
            BUILTIN_CA_FILE
        )));
    }
    let path = match kind {
        CaKind::Builtin => paths.builtin_ca(),
        CaKind::Extra => paths.extra_ca(),
    };
    let summary =
        validate_ca_pem(pem, kind, MAX_CERT_COUNT, &path.display().to_string()).map_err(|err| {
            WhError::new(
                err.code,
                format!("{}（文件 {}）", err.message, path.display()),
            )
        })?;
    let snapshot = snapshot_file(&path, CA_BUNDLE_MAX_BYTES, kind.file_name())?;
    if let Err(err) = atomic_write(&path, pem.as_bytes()) {
        return Err(with_rollback(err, std::slice::from_ref(&snapshot)));
    }
    let mut next = match status(paths, now_ms) {
        Ok(status) => status,
        Err(err) => return Err(with_rollback(err, std::slice::from_ref(&snapshot))),
    };
    let detail = serde_json::json!({
        "action": "write-ca",
        "kind": kind.as_str(),
        "mode": next.outbound.mode.as_str(),
        "cert_count": summary.cert_count,
        "sha256": summary.sha256,
        "duplicate_count": summary.duplicate_count,
    });
    let record = audit_record(
        actor,
        &format!("tls:outbound-ca:{}", kind.as_str()),
        detail,
        now_ms,
    );
    audit_or_rollback(audit, conn, &record, std::slice::from_ref(&snapshot))?;
    if summary.duplicate_count > 0 {
        next.notes.push(format!(
            "{} 含 {} 份重复证书（仅告警，不拒绝）",
            path.display(),
            summary.duplicate_count
        ));
    }
    Ok(next)
}

/// 按种类重置 CA 文件，成功写一条 `tls_config_change` 审计。
///
/// - [`CaKind::Builtin`]：从 [`BUILTIN_CA_BUNDLE`] 恢复 `ca-builtin.pem` 并重写种子清单
///   （清除“已修改”标记）；**不动** `ca-extra.pem`。
/// - [`CaKind::Extra`]：把 `ca-extra.pem` 清空为 0 字节（追加证书没有内置版本，属“清空”
///   而不是“恢复内置”）。
///
/// # Errors
///
/// 能力缺失、写盘或审计失败时返回 [`WhError`]；失败恢复本次涉及文件的原字节。
pub fn reset_ca(
    conn: &Connection,
    paths: &CertPaths,
    policy: &dyn Policy,
    actor: &Actor,
    audit: &dyn CertAudit,
    kind: CaKind,
    now_ms: u64,
) -> Result<TlsStatus, WhError> {
    require(policy, actor, Capability::SettingsCertWrite)?;
    let mut snapshots: Vec<FileSnapshot> = Vec::new();
    let mut bundle_sha = None;
    match kind {
        CaKind::Builtin => {
            snapshots.push(snapshot_file(
                &paths.builtin_ca(),
                CA_BUNDLE_MAX_BYTES,
                BUILTIN_CA_FILE,
            )?);
            snapshots.push(snapshot_file(
                &paths.seed_manifest(),
                SEED_MANIFEST_MAX_BYTES,
                SEED_MANIFEST_FILE,
            )?);
            let sha = sha256_hex(BUILTIN_CA_BUNDLE.as_bytes());
            let manifest = seeded_manifest(&sha, &sha, now_ms);
            let written = (|| -> Result<(), WhError> {
                atomic_write(&paths.builtin_ca(), BUILTIN_CA_BUNDLE.as_bytes())?;
                atomic_write(&paths.seed_manifest(), &json_bytes(&manifest)?)
            })();
            if let Err(err) = written {
                return Err(with_rollback(err, &snapshots));
            }
            bundle_sha = Some(sha);
        }
        CaKind::Extra => {
            snapshots.push(snapshot_file(
                &paths.extra_ca(),
                CA_BUNDLE_MAX_BYTES,
                EXTRA_CA_FILE,
            )?);
            if let Err(err) = atomic_write(&paths.extra_ca(), b"") {
                return Err(with_rollback(err, &snapshots));
            }
        }
    }
    let mut next = match status(paths, now_ms) {
        Ok(status) => status,
        Err(err) => return Err(with_rollback(err, &snapshots)),
    };
    let detail = match kind {
        CaKind::Builtin => {
            let cert_count = validate_ca_pem(
                BUILTIN_CA_BUNDLE,
                CaKind::Builtin,
                MAX_CERT_COUNT,
                BUILTIN_CA_FILE,
            )?
            .cert_count;
            serde_json::json!({
                "action": "reset-ca",
                "kind": kind.as_str(),
                "mode": next.outbound.mode.as_str(),
                "cert_count": cert_count,
                "sha256": bundle_sha.clone(),
                "bundled_version": BUILTIN_BUNDLE_VERSION,
            })
        }
        CaKind::Extra => serde_json::json!({
            "action": "reset-ca",
            "kind": kind.as_str(),
            "mode": next.outbound.mode.as_str(),
            "cert_count": 0,
            "extra_count": next
                .outbound
                .extra
                .as_ref()
                .map_or(0, |item| item.cert_count),
        }),
    };
    let record = audit_record(
        actor,
        &format!("tls:outbound-ca:{}", kind.as_str()),
        detail,
        now_ms,
    );
    audit_or_rollback(audit, conn, &record, &snapshots)?;
    next.notes.push(match kind {
        CaKind::Builtin => format!(
            "内置根证书副本已恢复为内置资产（{}，SHA-256 {}）",
            BUILTIN_BUNDLE_VERSION,
            bundle_sha.as_deref().unwrap_or("未知")
        ),
        CaKind::Extra => format!(
            "追加根证书已清空（{} 为 0 字节）",
            paths.extra_ca().display()
        ),
    });
    Ok(next)
}

/// 保存外部服务端证书链与未加密私钥（需显式确认），成功写审计。
///
/// 证书链每份必须可解析，私钥必须为未加密 PKCS#8 且与叶子证书公钥一致（ADR-040 决策 3）；
/// 私钥写盘后调用 [`whcode_platform::restrict_private_file`]。入站材料替换需重启，
/// 返回状态 `restart_required = true`。
///
/// # Errors
///
/// 能力缺失、未确认、证书／私钥超限、私钥带口令、配对不一致、写盘或审计失败时返回
/// [`WhError`]；失败后原文件字节不变，不留下半份证书／私钥。
#[allow(clippy::too_many_arguments)]
pub fn write_server_cert(
    conn: &Connection,
    paths: &CertPaths,
    policy: &dyn Policy,
    actor: &Actor,
    audit: &dyn CertAudit,
    cert_pem: &str,
    key_pem: &str,
    confirm: bool,
    now_ms: u64,
) -> Result<TlsStatus, WhError> {
    require(policy, actor, Capability::SettingsCertWrite)?;
    if !confirm {
        return Err(WhError::invalid_state(
            "上传外部服务端私钥会替换入站证书材料，属降档／敏感操作：必须显式确认\
             （confirm=true）；当前文件未改变",
        ));
    }
    let pair = inspect_server_pair(cert_pem, key_pem)?;
    let snapshots = server_snapshots(paths)?;
    if let Err(err) = write_server_material(paths, cert_pem, key_pem) {
        return Err(with_rollback(err, &snapshots));
    }
    let mut next = match status(paths, now_ms) {
        Ok(status) => status,
        Err(err) => return Err(with_rollback(err, &snapshots)),
    };
    let detail = serde_json::json!({
        "action": "write-server-cert",
        "mode": next.inbound.mode.as_str(),
        "cert_count": pair.cert_count,
        "sha256": pair.leaf.sha256,
        "key_present": true,
        "subject": pair.leaf.subject,
    });
    let record = audit_record(actor, "tls:inbound-server", detail, now_ms);
    audit_or_rollback(audit, conn, &record, &snapshots)?;
    next.restart_required = true;
    next.notes
        .push("服务端证书材料已替换；需重启 serve --web 后生效（不热切换监听）".to_string());
    Ok(next)
}

/// 生成本工作区新的 ECDSA P-256 自签证书并替换现有材料，成功写审计。
///
/// `confirm` 为 API 形状统一参数：重新生成不属降档操作（ADR-040 决策 4 只要求
/// 入站 `off`、空内置根与外部私钥确认），因此不改变行为。SAN 为去重后的
/// [`effective_sans`]；入站材料替换需重启，返回状态 `restart_required = true`。
///
/// # Errors
///
/// 能力缺失、SAN 非法、生成失败、写盘或审计失败时返回 [`WhError`]；失败后原文件字节不变。
#[allow(clippy::too_many_arguments)]
pub fn regenerate_server_cert(
    conn: &Connection,
    paths: &CertPaths,
    policy: &dyn Policy,
    actor: &Actor,
    audit: &dyn CertAudit,
    required_sans: &[String],
    _confirm: bool,
    now_ms: u64,
) -> Result<TlsStatus, WhError> {
    require(policy, actor, Capability::SettingsCertWrite)?;
    let sans = effective_sans(required_sans);
    let (cert_pem, key_pem, leaf) = generate_self_signed(&sans, now_ms)?;
    let snapshots = server_snapshots_for_repair(paths)?;
    if let Err(err) = write_server_material(paths, &cert_pem, &key_pem) {
        return Err(with_rollback(err, &snapshots));
    }
    let mut next = match status(paths, now_ms) {
        Ok(status) => status,
        Err(err) => return Err(with_rollback(err, &snapshots)),
    };
    let detail = serde_json::json!({
        "action": "regenerate-server-cert",
        "mode": next.inbound.mode.as_str(),
        "cert_count": 1,
        "sha256": &leaf.sha256,
        "subject": &leaf.subject,
        "sans": &leaf.sans,
    });
    let record = audit_record(actor, "tls:inbound-server", detail, now_ms);
    audit_or_rollback(audit, conn, &record, &snapshots)?;
    next.restart_required = true;
    next.notes.push(format!(
        "已生成新的自签证书（指纹 {}，SAN {}）；需重启 serve --web 后生效",
        leaf.sha256,
        leaf.sans.join("、")
    ));
    Ok(next)
}

/// 校验外部服务端证书链与私钥：字节上限、仅 CERTIFICATE 块、未加密私钥、公钥配对。
///
/// # Errors
///
/// 证书链或私钥超限、私钥带口令、证书不可解析、仅零份证书、或证书与私钥不匹配时返回
/// [`WhError`]（中文、含文件与补救方向）。
pub fn validate_server_pair(cert_pem: &str, key_pem: &str) -> Result<(), WhError> {
    inspect_server_pair(cert_pem, key_pem).map(|_| ())
}

/// 入站自签证书的默认 SAN 集合（[`DEFAULT_SANS`] + 本机名；已去重）。
///
/// 供 Web 端“重新生成自签证书”合并现有 SAN 使用：生成逻辑仍由
/// [`regenerate_server_cert`] 的 [`effective_sans`] 统一补齐与去重（T190）。
#[must_use]
pub fn default_sans() -> Vec<String> {
    let mut sans = Vec::new();
    for item in DEFAULT_SANS {
        push_san(&mut sans, item);
    }
    if let Some(name) = whcode_platform::hostname() {
        push_san(&mut sans, &name);
    }
    sans
}

/// 一次出站连通性校验的信任材料（`POST /api/certs/verify` 的输入；ADR-040 决策 4）。
///
/// 只读：读取时不种入、不写文件；`builtin_pem` 为内置副本原文，`extra_pem` 为追加证书原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustMaterial {
    /// 出站档位（`builtin`／`system`）。
    pub mode: TrustMode,
    /// `ca-builtin.pem` 的完整文本。
    pub builtin_pem: String,
    /// `ca-extra.pem` 的完整文本（文件缺失视为空串）。
    pub extra_pem: String,
}

/// 读取当前出站信任材料（设定档位 + 两个 PEM 文件），不种入、不写文件。
///
/// 每次探测重新读取，出站 CA 变更立即生效（不需要重启 `serve --web`）。
///
/// # Errors
///
/// 设置文件损坏或超限、`ca-builtin.pem` 缺失（可操作错误，指向 reset／system 档位）、
/// 任一文件超限或非 UTF-8 时返回 [`WhError`]。
pub fn trust_material(paths: &CertPaths) -> Result<TrustMaterial, WhError> {
    let settings = load_settings(paths)?;
    let builtin_path = paths.builtin_ca();
    if !builtin_path.is_file() {
        return Err(WhError::invalid_state(format!(
            "内置根证书副本缺失：{}；请执行证书库初始化（serve --web 启动时自动创建）或\
             POST /api/certs/ca/reset 恢复内置资产，或把出站档位改为 system 使用操作系统证书库\
             （ADR-040 决策 2）",
            builtin_path.display()
        )));
    }
    let builtin_pem = text_of(
        read_limited(&builtin_path, CA_BUNDLE_MAX_BYTES, BUILTIN_CA_FILE)?,
        &builtin_path,
    )?;
    let extra_path = paths.extra_ca();
    let extra_pem = if extra_path.is_file() {
        text_of(
            read_limited(&extra_path, CA_BUNDLE_MAX_BYTES, EXTRA_CA_FILE)?,
            &extra_path,
        )?
    } else {
        String::new()
    };
    Ok(TrustMaterial {
        mode: settings.outbound,
        builtin_pem,
        extra_pem,
    })
}

/// 单份证书的可展示元数据（`describe_cert_der` 的返回值；不含私钥）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertMeta {
    /// 主题（可分辨名称）。
    pub subject: String,
    /// SAN 集合（DNS 名与 IP 字面量，已归一化小写／去重以外的原序）。
    pub sans: Vec<String>,
    /// 生效时间（ISO 8601，RFC 3339）。
    pub not_before: Option<String>,
    /// 到期时间（ISO 8601，RFC 3339）。
    pub not_after: Option<String>,
    /// 证书 DER 的 SHA-256（小写十六进制）。
    pub sha256: String,
}

/// 解析单份 DER 证书，返回主题、SAN、有效期与指纹（供装配根探针复用）。
///
/// # Errors
///
/// DER 不可解析、尾部有多余字节或 SAN 扩展损坏时返回 [`WhError`]（`parse_error`）。
pub fn describe_cert_der(der: &[u8]) -> Result<CertMeta, WhError> {
    let metadata = metadata_from_der(der, "对端证书", None)?;
    Ok(CertMeta {
        subject: metadata.subject,
        sans: metadata.sans,
        not_before: iso8601(metadata.not_before),
        not_after: iso8601(metadata.not_after),
        sha256: metadata.sha256,
    })
}

/// 一次只握手校验的结果（`POST /api/certs/verify` 的成功载荷）。
///
/// 不含任何请求载荷与凭据；`tls_version` 由探针实现给出（如 `TLSv1_3`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// 目标主机（与请求一致）。
    pub host: String,
    /// 目标端口。
    pub port: u16,
    /// 叶子证书主题。
    pub subject: Option<String>,
    /// 叶子证书 SAN 集合。
    pub sans: Vec<String>,
    /// 叶子证书到期时间（ISO 8601）。
    pub not_after: Option<String>,
    /// 叶子证书 SHA-256（小写十六进制）。
    pub sha256: Option<String>,
    /// 协商到的 TLS 协议版本。
    pub tls_version: Option<String>,
}

/// 出站连通性校验抽象（实现属装配根，Web 层不引入网络依赖；ADR-040／ADR-031）。
pub trait CertProbe {
    /// 以给定信任材料对 `host:port` 做一次只握手校验（不发送 HTTP 请求与凭据）。
    ///
    /// # Errors
    ///
    /// 信任材料不可用、解析或连接失败、预算内未完成握手、证书校验失败时返回 [`WhError`]；
    /// 错误消息不得包含私钥或凭据。
    fn probe(
        &self,
        host: &str,
        port: u16,
        material: &TrustMaterial,
    ) -> Result<ProbeReport, WhError>;
}

// ── 初始化内部步骤 ───────────────────────────────────────────────────────────

fn ensure_settings(paths: &CertPaths, report: &mut InitReport) -> Result<TlsSettings, WhError> {
    let path = paths.settings();
    if path.exists() {
        return parse_settings(&path);
    }
    let settings = TlsSettings::default();
    atomic_write(&path, &json_bytes(&settings)?)?;
    report.created.push(SETTINGS_FILE.to_string());
    Ok(settings)
}

fn ensure_builtin_ca(
    paths: &CertPaths,
    now_ms: u64,
    report: &mut InitReport,
) -> Result<(), WhError> {
    let path = paths.builtin_ca();
    let bundled_sha = sha256_hex(BUILTIN_CA_BUNDLE.as_bytes());
    if !path.exists() {
        atomic_write(&path, BUILTIN_CA_BUNDLE.as_bytes())?;
        let manifest = seeded_manifest(&bundled_sha, &bundled_sha, now_ms);
        atomic_write(&paths.seed_manifest(), &json_bytes(&manifest)?)?;
        report.created.push(BUILTIN_CA_FILE.to_string());
        report.created.push(SEED_MANIFEST_FILE.to_string());
        return Ok(());
    }

    let bytes = read_limited(&path, CA_BUNDLE_MAX_BYTES, BUILTIN_CA_FILE)?;
    let current_sha = sha256_hex(&bytes);
    match load_seed_manifest(&paths.seed_manifest()) {
        Ok(manifest) if current_sha == manifest.seeded_sha256 && current_sha != bundled_sha => {
            atomic_write(&path, BUILTIN_CA_BUNDLE.as_bytes())?;
            let refreshed = seeded_manifest(&bundled_sha, &bundled_sha, now_ms);
            atomic_write(&paths.seed_manifest(), &json_bytes(&refreshed)?)?;
            report.refreshed_builtin_ca = true;
            report.notes.push(format!(
                "内置根证书副本未被修改，已随内置资产刷新：{} → {}（版本 {}）",
                current_sha, bundled_sha, BUILTIN_BUNDLE_VERSION
            ));
        }
        Ok(manifest) if current_sha != manifest.seeded_sha256 => {
            let newer = manifest.bundled_sha256 != bundled_sha;
            if newer {
                report.notes.push(format!(
                    "内置根证书副本 {} 已被修改，内置版本较新（{}）；保留用户副本，\
                     如需恢复请执行 reset",
                    path.display(),
                    BUILTIN_BUNDLE_VERSION
                ));
            } else {
                report.notes.push(format!(
                    "内置根证书副本 {} 已被用户修改；保留副本，如需恢复请执行 reset",
                    path.display()
                ));
            }
        }
        Ok(_) => {}
        Err(err) => {
            report.notes.push(format!(
                "种子清单 {} 缺失或损坏（{}）；未覆盖用户文件",
                paths.seed_manifest().display(),
                err.message
            ));
            if current_sha == bundled_sha {
                let manifest = seeded_manifest(&bundled_sha, &bundled_sha, now_ms);
                atomic_write(&paths.seed_manifest(), &json_bytes(&manifest)?)?;
                report
                    .notes
                    .push("工作区副本与内置资产一致，已补写种子清单".to_string());
            }
        }
    }

    // 不改写用户文件，但必须让不可用的副本在初始化报告中可见（信号链完备）。
    if let Err(err) = validate_ca_pem(
        &String::from_utf8_lossy(&bytes),
        CaKind::Builtin,
        MAX_CERT_COUNT,
        &path.display().to_string(),
    ) {
        report
            .notes
            .push(format!("内置根证书副本不可用：{}", err.message));
    }
    Ok(())
}

fn ensure_extra_ca(paths: &CertPaths, report: &mut InitReport) -> Result<(), WhError> {
    let path = paths.extra_ca();
    if !path.exists() {
        atomic_write(&path, b"")?;
        report.created.push(EXTRA_CA_FILE.to_string());
        return Ok(());
    }
    let bytes = read_limited(&path, CA_BUNDLE_MAX_BYTES, EXTRA_CA_FILE)?;
    if let Err(err) = validate_ca_pem(
        &String::from_utf8_lossy(&bytes),
        CaKind::Extra,
        MAX_CERT_COUNT,
        &path.display().to_string(),
    ) {
        report
            .notes
            .push(format!("追加根证书副本不可用：{}", err.message));
    }
    Ok(())
}

fn ensure_server_cert(
    paths: &CertPaths,
    inbound: InboundMode,
    required_sans: &[String],
    now_ms: u64,
    report: &mut InitReport,
) -> Result<(), WhError> {
    match inbound {
        InboundMode::Off => {
            report
                .notes
                .push("入站档位为 off：服务以明文 HTTP 运行，不生成服务端证书".to_string());
            return Ok(());
        }
        InboundMode::External => {
            let cert_present = paths.server_cert().is_file();
            let key_present = paths.server_key().is_file();
            if !cert_present || !key_present {
                report.notes.push(format!(
                    "入站档位为 external，但 {} 或 {} 缺失；serve --web 将拒绝启动，\
                     请上传匹配的证书链与未加密私钥",
                    paths.server_cert().display(),
                    paths.server_key().display()
                ));
            }
            return Ok(());
        }
        InboundMode::Builtin => {}
    }

    let sans = effective_sans(required_sans);
    let cert_exists = paths.server_cert().is_file();
    let key_exists = paths.server_key().is_file();
    let mut existing = None;
    let mut unusable = None;
    if cert_exists {
        match read_limited(
            &paths.server_cert(),
            SERVER_CERT_MAX_BYTES,
            SERVER_CERT_FILE,
        )
        .and_then(|bytes| {
            parse_certificate_blocks(bytes.as_slice(), &paths.server_cert().display().to_string())
        }) {
            Ok(certs) if !certs.is_empty() => existing = Some(certs[0].clone()),
            Ok(_) => {
                unusable = Some(format!("{} 不含 CERTIFICATE 块", SERVER_CERT_FILE));
            }
            Err(err) => unusable = Some(format!("{} 不可用：{}", SERVER_CERT_FILE, err.message)),
        }
    }

    let mut reason = None;
    if !cert_exists {
        reason = Some("入站自签证书不存在".to_string());
    } else if let Some(note) = &unusable {
        reason = Some(note.clone());
    } else if !key_exists {
        reason = Some("入站私钥不存在".to_string());
    } else if let Some(meta) = &existing {
        let days = days_remaining(meta.not_after, now_ms);
        if days < 0 {
            reason = Some(format!("入站自签证书已过期（{days} 天）"));
        } else if !cert_covers(meta, &sans) {
            reason = Some("入站自签证书 SAN 未覆盖 required_sans".to_string());
        } else if let Some(issue) = server_key_issue(&paths.server_key(), meta) {
            reason = Some(issue);
        } else if days < CERT_RENEW_WARN_DAYS {
            report.notes.push(format!(
                "入站自签证书将在 {days} 天后到期（不足 {CERT_RENEW_WARN_DAYS} 天）；\
                 请重新生成或改用 external"
            ));
        }
    }

    if let Some(reason) = reason {
        let old_fingerprint = existing.as_ref().map(|meta| meta.sha256.clone());
        let (cert_pem, key_pem, leaf) = generate_self_signed(&sans, now_ms)?;
        let snapshots = server_snapshots_for_repair(paths)?;
        if let Err(err) = write_server_material(paths, &cert_pem, &key_pem) {
            return Err(with_rollback(err, &snapshots));
        }
        report.regenerated_server_cert = true;
        report.notes.push(format!(
            "{reason}；已重新生成自签证书（旧指纹 {}，新指纹 {}）",
            old_fingerprint.as_deref().unwrap_or("无"),
            leaf.sha256
        ));
        if !cert_exists {
            report.created.push(SERVER_CERT_FILE.to_string());
        }
        if !key_exists {
            report.created.push(SERVER_KEY_FILE.to_string());
        }
    }
    Ok(())
}

// ── 设置与状态 ───────────────────────────────────────────────────────────────

fn parse_settings(path: &Path) -> Result<TlsSettings, WhError> {
    let bytes = read_limited(path, SETTINGS_MAX_BYTES, SETTINGS_FILE)?;
    let text = text_of(bytes, path)?;
    let settings: TlsSettings = serde_json::from_str(&text).map_err(|err| {
        WhError::parse_error(format!(
            "证书库设置 {} 解析失败：{err}；请修正 JSON 或删除该文件以恢复默认（出站 builtin、\
             入站 builtin）",
            path.display()
        ))
    })?;
    settings.validate()?;
    Ok(settings)
}

fn load_settings_with_notes(paths: &CertPaths, notes: &mut Vec<String>) -> TlsSettings {
    let path = paths.settings();
    if !path.exists() {
        notes.push(format!(
            "证书库设置不存在：{}；按默认值展示（出站 builtin、入站 builtin），\
             执行初始化后落盘",
            path.display()
        ));
        return TlsSettings::default();
    }
    match parse_settings(&path) {
        Ok(settings) => settings,
        Err(err) => {
            notes.push(format!(
                "证书库设置不可用，按默认值展示（文件 {}）：{}",
                path.display(),
                err.message
            ));
            TlsSettings::default()
        }
    }
}

fn ca_status(
    path: &Path,
    kind: CaKind,
    manifest: Option<&SeedManifest>,
    notes: &mut Vec<String>,
) -> Option<CertFileStatus> {
    let bundled_sha = sha256_hex(BUILTIN_CA_BUNDLE.as_bytes());
    let bundled_version = manifest.map(|item| item.bundled_version.clone());
    let bundled_seed_sha = manifest.map(|item| item.bundled_sha256.clone());
    let newer = manifest.is_some_and(|item| item.bundled_sha256 != bundled_sha);
    let path_text = path.display().to_string();

    if let Err(err) = std::fs::metadata(path) {
        if err.kind() == ErrorKind::NotFound {
            if kind == CaKind::Extra {
                return None;
            }
            notes.push(format!(
                "{} 不存在：{path_text}；执行证书库初始化或 reset 可重建内置副本",
                kind.file_name()
            ));
        } else {
            notes.push(format!(
                "{} 状态读取失败：{path_text}：{err}",
                kind.file_name()
            ));
        }
        return Some(CertFileStatus {
            path: path_text,
            cert_count: 0,
            sha256: String::new(),
            modified: false,
            bundled_version,
            bundled_sha256: bundled_seed_sha,
            newer_bundled_available: newer,
        });
    }

    let bytes = match read_limited(path, CA_BUNDLE_MAX_BYTES, kind.file_name()) {
        Ok(bytes) => bytes,
        Err(err) => {
            notes.push(format!("{} 读取失败：{}", kind.file_name(), err.message));
            return Some(CertFileStatus {
                path: path_text,
                cert_count: 0,
                sha256: String::new(),
                modified: false,
                bundled_version,
                bundled_sha256: bundled_seed_sha,
                newer_bundled_available: newer,
            });
        }
    };
    let sha = sha256_hex(&bytes);
    let modified = manifest.map_or(sha != bundled_sha, |item| item.seeded_sha256 != sha);
    let mut cert_count = 0usize;
    match text_of(bytes, path) {
        Ok(text) => match validate_ca_pem(&text, kind, MAX_CERT_COUNT, &path.display().to_string())
        {
            Ok(summary) => {
                cert_count = summary.cert_count;
                if summary.duplicate_count > 0 {
                    notes.push(format!(
                        "{} 含 {} 份重复证书（仅告警，不拒绝）",
                        kind.file_name(),
                        summary.duplicate_count
                    ));
                }
            }
            Err(err) => notes.push(format!("{} 解析失败：{}", kind.file_name(), err.message)),
        },
        Err(err) => notes.push(format!("{} 解析失败：{}", kind.file_name(), err.message)),
    }

    Some(CertFileStatus {
        path: path_text,
        cert_count,
        sha256: sha,
        modified,
        bundled_version,
        bundled_sha256: bundled_seed_sha,
        newer_bundled_available: newer,
    })
}

fn server_status(
    paths: &CertPaths,
    mode: InboundMode,
    now_ms: u64,
    notes: &mut Vec<String>,
) -> ServerCertStatus {
    let cert_path = paths.server_cert();
    let key_path = paths.server_key();
    let mut cert_present = false;
    let mut subject = None;
    let mut sans = Vec::new();
    let mut not_before = None;
    let mut not_after = None;
    let mut sha256 = None;
    let mut days = None;

    if cert_path.is_file() {
        match read_limited(&cert_path, SERVER_CERT_MAX_BYTES, SERVER_CERT_FILE) {
            Ok(bytes) => match parse_certificate_blocks(&bytes, &cert_path.display().to_string()) {
                Ok(certs) => match certs.first() {
                    Some(leaf) => {
                        cert_present = true;
                        subject = Some(leaf.subject.clone());
                        sans = leaf.sans.clone();
                        not_before = iso8601(leaf.not_before);
                        not_after = iso8601(leaf.not_after);
                        sha256 = Some(leaf.sha256.clone());
                        days = Some(days_remaining(leaf.not_after, now_ms));
                    }
                    None => notes.push(format!(
                        "{} 不含 CERTIFICATE 块：{cert_path_text}",
                        SERVER_CERT_FILE,
                        cert_path_text = cert_path.display()
                    )),
                },
                Err(err) => notes.push(format!("{} 解析失败：{}", SERVER_CERT_FILE, err.message)),
            },
            Err(err) => notes.push(format!("{} 读取失败：{}", SERVER_CERT_FILE, err.message)),
        }
    } else if mode != InboundMode::Off {
        notes.push(format!(
            "{} 不存在：{}",
            SERVER_CERT_FILE,
            cert_path.display()
        ));
    }

    let key_present = key_path.is_file();
    if !key_present && mode != InboundMode::Off {
        notes.push(format!(
            "{} 不存在：{}",
            SERVER_KEY_FILE,
            key_path.display()
        ));
    }

    match mode {
        InboundMode::Off => {
            notes.push("入站 TLS 已关闭，serve --web 以明文 HTTP 运行（ADR-023）".to_string());
        }
        InboundMode::Builtin if !cert_present || !key_present => notes.push(
            "入站档位为 builtin 但自签证书材料不完整；重启 serve --web 时会重新生成".to_string(),
        ),
        InboundMode::External if !cert_present || !key_present => notes.push(
            "入站档位为 external 但证书材料不完整；serve --web 将拒绝启动，请上传匹配的证书链与\
             未加密私钥"
                .to_string(),
        ),
        _ => {}
    }

    if let Some(remaining) = days {
        if remaining < 0 {
            notes.push(format!(
                "入站证书已过期（{remaining} 天）；builtin 档位会在启动时重新生成，\
                 external 档位不会自动替换"
            ));
        } else if remaining < CERT_RENEW_WARN_DAYS {
            notes.push(format!(
                "入站证书将在 {remaining} 天后到期（不足 {CERT_RENEW_WARN_DAYS} 天）；\
                 请重新生成或上传新证书"
            ));
        }
    }

    ServerCertStatus {
        mode,
        cert_present,
        key_present,
        subject,
        sans,
        not_before,
        not_after,
        sha256,
        days_remaining: days,
    }
}

// ── CA 校验 ─────────────────────────────────────────────────────────────────

fn validate_ca_pem(
    pem: &str,
    kind: CaKind,
    max_count: usize,
    source: &str,
) -> Result<CaSummary, WhError> {
    let bytes = pem.as_bytes();
    if bytes.len() > CA_BUNDLE_MAX_BYTES {
        return Err(WhError::parse_error(format!(
            "根证书文件 {source} 超过上限 {CA_BUNDLE_MAX_BYTES} 字节（当前 {} 字节）；\
             请裁剪后重试（ADR-040 决策 1 的拒绝阈值）",
            bytes.len()
        )));
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(WhError::parse_error(format!(
            "根证书文件 {source} 含 UTF-8 BOM；请保存为 UTF-8 无 BOM（仓库与运行期统一口径）"
        )));
    }
    let context = format!("根证书文件 {source}");
    let (cert_count, duplicate_count) = inspect_ca_blocks(bytes, &context, max_count)?;
    if cert_count == 0 && kind == CaKind::Builtin {
        return Err(WhError::parse_error(format!(
            "{source} 为空（不含任何 CERTIFICATE 块）：内置根证书不得为空；\
             如需停用内置库请把出站档位设为 system（ADR-040 决策 2）"
        )));
    }
    Ok(CaSummary {
        cert_count,
        sha256: sha256_hex(bytes),
        duplicate_count,
    })
}

fn inspect_ca_blocks(
    bytes: &[u8],
    context: &str,
    max_count: usize,
) -> Result<(usize, usize), WhError> {
    let mut count = 0usize;
    let mut duplicates = 0usize;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for block in x509_parser::pem::Pem::iter_from_buffer(bytes) {
        let pem = block.map_err(|err| {
            WhError::parse_error(format!(
                "{context}：PEM 块读取失败（{err}）；请确认每块有匹配的 BEGIN/END 行且为 Base64"
            ))
        })?;
        let label = pem.label.trim();
        if label.contains("PRIVATE KEY") {
            return Err(WhError::parse_error(format!(
                "{context} 含私钥块“{label}”：私钥不得混入证书文件；\
                 根证书文件只放 CERTIFICATE 块"
            )));
        }
        if label != "CERTIFICATE" {
            return Err(WhError::parse_error(format!(
                "{context} 含不允许的 PEM 块“{label}”：仅允许 CERTIFICATE 块（ADR-040 决策 1）"
            )));
        }
        let (remainder, _certificate) = x509_parser::parse_x509_certificate(&pem.contents)
            .map_err(|err| {
                WhError::parse_error(format!(
                    "{context}：第 {} 份证书 DER 解析失败（{err}）；请确认证书完整",
                    count + 1
                ))
            })?;
        if !remainder.is_empty() {
            return Err(WhError::parse_error(format!(
                "{context}：第 {} 份证书 DER 尾部有 {} 字节多余数据；请移除拼接残留",
                count + 1,
                remainder.len()
            )));
        }
        count += 1;
        if count > max_count {
            return Err(WhError::parse_error(format!(
                "{context} 的证书份数超过上限 {max_count}（已读到第 {count} 份）；\
                 请裁剪后重试（ADR-040 决策 1）"
            )));
        }
        if !seen.insert(sha256_hex(&pem.contents)) {
            duplicates += 1;
        }
    }
    Ok((count, duplicates))
}

// ── 服务端证书解析、校验与生成 ───────────────────────────────────────────────

#[derive(Debug, Clone)]
struct CertMetadata {
    subject: String,
    sans: Vec<String>,
    dns_sans: Vec<String>,
    ip_sans: Vec<String>,
    not_before: i64,
    not_after: i64,
    sha256: String,
    spki: Vec<u8>,
}

#[derive(Debug)]
struct ServerPairInfo {
    cert_count: usize,
    leaf: CertMetadata,
}

fn inspect_server_pair(cert_pem: &str, key_pem: &str) -> Result<ServerPairInfo, WhError> {
    if cert_pem.len() > SERVER_CERT_MAX_BYTES {
        return Err(WhError::parse_error(format!(
            "服务端证书链 {SERVER_CERT_FILE} 超过上限 {SERVER_CERT_MAX_BYTES} 字节\
             （当前 {} 字节）；请裁剪后重试",
            cert_pem.len()
        )));
    }
    if key_pem.len() > SERVER_KEY_MAX_BYTES {
        return Err(WhError::parse_error(format!(
            "服务端私钥 {SERVER_KEY_FILE} 超过上限 {SERVER_KEY_MAX_BYTES} 字节\
             （当前 {} 字节）；请裁剪后重试",
            key_pem.len()
        )));
    }
    let certificates = parse_certificate_blocks(cert_pem.as_bytes(), SERVER_CERT_FILE)?;
    let Some(leaf) = certificates.first() else {
        return Err(WhError::parse_error(format!(
            "服务端证书链 {SERVER_CERT_FILE} 至少需要一份 CERTIFICATE；当前无证书块"
        )));
    };
    if key_pem.contains("ENCRYPTED PRIVATE KEY") {
        return Err(WhError::parse_error(format!(
            "服务端私钥 {SERVER_KEY_FILE} 带口令（ENCRYPTED PRIVATE KEY）：\
             请提供未加密的 PEM 私钥（PKCS#8，BEGIN PRIVATE KEY）"
        )));
    }
    let key_pair = KeyPair::from_pem(key_pem).map_err(|err| {
        WhError::parse_error(format!(
            "服务端私钥 {SERVER_KEY_FILE} 解析失败（{err}）；请提供未加密的 PKCS#8 PEM 私钥\
             （BEGIN PRIVATE KEY；ring 后端不支持 BEGIN RSA PRIVATE KEY／\
             BEGIN EC PRIVATE KEY）"
        ))
    })?;
    let key_spki = key_pair.subject_public_key_info();
    if key_spki != leaf.spki {
        return Err(WhError::parse_error(format!(
            "服务端证书链与私钥不匹配：叶子证书公钥与 {SERVER_KEY_FILE} 的公钥不一致；\
             请上传同一密钥对（证书与私钥必须成对）"
        )));
    }
    Ok(ServerPairInfo {
        cert_count: certificates.len(),
        leaf: leaf.clone(),
    })
}

fn server_key_issue(path: &Path, leaf: &CertMetadata) -> Option<String> {
    let bytes = match read_limited(path, SERVER_KEY_MAX_BYTES, SERVER_KEY_FILE) {
        Ok(bytes) => bytes,
        Err(err) => return Some(format!("入站私钥不可用：{}", err.message)),
    };
    let text = match text_of(bytes, path) {
        Ok(text) => text,
        Err(err) => return Some(format!("入站私钥不可用：{}", err.message)),
    };
    if text.contains("ENCRYPTED PRIVATE KEY") {
        return Some("入站私钥带口令（ENCRYPTED PRIVATE KEY）；builtin 档位将重新生成".to_string());
    }
    let key_pair = match KeyPair::from_pem(&text) {
        Ok(key_pair) => key_pair,
        Err(err) => {
            return Some(format!("入站私钥解析失败（{err}）；builtin 档位将重新生成"));
        }
    };
    if key_pair.subject_public_key_info() != leaf.spki {
        return Some("入站私钥与证书公钥不匹配；builtin 档位将重新生成".to_string());
    }
    None
}

fn parse_certificate_blocks(bytes: &[u8], context: &str) -> Result<Vec<CertMetadata>, WhError> {
    let mut certificates: Vec<CertMetadata> = Vec::new();
    for block in x509_parser::pem::Pem::iter_from_buffer(bytes) {
        let pem = block.map_err(|err| {
            WhError::parse_error(format!(
                "{context}：PEM 块读取失败（{err}）；请确认每块有匹配的 BEGIN/END 行且为 Base64"
            ))
        })?;
        let label = pem.label.trim();
        if label.contains("PRIVATE KEY") {
            return Err(WhError::parse_error(format!(
                "{context} 含私钥块“{label}”：私钥不得混入证书文件；\
                 证书链只放 CERTIFICATE 块，私钥单独放 {SERVER_KEY_FILE}"
            )));
        }
        if label != "CERTIFICATE" {
            return Err(WhError::parse_error(format!(
                "{context} 含不允许的 PEM 块“{label}”：仅允许 CERTIFICATE 块"
            )));
        }
        let position = certificates.len() + 1;
        certificates.push(metadata_from_der(&pem.contents, context, Some(position))?);
    }
    Ok(certificates)
}

/// 解析单份 DER 证书（PEM 循环与 [`describe_cert_der`] 共用；`position` 为 1 基序号）。
fn metadata_from_der(
    der: &[u8],
    context: &str,
    position: Option<usize>,
) -> Result<CertMetadata, WhError> {
    use x509_parser::extensions::GeneralName;

    let label = position.map_or_else(|| "证书".to_string(), |index| format!("第 {index} 份证书"));
    let (remainder, parsed) = x509_parser::parse_x509_certificate(der).map_err(|err| {
        WhError::parse_error(format!(
            "{context}：{label} DER 解析失败（{err}）；请确认证书完整"
        ))
    })?;
    if !remainder.is_empty() {
        return Err(WhError::parse_error(format!(
            "{context}：{label} DER 尾部有 {} 字节多余数据；请移除拼接残留",
            remainder.len()
        )));
    }
    let mut sans = Vec::new();
    let mut dns_sans = Vec::new();
    let mut ip_sans = Vec::new();
    match parsed.subject_alternative_name() {
        Ok(Some(extension)) => {
            for name in &extension.value.general_names {
                match name {
                    GeneralName::DNSName(value) => {
                        let normalized = value.trim().to_ascii_lowercase();
                        if !normalized.is_empty() {
                            dns_sans.push(normalized.clone());
                            sans.push(normalized);
                        }
                    }
                    GeneralName::IPAddress(value) => {
                        if let Some(ip) = ip_address(value) {
                            let normalized = ip.to_string();
                            ip_sans.push(normalized.clone());
                            sans.push(normalized);
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(None) => {}
        Err(err) => {
            return Err(WhError::parse_error(format!(
                "{context}：{label}的 SAN 扩展解析失败（{err}）"
            )));
        }
    }
    Ok(CertMetadata {
        subject: parsed.subject().to_string(),
        sans,
        dns_sans,
        ip_sans,
        not_before: parsed.validity().not_before.timestamp(),
        not_after: parsed.validity().not_after.timestamp(),
        sha256: sha256_hex(der),
        spki: parsed.public_key().raw.to_vec(),
    })
}

fn generate_self_signed(
    sans: &[String],
    now_ms: u64,
) -> Result<(String, String, CertMetadata), WhError> {
    let now = now_datetime(now_ms)?;
    let key_pair = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|err| {
        WhError::internal(format!(
            "生成 ECDSA P-256 私钥失败：{err}；请确认构建使用 ring 提供者后重试"
        ))
    })?;
    let mut params = CertificateParams::new(sans.to_vec()).map_err(|err| {
        WhError::parse_error(format!(
            "自签证书 SAN 集合不可用：{err}；请检查 required_sans 中的主机名与 IP 字面量\
             （不得含下划线、空格或空项）"
        ))
    })?;
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, "WHCode local");
    params.distinguished_name = distinguished_name;
    params.is_ca = IsCa::NoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = now - Duration::days(1);
    params.not_after = params.not_before + Duration::days(CERT_VALIDITY_DAYS);
    let certificate = params.self_signed(&key_pair).map_err(|err| {
        WhError::internal(format!(
            "生成自签证书失败：{err}；请确认 ring 提供者可用后重试"
        ))
    })?;
    let cert_pem = normalize_lf(&certificate.pem());
    let key_pem = normalize_lf(&key_pair.serialize_pem());
    let metadata = parse_certificate_blocks(cert_pem.as_bytes(), "新生成的自签证书")?
        .into_iter()
        .next()
        .ok_or_else(|| WhError::internal("新生成的自签证书不含 CERTIFICATE 块（内部不一致）"))?;
    Ok((cert_pem, key_pem, metadata))
}

fn effective_sans(required_sans: &[String]) -> Vec<String> {
    let mut sans = Vec::new();
    for item in required_sans {
        push_san(&mut sans, item);
    }
    for item in DEFAULT_SANS {
        push_san(&mut sans, item);
    }
    if let Some(name) = whcode_platform::hostname() {
        push_san(&mut sans, &name);
    }
    sans
}

fn push_san(sans: &mut Vec<String>, raw: &str) {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return;
    }
    let normalized = trimmed
        .parse::<IpAddr>()
        .map_or_else(|_| trimmed.to_ascii_lowercase(), |ip| ip.to_string());
    if !sans.iter().any(|item| item == &normalized) {
        sans.push(normalized);
    }
}

fn cert_covers(leaf: &CertMetadata, required_sans: &[String]) -> bool {
    required_sans.iter().all(|item| {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            return true;
        }
        match trimmed.parse::<IpAddr>() {
            Ok(ip) => leaf.ip_sans.iter().any(|san| san == &ip.to_string()),
            Err(_) => {
                let normalized = trimmed.to_ascii_lowercase();
                leaf.dns_sans.iter().any(|san| san == &normalized)
            }
        }
    })
}

fn ip_address(bytes: &[u8]) -> Option<IpAddr> {
    match bytes.len() {
        4 => {
            let octets: [u8; 4] = bytes.try_into().ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(octets)))
        }
        16 => {
            let octets: [u8; 16] = bytes.try_into().ok()?;
            Some(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => None,
    }
}

// ── 文件、时间与审计通用工具 ─────────────────────────────────────────────────

fn create_dir_all(dir: &Path) -> Result<(), WhError> {
    std::fs::create_dir_all(dir).map_err(|err| {
        WhError::io(
            &format!(
                "创建证书库目录 {}（请确认父目录存在且当前用户可写）",
                dir.display()
            ),
            &err,
        )
    })
}

fn read_limited(path: &Path, limit: usize, what: &str) -> Result<Vec<u8>, WhError> {
    let file = File::open(path).map_err(|err| {
        WhError::io(
            &format!(
                "打开{what} {}（请确认文件存在且当前用户可读）",
                path.display()
            ),
            &err,
        )
    })?;
    let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let mut reader = file.take(cap);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|err| {
        WhError::io(
            &format!("读取{what} {}（文件可能被占用或磁盘故障）", path.display()),
            &err,
        )
    })?;
    if bytes.len() > limit {
        return Err(WhError::parse_error(format!(
            "{what} {} 超过上限 {limit} 字节（当前已读到 {} 字节）；请裁剪后重试\
             （ADR-040 决策 1 的拒绝阈值）",
            path.display(),
            bytes.len()
        )));
    }
    Ok(bytes)
}

fn text_of(bytes: Vec<u8>, path: &Path) -> Result<String, WhError> {
    String::from_utf8(bytes).map_err(|_| {
        WhError::parse_error(format!(
            "{} 不是 UTF-8 文本；PEM 与 JSON 必须为 UTF-8（无 BOM）",
            path.display()
        ))
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), WhError> {
    let Some(parent) = path.parent() else {
        return Err(WhError::invalid_state(format!(
            "写入路径 {} 没有父目录",
            path.display()
        )));
    };
    create_dir_all(parent)?;
    let temp = temp_path(path);
    let result = (|| -> Result<(), WhError> {
        {
            let mut file = File::create(&temp).map_err(|err| {
                WhError::io(
                    &format!("创建临时文件 {}（请确认目录可写）", temp.display()),
                    &err,
                )
            })?;
            file.write_all(bytes).map_err(|err| {
                WhError::io(
                    &format!("写入临时文件 {}（目标 {}）", temp.display(), path.display()),
                    &err,
                )
            })?;
            file.sync_all().map_err(|err| {
                WhError::io(&format!("刷新临时文件 {} 到磁盘", temp.display()), &err)
            })?;
        }
        std::fs::rename(&temp, path).map_err(|err| {
            WhError::io(
                &format!("原子替换 {}（临时文件 {}）", path.display(), temp.display()),
                &err,
            )
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let name = path.file_name().map_or_else(
        || "certs".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    path.with_file_name(format!(".{name}.tmp-{}-{stamp}-{seq}", std::process::id()))
}

#[derive(Debug)]
struct FileSnapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

fn snapshot_file(path: &Path, limit: usize, what: &str) -> Result<FileSnapshot, WhError> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(FileSnapshot {
            path: path.to_path_buf(),
            bytes: Some(read_limited(path, limit, what)?),
        }),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(FileSnapshot {
            path: path.to_path_buf(),
            bytes: None,
        }),
        Err(err) => Err(WhError::io(
            &format!("检查{what} {} 以便失败回滚", path.display()),
            &err,
        )),
    }
}

fn server_snapshots(paths: &CertPaths) -> Result<[FileSnapshot; 2], WhError> {
    Ok([
        snapshot_file(
            &paths.server_cert(),
            SERVER_CERT_MAX_BYTES,
            SERVER_CERT_FILE,
        )?,
        snapshot_file(&paths.server_key(), SERVER_KEY_MAX_BYTES, SERVER_KEY_FILE)?,
    ])
}

/// 修复路径（初始化与重新生成）的快照：已有文件超过上限时视为“不可回滚的旧材料”，
/// 回滚动作删除该文件而不是读取无界字节。用户写入路径仍用 [`server_snapshots`] 的严格口径。
fn server_snapshots_for_repair(paths: &CertPaths) -> Result<[FileSnapshot; 2], WhError> {
    Ok([
        snapshot_file_for_repair(
            &paths.server_cert(),
            SERVER_CERT_MAX_BYTES,
            SERVER_CERT_FILE,
        )?,
        snapshot_file_for_repair(&paths.server_key(), SERVER_KEY_MAX_BYTES, SERVER_KEY_FILE)?,
    ])
}

fn snapshot_file_for_repair(
    path: &Path,
    limit: usize,
    what: &str,
) -> Result<FileSnapshot, WhError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > u64::try_from(limit).unwrap_or(u64::MAX) => {
            Ok(FileSnapshot {
                path: path.to_path_buf(),
                bytes: None,
            })
        }
        _ => snapshot_file(path, limit, what),
    }
}

fn restore_snapshots(snapshots: &[FileSnapshot]) -> Vec<String> {
    let mut failures = Vec::new();
    for snapshot in snapshots {
        if let Err(err) = restore_snapshot(snapshot) {
            failures.push(format!("{}（{}）", snapshot.path.display(), err.message));
        }
    }
    failures
}

fn restore_snapshot(snapshot: &FileSnapshot) -> Result<(), WhError> {
    match &snapshot.bytes {
        Some(bytes) => {
            atomic_write(&snapshot.path, bytes)?;
            if snapshot
                .path
                .file_name()
                .is_some_and(|name| name == SERVER_KEY_FILE)
            {
                whcode_platform::restrict_private_file(&snapshot.path)?;
            }
            Ok(())
        }
        None => match std::fs::remove_file(&snapshot.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(WhError::io(
                &format!("删除失败路径新建文件 {}", snapshot.path.display()),
                &err,
            )),
        },
    }
}

fn with_rollback(mut err: WhError, snapshots: &[FileSnapshot]) -> WhError {
    let failures = restore_snapshots(snapshots);
    if failures.is_empty() {
        err.message = format!("{}；已回滚到原字节", err.message);
    } else {
        err.message = format!(
            "{}；回滚也失败，需人工检查：{}",
            err.message,
            failures.join("；")
        );
    }
    err
}

fn audit_or_rollback(
    audit: &dyn CertAudit,
    conn: &Connection,
    record: &AuditRecord,
    snapshots: &[FileSnapshot],
) -> Result<(), WhError> {
    match audit.record(conn, record) {
        Ok(()) => Ok(()),
        Err(err) => {
            let failures = restore_snapshots(snapshots);
            if failures.is_empty() {
                Err(WhError::new(
                    err.code,
                    format!(
                        "{}；变更未生效，文件已回滚到原字节（审计失败不得视为成功）",
                        err.message
                    ),
                ))
            } else {
                Err(WhError::new(
                    err.code,
                    format!(
                        "{}；回滚也失败，需人工检查：{}",
                        err.message,
                        failures.join("；")
                    ),
                ))
            }
        }
    }
}

fn write_server_material(paths: &CertPaths, cert_pem: &str, key_pem: &str) -> Result<(), WhError> {
    atomic_write(&paths.server_cert(), cert_pem.as_bytes())?;
    atomic_write(&paths.server_key(), key_pem.as_bytes())?;
    whcode_platform::restrict_private_file(&paths.server_key())
}

fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, WhError> {
    let mut text = serde_json::to_string_pretty(value).map_err(|err| {
        WhError::internal(format!(
            "序列化证书库 JSON 失败：{err}；请检查写入字段是否可序列化"
        ))
    })?;
    text.push('\n');
    Ok(text.into_bytes())
}

fn seeded_manifest(bundled_sha: &str, seeded_sha: &str, now_ms: u64) -> SeedManifest {
    SeedManifest {
        schema_version: SEED_MANIFEST_SCHEMA_VERSION,
        source: BUILTIN_BUNDLE_SOURCE.to_string(),
        bundled_version: BUILTIN_BUNDLE_VERSION.to_string(),
        bundled_sha256: bundled_sha.to_string(),
        seeded_sha256: seeded_sha.to_string(),
        seeded_at_ms: now_ms,
    }
}

fn load_seed_manifest(path: &Path) -> Result<SeedManifest, WhError> {
    let bytes = read_limited(path, SEED_MANIFEST_MAX_BYTES, SEED_MANIFEST_FILE)?;
    let text = text_of(bytes, path)?;
    let manifest: SeedManifest = serde_json::from_str(&text).map_err(|err| {
        WhError::parse_error(format!(
            "种子清单 {} 解析失败：{err}；请删除该文件以在下次初始化时重建",
            path.display()
        ))
    })?;
    if manifest.schema_version != SEED_MANIFEST_SCHEMA_VERSION {
        return Err(WhError::parse_error(format!(
            "种子清单 {} 的 schema_version 不受支持（当前 {}，期望 {}）；\
             请删除该文件以在下次初始化时重建",
            path.display(),
            manifest.schema_version,
            SEED_MANIFEST_SCHEMA_VERSION
        )));
    }
    for (field, value) in [
        ("bundled_sha256", &manifest.bundled_sha256),
        ("seeded_sha256", &manifest.seeded_sha256),
    ] {
        if !is_sha256_hex(value) {
            return Err(WhError::parse_error(format!(
                "种子清单 {} 的 {field} 不是 64 位小写十六进制 SHA-256；\
                 请删除该文件以在下次初始化时重建",
                path.display()
            )));
        }
    }
    Ok(manifest)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f',
    ];
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[usize::from(byte >> 4)]);
        out.push(HEX[usize::from(byte & 0x0f)]);
    }
    out
}

fn now_datetime(now_ms: u64) -> Result<OffsetDateTime, WhError> {
    let seconds = i64::try_from(now_ms / 1000).map_err(|_| {
        WhError::invalid_state("now_ms 超出可表示范围；请检查调用方时钟（Unix 毫秒）")
    })?;
    OffsetDateTime::from_unix_timestamp(seconds).map_err(|err| {
        WhError::invalid_state(format!(
            "now_ms 不是合法 Unix 时间戳（{now_ms}）：{err}；请检查调用方时钟"
        ))
    })
}

fn days_remaining(not_after: i64, now_ms: u64) -> i64 {
    let now_seconds = i64::try_from(now_ms / 1000).unwrap_or(i64::MAX);
    not_after.saturating_sub(now_seconds).div_euclid(86_400)
}

fn iso8601(timestamp: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|value| value.format(&Rfc3339).ok())
}

fn normalize_lf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn audit_record(
    actor: &Actor,
    target: &str,
    detail: serde_json::Value,
    now_ms: u64,
) -> AuditRecord {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    AuditRecord {
        id: format!("tls-{now_ms}-{seq}"),
        actor: actor.id.clone(),
        action: AuditAction::TlsConfigChange,
        target: target.to_string(),
        detail,
        ts_ms: now_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use std::sync::atomic::AtomicU64;
    use whcode_domain::ErrorCode;

    const NOW_MS: u64 = 1_793_000_000_000;
    const DAY_MS: u64 = 86_400_000;

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let root = std::env::temp_dir().join(format!(
                "whcode-cert-test-{}-{tag}-{stamp}-{seq}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("创建测试工作区");
            Self { root }
        }

        fn paths(&self) -> CertPaths {
            CertPaths::for_workspace(&self.root)
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn required_sans() -> Vec<String> {
        vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "::1".to_string(),
            "whcode-test.local".to_string(),
            "10.20.30.40".to_string(),
        ]
    }

    fn initialize(workspace: &TempWorkspace) -> InitReport {
        ensure_initialized(&workspace.paths(), &required_sans(), NOW_MS).expect("初始化证书库")
    }

    fn audit_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM audit", [], |row| row.get(0))
            .expect("统计审计行")
    }

    fn self_signed_pem(sans: &[&str]) -> (String, String) {
        let key = KeyPair::generate().expect("生成测试私钥");
        let params = CertificateParams::new(
            sans.iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>(),
        )
        .expect("测试证书参数");
        let cert = params.self_signed(&key).expect("生成测试证书");
        (
            normalize_lf(&cert.pem()),
            normalize_lf(&key.serialize_pem()),
        )
    }

    fn cert_with_validity(sans: &[&str], not_before: u64, not_after: u64) -> (String, String) {
        let key = KeyPair::generate().expect("生成测试私钥");
        let mut params = CertificateParams::new(
            sans.iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>(),
        )
        .expect("测试证书参数");
        params.not_before =
            OffsetDateTime::from_unix_timestamp(i64::try_from(not_before / 1000).expect("时间"))
                .expect("not_before");
        params.not_after =
            OffsetDateTime::from_unix_timestamp(i64::try_from(not_after / 1000).expect("时间"))
                .expect("not_after");
        let cert = params.self_signed(&key).expect("生成测试证书");
        (
            normalize_lf(&cert.pem()),
            normalize_lf(&key.serialize_pem()),
        )
    }

    struct FailingAudit;

    impl CertAudit for FailingAudit {
        fn record(&self, _conn: &Connection, _record: &AuditRecord) -> Result<(), WhError> {
            Err(WhError::internal("注入审计失败（测试）"))
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        atomic_write(path, bytes).expect("准备测试文件");
    }

    fn settings_bytes(settings: &TlsSettings) -> Vec<u8> {
        json_bytes(settings).expect("序列化设置")
    }

    /// 首次初始化：目录、六个文件、默认设置、SAN 与指纹；两工作区私钥不同。
    #[test]
    fn first_init_creates_files_and_workspace_unique_material() {
        let workspace = TempWorkspace::new("init");
        let paths = workspace.paths();
        let report = initialize(&workspace);

        assert!(paths.dir.is_dir(), "证书库目录必须创建");
        for path in [
            paths.settings(),
            paths.builtin_ca(),
            paths.extra_ca(),
            paths.seed_manifest(),
            paths.server_cert(),
            paths.server_key(),
        ] {
            assert!(path.is_file(), "{} 必须存在", path.display());
        }
        for name in [
            SETTINGS_FILE,
            BUILTIN_CA_FILE,
            EXTRA_CA_FILE,
            SEED_MANIFEST_FILE,
            SERVER_CERT_FILE,
            SERVER_KEY_FILE,
        ] {
            assert!(
                report.created.iter().any(|item| item == name),
                "{name} 必须记录在 created：{:?}",
                report.created
            );
        }
        assert!(report.regenerated_server_cert, "首次初始化必须生成自签证书");

        let settings = load_settings(&paths).expect("读取设置");
        assert_eq!(settings, TlsSettings::default());
        assert_eq!(
            read_ca(&paths, CaKind::Builtin).expect("读取内置 CA"),
            BUILTIN_CA_BUNDLE
        );
        assert!(
            read_ca(&paths, CaKind::Extra)
                .expect("读取追加 CA")
                .is_empty(),
            "首次初始化的追加证书为空"
        );

        let first_status = status(&paths, NOW_MS).expect("状态");
        assert_eq!(first_status.outbound.mode, TrustMode::Builtin);
        let builtin = first_status.outbound.builtin.expect("内置副本状态");
        assert_eq!(builtin.cert_count, 118, "内置包份数须与 SOURCES.md 一致");
        assert!(!builtin.modified, "首次种入不算修改");
        assert_eq!(
            builtin.bundled_version.as_deref(),
            Some(BUILTIN_BUNDLE_VERSION)
        );
        assert_eq!(builtin.sha256, sha256_hex(BUILTIN_CA_BUNDLE.as_bytes()));
        assert!(first_status.inbound.cert_present && first_status.inbound.key_present);
        assert!(
            first_status
                .inbound
                .subject
                .as_deref()
                .is_some_and(|subject| subject.contains("WHCode local")),
            "主题：{:?}",
            first_status.inbound.subject
        );
        for san in required_sans() {
            assert!(first_status.inbound.sans.contains(&san), "SAN 缺少 {san}");
        }
        let unique_sans: BTreeSet<&String> = first_status.inbound.sans.iter().collect();
        assert_eq!(
            unique_sans.len(),
            first_status.inbound.sans.len(),
            "SAN 必须去重：{:?}",
            first_status.inbound.sans
        );
        if let Some(name) = whcode_platform::hostname() {
            let normalized = name.trim().to_ascii_lowercase();
            assert!(
                first_status
                    .inbound
                    .sans
                    .iter()
                    .any(|san| san.eq_ignore_ascii_case(&normalized)),
                "SAN 必须覆盖本机名 {normalized}：{:?}",
                first_status.inbound.sans
            );
        }
        let not_before = first_status
            .inbound
            .not_before
            .as_deref()
            .expect("必须给出生效时间");
        let not_after = first_status
            .inbound
            .not_after
            .as_deref()
            .expect("必须给出到期时间");
        let span = OffsetDateTime::parse(not_after, &Rfc3339).expect("解析到期时间")
            - OffsetDateTime::parse(not_before, &Rfc3339).expect("解析生效时间");
        assert_eq!(
            span.whole_days(),
            CERT_VALIDITY_DAYS,
            "自签证书有效期必须为 {CERT_VALIDITY_DAYS} 天"
        );
        let sha = first_status.inbound.sha256.as_deref().expect("证书指纹");
        assert_eq!(sha.len(), 64);
        assert!(
            first_status
                .inbound
                .days_remaining
                .is_some_and(|days| days > 300)
        );
        assert!(!first_status.restart_required);
        assert_eq!(first_status.outbound.system_root_count, None);

        let server_key = std::fs::read_to_string(paths.server_key()).expect("读取私钥");
        assert!(
            !server_key.contains('\r'),
            "生成文件必须为 LF（rcgen 在 Windows 默认 CRLF，需归一）"
        );

        let second = TempWorkspace::new("init-second");
        initialize(&second);
        let second_status = status(&second.paths(), NOW_MS).expect("第二工作区状态");
        assert_ne!(
            first_status.inbound.sha256, second_status.inbound.sha256,
            "两工作区证书指纹必须不同"
        );
        assert_ne!(
            std::fs::read(paths.server_key()).expect("第一私钥"),
            std::fs::read(second.paths().server_key()).expect("第二私钥"),
            "两工作区私钥必须不同"
        );
    }

    /// 仅出站初始化（T191）：建目录、默认设置、内置副本与追加文件，但**不生成入站材料**。
    #[test]
    fn outbound_only_init_never_creates_inbound_material() {
        let workspace = TempWorkspace::new("outbound-only");
        let paths = workspace.paths();
        let report = ensure_outbound_initialized(&paths, NOW_MS).expect("仅出站初始化");

        for path in [
            paths.settings(),
            paths.builtin_ca(),
            paths.extra_ca(),
            paths.seed_manifest(),
        ] {
            assert!(path.is_file(), "{} 必须创建", path.display());
        }
        for name in [
            SETTINGS_FILE,
            BUILTIN_CA_FILE,
            EXTRA_CA_FILE,
            SEED_MANIFEST_FILE,
        ] {
            assert!(
                report.created.iter().any(|item| item == name),
                "{name} 必须记录在 created：{:?}",
                report.created
            );
        }
        assert!(
            !paths.server_cert().exists(),
            "仅出站初始化不得写入 {}",
            SERVER_CERT_FILE
        );
        assert!(
            !paths.server_key().exists(),
            "仅出站初始化不得写入 {}",
            SERVER_KEY_FILE
        );
        assert!(!report.regenerated_server_cert, "不得生成入站自签证书");
        assert!(
            report
                .created
                .iter()
                .all(|name| name != SERVER_CERT_FILE && name != SERVER_KEY_FILE),
            "created 不得含入站文件：{:?}",
            report.created
        );
        assert_eq!(
            read_ca(&paths, CaKind::Builtin).expect("读取内置 CA"),
            BUILTIN_CA_BUNDLE
        );

        let status = status(&paths, NOW_MS).expect("状态");
        assert_eq!(
            status.outbound.builtin.map_or(0, |item| item.cert_count),
            118,
            "出站内置副本必须可用"
        );
        assert!(
            !status.inbound.cert_present && !status.inbound.key_present,
            "出站初始化后入站材料仍应缺失"
        );

        // 幂等：再次调用不新建文件、不生成入站材料。
        let second = ensure_outbound_initialized(&paths, NOW_MS).expect("重复初始化");
        assert!(
            second.created.is_empty(),
            "重复调用不得再创建文件：{:?}",
            second.created
        );
        assert!(
            !paths.server_cert().exists() && !paths.server_key().exists(),
            "重复调用同样不得生成入站材料"
        );
    }

    /// 保真刷新：未修改副本随内置资产变化刷新并记录；已修改副本保留并提示。
    #[test]
    fn fidelity_refresh_and_modified_copy_preserved() {
        let workspace = TempWorkspace::new("refresh");
        let paths = workspace.paths();
        initialize(&workspace);

        // 构造“旧内置副本”：工作区文件是一份旧证书，清单记录它为种子且内置哈希为旧值。
        let (old_cert, _old_key) = self_signed_pem(&["old.local"]);
        let old_sha = sha256_hex(old_cert.as_bytes());
        write_file(&paths.builtin_ca(), old_cert.as_bytes());
        let old_manifest = SeedManifest {
            schema_version: SEED_MANIFEST_SCHEMA_VERSION,
            source: "old".to_string(),
            bundled_version: "2020-01-01".to_string(),
            bundled_sha256: "0".repeat(64),
            seeded_sha256: old_sha.clone(),
            seeded_at_ms: NOW_MS,
        };
        write_file(
            &paths.seed_manifest(),
            &json_bytes(&old_manifest).expect("旧清单"),
        );

        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("刷新初始化");
        assert!(report.refreshed_builtin_ca, "未修改副本必须随内置资产刷新");
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取刷新副本"),
            BUILTIN_CA_BUNDLE.as_bytes()
        );
        let bundle_sha = sha256_hex(BUILTIN_CA_BUNDLE.as_bytes());
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains(&old_sha) && note.contains(&bundle_sha)),
            "刷新提示须记录旧／新哈希：{:?}",
            report.notes
        );
        let manifest = load_seed_manifest(&paths.seed_manifest()).expect("刷新后的清单");
        assert_eq!(manifest.seeded_sha256, bundle_sha);
        assert_eq!(manifest.bundled_version, BUILTIN_BUNDLE_VERSION);

        // 已修改副本：追加一份证书后保留，提示“已修改”。
        let (extra_cert, _) = self_signed_pem(&["edited.local"]);
        let mut edited = std::fs::read(paths.builtin_ca()).expect("读取副本");
        edited.extend_from_slice(extra_cert.as_bytes());
        write_file(&paths.builtin_ca(), &edited);
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("保留初始化");
        assert!(!report.refreshed_builtin_ca, "已修改副本不得被刷新覆盖");
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取保留副本"),
            edited
        );
        assert!(
            report.notes.iter().any(|note| note.contains("已被")),
            "须提示已修改：{:?}",
            report.notes
        );
        let status = status(&paths, NOW_MS).expect("状态");
        assert!(
            status.outbound.builtin.is_some_and(|item| item.modified),
            "状态必须标记已修改"
        );

        // 清单缺失或损坏：不得覆盖用户文件，只在报告中标注。
        std::fs::remove_file(paths.seed_manifest()).expect("删除清单");
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("缺清单初始化");
        assert!(!report.refreshed_builtin_ca);
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取保留副本"),
            edited
        );
        assert!(
            report.notes.iter().any(|note| note.contains("缺失或损坏")),
            "清单缺失须提示：{:?}",
            report.notes
        );

        write_file(&paths.seed_manifest(), b"not json");
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("坏清单初始化");
        assert!(!report.refreshed_builtin_ca);
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取保留副本"),
            edited
        );
        assert!(
            report.notes.iter().any(|note| note.contains("缺失或损坏")),
            "清单损坏须提示：{:?}",
            report.notes
        );
    }

    /// 重置按 kind 分流：`builtin` 恢复内置资产与清单且不动追加文件；`extra` 清空为 0 字节
    /// 且不恢复内置（ADR-040 决策 4、T190）。
    #[test]
    fn reset_each_kind_touches_only_its_own_file() {
        let workspace = TempWorkspace::new("reset");
        let paths = workspace.paths();
        initialize(&workspace);
        let (extra_cert, _) = self_signed_pem(&["edited.local"]);
        let mut edited = std::fs::read(paths.builtin_ca()).expect("读取副本");
        edited.extend_from_slice(extra_cert.as_bytes());
        write_file(&paths.builtin_ca(), &edited);
        write_file(&paths.extra_ca(), extra_cert.as_bytes());

        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let status_after = reset_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Extra,
            NOW_MS,
        )
        .expect("重置追加证书");
        assert_eq!(
            std::fs::read(paths.extra_ca()).expect("读取追加"),
            Vec::<u8>::new(),
            "extra 重置必须清空为 0 字节"
        );
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取内置副本"),
            edited,
            "extra 重置不得触碰内置副本"
        );
        assert!(
            status_after
                .outbound
                .builtin
                .is_some_and(|item| item.modified),
            "内置副本仍为已修改"
        );
        assert_eq!(audit_count(&conn), 1);
        let (action, target): (String, String) = conn
            .query_row("SELECT action, target FROM audit", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("读取审计");
        assert_eq!(action, "tls_config_change");
        assert_eq!(target, "tls:outbound-ca:extra");

        let status_after = reset_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            NOW_MS,
        )
        .expect("重置内置副本");
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取重置副本"),
            BUILTIN_CA_BUNDLE.as_bytes()
        );
        let manifest = load_seed_manifest(&paths.seed_manifest()).expect("重置后的清单");
        assert_eq!(
            manifest.seeded_sha256,
            sha256_hex(BUILTIN_CA_BUNDLE.as_bytes())
        );
        let builtin = status_after.outbound.builtin.expect("内置状态");
        assert!(!builtin.modified, "重置后必须清除已修改标记");
        assert_eq!(
            status_after
                .outbound
                .extra
                .map_or(0, |item| item.cert_count),
            0,
            "builtin 重置不恢复追加证书"
        );
        assert_eq!(audit_count(&conn), 2);
        let builtin_audits: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE target = 'tls:outbound-ca:builtin'",
                [],
                |row| row.get(0),
            )
            .expect("统计内置重置审计");
        assert_eq!(builtin_audits, 1, "builtin 重置按自身 target 记审计");
    }

    /// 降档确认规则（ADR-040 决策 4）：空内置根与外部私钥必须 `confirm = true`；
    /// 重新生成自签证书不属降档，`confirm = false` 也执行。
    #[test]
    fn confirm_rules_match_degradation_semantics() {
        let workspace = TempWorkspace::new("confirm");
        let paths = workspace.paths();
        initialize(&workspace);
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let original_builtin = std::fs::read(paths.builtin_ca()).expect("原始内置");
        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            "  \n",
            false,
            NOW_MS,
        )
        .expect_err("空内置未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("confirm"), "{}", err.message);
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取"),
            original_builtin
        );

        let (cert, key) = self_signed_pem(&["external.local"]);
        let err = write_server_cert(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            &cert,
            &key,
            false,
            NOW_MS,
        )
        .expect_err("上传外部私钥未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("confirm"), "{}", err.message);

        let status_after = regenerate_server_cert(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            &required_sans(),
            false,
            NOW_MS,
        )
        .expect("重新生成不需要确认（不属降档）");
        assert!(status_after.restart_required);
        assert_eq!(audit_count(&conn), 1, "仅重新生成写一条审计");
    }

    /// 信任材料只读：两档位、追加文件缺失视为空；内置副本缺失报可操作错误且不种入文件。
    #[test]
    fn trust_material_reads_files_without_seeding() {
        let workspace = TempWorkspace::new("trust-material");
        let paths = workspace.paths();

        let err = trust_material(&paths).expect_err("未初始化时内置副本缺失必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
        for expected in [
            paths.builtin_ca().display().to_string(),
            "reset".to_string(),
            "system".to_string(),
        ] {
            assert!(
                err.message.contains(&expected),
                "消息须含 {expected}：{}",
                err.message
            );
        }
        assert!(
            !paths.builtin_ca().exists(),
            "读取信任材料不得种入文件（ADR-040 决策 4）"
        );

        initialize(&workspace);
        let material = trust_material(&paths).expect("初始化后可读取");
        assert_eq!(material.mode, TrustMode::Builtin);
        assert_eq!(material.builtin_pem, BUILTIN_CA_BUNDLE);
        assert!(material.extra_pem.is_empty(), "空追加文件视为空串");

        let (extra_cert, _) = self_signed_pem(&["extra.local"]);
        write_file(&paths.extra_ca(), extra_cert.as_bytes());
        let material = trust_material(&paths).expect("追加证书可读取");
        assert_eq!(material.extra_pem, extra_cert);

        std::fs::remove_file(paths.extra_ca()).expect("删除追加文件");
        let material = trust_material(&paths).expect("追加文件缺失不是错误");
        assert!(material.extra_pem.is_empty());

        write_file(
            &paths.settings(),
            &settings_bytes(&TlsSettings {
                outbound: TrustMode::System,
                ..TlsSettings::default()
            }),
        );
        let material = trust_material(&paths).expect("system 档位可读取");
        assert_eq!(material.mode, TrustMode::System);

        std::fs::remove_file(paths.builtin_ca()).expect("删除内置副本");
        let err = trust_material(&paths).expect_err("内置副本缺失必须报错");
        assert!(err.message.contains("ca-builtin.pem"), "{}", err.message);
    }

    /// 单份 DER 证书描述：主题、SAN、ISO8601 到期与 DER SHA-256；坏 DER 拒绝。
    #[test]
    fn describe_cert_der_reports_subject_sans_expiry_and_sha256() {
        let key = KeyPair::generate().expect("生成测试私钥");
        let params =
            CertificateParams::new(vec!["describe.local".to_string(), "127.0.0.1".to_string()])
                .expect("测试证书参数");
        let cert = params.self_signed(&key).expect("生成测试证书");
        let der = cert.der();
        let meta = describe_cert_der(der.as_ref()).expect("解析 DER 证书");

        assert!(meta.subject.contains("CN="), "主题：{}", meta.subject);
        assert!(
            meta.sans.contains(&"describe.local".to_string()),
            "SAN：{:?}",
            meta.sans
        );
        assert!(
            meta.sans.contains(&"127.0.0.1".to_string()),
            "SAN：{:?}",
            meta.sans
        );
        let not_after = meta.not_after.expect("必须给出生效时间");
        assert!(not_after.ends_with('Z'), "ISO 8601：{not_after}");
        assert!(meta.not_before.is_some());
        assert_eq!(meta.sha256, sha256_hex(der.as_ref()));
        assert_eq!(meta.sha256.len(), 64);

        let err = describe_cert_der(b"not a certificate").expect_err("非法 DER 必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("DER"), "{}", err.message);
    }

    /// CA 校验：超限、空内置、私钥块、不可解析、份数上限、重复告警；失败字节不变。
    #[test]
    fn ca_validation_rejects_invalid_input_and_keeps_bytes() {
        let workspace = TempWorkspace::new("ca-invalid");
        let paths = workspace.paths();
        initialize(&workspace);
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let original = std::fs::read(paths.builtin_ca()).expect("原始内置副本");

        let oversized = "A".repeat(CA_BUNDLE_MAX_BYTES + 1);
        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            &oversized,
            true,
            NOW_MS,
        )
        .expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("上限"), "{}", err.message);
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取"),
            original,
            "超限失败不得改变原文件"
        );

        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            "",
            false,
            NOW_MS,
        )
        .expect_err("空内置未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("confirm"), "{}", err.message);
        assert_eq!(std::fs::read(paths.builtin_ca()).expect("读取"), original);

        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            "",
            true,
            NOW_MS,
        )
        .expect_err("空内置即使确认也必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("不得为空"), "{}", err.message);
        assert_eq!(std::fs::read(paths.builtin_ca()).expect("读取"), original);

        let private_key = format!(
            "{}-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n",
            original_text(&paths)
        );
        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            &private_key,
            true,
            NOW_MS,
        )
        .expect_err("私钥块必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("私钥"), "{}", err.message);
        assert_eq!(std::fs::read(paths.builtin_ca()).expect("读取"), original);

        let wrong_label = format!(
            "{}-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n",
            original_text(&paths)
        );
        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            &wrong_label,
            true,
            NOW_MS,
        )
        .expect_err("非 CERTIFICATE 块必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(
            err.message.contains("仅允许 CERTIFICATE"),
            "{}",
            err.message
        );
        assert_eq!(std::fs::read(paths.builtin_ca()).expect("读取"), original);

        let broken = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            broken,
            true,
            NOW_MS,
        )
        .expect_err("不可解析证书必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert_eq!(std::fs::read(paths.builtin_ca()).expect("读取"), original);

        let (cert, _) = self_signed_pem(&["dup.local"]);
        let err = validate_ca_pem(&cert.repeat(3), CaKind::Extra, 2, "测试文件")
            .expect_err("份数超限必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("份数"), "{}", err.message);

        let duplicated = format!("{cert}{cert}");
        let status_after = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            CaKind::Builtin,
            &duplicated,
            true,
            NOW_MS,
        )
        .expect("重复证书仅告警");
        assert!(
            status_after.notes.iter().any(|note| note.contains("重复")),
            "状态须提示重复证书：{:?}",
            status_after.notes
        );
    }

    fn original_text(paths: &CertPaths) -> String {
        std::fs::read_to_string(paths.builtin_ca()).expect("读取内置副本")
    }

    /// 服务端材料：证书／私钥超限、加密私钥、配对不一致均拒绝；匹配通过。
    #[test]
    fn server_pair_validation_covers_limits_encryption_and_mismatch() {
        let (cert_one, key_one) = self_signed_pem(&["one.local"]);
        let (cert_two, key_two) = self_signed_pem(&["two.local"]);

        validate_server_pair(&cert_one, &key_one).expect("匹配的证书与私钥必须通过");
        let _ = (&cert_two, &key_two);

        let oversized_cert = "A".repeat(SERVER_CERT_MAX_BYTES + 1);
        let err = validate_server_pair(&oversized_cert, &key_one).expect_err("证书超限");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("上限"), "{}", err.message);

        let oversized_key = "A".repeat(SERVER_KEY_MAX_BYTES + 1);
        let err = validate_server_pair(&cert_one, &oversized_key).expect_err("私钥超限");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("上限"), "{}", err.message);

        let encrypted = "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n\
                         -----END ENCRYPTED PRIVATE KEY-----\n";
        let err = validate_server_pair(&cert_one, encrypted).expect_err("加密私钥必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("未加密"), "{}", err.message);

        let err = validate_server_pair(&cert_one, &key_two).expect_err("配对不一致必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("不匹配"), "{}", err.message);
    }

    /// 受管服务端材料：与文件字节一致、可配对；缺失列出路径与补救方向；超限拒绝。
    #[test]
    fn read_server_material_reports_missing_and_over_limit() {
        let workspace = TempWorkspace::new("server-material");
        let paths = workspace.paths();

        // 未初始化：缺失必须可操作（列出两个路径、上传入口与 --tls off）。
        let err = read_server_material(&paths).expect_err("缺失必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
        for expected in [
            paths.server_cert().display().to_string(),
            paths.server_key().display().to_string(),
            "/settings/certs".to_string(),
            "--tls off".to_string(),
        ] {
            assert!(
                err.message.contains(&expected),
                "消息须含 {expected}：{}",
                err.message
            );
        }

        initialize(&workspace);
        let material = read_server_material(&paths).expect("初始化后可读取");
        assert_eq!(
            material.cert_pem,
            std::fs::read_to_string(paths.server_cert()).expect("读取证书")
        );
        assert_eq!(
            material.key_pem,
            std::fs::read_to_string(paths.server_key()).expect("读取私钥")
        );
        validate_server_pair(&material.cert_pem, &material.key_pem)
            .expect("受管材料必须成对且可校验");

        // 证书超限：解析前拒绝，不截断。
        write_file(
            &paths.server_cert(),
            "A".repeat(SERVER_CERT_MAX_BYTES + 1).as_bytes(),
        );
        let err = read_server_material(&paths).expect_err("证书超限必须报错");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("上限"), "{}", err.message);

        // 私钥超限（初始化会修复上一轮的不可用证书）。
        initialize(&workspace);
        write_file(
            &paths.server_key(),
            "A".repeat(SERVER_KEY_MAX_BYTES + 1).as_bytes(),
        );
        let err = read_server_material(&paths).expect_err("私钥超限必须报错");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("上限"), "{}", err.message);
    }

    /// 外部服务端材料写入：未确认拒绝、确认后替换并标记重启、写审计。
    #[test]
    fn write_server_cert_requires_confirm_and_marks_restart() {
        let workspace = TempWorkspace::new("server-write");
        let paths = workspace.paths();
        initialize(&workspace);
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let original_cert = std::fs::read(paths.server_cert()).expect("原始证书");
        let original_key = std::fs::read(paths.server_key()).expect("原始私钥");

        let (cert, key) = self_signed_pem(&["external.local"]);
        let err = write_server_cert(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            &cert,
            &key,
            false,
            NOW_MS,
        )
        .expect_err("未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(
            std::fs::read(paths.server_cert()).expect("读取"),
            original_cert
        );
        assert_eq!(
            std::fs::read(paths.server_key()).expect("读取"),
            original_key
        );
        assert_eq!(audit_count(&conn), 0);

        let status_after = write_server_cert(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            &cert,
            &key,
            true,
            NOW_MS,
        )
        .expect("确认后写入");
        assert!(status_after.restart_required, "入站材料替换需重启");
        assert!(status_after.inbound.cert_present && status_after.inbound.key_present);
        assert!(
            status_after
                .inbound
                .sans
                .contains(&"external.local".to_string()),
            "状态须展示外部证书 SAN：{:?}",
            status_after.inbound.sans
        );
        assert_eq!(
            std::fs::read_to_string(paths.server_cert()).expect("读取"),
            cert
        );
        assert_eq!(
            std::fs::read_to_string(paths.server_key()).expect("读取"),
            key
        );
        assert_eq!(audit_count(&conn), 1);
        let (action, target): (String, String) = conn
            .query_row("SELECT action, target FROM audit", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("读取审计");
        assert_eq!(action, "tls_config_change");
        assert_eq!(target, "tls:inbound-server");
    }

    /// 重新生成：替换证书与私钥、指纹变化、标记重启并写审计。
    #[test]
    fn regenerate_server_cert_replaces_material_and_audits() {
        let workspace = TempWorkspace::new("server-regenerate");
        let paths = workspace.paths();
        initialize(&workspace);
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let before = status(&paths, NOW_MS).expect("初始状态").inbound.sha256;
        let before_key = std::fs::read(paths.server_key()).expect("初始私钥");

        let status_after = regenerate_server_cert(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            &required_sans(),
            true,
            NOW_MS,
        )
        .expect("重新生成");
        assert!(status_after.restart_required);
        assert_ne!(
            status_after.inbound.sha256, before,
            "重新生成后指纹必须变化"
        );
        assert_ne!(
            std::fs::read(paths.server_key()).expect("新私钥"),
            before_key,
            "重新生成后私钥必须变化"
        );
        let cert_pem = std::fs::read_to_string(paths.server_cert()).expect("读取证书");
        let key_pem = std::fs::read_to_string(paths.server_key()).expect("读取私钥");
        validate_server_pair(&cert_pem, &key_pem).expect("重新生成的证书与私钥必须匹配");
        assert_eq!(audit_count(&conn), 1);
    }

    /// 设置：默认、非法 schema、入站 off 确认、能力拒绝与审计。
    #[test]
    fn settings_updates_require_confirmation_capability_and_are_audited() {
        let workspace = TempWorkspace::new("settings");
        let paths = workspace.paths();
        assert_eq!(
            load_settings(&paths).expect("缺失返回默认"),
            TlsSettings::default()
        );
        initialize(&workspace);
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let original = std::fs::read(paths.settings()).expect("原始设置");

        let err = update_settings(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            TlsSettings {
                inbound: InboundMode::Off,
                ..TlsSettings::default()
            },
            false,
            NOW_MS,
        )
        .expect_err("入站 off 未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("confirm"), "{}", err.message);
        assert_eq!(std::fs::read(paths.settings()).expect("读取"), original);
        assert_eq!(audit_count(&conn), 0);

        let reader = Actor::new("cert-reader", [Capability::SettingsCertRead]);
        let err = update_settings(
            &conn,
            &paths,
            &policy,
            &reader,
            &StoreAudit,
            TlsSettings::default(),
            true,
            NOW_MS,
        )
        .expect_err("缺少写能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert_eq!(std::fs::read(paths.settings()).expect("读取"), original);
        assert_eq!(audit_count(&conn), 0, "拒绝不得写审计");

        let status_after = update_settings(
            &conn,
            &paths,
            &policy,
            &actor,
            &StoreAudit,
            TlsSettings {
                inbound: InboundMode::Off,
                ..TlsSettings::default()
            },
            true,
            NOW_MS,
        )
        .expect("确认后执行");
        assert_eq!(status_after.inbound.mode, InboundMode::Off);
        assert!(status_after.restart_required, "入站档位变化需重启");
        assert_eq!(
            load_settings(&paths).expect("读回").inbound,
            InboundMode::Off
        );
        assert_eq!(audit_count(&conn), 1);
        let (action, target, detail): (String, String, String) = conn
            .query_row(
                "SELECT action, target, CAST(CAST(detail_json AS JSON) AS VARCHAR) FROM audit",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("读取审计");
        assert_eq!(action, "tls_config_change");
        assert_eq!(target, "tls:settings");
        let detail: serde_json::Value = serde_json::from_str(&detail).expect("detail");
        assert_eq!(detail["inbound"], serde_json::json!("off"));
        assert!(detail.get("pem").is_none(), "审计不得含 PEM 正文");

        // 非法 schema：设置文件损坏时 load_settings 报错，status 以 notes 区分。
        write_file(
            &paths.settings(),
            br#"{"schema_version": 9, "outbound": "builtin", "inbound": "builtin"}"#,
        );
        let err = load_settings(&paths).expect_err("非法 schema 必须报错");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("schema_version"), "{}", err.message);
        let status = status(&paths, NOW_MS).expect("状态容错");
        assert!(
            status
                .notes
                .iter()
                .any(|note| note.contains("schema_version")),
            "状态须说明原因：{:?}",
            status.notes
        );
    }

    /// 审计失败回滚：文件字节恢复原状，返回错误而非成功。
    #[test]
    fn audit_failure_rolls_back_files() {
        let workspace = TempWorkspace::new("audit-fail");
        let paths = workspace.paths();
        initialize(&workspace);
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let original_builtin = std::fs::read(paths.builtin_ca()).expect("原始内置");
        let original_settings = std::fs::read(paths.settings()).expect("原始设置");

        let (replacement, _) = self_signed_pem(&["only.local"]);
        let err = write_ca(
            &conn,
            &paths,
            &policy,
            &actor,
            &FailingAudit,
            CaKind::Builtin,
            &replacement,
            true,
            NOW_MS,
        )
        .expect_err("审计失败必须返回错误");
        assert_eq!(
            std::fs::read(paths.builtin_ca()).expect("读取"),
            original_builtin,
            "审计失败必须恢复原字节"
        );
        assert!(err.message.contains("回滚"), "{}", err.message);
        assert_eq!(audit_count(&conn), 0);

        let err = update_settings(
            &conn,
            &paths,
            &policy,
            &actor,
            &FailingAudit,
            TlsSettings {
                outbound: TrustMode::System,
                ..TlsSettings::default()
            },
            true,
            NOW_MS,
        )
        .expect_err("设置审计失败必须返回错误");
        assert_eq!(
            std::fs::read(paths.settings()).expect("读取"),
            original_settings,
            "审计失败必须恢复原设置"
        );
        assert!(err.message.contains("回滚"), "{}", err.message);

        let original_cert = std::fs::read(paths.server_cert()).expect("原始证书");
        let original_key = std::fs::read(paths.server_key()).expect("原始私钥");
        let (cert, key) = self_signed_pem(&["audit-fail.local"]);
        let err = write_server_cert(
            &conn,
            &paths,
            &policy,
            &actor,
            &FailingAudit,
            &cert,
            &key,
            true,
            NOW_MS,
        )
        .expect_err("服务端材料审计失败必须返回错误");
        assert_eq!(
            std::fs::read(paths.server_cert()).expect("读取"),
            original_cert
        );
        assert_eq!(
            std::fs::read(paths.server_key()).expect("读取"),
            original_key
        );
        assert!(err.message.contains("回滚"), "{}", err.message);
    }

    /// 状态：未初始化、空文件、损坏文件可区分；系统档位 builtin 为 None。
    #[test]
    fn status_distinguishes_missing_empty_and_corrupt_files() {
        let workspace = TempWorkspace::new("status-shapes");
        let paths = workspace.paths();
        let blank = status(&paths, NOW_MS).expect("未初始化状态");
        assert!(
            blank.notes.iter().any(|note| note.contains("不存在")),
            "未初始化须提示缺失：{:?}",
            blank.notes
        );
        assert!(
            blank
                .outbound
                .builtin
                .as_ref()
                .is_some_and(|item| item.cert_count == 0),
            "缺少内置副本时份数为 0"
        );

        std::fs::create_dir_all(&paths.dir).expect("创建目录");
        write_file(&paths.settings(), &settings_bytes(&TlsSettings::default()));
        write_file(&paths.builtin_ca(), b"");
        let empty = status(&paths, NOW_MS).expect("空文件状态");
        assert!(
            empty.notes.iter().any(|note| note.contains("为空")),
            "空内置须与缺失区分：{:?}",
            empty.notes
        );

        write_file(&paths.builtin_ca(), b"not a pem\n");
        let corrupt = status(&paths, NOW_MS).expect("损坏文件状态");
        assert!(
            corrupt
                .notes
                .iter()
                .any(|note| note.contains("解析失败") || note.contains("PEM 块")),
            "损坏文件须说明解析失败：{:?}",
            corrupt.notes
        );
        assert_ne!(
            empty.outbound.builtin.map(|item| item.sha256),
            corrupt.outbound.builtin.map(|item| item.sha256),
            "空文件与损坏文件的哈希必须不同"
        );

        let system = TlsSettings {
            outbound: TrustMode::System,
            inbound: InboundMode::Off,
            ..TlsSettings::default()
        };
        write_file(&paths.settings(), &settings_bytes(&system));
        let status = status(&paths, NOW_MS).expect("系统档位状态");
        assert!(
            status.outbound.builtin.is_none(),
            "system 档位内置副本为 None"
        );
        // T188：system 档位读取操作系统证书库——成功须给出份数（0 由 notes 说明原因），
        // 失败须写 notes 而不是给出 0（不伪装成“正确空”）。
        match status.outbound.system_root_count {
            Some(count) => {
                assert!(
                    count > 0,
                    "系统库读取成功时份数应大于 0（0 须在 notes 中说明）：{count}"
                );
                let extra_count = status
                    .outbound
                    .extra
                    .as_ref()
                    .map_or(0, |item| item.cert_count);
                assert_eq!(
                    status.outbound.effective_root_count,
                    Some(count + extra_count),
                    "system 档位的生效根总数应为系统库份数 + 追加份数"
                );
            }
            None => {
                assert!(
                    status
                        .notes
                        .iter()
                        .any(|note| note.contains("系统证书库不可用")),
                    "读库失败须写 notes：{:?}",
                    status.notes
                );
                assert_eq!(
                    status.outbound.effective_root_count, None,
                    "系统库份数未知时不得给出猜测值"
                );
            }
        }
        assert_eq!(status.outbound.source, "system");
    }

    /// 不可写路径（证书库目录被同名文件占用）：返回可操作 io_error，不 panic。
    #[test]
    fn unwritable_cert_directory_is_actionable_io_error() {
        let workspace = TempWorkspace::new("unwritable");
        let config = workspace.root.join(".whcode");
        std::fs::create_dir_all(&config).expect("创建 .whcode");
        write_file(&config.join("certs"), b"occupied");
        let err = ensure_initialized(&workspace.paths(), &required_sans(), NOW_MS)
            .expect_err("证书库目录不可创建必须报错");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(
            err.message.contains("certs"),
            "消息须含路径：{}",
            err.message
        );
    }

    /// 过期自动重新生成；临近到期只告警不替换。
    #[test]
    fn expired_certificate_regenerates_and_near_expiry_warns() {
        let workspace = TempWorkspace::new("expiry");
        let paths = workspace.paths();
        initialize(&workspace);

        let effective = effective_sans(&required_sans());
        let sans: Vec<&str> = effective.iter().map(String::as_str).collect();
        let (expired_cert, expired_key) =
            cert_with_validity(&sans, NOW_MS - 10 * DAY_MS, NOW_MS - DAY_MS);
        write_file(&paths.server_cert(), expired_cert.as_bytes());
        write_file(&paths.server_key(), expired_key.as_bytes());
        let expired_sha = parse_certificate_blocks(expired_cert.as_bytes(), "过期测试证书")
            .expect("解析过期证书")
            .first()
            .expect("至少一份证书")
            .sha256
            .clone();
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("过期重新生成");
        assert!(report.regenerated_server_cert, "过期证书必须重新生成");
        let regenerated_note = report
            .notes
            .iter()
            .find(|note| note.contains("已过期"))
            .expect("须记录过期重新生成的原因");
        assert!(
            regenerated_note.contains("旧指纹") && regenerated_note.contains(&expired_sha),
            "须记录旧指纹 {expired_sha}：{regenerated_note}"
        );
        let status_after = status(&paths, NOW_MS).expect("状态");
        let new_sha = status_after.inbound.sha256.as_deref().expect("新指纹");
        assert!(
            regenerated_note.contains("新指纹") && regenerated_note.contains(new_sha),
            "须记录新指纹 {new_sha}：{regenerated_note}"
        );
        assert!(
            status_after
                .inbound
                .days_remaining
                .is_some_and(|days| days > 0)
        );

        let (soon_cert, soon_key) =
            cert_with_validity(&sans, NOW_MS - DAY_MS, NOW_MS + 10 * DAY_MS);
        write_file(&paths.server_cert(), soon_cert.as_bytes());
        write_file(&paths.server_key(), soon_key.as_bytes());
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("临近到期");
        assert!(!report.regenerated_server_cert, "临近到期不得自动替换");
        assert!(
            report.notes.iter().any(|note| {
                note.contains("到期") && note.contains(&format!("不足 {CERT_RENEW_WARN_DAYS} 天"))
            }),
            "须按 {CERT_RENEW_WARN_DAYS} 天阈值告警到期：{:?}",
            report.notes
        );
        let status_after = status(&paths, NOW_MS).expect("状态");
        assert_eq!(
            std::fs::read(paths.server_cert()).expect("读取"),
            soon_cert.as_bytes()
        );
        assert!(
            status_after
                .inbound
                .days_remaining
                .is_some_and(|days| days <= 10)
        );
    }

    /// 受管私钥超限或与证书不匹配：builtin 档位重新生成而不是带着不可用材料启动。
    #[test]
    fn unusable_builtin_key_triggers_regeneration() {
        let workspace = TempWorkspace::new("key-repair");
        let paths = workspace.paths();
        initialize(&workspace);

        write_file(
            &paths.server_key(),
            "A".repeat(SERVER_KEY_MAX_BYTES + 1).as_bytes(),
        );
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("超限私钥修复");
        assert!(report.regenerated_server_cert, "超限私钥必须触发重新生成");
        let cert_pem = std::fs::read_to_string(paths.server_cert()).expect("读取证书");
        let key_pem = std::fs::read_to_string(paths.server_key()).expect("读取私钥");
        validate_server_pair(&cert_pem, &key_pem).expect("修复后的材料必须匹配");

        let (_other_cert, other_key) = self_signed_pem(&["mismatch.local"]);
        write_file(&paths.server_key(), other_key.as_bytes());
        let report = ensure_initialized(&paths, &required_sans(), NOW_MS).expect("不匹配私钥修复");
        assert!(report.regenerated_server_cert, "不匹配私钥必须触发重新生成");
        let cert_pem = std::fs::read_to_string(paths.server_cert()).expect("读取证书");
        let key_pem = std::fs::read_to_string(paths.server_key()).expect("读取私钥");
        validate_server_pair(&cert_pem, &key_pem).expect("修复后的材料必须匹配");
    }

    /// 内置资产本身可用：可解析、无重复以外的结构问题、不含私钥。
    #[test]
    fn bundled_ca_asset_is_valid_and_private_key_free() {
        let summary = validate_ca_pem(
            BUILTIN_CA_BUNDLE,
            CaKind::Builtin,
            MAX_CERT_COUNT,
            "assets/certs/ca-bundle.pem",
        )
        .expect("内置资产必须可校验");
        assert_eq!(summary.cert_count, 118);
        assert_eq!(summary.sha256, sha256_hex(BUILTIN_CA_BUNDLE.as_bytes()));
        assert!(
            !BUILTIN_CA_BUNDLE.contains("PRIVATE KEY"),
            "内置资产不得含私钥块"
        );
        assert!(!BUILTIN_BUNDLE_VERSION.is_empty());
    }
}
