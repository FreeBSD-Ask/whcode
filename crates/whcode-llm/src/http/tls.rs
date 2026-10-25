//! # 出站信任装配（T188、ADR-040 决策 2）
//!
//! 把工作区证书库的两档信任来源变成一次可复用、可校验的装配结果：
//!
//! - `builtin`：`ca-builtin.pem`（内置 Mozilla 根证书副本）为基线，`ca-extra.pem` 追加；
//! - `system`：操作系统证书库（Windows ROOT 存储，经 `rustls-native-certs`）为基线，
//!   `ca-extra.pem` 追加。
//!
//! 三个不变量在此处落为代码约束，而不是注释约定：
//!
//! 1. **永不关闭校验**：[`build_agent`] 只设置信任根集合，不提供跳过证书链或主机名校验的
//!    参数，也不存在“信任所有”档位（ADR-040 决策 2）。
//! 2. **系统库为空不得静默回落**：读取成功但 0 份证书时返回可操作错误，不改用内置库。
//! 3. **有界**：单文件 PEM 上限 [`MAX_ROOT_PEM_BYTES`]、信任根总数上限 [`MAX_ROOT_CERTS`]、
//!    [`probe`] 总预算 [`PROBE_TIMEOUT`]；超限一律报错，不截断、不静默丢弃。
//!
//! [`probe`] 只做 TLS 握手：不发送任何 HTTP 请求、不携带凭据，返回对端证书链与协议版本，
//! 供 `POST /api/certs/verify` 的连通性校验使用（ADR-040 决策 4）。
//!
//! 错误消息一律为中文且可操作，不包含私钥或凭据内容；生产路径不使用 `unwrap`／`expect`。

use std::collections::HashSet;
use std::io::{BufReader, ErrorKind};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::pem::SectionKind;
use rustls::pki_types::{CertificateDer, ServerName};
use whcode_domain::{ErrorCode, TrustMode, WhError};

/// 信任根总数上限（份数；内置／系统库与追加证书合并后计数）。
pub const MAX_ROOT_CERTS: usize = 2048;
/// 单个根证书 PEM 文件的字节上限（内置副本与追加文件各自适用）。
pub const MAX_ROOT_PEM_BYTES: usize = 1024 * 1024;
/// 连通性校验的总预算：DNS 解析之后逐个地址的连接与握手都必须在此预算内完成。
///
/// 已知限制：DNS 解析本身可能超过该预算（`ToSocketAddrs` 同步解析，无超时参数），
/// 本预算只覆盖“解析完成之后”的连接与握手阶段。
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// 信任装配结果：生效根证书 DER 集合与各来源份数（供状态与测试断言）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustLoad {
    /// 生效信任根的 DER 编码，已去重并保留首次出现顺序。
    pub roots: Vec<Vec<u8>>,
    /// 操作系统证书库读取份数（仅 `system` 档位为 `Some`；0 已在此前报错）。
    pub system_count: Option<usize>,
    /// 内置根证书副本份数（仅 `builtin` 档位计数）。
    pub builtin_count: usize,
    /// 追加根证书份数（`ca-extra.pem` 可解析的 CERTIFICATE 块数，含与基线重复者）。
    pub extra_count: usize,
}

/// 一次只握手的校验结果（不含任何请求载荷）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// 对端提供的证书链（叶子在前，DER 编码，顺序为服务端发送顺序）。
    pub leaf_chain_der: Vec<Vec<u8>>,
    /// 协商到的 TLS 协议版本（如 `TLSv1_3`）；由 `rustls` 以调试形式给出。
    pub tls_version: Option<String>,
}

/// 解析给定档位的信任根集合（`roots` 为生效并集，已去重）。
///
/// `builtin` 档位以 `builtin_pem` 为基线；`system` 档位忽略 `builtin_pem`，改读操作系统
/// 证书库；两者的 `extra_pem` 都追加。
///
/// # Errors
///
/// - PEM 超限、含私钥块、含非 CERTIFICATE 块、非空但无 CERTIFICATE 块或证书不可解析时为
///   [`ErrorCode::ParseError`]；
/// - `builtin` 档位内置副本为空、`system` 档位读库失败或 0 份证书、信任根总数超限时为
///   [`ErrorCode::InvalidState`]（不回落到另一档位）。
pub fn load_roots(
    mode: TrustMode,
    builtin_pem: &str,
    extra_pem: &str,
) -> Result<TrustLoad, WhError> {
    load_roots_with_system(system_roots, mode, builtin_pem, extra_pem)
}

/// 构造启用 `builtin`／`system` 信任的 `ureq` Agent（不关闭任何校验）。
///
/// 只设置信任根集合：TLS 提供者、协议版本、SNI 与主机名校验均保持 `ureq` 默认。
///
/// # Errors
///
/// 与 [`load_roots`] 相同：信任来源不可用或输入超限时报错，不返回“信任所有”的客户端。
pub fn build_agent(
    mode: TrustMode,
    builtin_pem: &str,
    extra_pem: &str,
) -> Result<ureq::Agent, WhError> {
    let load = load_roots(mode, builtin_pem, extra_pem)?;
    Ok(agent_from_roots(&load.roots))
}

/// 以当前信任完成一次只握手校验，返回对端证书链与协议版本。
///
/// 不发送 HTTP 请求、不携带凭据；连接与握手共用 [`PROBE_TIMEOUT`] 预算。
///
/// # Errors
///
/// 信任来源不可用（同 [`load_roots`]）、主机名不合法、DNS 解析失败、连接失败、预算内未完成
/// 握手（[`ErrorCode::Timeout`]）或证书校验失败（未受信／主机名不匹配／已过期，
/// [`ErrorCode::InvalidState`]）时返回结构化错误。
pub fn probe(
    host: &str,
    port: u16,
    mode: TrustMode,
    builtin_pem: &str,
    extra_pem: &str,
) -> Result<ProbeOutcome, WhError> {
    let load = load_roots(mode, builtin_pem, extra_pem)?;
    probe_roots(host, port, &load.roots)
}

/// 允许注入系统库读取器的装配入口。
///
/// 生产路径由 [`load_roots`] 传入真实读取器（[`system_roots`]）；测试传入固定根集合，
/// 从而在不读真实操作系统证书库的前提下覆盖 `system` 档位。`pub(crate)` 保证不对外暴露
/// “自定义信任来源”的绕过面。
pub(crate) fn load_roots_with_system(
    loader: impl FnOnce() -> Result<Vec<Vec<u8>>, WhError>,
    mode: TrustMode,
    builtin_pem: &str,
    extra_pem: &str,
) -> Result<TrustLoad, WhError> {
    let mut roots: Vec<Vec<u8>> = Vec::new();
    let mut seen: HashSet<Vec<u8>> = HashSet::new();
    let mut system_count = None;
    let mut builtin_count = 0usize;
    match mode {
        TrustMode::Builtin => {
            let builtin = parse_pem_roots(builtin_pem, "内置根证书副本 ca-builtin.pem")?;
            if builtin.is_empty() {
                return Err(WhError::invalid_state(
                    "内置根证书副本 ca-builtin.pem 不含任何 CERTIFICATE 块：builtin 档位不得以空信任\
                     集启动；请执行证书库 reset 恢复内置资产，或把出站档位改为 system（ADR-040 决策 2）",
                ));
            }
            builtin_count = builtin.len();
            push_unique(&mut roots, &mut seen, builtin, "内置根证书副本")?;
        }
        TrustMode::System => {
            let system = loader()?;
            if system.is_empty() {
                return Err(WhError::invalid_state(
                    "操作系统证书库读取成功但为 0 份证书：system 档位不得回落到内置库；请确认 OS 根\
                     证书存储可用（Windows：certlm.msc 的“受信任的根证书颁发机构”），或把出站档位\
                     改回 builtin（ADR-040 决策 2）",
                ));
            }
            if system.len() > MAX_ROOT_CERTS {
                return Err(WhError::invalid_state(format!(
                    "操作系统证书库返回 {} 份证书，超过上限 {MAX_ROOT_CERTS}；请清理 OS 根证书存储\
                     后重试（ADR-040 决策 2 的有界口径）",
                    system.len()
                )));
            }
            system_count = Some(system.len());
            push_unique(&mut roots, &mut seen, system, "操作系统证书库")?;
        }
    }
    let extra = parse_pem_roots(extra_pem, "追加根证书 ca-extra.pem")?;
    let extra_count = extra.len();
    push_unique(&mut roots, &mut seen, extra, "追加根证书")?;
    if roots.is_empty() {
        return Err(WhError::invalid_state(
            "生效信任根为 0 份：不得以空信任集发起 TLS 连接（ADR-040 决策 2）",
        ));
    }
    Ok(TrustLoad {
        roots,
        system_count,
        builtin_count,
        extra_count,
    })
}

/// 读取操作系统证书库（Windows ROOT 存储，经 `rustls-native-certs`）。
///
/// 读取过程中的任一错误都使整个读取失败：部分成功的信任集合会掩盖缺失的根，属
/// “错误的空”，不得视为可用（ADR-040 决策 2、信号链完备原则）。
fn system_roots() -> Result<Vec<Vec<u8>>, WhError> {
    let result = rustls_native_certs::load_native_certs();
    if let Some(first) = result.errors.first() {
        return Err(WhError::invalid_state(format!(
            "读取操作系统证书库失败（{} 个错误，首个：{first}）；请确认 OS 根证书存储可访问，\
             或把出站档位改回 builtin（system 档位不回落内置库，ADR-040 决策 2）",
            result.errors.len()
        )));
    }
    Ok(result
        .certs
        .iter()
        .map(|cert| cert.as_ref().to_vec())
        .collect())
}

/// 解析单份 PEM 中的全部 CERTIFICATE 块（仅接受证书，拒绝私钥与其它块）。
fn parse_pem_roots(pem: &str, source: &str) -> Result<Vec<Vec<u8>>, WhError> {
    if pem.len() > MAX_ROOT_PEM_BYTES {
        return Err(WhError::parse_error(format!(
            "{source} 超过上限 {MAX_ROOT_PEM_BYTES} 字节（当前 {} 字节）；请裁剪后重试\
             （ADR-040 决策 1 的拒绝阈值）",
            pem.len()
        )));
    }
    reject_private_key_blocks(pem, source)?;
    let mut store = rustls::RootCertStore::empty();
    let mut roots: Vec<Vec<u8>> = Vec::new();
    let mut reader = BufReader::new(pem.as_bytes());
    loop {
        let section = rustls_pki_types::pem::from_buf(&mut reader).map_err(|err| {
            WhError::parse_error(format!(
                "{source}：PEM 块读取失败（{err}）；请确认每块有匹配的 BEGIN/END 行且为 Base64"
            ))
        })?;
        let Some((kind, der)) = section else { break };
        match kind {
            SectionKind::Certificate => {}
            SectionKind::RsaPrivateKey | SectionKind::PrivateKey | SectionKind::EcPrivateKey => {
                return Err(WhError::parse_error(format!(
                    "{source} 含私钥块：私钥不得混入根证书文件；根证书文件只放 CERTIFICATE 块"
                )));
            }
            other => {
                return Err(WhError::parse_error(format!(
                    "{source} 含不允许的 PEM 块（{other:?}）：仅允许 CERTIFICATE 块\
                     （ADR-040 决策 1）"
                )));
            }
        }
        if roots.len() >= MAX_ROOT_CERTS {
            return Err(WhError::parse_error(format!(
                "{source} 的证书份数超过上限 {MAX_ROOT_CERTS}；请裁剪后重试（ADR-040 决策 1）"
            )));
        }
        store
            .add(CertificateDer::from(der.as_slice()))
            .map_err(|err| {
                WhError::parse_error(format!(
                    "{source}：第 {} 份证书无法作为信任根解析（{err}）；请确认它是完整的 X.509 \
                     证书（DER 结构完整）",
                    roots.len() + 1
                ))
            })?;
        roots.push(der);
    }
    if roots.is_empty() && !pem.trim().is_empty() {
        return Err(WhError::parse_error(format!(
            "{source} 非空但不含任何 CERTIFICATE 块：无法确认这是证书文件；空文件才表示“无证书”"
        )));
    }
    Ok(roots)
}

/// 扫描 PEM 的 BEGIN 行，拒绝任何带私钥标签的块。
///
/// 底层 PEM 解析器会跳过未知标签（例如 `ENCRYPTED PRIVATE KEY`），因此私钥检测必须在
/// 解析前按标签显式执行，避免私钥被静默忽略而不是被拒绝。
fn reject_private_key_blocks(pem: &str, source: &str) -> Result<(), WhError> {
    for line in pem.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("-----BEGIN ") {
            continue;
        }
        let label = trimmed
            .trim_start_matches("-----BEGIN ")
            .trim_end_matches("-----")
            .trim();
        if label.contains("PRIVATE KEY") {
            return Err(WhError::parse_error(format!(
                "{source} 含私钥块“{label}”：私钥不得混入根证书文件；根证书文件只放 CERTIFICATE 块"
            )));
        }
    }
    Ok(())
}

/// 追加去重后的证书，并在合并后总数超限时报错（保留首次出现顺序）。
fn push_unique(
    roots: &mut Vec<Vec<u8>>,
    seen: &mut HashSet<Vec<u8>>,
    certs: Vec<Vec<u8>>,
    source: &str,
) -> Result<(), WhError> {
    for der in certs {
        if seen.contains(&der) {
            continue;
        }
        if roots.len() >= MAX_ROOT_CERTS {
            return Err(WhError::invalid_state(format!(
                "信任根总数超过上限 {MAX_ROOT_CERTS}（合并 {source} 时触发）；请减少 ca-extra.pem \
                 中的证书份数，或改用内置库作为基线（ADR-040 决策 2）"
            )));
        }
        seen.insert(der.clone());
        roots.push(der);
    }
    Ok(())
}

/// 由信任根 DER 构造 `ureq` Agent：只设置根集合，其余保持默认（校验恒开）。
fn agent_from_roots(roots: &[Vec<u8>]) -> ureq::Agent {
    let certs: Vec<ureq::tls::Certificate<'static>> = roots
        .iter()
        .map(|der| ureq::tls::Certificate::from_der(der).to_owned())
        .collect();
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::from(certs))
        .build();
    let config = ureq::Agent::config_builder().tls_config(tls).build();
    ureq::Agent::new_with_config(config)
}

/// 由信任根 DER 构造 `rustls` 客户端配置（不启用客户端认证，校验恒开）。
fn client_config(roots: &[Vec<u8>]) -> Result<rustls::ClientConfig, WhError> {
    let mut store = rustls::RootCertStore::empty();
    for (index, der) in roots.iter().enumerate() {
        store
            .add(CertificateDer::from(der.as_slice()))
            .map_err(|err| {
                WhError::parse_error(format!(
                    "第 {} 份信任根无法作为信任锚解析（{err}）；请确认它是完整的 X.509 证书",
                    index + 1
                ))
            })?;
    }
    if store.is_empty() {
        return Err(WhError::invalid_state(
            "生效信任根为 0 份：不得以空信任集发起 TLS 握手（ADR-040 决策 2）",
        ));
    }
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(store)
        .with_no_client_auth())
}

/// 以给定信任根完成一次握手（`probe` 的实体；测试经此注入系统库根集合）。
fn probe_roots(host: &str, port: u16, roots: &[Vec<u8>]) -> Result<ProbeOutcome, WhError> {
    let config = client_config(roots)?;
    let name = ServerName::try_from(host.to_string()).map_err(|_| {
        WhError::parse_error(format!(
            "目标主机“{host}”不是合法的 DNS 名称或 IP 字面量，无法做 TLS 主机名校验；请改用证书 \
             SAN 覆盖的主机名（不得关闭校验，ADR-040 决策 2）"
        ))
    })?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut stream = connect_within(host, port, deadline)?;
    let mut connection = rustls::ClientConnection::new(Arc::new(config), name).map_err(|err| {
        WhError::invalid_state(format!(
            "建立 TLS 会话失败（{err}）；请确认目标 {host}:{port} 提供 TLS 服务"
        ))
    })?;
    while connection.is_handshaking() {
        let Some(remaining) = remaining_budget(deadline) else {
            return Err(probe_timeout(host, port));
        };
        set_io_timeout(&stream, remaining)?;
        match connection.complete_io(&mut stream) {
            Ok(_) => {}
            Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(probe_timeout(host, port));
            }
            Err(err) => return Err(handshake_error(&err, host, port)),
        }
    }
    let chain: Vec<Vec<u8>> = connection
        .peer_certificates()
        .map(|certs| certs.iter().map(|cert| cert.as_ref().to_vec()).collect())
        .unwrap_or_default();
    if chain.is_empty() {
        return Err(WhError::invalid_state(format!(
            "与 {host}:{port} 的 TLS 握手完成但未取得对端证书链：无法确认信任状态；请确认对端为 \
             标准 TLS 服务"
        )));
    }
    Ok(ProbeOutcome {
        leaf_chain_der: chain,
        tls_version: connection
            .protocol_version()
            .map(|version| format!("{version:?}")),
    })
}

/// 在预算内连接目标（逐个尝试解析出的地址）。
fn connect_within(host: &str, port: u16, deadline: Instant) -> Result<TcpStream, WhError> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|err| {
            WhError::invalid_state(format!(
                "解析目标 {host}:{port} 失败（{err}）；请检查主机名与 DNS 配置。已知限制：DNS 解析\
                 本身不受 {} 秒总预算约束（ADR-040 决策 4）",
                PROBE_TIMEOUT.as_secs()
            ))
        })?
        .collect();
    if addrs.is_empty() {
        return Err(WhError::invalid_state(format!(
            "目标 {host}:{port} 未解析出任何地址；请检查主机名与 DNS 配置"
        )));
    }
    let mut last_error = None;
    for addr in addrs {
        let Some(remaining) = remaining_budget(deadline) else {
            break;
        };
        match TcpStream::connect_timeout(&addr, remaining) {
            Ok(stream) => return Ok(stream),
            Err(err) => last_error = Some((addr, err)),
        }
    }
    if remaining_budget(deadline).is_none() {
        return Err(probe_timeout(host, port));
    }
    match last_error {
        Some((addr, err)) => Err(WhError::io(
            &format!(
                "连接 {addr}（目标 {host}:{port}）失败；请确认目标端口已监听、网络可达且未被代理拦截"
            ),
            &err,
        )),
        None => Err(probe_timeout(host, port)),
    }
}

/// 预算剩余量；已用尽返回 `None`。
fn remaining_budget(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    (!remaining.is_zero()).then_some(remaining)
}

fn set_io_timeout(stream: &TcpStream, remaining: Duration) -> Result<(), WhError> {
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|err| WhError::io("设置 TLS 握手读超时（套接字可能已关闭或权限不足）", &err))?;
    stream
        .set_write_timeout(Some(remaining))
        .map_err(|err| WhError::io("设置 TLS 握手写超时（套接字可能已关闭或权限不足）", &err))
}

fn probe_timeout(host: &str, port: u16) -> WhError {
    WhError::new(
        ErrorCode::Timeout,
        format!(
            "TLS 握手未能在 {} 秒总预算内完成（目标 {host}:{port}）；请确认目标可达后重试\
             （ADR-040 决策 4 的单次 2 秒上限）",
            PROBE_TIMEOUT.as_secs()
        ),
    )
}

/// 把握手失败映射为可操作错误（未受信／主机名不匹配／已过期／其它）。
fn handshake_error(err: &std::io::Error, host: &str, port: u16) -> WhError {
    let Some(tls_error) = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    else {
        return WhError::io(&format!("与 {host}:{port} 的 TLS 握手失败"), err);
    };
    match tls_error {
        rustls::Error::InvalidCertificate(cert_error) => match cert_error {
            rustls::CertificateError::UnknownIssuer => WhError::invalid_state(format!(
                "与 {host}:{port} 的 TLS 握手被拒绝：证书链的签发者不在当前信任集合内；请把对应\
                 根证书加入 ca-extra.pem，或把出站档位改为 system（不得关闭校验，ADR-040 决策 2）"
            )),
            rustls::CertificateError::NotValidForName
            | rustls::CertificateError::NotValidForNameContext { .. } => {
                WhError::invalid_state(format!(
                    "与 {host}:{port} 的 TLS 握手被拒绝：对端证书 SAN 不包含目标主机“{host}”；\
                     请改用证书覆盖的主机名重试"
                ))
            }
            rustls::CertificateError::Expired | rustls::CertificateError::ExpiredContext { .. } => {
                WhError::invalid_state(format!(
                    "与 {host}:{port} 的 TLS 握手被拒绝：对端证书已过期；请联系服务方更换证书"
                ))
            }
            other => WhError::invalid_state(format!(
                "与 {host}:{port} 的 TLS 握手被拒绝（证书校验失败：{other:?}）；请检查对端证书\
                 与当前信任集合"
            )),
        },
        other => WhError::invalid_state(format!(
            "与 {host}:{port} 的 TLS 握手失败（{other}）；请确认目标为 TLS 服务且使用受信证书"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedIssuer, DistinguishedName, DnType,
        ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, date_time_ymd,
    };
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};

    /// 内置根证书资产（Task 2 入库；与 `assets/certs/SOURCES.md` 同源）。
    const BUILTIN_CA_BUNDLE: &str = include_str!("../../../../assets/certs/ca-bundle.pem");

    /// 测试 PKI：自签 CA（信任锚）+ 由它签发的叶子证书（服务端证书）。
    struct TestPki {
        ca_pem: String,
        ca_der: Vec<u8>,
        chain_der: Vec<CertificateDer<'static>>,
        key_der: Vec<u8>,
    }

    /// 生成 ECDSA P-256 测试 PKI；CA 有效期 2020—2035，叶子有效期由调用方给定。
    ///
    /// `label` 进入 CA 与叶子的 CN：不同 PKI 的可分辨名称必须不同，否则“不受信”会表现为签名
    /// 校验失败（同名但异钥），而不是“签发者未知”。
    fn test_pki(
        label: &str,
        sans: &[&str],
        not_before: (i32, u8, u8),
        not_after: (i32, u8, u8),
    ) -> TestPki {
        let ca_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("生成 CA 密钥");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("CA 参数");
        let mut ca_dn = DistinguishedName::new();
        ca_dn.push(DnType::CommonName, format!("WHCode Test {label} CA"));
        ca_params.distinguished_name = ca_dn;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        ca_params.not_before = date_time_ymd(2020, 1, 1);
        ca_params.not_after = date_time_ymd(2035, 1, 1);
        let ca = CertifiedIssuer::self_signed(ca_params, ca_key).expect("生成 CA 证书");

        let leaf_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("生成叶子密钥");
        let mut leaf_params = CertificateParams::new(
            sans.iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>(),
        )
        .expect("叶子参数");
        let mut leaf_dn = DistinguishedName::new();
        leaf_dn.push(DnType::CommonName, format!("WHCode Test {label} Server"));
        leaf_params.distinguished_name = leaf_dn;
        leaf_params.is_ca = IsCa::NoCa;
        leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        leaf_params.not_before = date_time_ymd(not_before.0, not_before.1, not_before.2);
        leaf_params.not_after = date_time_ymd(not_after.0, not_after.1, not_after.2);
        let leaf = leaf_params.signed_by(&leaf_key, &ca).expect("签发叶子证书");

        TestPki {
            ca_pem: ca.pem(),
            ca_der: ca.der().to_vec(),
            chain_der: vec![leaf.der().clone(), ca.der().clone()],
            key_der: leaf_key.serialize_der(),
        }
    }

    /// 本机回环最小 TLS 服务：单连接一次握手，线程有界并在 `Drop` 时 join。
    struct TestServer {
        port: u16,
        stop: Arc<AtomicBool>,
        handle: Option<JoinHandle<()>>,
    }

    impl TestServer {
        fn start(chain: Vec<CertificateDer<'static>>, key: Vec<u8>) -> Self {
            let config = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(chain, PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))
                .expect("测试服务端 TLS 配置");
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("绑定回环测试端口");
            listener.set_nonblocking(true).expect("非阻塞监听");
            let port = listener.local_addr().expect("读取本地端口").port();
            let stop = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&stop);
            let handle = std::thread::spawn(move || {
                let config = Arc::new(config);
                // 有界轮询：最多 5 秒或直到 `stop` 置位；不为每个连接新建线程。
                let deadline = Instant::now() + Duration::from_secs(5);
                while !flag.load(Ordering::Relaxed) && Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _peer)) => {
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
                            if let Ok(mut connection) =
                                rustls::ServerConnection::new(Arc::clone(&config))
                            {
                                // 客户端校验失败会主动断开：此处错误属预期，不视为测试失败。
                                let _ = connection.complete_io(&mut stream);
                            }
                        }
                        Err(err) if err.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                port,
                stop,
                handle: Some(handle),
            }
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    /// 注入固定系统库根集合执行握手（覆盖 `system` 档位而不读真实 OS 证书库）。
    fn probe_with_system(
        loader: impl FnOnce() -> Result<Vec<Vec<u8>>, WhError>,
        host: &str,
        port: u16,
        mode: TrustMode,
        builtin_pem: &str,
        extra_pem: &str,
    ) -> Result<ProbeOutcome, WhError> {
        let load = load_roots_with_system(loader, mode, builtin_pem, extra_pem)?;
        probe_roots(host, port, &load.roots)
    }

    /// 1. builtin 档位：受信根握手成功，返回叶子链与协议版本。
    #[test]
    fn builtin_trusted_root_completes_handshake_and_returns_chain() {
        let pki = test_pki(
            "trusted",
            &["localhost", "127.0.0.1"],
            (2024, 1, 1),
            (2035, 1, 1),
        );
        let server = TestServer::start(pki.chain_der.clone(), pki.key_der.clone());
        let outcome = probe(
            "localhost",
            server.port,
            TrustMode::Builtin,
            &pki.ca_pem,
            "",
        )
        .expect("受信根必须握手成功");
        assert_eq!(
            outcome.leaf_chain_der.len(),
            2,
            "服务端发出的链为叶子与签发 CA：{:?}",
            outcome.leaf_chain_der.len()
        );
        assert_eq!(outcome.leaf_chain_der[0], pki.chain_der[0].as_ref());
        assert_eq!(outcome.leaf_chain_der[1], pki.chain_der[1].as_ref());
        let version = outcome.tls_version.expect("必须报告协议版本");
        assert!(version.starts_with("TLSv1_"), "协议版本：{version}");
    }

    /// 2. 未受信根：握手失败且错误可操作（指向 ca-extra.pem／system 档位）。
    #[test]
    fn untrusted_root_is_rejected_with_actionable_error() {
        let server_pki = test_pki("server", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let other_pki = test_pki("unrelated", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let server = TestServer::start(server_pki.chain_der.clone(), server_pki.key_der.clone());
        let err = probe(
            "localhost",
            server.port,
            TrustMode::Builtin,
            &other_pki.ca_pem,
            "",
        )
        .expect_err("未受信根必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable, "未受信属配置问题，不可重试");
        assert!(err.message.contains("签发者"), "{}", err.message);
        assert!(err.message.contains("ca-extra.pem"), "{}", err.message);
        assert!(
            !err.message.contains("PRIVATE KEY"),
            "错误消息不得包含私钥内容：{}",
            err.message
        );
    }

    /// 3. 主机名不匹配（两个方向）都必须失败。
    #[test]
    fn hostname_mismatch_is_rejected_in_both_directions() {
        let dns_only = test_pki("dns-only", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let server = TestServer::start(dns_only.chain_der.clone(), dns_only.key_der.clone());
        let err = probe(
            "127.0.0.1",
            server.port,
            TrustMode::Builtin,
            &dns_only.ca_pem,
            "",
        )
        .expect_err("证书仅含 localhost 时以 IP 校验必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("SAN"), "{}", err.message);
        drop(server);

        let ip_only = test_pki("ip-only", &["127.0.0.1"], (2024, 1, 1), (2035, 1, 1));
        let server = TestServer::start(ip_only.chain_der.clone(), ip_only.key_der.clone());
        let err = probe(
            "localhost",
            server.port,
            TrustMode::Builtin,
            &ip_only.ca_pem,
            "",
        )
        .expect_err("证书仅含 IP 时以主机名校验必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("SAN"), "{}", err.message);
    }

    /// 4. 过期证书：失败并明确说明已过期。
    #[test]
    fn expired_certificate_is_rejected() {
        let expired = test_pki("expired", &["localhost"], (2020, 1, 1), (2021, 1, 1));
        let server = TestServer::start(expired.chain_der.clone(), expired.key_der.clone());
        let err = probe(
            "localhost",
            server.port,
            TrustMode::Builtin,
            &expired.ca_pem,
            "",
        )
        .expect_err("过期证书必须失败");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("已过期"), "{}", err.message);
    }

    /// 5a. builtin 档位：`ca-extra` 中的根生效（基线为另一份不受信根）。
    #[test]
    fn extra_root_takes_effect_in_builtin_mode() {
        let server_pki = test_pki("extra-builtin", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let unrelated = test_pki(
            "unrelated-builtin",
            &["localhost"],
            (2024, 1, 1),
            (2035, 1, 1),
        );
        let server = TestServer::start(server_pki.chain_der.clone(), server_pki.key_der.clone());
        let load = load_roots(TrustMode::Builtin, &unrelated.ca_pem, &server_pki.ca_pem)
            .expect("追加根必须装配成功");
        assert_eq!(load.builtin_count, 1);
        assert_eq!(load.extra_count, 1);
        assert_eq!(load.system_count, None);
        assert_eq!(load.roots.len(), 2);

        let outcome = probe(
            "localhost",
            server.port,
            TrustMode::Builtin,
            &unrelated.ca_pem,
            &server_pki.ca_pem,
        )
        .expect("追加根必须使握手成功");
        assert_eq!(outcome.leaf_chain_der.len(), 2);
    }

    /// 5b. system 档位：`ca-extra` 中的根生效（系统库用注入的固定根，不读真实 OS 库）。
    #[test]
    fn extra_root_takes_effect_in_system_mode() {
        let server_pki = test_pki("extra-system", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let system_pki = test_pki("system-store", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let server = TestServer::start(server_pki.chain_der.clone(), server_pki.key_der.clone());
        let system_der = system_pki.ca_der.clone();
        let load = load_roots_with_system(
            move || Ok(vec![system_der]),
            TrustMode::System,
            "",
            &server_pki.ca_pem,
        )
        .expect("system 档位叠加追加根必须装配成功");
        assert_eq!(load.system_count, Some(1));
        assert_eq!(load.builtin_count, 0);
        assert_eq!(load.extra_count, 1);
        assert_eq!(load.roots.len(), 2);

        let extra_pem = server_pki.ca_pem.clone();
        let system_der = system_pki.ca_der.clone();
        let outcome = probe_with_system(
            move || Ok(vec![system_der]),
            "localhost",
            server.port,
            TrustMode::System,
            "",
            &extra_pem,
        )
        .expect("system 档位的追加根必须使握手成功");
        assert_eq!(outcome.leaf_chain_der.len(), 2);
    }

    /// 6. 系统库 0 份证书：可操作错误，且不回落到内置库。
    #[test]
    fn empty_system_store_is_actionable_and_never_falls_back() {
        let builtin = test_pki(
            "builtin-fallback",
            &["localhost"],
            (2024, 1, 1),
            (2035, 1, 1),
        );
        let err = load_roots_with_system(|| Ok(vec![]), TrustMode::System, &builtin.ca_pem, "")
            .expect_err("系统库 0 份证书必须报错（不得回落内置库）");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("0 份"), "{}", err.message);
        assert!(err.message.contains("不得回落"), "{}", err.message);
        assert!(err.message.contains("builtin"), "{}", err.message);

        let err = load_roots_with_system(
            || Err(WhError::invalid_state("注入的读库失败（测试）")),
            TrustMode::System,
            "",
            "",
        )
        .expect_err("读库失败必须原样失败");
        assert!(err.message.contains("注入的读库失败"), "{}", err.message);
    }

    /// 7. PEM 输入拒绝面：超限、私钥块、不可解析、非空无证书、总份数超限。
    #[test]
    fn pem_inputs_are_rejected_at_every_limit() {
        let pki = test_pki("limits", &["localhost"], (2024, 1, 1), (2035, 1, 1));

        let oversized = "A".repeat(MAX_ROOT_PEM_BYTES + 1);
        let err = load_roots(TrustMode::Builtin, &oversized, "").expect_err("超过字节上限必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(err.message.contains("上限"), "{}", err.message);

        for label in ["PRIVATE KEY", "ENCRYPTED PRIVATE KEY", "EC PRIVATE KEY"] {
            let pem = format!(
                "{}-----BEGIN {label}-----\nAAAA\n-----END {label}-----\n",
                pki.ca_pem
            );
            let err = load_roots(TrustMode::Builtin, &pem, "").expect_err("含私钥块必须拒绝");
            assert_eq!(err.code, ErrorCode::ParseError, "{label}");
            assert!(err.message.contains("私钥"), "{}", err.message);
        }

        let broken = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        let err = load_roots(TrustMode::Builtin, broken, "").expect_err("DER 不可解析必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(
            err.message.contains("无法作为信任根解析"),
            "{}",
            err.message
        );

        let err = load_roots(TrustMode::Builtin, &pki.ca_pem, "not a pem\n")
            .expect_err("非空但无 CERTIFICATE 块必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);
        assert!(
            err.message.contains("不含任何 CERTIFICATE 块"),
            "{}",
            err.message
        );

        let empty =
            load_roots(TrustMode::Builtin, &pki.ca_pem, "").expect("空的追加文件表示无追加");
        assert_eq!(empty.extra_count, 0);
        assert_eq!(empty.roots.len(), 1);

        let err = load_roots(TrustMode::Builtin, "", "").expect_err("空内置副本不得以空信任集启动");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("reset"), "{}", err.message);

        // 份数上限：单文件字节上限（1 MiB）先于 2048 份触发，故经注入的系统库读取器验证。
        let repeated = vec![pki.ca_der.clone(); MAX_ROOT_CERTS + 1];
        let err = load_roots_with_system(|| Ok(repeated), TrustMode::System, "", "")
            .expect_err("信任根总数超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("上限"), "{}", err.message);
    }

    /// 去重保留首次出现顺序：内置 A、B + 追加 B、C → A、B、C。
    #[test]
    fn duplicate_roots_are_dropped_keeping_first_occurrence_order() {
        let first = test_pki("first", &["a.local"], (2024, 1, 1), (2035, 1, 1));
        let second = test_pki("second", &["b.local"], (2024, 1, 1), (2035, 1, 1));
        let third = test_pki("third", &["c.local"], (2024, 1, 1), (2035, 1, 1));
        let builtin = format!("{}{}", first.ca_pem, second.ca_pem);
        let extra = format!("{}{}", second.ca_pem, third.ca_pem);
        let load = load_roots(TrustMode::Builtin, &builtin, &extra).expect("重复证书只去重");
        assert_eq!(load.builtin_count, 2);
        assert_eq!(load.extra_count, 2, "追加份数按可解析块计数（含重复）");
        assert_eq!(load.roots.len(), 3, "并集只剩 3 份");
        assert_eq!(load.roots[0], first.ca_der);
        assert_eq!(load.roots[1], second.ca_der);
        assert_eq!(load.roots[2], third.ca_der);
    }

    /// 8. `build_agent` 成功构造，且校验未被关闭、根集合为具体证书。
    #[test]
    fn build_agent_keeps_verification_enabled_with_specific_roots() {
        let pki = test_pki("agent", &["localhost"], (2024, 1, 1), (2035, 1, 1));
        let agent = build_agent(TrustMode::Builtin, &pki.ca_pem, "").expect("构造 Agent");
        let tls = agent.config().tls_config();
        assert!(!tls.disable_verification(), "不得关闭证书校验");
        assert!(tls.use_sni(), "保留默认 SNI");
        match tls.root_certs() {
            ureq::tls::RootCerts::Specific(certs) => assert_eq!(certs.len(), 1),
            other => panic!("必须使用具体根集合，而非默认 webpki 或平台验证器：{other:?}"),
        }

        let err = build_agent(TrustMode::Builtin, "", "").expect_err("空信任集必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    /// 内置资产可被本模块完整装配（真实证书包，非公网访问）。
    #[test]
    fn bundled_asset_loads_without_network() {
        let load =
            load_roots(TrustMode::Builtin, BUILTIN_CA_BUNDLE, "").expect("内置资产必须可装配");
        assert!(
            load.builtin_count > 0,
            "内置资产份数：{}",
            load.builtin_count
        );
        assert_eq!(load.roots.len(), load.builtin_count);
        assert_eq!(load.system_count, None);
    }
}
