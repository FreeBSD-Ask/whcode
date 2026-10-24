//! SSH 服务端（T096、FR-607/608、ADR-005/030）。
//!
//! ## 交付边界（M7，进程内测试为准）
//!
//! - **监听**：bind 地址来自调用方，默认 [`DEFAULT_BIND_ADDR`]（仅回环 + OS 分配端口，
//!   启动时打印实际地址）；FR-606 的对外监听门禁思路同样适用于 SSH——默认不放外部面。
//! - **认证**：仅公钥、仅 Ed25519（FR-611 安全边界）；授权清单来自
//!   `<workspace>/ssh/authorized_keys`（[`crate::hostkey`]）。russh 在签名验证通过后
//!   才回调 [`SshSessionHandler::auth_publickey`]，此处只做清单判定。
//! - **会话隔离**：每条连接一个独立 [`SessionContext`]（会话 ID、授权 Actor、有界
//!   通道状态）；连接之间不共享任何可变状态（共享的只有不可变配置与单写者执行器/审计）。
//! - **终端尺寸**：pty-req / window-change 记录进 [`SessionContext`]，并随受限命令执行
//!   传递给执行器（审计可见 `terminal=列x行`）。不模拟交互 PTY——`shell_request` 一律
//!   拒绝（FR-608 默认无 Shell；交互终端属 M8）。
//! - **安全退出**：channel close 即回收通道状态；连接断开（russh 结束会话任务）时
//!   Handler `Drop` 归还活跃连接计数并写审计——RAII，无泄漏路径；命令队列满/超时有界。
//!
//! ## russh 0.64 适配说明
//!
//! 服务端以 [`russh::server::Server`]（`new_client` + `run_on_socket`）+ [`russh::server::Handler`]
//! （0.64 为 RPITIT 形态：直接定义 `async fn`，无 async-trait 宏）实现；写回客户端用
//! `Session` 的同步方法（缓冲后由会话循环刷出）；主动关闭用
//! `RunningServerHandle::shutdown`。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use duckdb::Connection;
use russh::ChannelId;
use russh::Pty;
use russh::keys::{Algorithm, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Handler, Msg, Server as RusshServer, Session};
use whcode_app::{Actor, Capability};
use whcode_domain::{ErrorCode, WhError};

use crate::adapter::{AuditLog, AuditRecord, CommandExecutor, CommandOutcome, now_unix_ms};
use crate::hostkey::{self, HostKey};
use crate::policy::{self, Decision, SshPolicy};

/// 默认监听地址：仅回环 + OS 分配端口（启动时打印实际地址；对外监听须显式给出并自担配置责任）。
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:0";
/// 单连接通道数上限（有界通道状态；超限拒绝新通道）。
pub const DEFAULT_MAX_CHANNELS_PER_CONNECTION: usize = 8;

/// 终端尺寸（列 × 行；像素尺寸 M7 不使用，仅按协议接收）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtySize {
    /// 列数。
    pub cols: u32,
    /// 行数。
    pub rows: u32,
}

/// 单条 SSH 连接的会话上下文（会话隔离的载体）。
///
/// 每条连接一个实例，互不共享：会话 ID、认证后的 Actor、有界通道状态。
/// 通道以编号（`ChannelId::number()`，`u32`）为键——russh 不公开 `ChannelId` 构造，
/// 用编号做键也让上下文逻辑可脱离会话做进程内测试。
#[derive(Debug)]
pub struct SessionContext {
    /// 会话 ID（进程内唯一，`ssh-N`）。
    pub session_id: String,
    /// 认证成功后的授权 Actor（认证前为 `None`——此时不允许打开通道）。
    pub actor: Option<Actor>,
    channels: HashMap<u32, Option<PtySize>>,
    max_channels: usize,
}

impl SessionContext {
    fn new(session_id: String, max_channels: usize) -> Self {
        Self {
            session_id,
            actor: None,
            channels: HashMap::new(),
            max_channels,
        }
    }

    /// 打开通道（超过上限拒绝——通道状态有界）。
    fn open_channel(&mut self, channel: u32) -> bool {
        if self.channels.len() >= self.max_channels {
            return false;
        }
        self.channels.insert(channel, None);
        true
    }

    /// 关闭并回收通道状态。
    fn close_channel(&mut self, channel: u32) {
        self.channels.remove(&channel);
    }

    /// 记录 pty-req 的初始尺寸；通道未知返回 `false`。
    fn record_pty(&mut self, channel: u32, size: PtySize) -> bool {
        match self.channels.get_mut(&channel) {
            Some(slot) => {
                *slot = Some(size);
                true
            }
            None => false,
        }
    }

    /// 记录 window-change 的新尺寸；通道未知返回 `false`。
    fn record_window_change(&mut self, channel: u32, size: PtySize) -> bool {
        self.record_pty(channel, size)
    }

    /// 当前通道的终端尺寸（随命令执行传递）。
    #[must_use]
    pub fn window_size(&self, channel: u32) -> Option<PtySize> {
        self.channels.get(&channel).copied().flatten()
    }

    /// 当前打开的通道数。
    #[must_use]
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

/// 服务端配置（能力授予、策略与通道上限）。
#[derive(Debug, Clone)]
pub struct SshServerConfig {
    /// SSH 会话 Actor 被授予的应用能力（默认只读：SessionRead + TaskRead）。
    /// 会话级写命令（如 `session new`）需要在这里显式加入对应能力。
    pub actor_capabilities: std::collections::BTreeSet<Capability>,
    /// 受限入口策略（Shell 显式授权；默认全关——FR-608）。
    pub policy: SshPolicy,
    /// 单连接通道数上限。
    pub max_channels_per_connection: usize,
}

impl Default for SshServerConfig {
    fn default() -> Self {
        Self {
            actor_capabilities: [Capability::SessionRead, Capability::TaskRead]
                .into_iter()
                .collect(),
            policy: SshPolicy::default(),
            max_channels_per_connection: DEFAULT_MAX_CHANNELS_PER_CONNECTION,
        }
    }
}

/// 服务端共享状态（不可变配置 + 原子计数；连接间唯一共享物，无可变业务状态）。
struct ServerShared {
    config: SshServerConfig,
    authorized_keys: Vec<PublicKey>,
    executor: Arc<CommandExecutor>,
    audit: Arc<AuditLog>,
    active_connections: AtomicU64,
    session_seq: AtomicU64,
}

/// 授权判定：按 `SHA256` 指纹（只覆盖公钥数据）比较。
///
/// 不用 `PublicKey` 的 `PartialEq`：它把注释（comment）也算进相等性，而 OpenSSH 客户端
/// 上送的公钥不带注释、`authorized_keys` 行通常带注释——按结构体相等会误拒真实客户端
/// （系统 OpenSSH 冒烟实测命中）。指纹是公钥数据的抗碰撞摘要，与“是否同一把密钥”等价。
fn authorized_contains(authorized_keys: &[PublicKey], offered: &PublicKey) -> bool {
    let offered_fingerprint = hostkey::fingerprint(offered);
    authorized_keys
        .iter()
        .any(|authorized| hostkey::fingerprint(authorized) == offered_fingerprint)
}

impl ServerShared {
    fn record(&self, session_id: &str, actor_id: &str, action: &str, ok: bool, detail: String) {
        self.audit.record(AuditRecord {
            at_ms: now_unix_ms(),
            session_id: session_id.to_string(),
            actor_id: actor_id.to_string(),
            action: action.to_string(),
            wrote: false,
            ok,
            detail,
        });
    }
}

/// SSH 服务端：装配完成后的可运行单元。
pub struct SshServer {
    host_key: HostKey,
    shared: Arc<ServerShared>,
}

impl SshServer {
    /// 装配服务端（主机密钥、授权清单、单写者执行器与审计均由调用方注入）。
    #[must_use]
    pub fn new(
        config: SshServerConfig,
        host_key: HostKey,
        authorized_keys: Vec<PublicKey>,
        executor: Arc<CommandExecutor>,
        audit: Arc<AuditLog>,
    ) -> Self {
        Self {
            host_key,
            shared: Arc::new(ServerShared {
                config,
                authorized_keys,
                executor,
                audit,
                active_connections: AtomicU64::new(0),
                session_seq: AtomicU64::new(0),
            }),
        }
    }

    /// 主机密钥（指纹对外输出用）。
    #[must_use]
    pub fn host_key(&self) -> &HostKey {
        &self.host_key
    }

    /// 当前活跃连接数（断开回收的可观测信号）。
    #[must_use]
    pub fn active_connections(&self) -> u64 {
        self.shared.active_connections.load(Ordering::SeqCst)
    }

    /// 审计日志句柄。
    #[must_use]
    pub fn audit(&self) -> &Arc<AuditLog> {
        &self.shared.audit
    }

    /// russh 服务端配置：仅公钥认证方法、本机主机密钥、回环默认超时。
    #[must_use]
    pub fn russh_config(&self, auth_rejection_time: Duration) -> Arc<russh::server::Config> {
        Arc::new(russh::server::Config {
            // 仅公钥（FR-611：仅公钥认证）；密码/键盘交互不在方法集内。
            methods: russh::MethodSet::from(&[russh::MethodKind::PublicKey][..]),
            keys: vec![self.host_key.private_key().clone()],
            auth_rejection_time,
            // 初始 none 探测立即拒绝，避免拖慢测试与首连。
            auth_rejection_time_initial: Some(Duration::ZERO),
            inactivity_timeout: Some(Duration::from_secs(600)),
            ..Default::default()
        })
    }

    /// 连接接受器（实现 [`russh::server::Server`]；可复制，服务端本体保留观测句柄）。
    #[must_use]
    pub fn acceptor(&self) -> SshAcceptor {
        SshAcceptor {
            shared: Arc::clone(&self.shared),
        }
    }
}

/// [`russh::server::Server`] 实现：每条连接产出独立 Handler（会话隔离）。
#[derive(Clone)]
pub struct SshAcceptor {
    shared: Arc<ServerShared>,
}

impl RusshServer for SshAcceptor {
    type Handler = SshSessionHandler;

    fn new_client(&mut self, peer: Option<SocketAddr>) -> Self::Handler {
        let seq = self.shared.session_seq.fetch_add(1, Ordering::SeqCst);
        let session_id = format!("ssh-{seq}");
        self.shared
            .active_connections
            .fetch_add(1, Ordering::SeqCst);
        self.shared.record(
            &session_id,
            "unknown",
            "session-opened",
            true,
            peer.map(|p| format!("peer={p}")).unwrap_or_default(),
        );
        SshSessionHandler {
            shared: Arc::clone(&self.shared),
            context: SessionContext::new(
                session_id,
                self.shared.config.max_channels_per_connection,
            ),
        }
    }
}

/// 每连接 Handler：认证、通道生命周期与受限命令执行。
pub struct SshSessionHandler {
    shared: Arc<ServerShared>,
    context: SessionContext,
}

impl SshSessionHandler {
    /// 审计辅助（拒绝类动作也必须留痕——Check 97 的“显式授权路径有审计”）。
    fn record(&self, actor_id: &str, action: &str, ok: bool, detail: String) {
        self.shared
            .record(&self.context.session_id, actor_id, action, ok, detail);
    }
}

impl Drop for SshSessionHandler {
    fn drop(&mut self) {
        // 安全退出：连接结束（任何原因）即归还计数并写审计。执行器持有的请求在调用方
        // 消失后回执失败被忽略——无悬挂资源。
        self.shared
            .active_connections
            .fetch_sub(1, Ordering::SeqCst);
        self.shared.record(
            &self.context.session_id,
            self.context
                .actor
                .as_ref()
                .map(|a| a.id.as_str())
                .unwrap_or("unknown"),
            "session-closed",
            true,
            String::new(),
        );
    }
}

impl Handler for SshSessionHandler {
    type Error = russh::Error;

    /// 公钥认证（仅 Ed25519 + 授权清单命中；FR-607/611）。
    async fn auth_publickey(
        &mut self,
        user: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        if public_key.algorithm() != Algorithm::Ed25519 {
            self.record(
                user,
                "auth-denied",
                false,
                format!("algorithm={}", public_key.algorithm()),
            );
            return Ok(Auth::reject());
        }
        if !authorized_contains(&self.shared.authorized_keys, public_key) {
            self.record(
                user,
                "auth-denied",
                false,
                format!("fingerprint={}", hostkey::fingerprint(public_key)),
            );
            return Ok(Auth::reject());
        }
        let actor = Actor::new(
            format!("ssh:{user}"),
            self.shared.config.actor_capabilities.iter().copied(),
        );
        self.record(
            &actor.id,
            "auth-accepted",
            true,
            format!("fingerprint={}", hostkey::fingerprint(public_key)),
        );
        self.context.actor = Some(actor);
        Ok(Auth::Accept)
    }

    /// 打开会话通道（认证后；数量有界，超限拒绝）。
    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.context.actor.is_none() {
            // 未认证不允许打开通道（防御：协议上不应到达）。
            return Ok(()); // 丢弃 reply → 自动以 AdministrativelyProhibited 拒绝
        }
        if !self.context.open_channel(channel.id().number()) {
            self.record(
                self.context
                    .actor
                    .as_ref()
                    .map(|a| a.id.as_str())
                    .unwrap_or("unknown"),
                "channel-rejected",
                false,
                format!(
                    "通道数达到上限 {}（有界拒绝）",
                    self.shared.config.max_channels_per_connection
                ),
            );
            return Ok(()); // 丢弃 reply → 自动拒绝
        }
        reply.accept().await;
        Ok(())
    }

    /// 受限命令执行：策略判定 → 单写者执行器（应用服务）→ 结构化结果。
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(actor) = self.context.actor.clone() else {
            session.channel_failure(channel)?;
            return Ok(());
        };
        let command = String::from_utf8_lossy(data);
        let terminal = self.context.window_size(channel.number());
        let terminal_pair = terminal.map(|size| (size.cols, size.rows));

        match policy::classify(&self.shared.config.policy, &actor, &command) {
            Decision::WhCode(whcode_command) => {
                session.channel_success(channel)?;
                let outcome = match self
                    .shared
                    .executor
                    .execute(
                        self.context.session_id.clone(),
                        actor,
                        terminal_pair,
                        whcode_command,
                    )
                    .await
                {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        // 执行器层失败（背压/超时/线程退出）：结构化非 0 退出码。
                        let exit_code = match err.code {
                            ErrorCode::Timeout => crate::adapter::EXIT_TIMEOUT,
                            ErrorCode::Backpressure => crate::policy::EXIT_POLICY_DENIED,
                            _ => crate::adapter::EXIT_EXECUTION_ERROR,
                        };
                        CommandOutcome::failure(
                            exit_code,
                            format!("{}（错误码 {}）", err, err.code.as_str()),
                            "执行器未返回结果".to_string(),
                        )
                    }
                };
                write_outcome(session, channel, outcome)?;
            }
            Decision::SystemShell { .. } => {
                // 显式授权通过：审计留痕；实际执行属 M8 运行时 payload（ADR-024）。
                self.record(
                    &actor.id,
                    "shell-granted",
                    true,
                    "显式授权命中（执行属 M8，未实现）".to_string(),
                );
                session.channel_success(channel)?;
                write_outcome(
                    session,
                    channel,
                    CommandOutcome::failure(
                        crate::adapter::EXIT_EXECUTION_ERROR,
                        "系统 Shell 已获授权，但执行属 M8 运行时 payload（ADR-024）：\
                         本版本未实现，不启动任何子进程"
                            .to_string(),
                        "shell 执行未实现（显式边界）".to_string(),
                    ),
                )?;
            }
            Decision::Denied { reason, .. } => {
                self.record(&actor.id, "exec-denied", false, reason.clone());
                session.channel_success(channel)?;
                write_outcome(
                    session,
                    channel,
                    CommandOutcome::failure(
                        policy::EXIT_POLICY_DENIED,
                        format!("拒绝：{reason}"),
                        "policy denied".to_string(),
                    ),
                )?;
            }
        }
        Ok(())
    }

    /// 交互式 Shell 通道：M7 一律拒绝（默认无 Shell；授权的 Shell 也走 `shell` 命令的
    /// 结构化路径，不提供交互 PTY——交互终端属 M8）。
    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.record(
            self.context
                .actor
                .as_ref()
                .map(|a| a.id.as_str())
                .unwrap_or("unknown"),
            "shell-request-rejected",
            false,
            "交互式 Shell 通道不提供（FR-608 默认；交互终端属 M8）".to_string(),
        );
        session.channel_failure(channel)?;
        Ok(())
    }

    /// 记录 pty-req 终端尺寸（不模拟终端；尺寸随受限命令执行传递）。
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let size = PtySize {
            cols: col_width,
            rows: row_height,
        };
        if self.context.record_pty(channel.number(), size) {
            self.record(
                self.context
                    .actor
                    .as_ref()
                    .map(|a| a.id.as_str())
                    .unwrap_or("unknown"),
                "pty",
                true,
                format!("terminal={}x{}", size.cols, size.rows),
            );
            session.channel_success(channel)?;
        } else {
            session.channel_failure(channel)?;
        }
        Ok(())
    }

    /// 记录 window-change（RFC 4254 §7.3：该请求无应答，不回 success/failure）。
    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let size = PtySize {
            cols: col_width,
            rows: row_height,
        };
        let recorded = self.context.record_window_change(channel.number(), size);
        self.record(
            self.context
                .actor
                .as_ref()
                .map(|a| a.id.as_str())
                .unwrap_or("unknown"),
            "resize",
            recorded,
            format!("terminal={}x{}", size.cols, size.rows),
        );
        Ok(())
    }

    /// 通道关闭：回收通道状态（有界通道表的释放路径）。
    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.context.close_channel(channel.number());
        Ok(())
    }

    /// 不消费 stdin 数据（受限命令为一次性 exec；静默忽略）。
    async fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// 把执行结果写回通道：stdout/stderr → 退出状态 → EOF → 关闭（结构化收尾）。
fn write_outcome(
    session: &mut Session,
    channel: ChannelId,
    outcome: CommandOutcome,
) -> Result<(), russh::Error> {
    if !outcome.stdout.is_empty() {
        session.data(channel, outcome.stdout.into_bytes())?;
    }
    if !outcome.stderr.is_empty() {
        session.extended_data(channel, 1, outcome.stderr.into_bytes())?;
    }
    let exit_status = u32::try_from(outcome.exit_code.max(0)).unwrap_or(u32::MAX);
    session.exit_status_request(channel, exit_status)?;
    session.eof(channel)?;
    session.close(channel)?;
    Ok(())
}

/// `whcode serve --ssh` 的阻塞入口：装配状态目录 → 单写者执行器 → current_thread
/// 运行时监听（线程数有界：1 事件循环线程 + 1 执行线程，与会话数无关——ADR-030）。
///
/// # Errors
///
/// 主机密钥/授权清单装配失败、运行时创建、绑定或监听失败时返回 [`WhError`]。
pub fn serve_ssh_blocking(params: SshServeParams) -> Result<(), WhError> {
    let host_key = hostkey::load_or_create_host_key(&params.workspace_dir)?;
    println!(
        "WHCode SSH 主机密钥指纹：{}（{}）",
        host_key.fingerprint(),
        host_key.path().display()
    );
    let authorized_keys = hostkey::load_authorized_keys(&params.workspace_dir)?;
    println!(
        "WHCode SSH 授权公钥：{} 把（仅 Ed25519；清单 {}）",
        authorized_keys.len(),
        hostkey::authorized_keys_path(&params.workspace_dir).display()
    );

    let audit = Arc::new(AuditLog::new());
    let executor = Arc::new(CommandExecutor::start(
        params.connection,
        Arc::clone(&audit),
    )?);
    let server = SshServer::new(params.config, host_key, authorized_keys, executor, audit);
    let bind = params.bind;

    // current_thread 运行时：主线程即事件循环（不引入 rt-multi-thread，见 ADR-030）。
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("创建 SSH 运行时失败：{err}")))?;
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(bind.as_str())
            .await
            .map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("绑定 SSH 监听失败（{bind}）：{err}"),
                )
            })?;
        let addr = listener
            .local_addr()
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("读取监听地址失败：{err}")))?;
        println!(
            "WHCode SSH 服务：ssh -p {} <user>@{}（Ctrl-C 退出）",
            addr.port(),
            addr.ip()
        );
        let config = server.russh_config(Duration::from_millis(300));
        let mut acceptor = server.acceptor();
        let running = acceptor.run_on_socket(config, &listener);
        // 阻塞到关闭或接受失败（Ctrl-C 由进程终止，语义同 serve --web）。
        running
            .await
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("SSH 服务循环失败：{err}")))?;
        Ok(())
    })
}

/// `serve_ssh_blocking` 的装配参数。
pub struct SshServeParams {
    /// 工作区目录（`ssh/` 状态目录的根）。
    pub workspace_dir: std::path::PathBuf,
    /// 装配根打开的唯一数据库连接（Check 98：单写者）。
    pub connection: Connection,
    /// 监听地址（默认 [`DEFAULT_BIND_ADDR`]）。
    pub bind: String,
    /// 服务端配置（能力授予、策略、通道上限）。
    pub config: SshServerConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use russh::client;
    use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
    use whcode_app::Capability;

    use crate::policy::SshPolicy;
    use crate::trust::{HostKeyVerdict, KnownHosts};

    static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 测试用临时工作区；析构时尽力删除。
    struct TempWorkspace {
        path: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("whcode-ssh-server-{}-{seq}", std::process::id()));
            std::fs::create_dir_all(&path).expect("创建临时目录");
            Self { path }
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn write_key_file(path: &Path, key: &PrivateKey) {
        let encoded = key
            .to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .expect("编码私钥");
        let mut file = std::fs::File::create(path).expect("创建私钥文件");
        file.write_all(encoded.as_bytes()).expect("写入私钥");
    }

    /// 测试迁移夹具：include_str 复用 whcode-store 的迁移 SQL（前端清单不得依赖
    /// whcode-store——分层门禁；与 whcode-web 测试同一做法）。
    const TEST_MIGRATIONS: &str = concat!(
        include_str!("../../whcode-store/migrations/0001_initial.sql"),
        include_str!("../../whcode-store/migrations/0002_event_trace_indexes.sql"),
        include_str!("../../whcode-store/migrations/0003_query_indexes.sql"),
        include_str!("../../whcode-store/migrations/0004_session_id_sequence.sql"),
        include_str!("../../whcode-store/migrations/0005_session_stage.sql"),
        include_str!("../../whcode-store/migrations/0006_events_variant.sql"),
    );

    fn migrated_memory_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        conn.execute_batch(TEST_MIGRATIONS).expect("迁移");
        conn
    }

    /// 测试夹具：工作区 + 主机密钥 + 授权清单（客户端 Ed25519 公钥）+ 单写者执行器。
    struct TestFixture {
        workspace: TempWorkspace,
        server: SshServer,
        /// 库侧观察连接（测试基建，只读）。
        observer: Connection,
        client_key_path: PathBuf,
        client_user: String,
    }

    impl TestFixture {
        /// `policy`/`capabilities` 为 `None` 时取默认（只读能力 + 无 Shell）。
        fn new(policy: Option<SshPolicy>, capabilities: Option<&[Capability]>) -> Self {
            let workspace = TempWorkspace::new();
            let host_key = hostkey::load_or_create_host_key(&workspace.path).expect("生成主机密钥");
            let client_key =
                PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("客户端密钥");
            let client_key_path = workspace.path.join("client.key");
            write_key_file(&client_key_path, &client_key);
            std::fs::write(
                hostkey::authorized_keys_path(&workspace.path),
                client_key
                    .public_key()
                    .to_openssh()
                    .expect("编码公钥")
                    .as_bytes(),
            )
            .expect("写入授权清单");

            let observer = migrated_memory_conn();
            let server_conn = observer.try_clone().expect("同实例观察连接");
            let audit = Arc::new(AuditLog::new());
            let executor =
                Arc::new(CommandExecutor::start(server_conn, Arc::clone(&audit)).expect("执行器"));
            let mut config = SshServerConfig::default();
            if let Some(policy) = policy {
                config.policy = policy;
            }
            if let Some(capabilities) = capabilities {
                config.actor_capabilities = capabilities.iter().copied().collect();
            }
            let authorized_keys =
                hostkey::load_authorized_keys(&workspace.path).expect("加载授权清单");
            let server = SshServer::new(config, host_key, authorized_keys, executor, audit);
            Self {
                workspace,
                server,
                observer,
                client_key_path,
                client_user: "tester".to_string(),
            }
        }

        fn audit(&self) -> Arc<AuditLog> {
            Arc::clone(self.server.audit())
        }
    }

    /// 测试客户端 Handler：可选接入 known_hosts 信任文件（TOFU 语义）。
    struct TestClientHandler {
        trust: Option<(KnownHosts, u16)>,
    }

    impl client::Handler for TestClientHandler {
        type Error = russh::Error;

        async fn check_server_key(
            &mut self,
            server_public_key: &PublicKeyOrCertificate,
        ) -> Result<bool, Self::Error> {
            let Some((hosts, port)) = &self.trust else {
                return Ok(true);
            };
            let PublicKeyOrCertificate::PublicKey { key, .. } = server_public_key else {
                return Ok(false); // 证书不在本策略内
            };
            let verdict = match hosts.verify("127.0.0.1", *port, key) {
                Ok(verdict) => verdict,
                Err(_) => return Ok(false), // 不可读：fail closed
            };
            match verdict {
                HostKeyVerdict::Trusted => Ok(true),
                HostKeyVerdict::FirstSeen => Ok(hosts.record("127.0.0.1", *port, key).is_ok()),
                HostKeyVerdict::Mismatch { .. } => Ok(false),
            }
        }
    }

    /// 客户端一次 exec 的收集结果。
    #[derive(Debug, Default, PartialEq, Eq)]
    struct ClientOutput {
        stdout: String,
        stderr: String,
        exit_code: Option<u32>,
    }

    /// 客户端测试错误（认证被拒与传输失败区分开）。
    #[derive(Debug)]
    enum ClientError {
        AuthRejected,
        /// 传输层失败（载荷仅用于诊断输出）。
        Transport(#[allow(dead_code)] russh::Error),
    }

    /// 连接 + 公钥认证 + 打开通道 + exec + 收集输出 + 显式断开。
    async fn ssh_exec(
        port: u16,
        key_path: &Path,
        user: &str,
        command: &str,
        trust: Option<KnownHosts>,
    ) -> Result<ClientOutput, ClientError> {
        let key = load_secret_key(key_path, None).expect("加载客户端私钥");
        let handler = TestClientHandler {
            trust: trust.map(|hosts| (hosts, port)),
        };
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        let mut session = client::connect(config, ("127.0.0.1", port), handler)
            .await
            .map_err(ClientError::Transport)?;
        let auth = session
            .authenticate_publickey(
                user.to_string(),
                PrivateKeyWithHashAlg::new(Arc::new(key), None),
            )
            .await
            .map_err(ClientError::Transport)?;
        if !auth.success() {
            let _ = session
                .disconnect(russh::Disconnect::ByApplication, "auth rejected", "")
                .await;
            return Err(ClientError::AuthRejected);
        }
        let mut channel = session
            .channel_open_session()
            .await
            .map_err(ClientError::Transport)?;
        channel
            .exec(true, command.to_string())
            .await
            .map_err(ClientError::Transport)?;
        let mut output = ClientOutput::default();
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.stdout.push_str(&String::from_utf8_lossy(&data));
                }
                Some(russh::ChannelMsg::ExtendedData { data, ext: 1 }) => {
                    output.stderr.push_str(&String::from_utf8_lossy(&data));
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    output.exit_code = Some(exit_status);
                }
                Some(_) => {}
                None => break,
            }
        }
        // 显式断开：服务端会话任务结束 → Handler Drop → 活跃连接归还（安全退出路径）。
        let _ = session
            .disconnect(russh::Disconnect::ByApplication, "done", "")
            .await;
        Ok(output)
    }

    /// 连接 + 认证 + 打开通道 + pty + window-change + exec + 收集（终端尺寸链路测试）。
    async fn ssh_exec_with_pty(
        port: u16,
        key_path: &Path,
        user: &str,
        command: &str,
        pty: (u32, u32),
        resize: Option<(u32, u32)>,
    ) -> Result<ClientOutput, ClientError> {
        let key = load_secret_key(key_path, None).expect("加载客户端私钥");
        let handler = TestClientHandler { trust: None };
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        let mut session = client::connect(config, ("127.0.0.1", port), handler)
            .await
            .map_err(ClientError::Transport)?;
        let auth = session
            .authenticate_publickey(
                user.to_string(),
                PrivateKeyWithHashAlg::new(Arc::new(key), None),
            )
            .await
            .map_err(ClientError::Transport)?;
        assert!(auth.success(), "夹具密钥必须通过认证");
        let mut channel = session
            .channel_open_session()
            .await
            .map_err(ClientError::Transport)?;
        channel
            .request_pty(true, "dumb", pty.0, pty.1, 0, 0, &[])
            .await
            .map_err(ClientError::Transport)?;
        if let Some((cols, rows)) = resize {
            channel
                .window_change(cols, rows, 0, 0)
                .await
                .map_err(ClientError::Transport)?;
        }
        channel
            .exec(true, command.to_string())
            .await
            .map_err(ClientError::Transport)?;
        let mut output = ClientOutput::default();
        loop {
            match channel.wait().await {
                Some(russh::ChannelMsg::Data { data }) => {
                    output.stdout.push_str(&String::from_utf8_lossy(&data));
                }
                Some(russh::ChannelMsg::ExtendedData { data, ext: 1 }) => {
                    output.stderr.push_str(&String::from_utf8_lossy(&data));
                }
                Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                    output.exit_code = Some(exit_status);
                }
                Some(_) => {}
                None => break,
            }
        }
        let _ = session
            .disconnect(russh::Disconnect::ByApplication, "done", "")
            .await;
        Ok(output)
    }

    /// 轮询直到条件成立（资源回收是异步的，给 5 s 预算）。
    async fn wait_until(mut condition: impl FnMut() -> bool, what: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(tokio::time::Instant::now() < deadline, "等待超时：{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn publickey_auth_success_and_whitelist_exec_round_trip() {
        let fixture = TestFixture::new(None, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "sessions",
            None,
        );
        let output = tokio::select! {
            result = ops => result.expect("合法公钥必须通过认证并执行成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(0), "stderr：{}", output.stderr);
        assert!(
            output.stdout.contains("还没有会话"),
            "正确的空必须显式：{}",
            output.stdout
        );

        running.handle().shutdown("测试结束".to_string());
        let drained = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        assert!(drained.is_ok(), "服务端应响应关闭并退出");
    }

    #[tokio::test]
    async fn unauthorized_public_key_is_rejected_and_audited() {
        let fixture = TestFixture::new(None, None);
        // 未被授权的第二把 Ed25519 密钥。
        let outsider = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("密钥");
        let outsider_path = fixture.workspace.path.join("outsider.key");
        write_key_file(&outsider_path, &outsider);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = ssh_exec(port, &outsider_path, &fixture.client_user, "sessions", None);
        let outcome = tokio::select! {
            result = ops => result,
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert!(
            matches!(outcome, Err(ClientError::AuthRejected)),
            "未授权公钥必须在认证阶段被拒：{outcome:?}"
        );

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        let audit = fixture.audit().snapshot();
        assert!(
            audit
                .iter()
                .any(|e| e.action == "auth-denied" && e.detail.contains("fingerprint=")),
            "认证拒绝必须留审计（只记指纹）：{audit:?}"
        );
        assert!(
            audit.iter().all(|e| !e.detail.contains("BEGIN OPENSSH")),
            "审计不得携带密钥材料"
        );
    }

    #[tokio::test]
    async fn non_ed25519_public_key_is_rejected_by_algorithm() {
        let fixture = TestFixture::new(None, None);
        let ecdsa_key = PrivateKey::random(
            &mut rand::rng(),
            Algorithm::Ecdsa {
                curve: russh::keys::EcdsaCurve::NistP256,
            },
        )
        .expect("生成 P-256 密钥");
        let ecdsa_path = fixture.workspace.path.join("ecdsa.key");
        write_key_file(&ecdsa_path, &ecdsa_key);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = ssh_exec(port, &ecdsa_path, &fixture.client_user, "sessions", None);
        let outcome = tokio::select! {
            result = ops => result,
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert!(
            matches!(outcome, Err(ClientError::AuthRejected)),
            "非 Ed25519 公钥必须被拒：{outcome:?}"
        );

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        let audit = fixture.audit().snapshot();
        assert!(
            audit
                .iter()
                .any(|e| e.action == "auth-denied" && e.detail.contains("algorithm=")),
            "算法拒绝必须留审计：{audit:?}"
        );
    }

    #[tokio::test]
    async fn outside_whitelist_command_is_denied_structured() {
        let fixture = TestFixture::new(None, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "rm -rf /",
            None,
        );
        let output = tokio::select! {
            result = ops => result.expect("连接本身应成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(policy::EXIT_POLICY_DENIED as u32));
        assert!(
            output.stderr.contains("不在 SSH 受限白名单"),
            "stderr 必须结构化说明拒绝原因：{}",
            output.stderr
        );

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        let audit = fixture.audit().snapshot();
        assert!(
            audit.iter().any(|e| e.action == "exec-denied"),
            "白名单外拒绝必须留审计：{audit:?}"
        );
    }

    #[tokio::test]
    async fn shell_is_denied_by_default_and_structured_when_granted() {
        // 默认策略：拒绝。
        let fixture = TestFixture::new(None, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);
        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "shell ls -la",
            None,
        );
        let output = tokio::select! {
            result = ops => result.expect("连接应成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(policy::EXIT_POLICY_DENIED as u32));
        assert!(
            output.stderr.contains("未启用"),
            "默认拒绝必须说明原因：{}",
            output.stderr
        );
        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;

        // 显式授权（开关 + 名单）：放行到 SystemShell 判定，执行结构化为“M8 未实现”。
        let granted = TestFixture::new(Some(SshPolicy::shell_for(["ssh:tester"])), None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = granted.server.acceptor();
        let running = acceptor.run_on_socket(
            granted.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);
        let ops = ssh_exec(
            port,
            &granted.client_key_path,
            &granted.client_user,
            "shell ls -la",
            None,
        );
        let output = tokio::select! {
            result = ops => result.expect("连接应成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_ne!(output.exit_code, Some(0), "未实现边界必须非 0");
        assert!(
            output.stderr.contains("M8") && output.stderr.contains("未实现"),
            "放行结果必须显式写明未实现边界：{}",
            output.stderr
        );
        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        let audit = granted.audit().snapshot();
        assert!(
            audit.iter().any(|e| e.action == "shell-granted"),
            "Shell 显式授权必须留审计（Check 97）：{audit:?}"
        );
    }

    #[tokio::test]
    async fn pty_and_window_change_are_recorded_and_passed_to_execution() {
        let fixture = TestFixture::new(None, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = ssh_exec_with_pty(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "sessions",
            (120, 40),
            Some((200, 50)),
        );
        let output = tokio::select! {
            result = ops => result.expect("pty + resize + exec 链路应成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(0), "stderr：{}", output.stderr);

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        let audit = fixture.audit().snapshot();
        assert!(
            audit
                .iter()
                .any(|e| e.action == "pty" && e.detail.contains("terminal=120x40")),
            "pty-req 尺寸必须记录：{audit:?}"
        );
        assert!(
            audit
                .iter()
                .any(|e| e.action == "resize" && e.detail.contains("terminal=200x50")),
            "window-change 尺寸必须记录：{audit:?}"
        );
        let exec = audit
            .iter()
            .find(|e| e.action.starts_with("exec:"))
            .expect("执行留审计");
        assert_eq!(
            exec.detail, "terminal=200x50",
            "受限命令执行必须携带最新终端尺寸"
        );
    }

    #[tokio::test]
    async fn disconnect_reclaims_session_resources() {
        let fixture = TestFixture::new(None, None);
        assert_eq!(fixture.server.active_connections(), 0);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "sessions",
            None,
        );
        let output = tokio::select! {
            result = ops => result.expect("执行应成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(0));
        wait_until(
            || fixture.server.active_connections() == 0,
            "断开后活跃连接数必须归零（安全退出）",
        )
        .await;
        let audit = fixture.audit().snapshot();
        assert!(
            audit.iter().any(|e| e.action == "session-closed"),
            "断开必须写会话关闭审计：{audit:?}"
        );

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
    }

    #[tokio::test]
    async fn three_concurrent_ssh_sessions_write_through_single_connection() {
        // Check 98：3 个真实 SSH 会话并发执行写命令，写路径只有执行线程持有的那条连接。
        let fixture = TestFixture::new(
            None,
            Some(&[Capability::SessionCreate, Capability::SessionRead]),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        let ops = async {
            let mut handles = Vec::new();
            for i in 0..3 {
                let key_path = fixture.client_key_path.clone();
                let user = fixture.client_user.clone();
                handles.push(tokio::spawn(async move {
                    ssh_exec(
                        port,
                        &key_path,
                        &user,
                        &format!("session new 并发会话{i}"),
                        None,
                    )
                    .await
                }));
            }
            let mut outputs = Vec::new();
            for handle in handles {
                outputs.push(handle.await.expect("任务完成").expect("执行成功"));
            }
            outputs
        };
        let outputs = tokio::select! {
            outputs = ops => outputs,
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        for output in &outputs {
            assert_eq!(output.exit_code, Some(0), "stderr：{}", output.stderr);
            assert!(
                output.stdout.starts_with("created S-"),
                "：{}",
                output.stdout
            );
        }

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;

        // 审计：3 条写（wrote = true），来自三个独立 ssh 会话。
        let audit = fixture.audit().snapshot();
        let writes: Vec<_> = audit.iter().filter(|e| e.wrote).collect();
        assert_eq!(writes.len(), 3, "写审计恰好 3 条：{audit:?}");
        let distinct_sessions: std::collections::BTreeSet<_> =
            writes.iter().map(|e| e.session_id.as_str()).collect();
        assert_eq!(
            distinct_sessions.len(),
            3,
            "三条写来自三个独立会话（会话隔离）：{writes:?}"
        );
        // 库侧：3 个会话都在（写只经过执行线程持有的那条连接）。
        let count: i64 = fixture
            .observer
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .expect("查询会话数");
        assert_eq!(count, 3, "并发写全部经单条连接落库");
    }

    #[tokio::test]
    async fn host_key_tofu_first_seen_trusted_then_mismatch_rejected() {
        let fixture = TestFixture::new(None, None);
        let hosts = KnownHosts::in_workspace(&fixture.workspace.path);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("绑定");
        let port = listener.local_addr().expect("端口").port();
        let mut acceptor = fixture.server.acceptor();
        let running = acceptor.run_on_socket(
            fixture.server.russh_config(Duration::from_millis(1)),
            &listener,
        );
        tokio::pin!(running);

        // 第一次连接：FirstSeen → 记录指纹 → 成功。
        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "sessions",
            Some(KnownHosts::at(hosts.path().to_path_buf())),
        );
        let output = tokio::select! {
            result = ops => result.expect("首次连接（TOFU 记录）应成功"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(0));
        let server_fingerprint = fixture.server.host_key().fingerprint().to_string();
        let recorded = std::fs::read_to_string(hosts.path()).expect("读取信任文件");
        assert!(
            recorded.contains(&format!("[127.0.0.1]:{port} {server_fingerprint}")),
            "首次连接必须把服务端指纹写入信任文件：{recorded}"
        );

        // 第二次连接：Trusted → 成功（不重复记录）。
        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "sessions",
            Some(KnownHosts::at(hosts.path().to_path_buf())),
        );
        let output = tokio::select! {
            result = ops => result.expect("第二次连接应命中信任记录"),
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert_eq!(output.exit_code, Some(0));
        assert_eq!(
            std::fs::read_to_string(hosts.path())
                .expect("读取")
                .lines()
                .count(),
            1,
            "命中记录不得重复写入"
        );

        // 第三次：同主机端口被人工改成攻击者指纹 → 拒绝（连接失败），且不重写信任文件。
        let attacker = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("密钥");
        let entry = format!(
            "[127.0.0.1]:{port} {}",
            hostkey::fingerprint(attacker.public_key())
        );
        std::fs::write(hosts.path(), entry.as_bytes()).expect("写入攻击者指纹");
        let ops = ssh_exec(
            port,
            &fixture.client_key_path,
            &fixture.client_user,
            "sessions",
            Some(KnownHosts::at(hosts.path().to_path_buf())),
        );
        let outcome = tokio::select! {
            result = ops => result,
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };
        assert!(
            matches!(outcome, Err(ClientError::Transport(_))),
            "指纹不匹配必须在连接阶段被拒：{outcome:?}"
        );
        let after = std::fs::read_to_string(hosts.path()).expect("读取");
        assert!(
            after.contains(&hostkey::fingerprint(attacker.public_key())),
            "不匹配时信任文件保持人工记录（不得自动改写）：{after}"
        );

        running.handle().shutdown("测试结束".to_string());
        let _ = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
    }

    /// 回归（系统 OpenSSH 冒烟命中）：authorized_keys 行带注释、客户端上送不带注释，
    /// 授权判定必须仍命中（PublicKey 的 PartialEq 含 comment，不能直接比结构体）。
    #[test]
    fn authorized_keys_with_comments_match_commentless_client_keys() {
        let client = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("密钥");
        let with_comment = format!(
            "{} user@host",
            client.public_key().to_openssh().expect("编码公钥")
        );
        let parsed = russh::keys::PublicKey::from_openssh(&with_comment).expect("解析带注释公钥");
        assert!(
            authorized_contains(std::slice::from_ref(&parsed), client.public_key()),
            "注释不得参与密钥同一性判定"
        );
        let other = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("密钥");
        assert!(
            !authorized_contains(&[parsed], other.public_key()),
            "不同密钥必须拒绝"
        );
    }

    #[test]
    fn session_context_bounds_channels_and_tracks_sizes() {
        let mut context = SessionContext::new("ssh-0".to_string(), 2);
        assert!(context.actor.is_none());
        assert!(context.open_channel(1));
        assert!(context.open_channel(2));
        assert!(!context.open_channel(3), "超过上限必须拒绝（有界）");
        assert!(context.record_pty(1, PtySize { cols: 80, rows: 24 }));
        assert_eq!(context.window_size(1), Some(PtySize { cols: 80, rows: 24 }));
        assert!(context.record_window_change(
            1,
            PtySize {
                cols: 100,
                rows: 30
            }
        ));
        assert_eq!(
            context.window_size(1),
            Some(PtySize {
                cols: 100,
                rows: 30
            })
        );
        assert!(
            !context.record_pty(9, PtySize { cols: 1, rows: 1 }),
            "未知通道"
        );
        assert_eq!(context.channel_count(), 2);
        context.close_channel(1);
        assert_eq!(context.channel_count(), 1);
        assert!(context.open_channel(3), "关闭后可复用配额");
    }
}
