//! # 入站 TLS 装配与非阻塞连接层（T189、ADR-040 决策 3）
//!
//! 本模块是**装配根**的 TLS 落点：把工作区证书库里的 PEM 材料变成
//! `rustls::ServerConfig`，并把明文 socket 与 TLS 会话统一到 [`ConnIo`]，使既有
//! `serve_loop` 的单线程非阻塞状态机只面对一种读写接口。
//!
//! 边界（ADR-040“代码归属”与 ADR-031）：
//!
//! - `rustls` 只出现在本 crate（入站握手）与 `whcode-llm`（出站，`http` feature）；
//!   `whcode-web` 保持零网络依赖——握手状态机不得迁入前端 crate，否则该前端 crate
//!   将获得 `rustls` 依赖并与 ADR-031 的边界冲突。
//! - **零 `unsafe`、零新增常驻线程**：握手在既有服务循环线程内推进；每连接只有
//!   定长缓冲，不创建线程、不排队等待。
//! - **不放宽认证**：TLS 只加密链路；token 与 Origin 判定仍由 `whcode-web::security`
//!   完成（ADR-023／ADR-022／ADR-040 决策 3“安全边界不变”）。
//! - **不泄漏秘密**：错误消息只含协议层原因，不含私钥或证书正文。
//!
//! 缓冲上限（NFR-215；超限即断开，不静默截断）：
//!
//! - 密文读入 [`MAX_TLS_CIPHERTEXT`]：单次从 socket 读入并交给 rustls 的密文字节；
//! - 单次明文字节 [`MAX_TLS_PLAINTEXT`]：一次 `read_plain` 最多交付的明文；
//! - 出站暂存 [`MAX_TLS_PENDING_OUT`]：rustls 明文发送缓冲满时先暂存，**满即返回
//!   `WouldBlock`**——调用方保留未接受字节，不吞数据、不无界增长。
//!
//! rustls 自身的发送缓冲同样有界（`DEFAULT_BUFFER_LIMIT = 64 KiB`），本模块不修改该
//! 上限，只在它之外提供一层显式暂存，使“接受的明文字节数”始终可归因。

use std::io::{BufReader, Error, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;

use rustls::pki_types::pem::{self, SectionKind};
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer,
};
use rustls::{ServerConfig, ServerConnection};
use whcode_domain::{SERVER_CERT_MAX_BYTES, SERVER_KEY_MAX_BYTES, WhError};

/// 单次从 socket 读入的密文字节上限（超过即断开，不静默截断）。
pub const MAX_TLS_CIPHERTEXT: usize = 64 * 1024;
/// 一次 `read_plain` 最多交付的明文字节上限。
pub const MAX_TLS_PLAINTEXT: usize = 64 * 1024;
/// 出站明文暂存上限（rustls 明文发送缓冲满时的显式暂存；满即 `WouldBlock`）。
pub const MAX_TLS_PENDING_OUT: usize = 256 * 1024;
/// 单次 socket 读取的密文块大小（读入后立即交给 rustls，不跨 tick 积压）。
const TLS_READ_CHUNK: usize = 4096;
/// 单次 `read_plain` 最多推进的“读密文→处理”轮数：保证每 tick 工作量有界。
const MAX_TLS_READ_ROUNDS: usize = 8;
/// 单次 socket 读块必须小于密文暂存上限：编译期保证，避免上限被误改成更小的值。
const _: () = assert!(TLS_READ_CHUNK <= MAX_TLS_CIPHERTEXT);

/// 由受管 PEM 材料构造入站 TLS 服务端配置（只读共享，每连接克隆 `Arc`）。
///
/// 证书链只接受 `CERTIFICATE` 块（至少一份，字节上限 [`SERVER_CERT_MAX_BYTES`]）；
/// 私钥只接受**未加密** PKCS#8／PKCS#1／SEC1（`ENCRYPTED PRIVATE KEY` 明确拒绝，
/// 字节上限 [`SERVER_KEY_MAX_BYTES`]）。错误消息中文且可操作，不含 PEM 正文。
///
/// # Errors
///
/// 证书链或私钥超限、含不允许的 PEM 块、私钥带口令、无证书块、或证书与私钥不匹配
/// 时返回 [`WhError`]（`parse_error`）。
pub fn server_config(cert_pem: &str, key_pem: &str) -> Result<Arc<ServerConfig>, WhError> {
    let chain = parse_certificate_chain(cert_pem)?;
    let key = parse_private_key(key_pem)?;
    // 显式使用 ring 提供者：不依赖“进程默认提供者”的安装顺序，也避免
    // `ServerConfig::builder()` 在无法解析提供者时 panic（ADR-040 决策 5 的单一提供者）。
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(rustls::DEFAULT_VERSIONS)
            .map_err(|err| {
                WhError::parse_error(format!(
                    "入站 TLS 协议版本不可用：{err}；请确认构建启用了 ring 提供者与 TLS 1.2／1.3"
                ))
            })?
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .map_err(|err| {
                WhError::parse_error(format!(
                    "构造入站 TLS 服务端配置失败：{err}；请确认证书链与私钥匹配（同一密钥对），\
             且私钥为未加密的 PKCS#8／PKCS#1／SEC1 PEM（ADR-040 决策 3）"
                ))
            })?;
    Ok(Arc::new(config))
}

/// 解析证书链 PEM（仅 `CERTIFICATE` 块；至少一份；上限与 app 相同）。
fn parse_certificate_chain(pem: &str) -> Result<Vec<CertificateDer<'static>>, WhError> {
    if pem.len() > SERVER_CERT_MAX_BYTES {
        return Err(WhError::parse_error(format!(
            "入站证书链 {SERVER_CERT_MAX_BYTES} 字节上限被突破（当前 {} 字节）；请裁剪后重试\
             （ADR-040 决策 1 的拒绝阈值）",
            pem.len()
        )));
    }
    if pem.contains("PRIVATE KEY") {
        return Err(WhError::parse_error(
            "入站证书链含私钥块：私钥不得混入证书文件；证书链只放 CERTIFICATE 块，\
             私钥单独放 server-key.pem（ADR-040 决策 1）",
        ));
    }
    let mut chain: Vec<CertificateDer<'static>> = Vec::new();
    let mut reader = BufReader::new(pem.as_bytes());
    loop {
        let section = pem::from_buf(&mut reader).map_err(|err| {
            WhError::parse_error(format!(
                "入站证书链 PEM 块读取失败（{err}）；请确认每块有匹配的 BEGIN/END 行且为 Base64"
            ))
        })?;
        let Some((kind, der)) = section else { break };
        match kind {
            SectionKind::Certificate => chain.push(CertificateDer::from(der)),
            other => {
                return Err(WhError::parse_error(format!(
                    "入站证书链含不允许的 PEM 块（{other:?}）：仅允许 CERTIFICATE 块"
                )));
            }
        }
    }
    if chain.is_empty() {
        return Err(WhError::parse_error(
            "入站证书链至少需要一份 CERTIFICATE 块；当前无证书",
        ));
    }
    Ok(chain)
}

/// 解析私钥 PEM（仅未加密 PKCS#8／PKCS#1／SEC1；拒绝带口令私钥与多把私钥）。
fn parse_private_key(pem: &str) -> Result<PrivateKeyDer<'static>, WhError> {
    if pem.len() > SERVER_KEY_MAX_BYTES {
        return Err(WhError::parse_error(format!(
            "入站私钥 {SERVER_KEY_MAX_BYTES} 字节上限被突破（当前 {} 字节）；请裁剪后重试\
             （ADR-040 决策 1 的拒绝阈值）",
            pem.len()
        )));
    }
    // 底层 PEM 解析器会静默跳过未知标签（含 `ENCRYPTED PRIVATE KEY`），因此带口令
    // 私钥必须在解析前按 BEGIN 行显式拒绝，不能“解析不到就当作没有”。
    if pem.contains("ENCRYPTED PRIVATE KEY") {
        return Err(WhError::parse_error(
            "入站私钥带口令（ENCRYPTED PRIVATE KEY）：请提供未加密的 PEM 私钥\
             （PKCS#8 BEGIN PRIVATE KEY／PKCS#1／SEC1；ADR-040 决策 3）",
        ));
    }
    let mut key: Option<PrivateKeyDer<'static>> = None;
    let mut reader = BufReader::new(pem.as_bytes());
    loop {
        let section = pem::from_buf(&mut reader).map_err(|err| {
            WhError::parse_error(format!(
                "入站私钥 PEM 块读取失败（{err}）；请确认每块有匹配的 BEGIN/END 行且为 Base64"
            ))
        })?;
        let Some((kind, der)) = section else { break };
        let parsed = match kind {
            SectionKind::PrivateKey => PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der)),
            SectionKind::RsaPrivateKey => PrivateKeyDer::Pkcs1(PrivatePkcs1KeyDer::from(der)),
            SectionKind::EcPrivateKey => PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(der)),
            SectionKind::Certificate => {
                return Err(WhError::parse_error(
                    "入站私钥文件含 CERTIFICATE 块：证书与私钥必须分开存放\
                     （server-cert.pem／server-key.pem）",
                ));
            }
            other => {
                return Err(WhError::parse_error(format!(
                    "入站私钥文件含不允许的 PEM 块（{other:?}）：仅允许未加密私钥块"
                )));
            }
        };
        if key.is_some() {
            return Err(WhError::parse_error(
                "入站私钥文件含多把私钥：一次只能提供与叶子证书配对的单把私钥",
            ));
        }
        key = Some(parsed);
    }
    key.ok_or_else(|| {
        WhError::parse_error(
            "入站私钥文件不含私钥块：请提供未加密的 PKCS#8（BEGIN PRIVATE KEY）、\
             PKCS#1（BEGIN RSA PRIVATE KEY）或 SEC1（BEGIN EC PRIVATE KEY）PEM",
        )
    })
}

/// 连接读写层：明文 socket 或 TLS 会话（对外暴露同一套非阻塞语义）。
pub(crate) enum ConnIo {
    /// 明文 `TCP`（`--tls off` 与既有测试路径）。
    Plain(TcpStream),
    /// TLS 会话：密文经 socket，明文经 rustls。
    Tls(Box<TlsConn>),
}

/// 单个 TLS 连接的非阻塞状态（每连接一份，缓冲有界）。
pub(crate) struct TlsConn {
    /// 底层 socket（非阻塞；RAII 关闭）。
    stream: TcpStream,
    /// rustls 服务端会话（握手与加解密状态）。
    conn: ServerConnection,
    /// 本次从 socket 读入的密文暂存（交给 rustls 后清空）。
    ciphertext: Vec<u8>,
    /// 本次从 rustls 取出的明文暂存（拷贝给调用方后清空）。
    plaintext: Vec<u8>,
    /// 已被 `write_plain` 接受、但 rustls 明文发送缓冲暂时放不下的明文字节。
    pending_out: Vec<u8>,
}

impl ConnIo {
    /// 明文连接。
    pub(crate) fn plain(stream: TcpStream) -> Self {
        Self::Plain(stream)
    }

    /// TLS 连接（`conn` 由 [`ServerConnection::new`] 以同一 `ServerConfig` 创建）。
    pub(crate) fn tls(stream: TcpStream, conn: ServerConnection) -> Self {
        Self::Tls(Box::new(TlsConn {
            stream,
            conn,
            ciphertext: Vec::new(),
            plaintext: Vec::new(),
            pending_out: Vec::new(),
        }))
    }

    /// 明文读取：与 `TcpStream::read` 语义完全一致（`WouldBlock`、`Ok(0)` EOF、部分读）。
    ///
    /// TLS 分支：读密文 → `read_tls` → `process_new_packets` → 交付 rustls 已解出的明文；
    /// 握手未完成且无明文时返回 `WouldBlock`；握手/解密错误映射为
    /// `InvalidData`（调用方关闭连接，不重试）。
    ///
    /// # Errors
    ///
    /// 底层 socket 错误按原样传播；TLS 协议错误与缓冲超限返回
    /// [`ErrorKind::InvalidData`]。
    pub(crate) fn read_plain(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(out),
            Self::Tls(conn) => conn.read_plain(out),
        }
    }

    /// 明文写入：返回**本次接受的明文字节数**（TLS 分支可小于 `data.len()`）。
    ///
    /// 明文分支与 `TcpStream::write` 完全一致（部分写、`WouldBlock`）。TLS 分支把能交给
    /// rustls 的字节立即加密，其余进入 [`TlsConn::pending_out`]；暂存满且 rustls 也无法
    /// 接收时返回 `WouldBlock`——不吞掉任何字节。
    ///
    /// # Errors
    ///
    /// 底层 socket 错误与 TLS 致命状态按 `InvalidData` 传播。
    pub(crate) fn write_plain(&mut self, data: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(data),
            Self::Tls(conn) => conn.write_plain(data),
        }
    }

    /// 刷出待写密文（`WouldBlock` 保留待写，不视为错误）。
    ///
    /// # Errors
    ///
    /// 底层 socket 写错误按原样传播。
    pub(crate) fn flush(&mut self) -> std::io::Result<()> {
        self.flush_written().map(|_| ())
    }

    /// 刷出待写密文并返回本次写入 socket 的密文字节数（供服务循环判进展）。
    ///
    /// # Errors
    ///
    /// 底层 socket 写错误按原样传播；`WouldBlock` 视为 0 字节并保留待写。
    pub(crate) fn flush_written(&mut self) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.flush().map(|()| 0),
            Self::Tls(conn) => conn.flush_written(),
        }
    }

    /// 是否仍在 TLS 握手中（明文恒为 `false`）。
    pub(crate) fn is_handshaking(&self) -> bool {
        match self {
            Self::Plain(_) => false,
            Self::Tls(conn) => conn.conn.is_handshaking(),
        }
    }

    /// 对端地址（诊断用；不含任何凭据）。
    ///
    /// # Errors
    ///
    /// socket 已断开时按原样传播。
    pub(crate) fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        match self {
            Self::Plain(stream) => stream.peer_addr(),
            Self::Tls(conn) => conn.stream.peer_addr(),
        }
    }

    /// 是否仍有未写出的字节（暂存明文或 rustls 待写密文）。
    ///
    /// 服务循环据此判断“响应写完”是否真的送达：TLS 下写入被接受不等于已发送。
    pub(crate) fn has_pending_output(&self) -> bool {
        match self {
            Self::Plain(_) => false,
            Self::Tls(conn) => !conn.pending_out.is_empty() || conn.conn.wants_write(),
        }
    }
}

impl TlsConn {
    /// 非阻塞读取并解密（见 [`ConnIo::read_plain`] 的语义）。
    fn read_plain(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let mut rounds = 0usize;
        loop {
            // 暂存明文先交给 rustls，再刷出可能产生的密文。
            self.drain_staged()?;
            if let Some(read) = self.take_plaintext(out)? {
                self.flush_lenient()?;
                return Ok(read);
            }
            // 握手完成后既无明文、也不再想读，即对端已发 close_notify（EOF）。
            if !self.conn.is_handshaking() && !self.conn.wants_read() {
                self.flush_lenient()?;
                return Ok(0);
            }
            if rounds >= MAX_TLS_READ_ROUNDS {
                // 已读满本轮配额：交回控制权，下个 tick 继续（不无限占用循环线程）。
                self.flush_lenient()?;
                return Err(would_block("本轮读配额已用尽，下一 tick 继续"));
            }
            rounds += 1;
            let mut chunk = [0u8; TLS_READ_CHUNK];
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    // 对端关闭（干净 EOF）：调用方按既有语义断开。
                    self.flush_lenient()?;
                    return Ok(0);
                }
                Ok(read) => {
                    // 密文暂存跨 tick 保留：`read_tls` 可能只消费到记录边界，
                    // 余下字节必须留到下一次，不能清空（否则下一记录从中途解析，
                    // 表现为“cannot decrypt peer's message”）。
                    if self.ciphertext.len() + read > MAX_TLS_CIPHERTEXT {
                        return Err(invalid_data(format!(
                            "TLS 密文暂存超过上限 {MAX_TLS_CIPHERTEXT} 字节；连接已关闭（NFR-215）"
                        )));
                    }
                    self.ciphertext.extend_from_slice(&chunk[..read]);
                    let mut slice: &[u8] = &self.ciphertext;
                    let consumed = self.conn.read_tls(&mut slice).map_err(|err| {
                        invalid_data(format!("TLS 密文读取失败（{err}）；连接已关闭"))
                    })?;
                    if consumed == 0 {
                        // 暂存非空却未被消费：交回控制权，下个 tick 重试，
                        // 不能当作 EOF（那会静默吞掉未解析的密文）。
                        self.flush_lenient()?;
                        return Err(would_block(
                            "TLS 密文暂存尚未被 rustls 消费，下一 tick 继续",
                        ));
                    }
                    self.ciphertext.drain(..consumed);
                    self.conn.process_new_packets().map_err(|err| {
                        invalid_data(format!(
                            "TLS 握手或解密失败（{err}）；连接已关闭（ADR-040 决策 3）"
                        ))
                    })?;
                    self.flush_lenient()?;
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    self.flush_lenient()?;
                    return Err(err);
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// 非阻塞写入（见 [`ConnIo::write_plain`] 的语义）。
    fn write_plain(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        self.drain_staged()?;
        let mut accepted = match self.conn.writer().write(data) {
            Ok(written) => written,
            Err(err) if err.kind() == ErrorKind::WouldBlock => 0,
            Err(err) => return Err(err),
        };
        if accepted < data.len() {
            let room = MAX_TLS_PENDING_OUT.saturating_sub(self.pending_out.len());
            let take = (data.len() - accepted).min(room);
            if take > 0 {
                self.pending_out
                    .extend_from_slice(&data[accepted..accepted + take]);
                accepted += take;
            }
        }
        if accepted == 0 {
            self.flush_lenient()?;
            return Err(would_block(format!(
                "TLS 出站缓冲已满（暂存上限 {MAX_TLS_PENDING_OUT} 字节）；未接受任何字节，\
                 调用方须保留数据并稍后重试"
            )));
        }
        let _ = self.flush_written()?;
        Ok(accepted)
    }

    /// 把暂存明文交给 rustls（能接收多少交多少；剩余保留）。
    fn drain_staged(&mut self) -> std::io::Result<()> {
        if self.pending_out.is_empty() {
            return Ok(());
        }
        let staged = std::mem::take(&mut self.pending_out);
        let taken = self.conn.writer().write(&staged)?;
        if taken < staged.len() {
            self.pending_out.extend_from_slice(&staged[taken..]);
        }
        Ok(())
    }

    /// 取出 rustls 已解出的明文（无明文时返回 `None`；长度受 [`MAX_TLS_PLAINTEXT`] 约束）。
    fn take_plaintext(&mut self, out: &mut [u8]) -> std::io::Result<Option<usize>> {
        let limit = out.len().min(MAX_TLS_PLAINTEXT);
        if self.plaintext.len() < limit {
            self.plaintext.resize(limit, 0);
        }
        match self.conn.reader().read(&mut self.plaintext[..limit]) {
            Ok(0) => Ok(None),
            Ok(read) => {
                out[..read].copy_from_slice(&self.plaintext[..read]);
                Ok(Some(read))
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// 刷出待写密文并返回写入字节数（`WouldBlock` 保留待写）。
    fn flush_written(&mut self) -> std::io::Result<usize> {
        self.drain_staged()?;
        let mut written = 0usize;
        loop {
            match self.conn.write_tls(&mut self.stream) {
                Ok(0) => return Ok(written),
                Ok(count) => written += count,
                Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(written),
                Err(err) => return Err(err),
            }
        }
    }

    /// 服务循环内部的“尽力刷出”：不把 `WouldBlock` 当作失败。
    fn flush_lenient(&mut self) -> std::io::Result<()> {
        self.flush_written().map(|_| ())
    }
}

fn would_block(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::WouldBlock, message.into())
}

fn invalid_data(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, ServerName};

    use super::{ConnIo, MAX_TLS_PENDING_OUT, MAX_TLS_PLAINTEXT, server_config};

    /// 测试用自签证书与私钥（运行期生成，不写入仓库）。
    fn test_material() -> (String, String) {
        let key = rcgen::KeyPair::generate().expect("生成测试私钥");
        let params =
            rcgen::CertificateParams::new(vec!["localhost".to_string()]).expect("证书参数");
        let cert = params.self_signed(&key).expect("生成测试证书");
        (cert.pem(), key.serialize_pem())
    }

    fn trusting_client(cert_pem: &str) -> Arc<rustls::ClientConfig> {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(cert_pem.as_bytes()).expect("证书 DER"))
            .expect("加入信任根");
        Arc::new(
            rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_protocol_versions(rustls::DEFAULT_VERSIONS)
            .expect("客户端协议版本")
            .with_root_certificates(roots)
            .with_no_client_auth(),
        )
    }

    /// 服务端 `ConnIo` + 客户端 socket + 服务端证书 PEM。
    fn tls_pair() -> (ConnIo, TcpStream, String) {
        let (cert_pem, key_pem) = test_material();
        let config = server_config(&cert_pem, &key_pem).expect("服务端配置");
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定随机端口");
        let addr = listener.local_addr().expect("读取端口");
        let peer = TcpStream::connect(addr).expect("连接");
        let (stream, _) = listener.accept().expect("接受连接");
        stream.set_nonblocking(true).expect("服务端非阻塞");
        let conn = rustls::ServerConnection::new(config).expect("服务端会话");
        (ConnIo::tls(stream, conn), peer, cert_pem)
    }

    fn client_for(cert_pem: &str) -> rustls::ClientConnection {
        let config = trusting_client(cert_pem);
        rustls::ClientConnection::new(config, ServerName::try_from("localhost").expect("服务器名"))
            .expect("客户端会话")
    }

    /// 测试内驱动完整握手（双方非阻塞，测试线程顺序推进）。
    fn drive_handshake(
        io: &mut ConnIo,
        client: &mut rustls::ClientConnection,
        peer: &mut TcpStream,
    ) {
        peer.set_nonblocking(true).expect("客户端非阻塞");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut sink = [0u8; 4096];
        while (io.is_handshaking() || client.is_handshaking()) && Instant::now() < deadline {
            loop {
                match client.write_tls(peer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("客户端写密文失败：{err}"),
                }
            }
            match io.read_plain(&mut sink) {
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => panic!("服务端读失败：{err}"),
            }
            io.flush().expect("服务端刷出密文");
            loop {
                match client.read_tls(peer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("客户端读密文失败：{err}"),
                }
            }
            client.process_new_packets().expect("客户端处理握手包");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!io.is_handshaking(), "服务端握手必须在预算内完成");
        assert!(!client.is_handshaking(), "客户端握手必须在预算内完成");
    }

    /// 匹配材料可构造配置；不匹配、带口令私钥、超限与空链均被拒绝且消息可操作。
    #[test]
    fn server_config_validates_material() {
        let (cert_pem, key_pem) = test_material();
        server_config(&cert_pem, &key_pem).expect("匹配的证书与私钥必须通过");

        let (other_cert, other_key) = test_material();
        let err = server_config(&other_cert, &key_pem).expect_err("配对不一致必须拒绝");
        assert!(err.message.contains("匹配"), "{}", err.message);
        assert!(err.message.contains("PKCS#8"), "{}", err.message);

        let encrypted = "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n\
                         -----END ENCRYPTED PRIVATE KEY-----\n";
        let err = server_config(&cert_pem, encrypted).expect_err("带口令私钥必须拒绝");
        assert!(err.message.contains("未加密"), "{}", err.message);

        let oversized_cert = "A".repeat(whcode_domain::SERVER_CERT_MAX_BYTES + 1);
        let err = server_config(&oversized_cert, &key_pem).expect_err("证书超限必须拒绝");
        assert!(err.message.contains("上限"), "{}", err.message);

        let oversized_key = "A".repeat(whcode_domain::SERVER_KEY_MAX_BYTES + 1);
        let err = server_config(&cert_pem, &oversized_key).expect_err("私钥超限必须拒绝");
        assert!(err.message.contains("上限"), "{}", err.message);

        let err = server_config("", &key_pem).expect_err("空证书链必须拒绝");
        assert!(err.message.contains("至少"), "{}", err.message);
        let _ = other_key;
    }

    /// 明文分支与 `TcpStream` 语义一致：空读 `WouldBlock`、读到字节、写返回字节数、EOF 为 0。
    #[test]
    fn plain_branch_matches_stream_semantics() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("绑定随机端口");
        let addr = listener.local_addr().expect("读取端口");
        let mut peer = TcpStream::connect(addr).expect("连接");
        let (stream, _) = listener.accept().expect("接受连接");
        stream.set_nonblocking(true).expect("服务端非阻塞");
        peer.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("客户端读超时");
        let mut io = ConnIo::plain(stream);

        assert!(!io.is_handshaking(), "明文连接不在握手中");
        assert!(!io.has_pending_output(), "明文无 TLS 待写缓冲");
        assert!(io.peer_addr().is_ok(), "对端地址必须可读");
        let mut buf = [0u8; 16];
        assert_eq!(
            io.read_plain(&mut buf).expect_err("空连接读").kind(),
            std::io::ErrorKind::WouldBlock
        );

        peer.write_all(b"hello").expect("客户端写");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut read = 0usize;
        while read == 0 && Instant::now() < deadline {
            match io.read_plain(&mut buf) {
                Ok(0) => break,
                Ok(count) => read = count,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("读失败：{err}"),
            }
        }
        assert_eq!(&buf[..read], b"hello");
        assert_eq!(io.write_plain(b"world").expect("写"), 5);
        let mut got = Vec::new();
        let mut chunk = [0u8; 8];
        while got.len() < 5 {
            let count = peer.read(&mut chunk).expect("客户端读");
            if count == 0 {
                break;
            }
            got.extend_from_slice(&chunk[..count]);
        }
        assert_eq!(got, b"world");

        drop(peer);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match io.read_plain(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => panic!("EOF 前读失败：{err}"),
            }
            assert!(Instant::now() < deadline, "对端关闭后必须读到 EOF（Ok(0)）");
        }
    }

    /// 分片 ClientHello（客户端逐字节写）仍能完成握手，并可收发应用数据。
    #[test]
    fn fragmented_client_hello_completes_handshake() {
        let (mut io, mut peer, cert_pem) = tls_pair();
        let mut client = client_for(&cert_pem);
        peer.set_write_timeout(Some(Duration::from_secs(5)))
            .expect("客户端写超时");
        let mut first = Vec::new();
        loop {
            match client.write_tls(&mut first) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) => panic!("取首飞字节失败：{err}"),
            }
        }
        assert!(!first.is_empty(), "ClientHello 首飞不得为空");
        for byte in &first {
            peer.write_all(std::slice::from_ref(byte))
                .expect("逐字节写 ClientHello");
            std::thread::sleep(Duration::from_millis(1));
        }
        drive_handshake(&mut io, &mut client, &mut peer);
    }

    /// 超大握手声明与非法握手字节映射为 `InvalidData`（调用方关闭连接）。
    #[test]
    fn malformed_handshake_is_rejected_as_invalid_data() {
        // 握手消息声明 65536 字节（rustls 上限 0xFFFF）。
        let (mut io, mut peer, _cert) = tls_pair();
        peer.write_all(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x01, 0x01, 0x00, 0x00])
            .expect("写超大握手声明");
        let mut buf = [0u8; 16];
        let err = read_until_error(&mut io, &mut buf);
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
        assert!(err.to_string().contains("TLS"), "{err}");

        // 握手前收到应用数据记录（协议顺序非法）：立即拒绝，不等待后续字节。
        let (mut io, mut peer, _cert) = tls_pair();
        peer.write_all(&[0x17, 0x03, 0x03, 0x00, 0x01, 0x00])
            .expect("写握手前应用数据");
        let err = read_until_error(&mut io, &mut buf);
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
    }

    fn read_until_error(io: &mut ConnIo, out: &mut [u8]) -> std::io::Error {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen: Vec<String> = Vec::new();
        loop {
            match io.read_plain(out) {
                Ok(read) => seen.push(format!("Ok({read})")),
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    seen.push("WouldBlock".to_string());
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(err) => return err,
            }
            assert!(
                Instant::now() < deadline,
                "畸形握手必须在预算内被拒绝（而不是无限等待）；观察到 {seen:?}"
            );
        }
    }

    /// 出站暂存严格有界：满时返回 `WouldBlock` 且不吞掉调用方字节。
    #[test]
    fn pending_output_is_bounded_and_never_swallows_bytes() {
        let (mut io, mut peer, cert_pem) = tls_pair();
        let mut client = client_for(&cert_pem);
        drive_handshake(&mut io, &mut client, &mut peer);

        // 人为填满暂存与 rustls 明文发送缓冲：断言上限而不依赖内核缓冲大小。
        if let ConnIo::Tls(conn) = &mut io {
            conn.pending_out
                .extend_from_slice(&vec![b'x'; MAX_TLS_PENDING_OUT]);
            assert!(
                conn.pending_out.len() <= MAX_TLS_PENDING_OUT,
                "暂存不得超过上限"
            );
            let filler = [b'y'; 64 * 1024];
            let accepted = conn.conn.writer().write(&filler).expect("填充发送缓冲");
            assert!(accepted > 0, "握手完成后应能接受明文字节");
        }
        let before = match &io {
            ConnIo::Tls(conn) => conn.pending_out.len(),
            ConnIo::Plain(_) => 0,
        };
        let err = io
            .write_plain(b"more")
            .expect_err("出站缓冲满必须 WouldBlock");
        assert_eq!(err.kind(), std::io::ErrorKind::WouldBlock, "{err}");
        match &io {
            ConnIo::Tls(conn) => assert_eq!(
                conn.pending_out.len(),
                before,
                "拒绝时不得改动已暂存字节（不吞数据）"
            ),
            ConnIo::Plain(_) => unreachable!("TLS 分支"),
        }
    }

    /// Check 284：部分写必须只接受可用容量，并在后续 tick 续写直到全部送达。
    ///
    /// 载荷取 `rustls 发送缓冲上限（64 KiB）+ 暂存上限（256 KiB）+ 余量`，保证首次
    /// `write_plain` 必然只能接受前缀；其余字节由调用方保留并在暂存腾空后再次提交。
    #[test]
    fn partial_write_accepts_capacity_and_resumes_until_all_bytes_delivered() {
        let (mut io, mut peer, cert_pem) = tls_pair();
        let mut client = client_for(&cert_pem);
        drive_handshake(&mut io, &mut client, &mut peer);

        let total = 400 * 1024;
        let payload: Vec<u8> = (0..total).map(|index| (index % 251) as u8).collect();
        let accepted = io
            .write_plain(&payload)
            .expect("部分写必须返回本次接受的字节数");
        assert!(accepted > 0, "首次写入必须至少接受一段前缀");
        assert!(
            accepted < payload.len(),
            "载荷必须超过可用容量才能验证部分写：accepted={accepted} total={total}"
        );

        let mut sent = accepted;
        let mut delivered = Vec::with_capacity(total);
        let mut sink = vec![0u8; 64 * 1024];
        let deadline = Instant::now() + Duration::from_secs(20);
        while delivered.len() < total && Instant::now() < deadline {
            io.flush().expect("刷出待写密文");
            // 每次读入后立即 process：rustls 的接收缓冲有界，攒够一批再处理会以
            // “message buffer full”失败（那是有界性的表现，不是被测路径的失败）。
            loop {
                match client.read_tls(&mut peer) {
                    Ok(0) => break,
                    Ok(_) => {
                        client.process_new_packets().expect("客户端处理应用数据");
                        loop {
                            match client.reader().read(&mut sink) {
                                Ok(0) => break,
                                Ok(read) => delivered.extend_from_slice(&sink[..read]),
                                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(err) => panic!("客户端读明文失败：{err}"),
                            }
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("客户端读密文失败：{err}"),
                }
            }
            while sent < payload.len() {
                match io.write_plain(&payload[sent..]) {
                    Ok(read) => sent += read,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("续写失败：{err}"),
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            delivered, payload,
            "部分写续传后客户端必须收到与提交完全一致的字节"
        );
    }

    /// Check 284：单次明文读取不超过 `MAX_TLS_PLAINTEXT`，且密文块不会超过其上限。
    #[test]
    fn read_plain_never_exceeds_plaintext_limit() {
        let (mut io, mut peer, cert_pem) = tls_pair();
        let mut client = client_for(&cert_pem);
        drive_handshake(&mut io, &mut client, &mut peer);
        // 把客户端可能尚未写出的握手收尾（Finished）先送达并让服务端处理，
        // 再写应用数据；否则 rustls 会在密钥切换边界报“cannot decrypt”。
        loop {
            match client.write_tls(&mut peer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => panic!("客户端握手收尾失败：{err}"),
            }
        }
        let mut handshake_sink = [0u8; 4096];
        match io.read_plain(&mut handshake_sink) {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(err) => panic!("服务端握手收尾失败：{err}"),
        }

        // 阳性对照：先传一条小消息，证明握手后的应用数据方向确实可用；
        // 失败时能把“握手状态问题”与“大载荷/缓冲问题”分开。
        client
            .writer()
            .write_all(b"probe")
            .expect("客户端写探针消息");
        loop {
            match client.write_tls(&mut peer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => panic!("客户端写探针密文失败：{err}"),
            }
        }
        let mut probe = [0u8; 16];
        let probe_len = loop {
            match io.read_plain(&mut probe) {
                Ok(0) => panic!("探针消息未送达"),
                Ok(read) => break read,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(err) => panic!("服务端读探针失败：{err}"),
            }
        };
        assert_eq!(&probe[..probe_len], b"probe", "应用数据方向必须可用");

        let payload: Vec<u8> = (0..(MAX_TLS_PLAINTEXT + 4096))
            .map(|i| (i % 239) as u8)
            .collect();
        // rustls 的明文发送缓冲有界（64 KiB），一次 write_all 会以 WriteZero 失败；
        // 按块写入并在块间泵动服务端读取，才是非阻塞连接的真实用法。
        let mut out = vec![0u8; MAX_TLS_PLAINTEXT * 2];
        let mut delivered = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut offset = 0usize;
        while offset < payload.len() {
            let end = (offset + 32 * 1024).min(payload.len());
            client
                .writer()
                .write_all(&payload[offset..end])
                .expect("客户端写应用数据");
            loop {
                match client.write_tls(&mut peer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("客户端写密文失败：{err}"),
                }
            }
            offset = end;
            loop {
                match io.read_plain(&mut out) {
                    Ok(0) => break,
                    Ok(read) => {
                        assert!(
                            read <= MAX_TLS_PLAINTEXT,
                            "单次明文读取 {read} 超过上限 {MAX_TLS_PLAINTEXT}"
                        );
                        delivered.extend_from_slice(&out[..read]);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("服务端读失败：{err}"),
                }
            }
        }

        while delivered.len() < payload.len() && Instant::now() < deadline {
            loop {
                match client.write_tls(&mut peer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(err) => panic!("客户端写密文失败：{err}"),
                }
            }
            match io.read_plain(&mut out) {
                Ok(0) => break,
                Ok(read) => {
                    assert!(
                        read <= MAX_TLS_PLAINTEXT,
                        "单次明文读取 {read} 超过上限 {MAX_TLS_PLAINTEXT}"
                    );
                    delivered.extend_from_slice(&out[..read]);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(err) => panic!("服务端读失败：{err}"),
            }
        }
        assert_eq!(delivered, payload, "有界读取不得截断或丢失字节");
    }
}
