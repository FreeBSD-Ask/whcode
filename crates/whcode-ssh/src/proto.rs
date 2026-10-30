//! SSH 客户端：意图—执行—登记（tasks.md T025、ADR-007；主机密钥 TOFU 校验——ADR-005/030）。
//!
//! **范围（平台承诺已修订）**：OpenBSD 与 DragonFly 不再作为原生安装目标，改由 WHCode 内置
//! SSH **客户端**控制；M0 只交付“经 SSH 通道执行命令”的风险原型，完整客户端能力与多平台
//! 验收属 M7（tasks.md T096—T099）。本模块**不**实现 SSH 服务端（服务端见 [`crate::server`]）。
//!
//! 三段协议（ADR-007）与语义规则：
//! 1. **校验先于意图**：`host`/`user`/`command` 为空、私钥缺失或不可读、`timeout_ms` 为 0
//!    （无界超时）时返回 [`WhError::invalid_state`]，**不**发生任何登记副作用；
//! 2. **意图先于连接**：校验通过后、建立连接之前必须调用 [`ExecRegistry::record_intent`]；
//! 3. 连接或认证失败：状态 `failed`、`exit_code` 为 `None`，`note` 写明失败阶段；**仍然**
//!    调用 [`ExecRegistry::record_result`] 并返回 `Ok(report)`；
//! 4. 命令已发出但结果不可知（超时、通道异常关闭且未收到退出状态）：状态 `unknown`，`note`
//!    明确写出“命令可能已执行，必须核对，不得盲目重放”；
//! 5. 收到明确退出状态：`0` → `completed`，非 `0` → `failed`；
//! 6. 任何情况下**不得**把不确定情形记为 `completed`/`failed`；
//! 7. 输出有界：只保留**尾部** [`MAX_TAIL_BYTES`] 字节，超出时 `truncated = true` 且在
//!    `note` 中写明截断事实（不得静默丢弃原始输出这一事实）；
//! 8. [`ExecRegistry::record_result`] 失败时整个调用返回该错误（登记失败不得视为成功）。
//!
//! ## 主机密钥校验（TOFU——ADR-005 阻断项已结清）
//!
//! [`SshTarget::known_hosts_path`] 指向 known_hosts 风格信任文件（[`crate::trust`]）：
//! - **未记录**（首次使用）：记录服务端指纹到信任文件，并把指纹打印到 stderr 供**人工
//!   核对**后放行（TOFU；指纹 `SHA256:…`，不含密钥材料）；
//! - **已记录且匹配**：放行；
//! - **已记录但不匹配**：结构化拒绝——连接中止，`report.status = failed`，`note` 写明
//!   “主机密钥与信任记录不匹配”与期望指纹，命令未发出；信任文件不被改写（改绑必须人工）；
//! - 信任文件不可读：同样拒绝（fail closed，不当成首次使用）。
//!
//! 已知限制（本原型不承诺、且未验证的能力）：
//! - **不产出 `timed_out`**：SSH 通道无法确认远端进程确实已终止，因此超时一律上报 `unknown`
//!   （ADR-007 要求不确定不得谎报）；只有能确认终止时才允许 `timed_out`；
//! - 口令保护的私钥不支持（以空口令加载）；
//! - 不向远端传递任何本地环境变量；`note` 与登记内容**不**包含私钥内容、口令或指纹之外的
//!   密钥材料。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use russh::client;
use russh::keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key};
use russh::{ChannelMsg, Disconnect};
use whcode_domain::{ErrorCode, RuntimeExecutionStatus, WhError};

use crate::hostkey::fingerprint;
use crate::trust::{HostKeyVerdict, KnownHosts};

/// 保留的输出上限（字节）。只保留**尾部**，超出部分丢弃但必须显式标记（规则 7）。
pub const MAX_TAIL_BYTES: usize = 64 * 1024;

/// 不确定情形固定的提示语：要求核对、禁止盲目重放（规则 4、FR-410）。
const INDETERMINATE_ADVICE: &str = "命令可能已执行，必须核对，不得盲目重放";

/// SSH 执行目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    /// 主机名或 IP 字面量。
    pub host: String,
    /// SSH 端口。
    pub port: u16,
    /// 远端登录用户名。
    pub user: String,
    /// 私钥路径（OpenSSH 格式；口令保护的私钥在本原型中不支持）。
    pub private_key_path: PathBuf,
    /// 在远端执行的命令（由远端 shell 解释，因此需由调用方保证参数转义）。
    ///
    /// 默认入口只允许远端 `whcode` 命令（见 [`validate_target`]）；任意系统 Shell 属
    /// **显式独立权限**，必须把 [`SshTarget::allow_shell`] 置为 `true` 并留审计（FR-608/611）。
    pub command: String,
    /// 是否允许执行任意远端命令（默认 `false`）。
    ///
    /// 这是客户端的“显式 Shell 授权”开关：置 `true` 时意图登记会带上 `allow_shell` 标记，
    /// 结果说明也会注明 `shell-granted`，便于审计追溯（ADR-007）。
    pub allow_shell: bool,
    /// 整个调用的超时上限（毫秒）。必须大于 0：默认超时必须是有界的（安全默认）。
    pub timeout_ms: u64,
    /// 主机密钥信任文件（known_hosts 风格，TOFU——见 [`crate::trust`]）。必填：没有信任
    /// 文件的客户端不提供中间人防护（ADR-005 阻断项），调用方必须显式给出路径。
    pub known_hosts_path: PathBuf,
}

/// 一次远程执行的结果报告。
///
/// `status` 复用 [`RuntimeExecutionStatus`]（§3.1 唯一出处），本模块**不**另造状态枚举。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecReport {
    /// 执行状态。
    pub status: RuntimeExecutionStatus,
    /// 远端退出码；未收到明确退出状态时为 `None`。
    pub exit_code: Option<i32>,
    /// 标准输出尾部（最多 [`MAX_TAIL_BYTES`] 字节，按 UTF-8 有损解码）。
    pub stdout_tail: String,
    /// 标准错误尾部（最多 [`MAX_TAIL_BYTES`] 字节，按 UTF-8 有损解码）。
    pub stderr_tail: String,
    /// 调用耗时（毫秒）。
    pub duration_ms: u64,
    /// 任一输出流是否发生截断（规则 7）。
    pub truncated: bool,
    /// 人可读说明：失败阶段、截断事实或“必须核对”的提示。**不得**含私钥内容或口令。
    pub note: String,
}

/// 执行协议登记接口（ADR-007 的“意图—结果”两段登记）。
///
/// 实现方返回错误时调用方**不得**把操作视为成功：意图登记失败则不得发起连接，结果登记失败
/// 则整个调用返回该错误。
pub trait ExecRegistry {
    /// 记录执行意图（必须在建立连接之前调用）。
    fn record_intent(&self, target: &SshTarget) -> Result<(), WhError>;

    /// 登记执行结果（含连接/认证失败与不确定状态）。
    fn record_result(&self, report: &ExecReport) -> Result<(), WhError>;
}

/// 已登记的协议调用，供测试断言调用顺序与内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryCall {
    /// 意图记录。
    Intent {
        /// 目标主机。
        host: String,
        /// 目标端口。
        port: u16,
        /// 登录用户。
        user: String,
        /// 待执行命令。
        command: String,
        /// 是否使用了显式 Shell 授权（审计可见；默认 `false`）。
        allow_shell: bool,
    },
    /// 结果登记。
    Result {
        /// 执行状态。
        status: RuntimeExecutionStatus,
        /// 退出码。
        exit_code: Option<i32>,
        /// 说明。
        note: String,
    },
}

/// 内存登记缓冲（`Mutex<Vec<...>>`），供测试断言调用顺序与内容。
///
/// 注意：本类型不持久化，**不得**作为发布路径的实现（持久化登记属 M2 的 store 层）。
#[derive(Debug, Default)]
pub struct CollectingExecRegistry {
    calls: Mutex<Vec<RegistryCall>>,
}

impl CollectingExecRegistry {
    /// 空缓冲。
    pub fn new() -> Self {
        Self::default()
    }

    /// 已登记的调用快照（按发生顺序）。
    pub fn calls(&self) -> Vec<RegistryCall> {
        self.calls
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 已登记调用数。
    pub fn len(&self) -> usize {
        self.calls.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    /// 是否没有任何登记调用（校验失败时应为 `true`）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ExecRegistry for CollectingExecRegistry {
    fn record_intent(&self, target: &SshTarget) -> Result<(), WhError> {
        let mut guard = self
            .calls
            .lock()
            .map_err(|_| WhError::internal("执行登记缓冲锁中毒：意图未登记，不得发起连接"))?;
        guard.push(RegistryCall::Intent {
            host: target.host.clone(),
            port: target.port,
            user: target.user.clone(),
            command: target.command.clone(),
            allow_shell: target.allow_shell,
        });
        Ok(())
    }

    fn record_result(&self, report: &ExecReport) -> Result<(), WhError> {
        let mut guard = self
            .calls
            .lock()
            .map_err(|_| WhError::internal("执行登记缓冲锁中毒：结果未登记，调用不得视为成功"))?;
        guard.push(RegistryCall::Result {
            status: report.status,
            exit_code: report.exit_code,
            note: report.note.clone(),
        });
        Ok(())
    }
}

/// 经 SSH 在远端执行 `target.command`，按三段协议登记并返回报告。
///
/// 返回值语义：
/// - `Err` 只表示**协议前提被破坏**：目标非法（规则 1）或登记失败（规则 8）；
/// - `Ok(report)` 表示协议已完整执行并已登记；远端失败、认证失败与“结果不确定”都通过
///   `report.status` 表达（规则 3、4）。
///
/// # Errors
/// 目标校验失败返回 [`ErrorCode::InvalidState`](whcode_domain::ErrorCode::InvalidState)，
/// 命令策略拒绝（默认入口的任意系统 Shell）返回
/// [`ErrorCode::PermissionDenied`](whcode_domain::ErrorCode::PermissionDenied)；两者均不可重试。
/// 登记失败返回登记实现方给出的错误。
pub async fn run_command(
    target: &SshTarget,
    registry: &dyn ExecRegistry,
) -> Result<ExecReport, WhError> {
    // 规则 1：校验先于意图——校验失败不得产生任何副作用。
    validate_target(target)?;
    // 规则 2：意图先于连接——登记失败则不得发起连接。
    registry.record_intent(target)?;

    let mut report = execute(target).await;
    if target.allow_shell {
        // 显式 Shell 授权必须出现在结果说明里（审计可追溯；默认路径永远不带）。
        report.note = format!("shell-granted（显式授权）：{}", report.note);
    }

    // 规则 8：结果登记失败时整体失败。
    registry.record_result(&report)?;
    Ok(report)
}

/// 校验目标；字段非法返回 `invalid_state`（不可重试），命令策略拒绝返回
/// `permission_denied`（不可重试）。
fn validate_target(target: &SshTarget) -> Result<(), WhError> {
    if target.host.trim().is_empty() {
        return Err(WhError::invalid_state("SSH 目标非法：host 为空"));
    }
    if target.user.trim().is_empty() {
        return Err(WhError::invalid_state("SSH 目标非法：user 为空"));
    }
    if target.command.trim().is_empty() {
        return Err(WhError::invalid_state("SSH 目标非法：command 为空"));
    }
    validate_command_policy(target)?;
    if target.timeout_ms == 0 {
        // 安全默认：超时必须是有界的，0 视为非法配置而非“无限等待”。
        return Err(WhError::invalid_state(
            "SSH 目标非法：timeout_ms 为 0，超时必须有界",
        ));
    }
    let key_path = &target.private_key_path;
    let metadata = std::fs::metadata(key_path).map_err(|err| {
        WhError::invalid_state(format!("私钥不可用：{}（{err}）", key_path.display()))
    })?;
    if !metadata.is_file() {
        return Err(WhError::invalid_state(format!(
            "私钥不是普通文件：{}",
            key_path.display()
        )));
    }
    // 只验证可读性，不读取内容、不记录内容（私钥材料不得进入日志或报告）。
    std::fs::File::open(key_path).map_err(|err| {
        WhError::invalid_state(format!("私钥不可读：{}（{err}）", key_path.display()))
    })?;
    Ok(())
}

/// 客户端默认入口允许的远端程序：只允许 WHCode 自身的命令。
///
/// 这是“默认无任意系统 Shell”的客户端一侧（FR-608/611）：远端 `sh`／`bash`／`powershell`
/// 等一律拒绝，除非 [`SshTarget::allow_shell`] 显式打开（并留审计）。
const ALLOWED_REMOTE_PROGRAMS: [&str; 2] = ["whcode", "whcode.exe"];

/// 客户端命令策略：默认只放行远端 `whcode`（含绝对路径与 `.exe` 变体）。
fn validate_command_policy(target: &SshTarget) -> Result<(), WhError> {
    if target.allow_shell {
        return Ok(());
    }
    let program = target
        .command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    if ALLOWED_REMOTE_PROGRAMS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(program))
    {
        return Ok(());
    }
    Err(WhError::new(
        ErrorCode::PermissionDenied,
        format!(
            "SSH 客户端默认入口只允许远端 whcode 命令；{program:?} 属任意系统 Shell，\
             需显式 allow_shell 授权并留审计（FR-608/611）"
        ),
    ))
}

/// 客户端的会话级处理器：主机密钥 TOFU 校验（ADR-005 阻断项结清）。
///
/// - 证书形态不支持（仅公钥）；信任文件不可读按拒绝处理（fail closed）；
/// - 首次使用记录指纹并打印到 stderr 供人工核对（TOFU）；
/// - 指纹不匹配：拒绝连接，并把结构化原因写入 `mismatch` 槽位——`execute` 据此给出
///   可操作的失败说明（`client::connect` 失败只给笼统错误，细节必须自己带出来）。
struct ProtoClient {
    trust: KnownHosts,
    host: String,
    port: u16,
    /// 指纹不匹配时的结构化说明（`Arc<Mutex<_>>`：handler 被移入 connect，失败后无法取回）。
    mismatch: Arc<Mutex<Option<String>>>,
}

impl client::Handler for ProtoClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_public_key else {
            // 证书主机密钥不在本客户端策略内（仅 Ed25519 公钥，FR-611）。
            return Ok(false);
        };
        match self.trust.verify(&self.host, self.port, key) {
            Ok(HostKeyVerdict::Trusted) => Ok(true),
            Ok(HostKeyVerdict::FirstSeen) => {
                // TOFU：记录指纹 + 显式打印供人工核对（stderr，不进事件正文）。
                if let Err(err) = self.trust.record(&self.host, self.port, key) {
                    eprintln!("WHCode SSH：记录主机指纹失败（{err}）：按拒绝处理");
                    return Ok(false);
                }
                eprintln!(
                    "WHCode SSH：首次连接 {}:{}，主机密钥指纹 {}（已记录到 {}，请人工核对；\
                     之后指纹变化将被拒绝）",
                    self.host,
                    self.port,
                    fingerprint(key),
                    self.trust.path().display()
                );
                Ok(true)
            }
            Ok(HostKeyVerdict::Mismatch {
                line,
                expected_fingerprint,
            }) => {
                let detail = format!(
                    "主机密钥与信任记录不匹配（信任文件第 {line} 行期望 {expected_fingerprint}，\
                     实际 {}）：拒绝连接（中间人风险；如确为服务端换钥，请人工更新 {}）",
                    fingerprint(key),
                    self.trust.path().display()
                );
                eprintln!("WHCode SSH：{detail}");
                if let Ok(mut slot) = self.mismatch.lock() {
                    *slot = Some(detail);
                }
                Ok(false)
            }
            Err(err) => {
                // 信任文件不可读：fail closed（不当成首次使用重新记录）。
                eprintln!("WHCode SSH：主机密钥校验失败（{err}）：拒绝连接");
                Ok(false)
            }
        }
    }
}

/// 执行阶段：连接、认证、发命令、收集输出，并把结果映射为 [`ExecReport`]。
///
/// 本函数不返回 `Err`：所有失败都以报告形式表达，以便调用方无条件登记结果（规则 3、8）。
async fn execute(target: &SshTarget) -> ExecReport {
    let started = Instant::now();
    let budget = Duration::from_millis(target.timeout_ms);
    // 单一时限覆盖整个调用：各阶段只消费剩余预算，不得每阶段各用一份完整预算
    // （否则实际总耗时可达到 timeout_ms 的数倍，与字段语义不符）。
    let deadline = started + budget;

    let key = match load_secret_key(&target.private_key_path, None) {
        Ok(key) => key,
        Err(err) => {
            return failure_report(
                started,
                format!("私钥加载失败：{err}（命令未发出；口令保护的私钥在本原型不支持）"),
            );
        }
    };

    let config = Arc::new(client::Config {
        inactivity_timeout: Some(budget),
        ..Default::default()
    });

    // 主机密钥 TOFU 校验：不匹配/不可读的结构化细节经槽位带回。
    let mismatch: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let handler = ProtoClient {
        trust: KnownHosts::at(target.known_hosts_path.clone()),
        host: target.host.clone(),
        port: target.port,
        mismatch: Arc::clone(&mismatch),
    };

    // 阶段一：连接。此阶段失败意味着命令从未发出（规则 3）。
    let mut session = match tokio::time::timeout(
        budget,
        client::connect(config, (target.host.as_str(), target.port), handler),
    )
    .await
    {
        Ok(Ok(session)) => session,
        Ok(Err(err)) => {
            let note = match mismatch.lock().ok().and_then(|mut slot| slot.take()) {
                Some(detail) => format!("主机密钥校验拒绝：{detail}（命令未发出）"),
                None => format!("连接阶段失败：{err}（命令未发出）"),
            };
            return failure_report(started, note);
        }
        Err(_) => {
            return failure_report(
                started,
                format!("连接阶段超时（{} ms）：命令未发出", target.timeout_ms),
            );
        }
    };

    // 阶段二：公钥认证。失败时命令仍未发出（规则 3）。
    let rsa_hash = match tokio::time::timeout(
        remaining_budget(deadline),
        session.best_supported_rsa_hash(),
    )
    .await
    {
        Ok(Ok(hash)) => hash.flatten(),
        Ok(Err(err)) => {
            return failure_report(started, format!("认证阶段失败：{err}（命令未发出）"));
        }
        Err(_) => return failure_report(started, "认证阶段超时：命令未发出".to_string()),
    };
    let auth = tokio::time::timeout(
        remaining_budget(deadline),
        session.authenticate_publickey(
            target.user.clone(),
            PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash),
        ),
    )
    .await;
    match auth {
        Ok(Ok(result)) if result.success() => {}
        Ok(Ok(_)) => {
            return failure_report(
                started,
                "认证阶段失败：服务端拒绝公钥（命令未发出）".to_string(),
            );
        }
        Ok(Err(err)) => {
            return failure_report(started, format!("认证阶段失败：{err}（命令未发出）"));
        }
        Err(_) => return failure_report(started, "认证阶段超时：命令未发出".to_string()),
    }

    // 阶段三：打开会话通道。失败时命令仍未发出。
    let mut channel = match tokio::time::timeout(
        remaining_budget(deadline),
        session.channel_open_session(),
    )
    .await
    {
        Ok(Ok(channel)) => channel,
        Ok(Err(err)) => {
            return failure_report(started, format!("通道阶段失败：{err}（命令未发出）"));
        }
        Err(_) => return failure_report(started, "通道阶段超时：命令未发出".to_string()),
    };

    // 阶段四：发出执行请求。此调用之后，命令**可能**已在远端启动——此后任何异常都不得判为
    // `completed`/`failed`（规则 4、6）。
    let send = tokio::time::timeout(
        remaining_budget(deadline),
        channel.exec(true, target.command.clone()),
    )
    .await;
    match send {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            return indeterminate_report(
                started,
                TailBuffer::default(),
                TailBuffer::default(),
                format!("执行请求发送失败（{err}）：{INDETERMINATE_ADVICE}"),
            );
        }
        Err(_) => {
            return indeterminate_report(
                started,
                TailBuffer::default(),
                TailBuffer::default(),
                format!("执行请求发送超时：{INDETERMINATE_ADVICE}"),
            );
        }
    }

    // 阶段五：收集输出与退出状态。预算用尽即停止等待——远端是否仍在运行无法确认。
    let mut stdout = TailBuffer::default();
    let mut stderr = TailBuffer::default();
    let mut exit_code: Option<u32> = None;
    let mut exit_signal: Option<String> = None;
    let mut timed_out = false;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            break;
        }
        match tokio::time::timeout(remaining, channel.wait()).await {
            Ok(Some(ChannelMsg::Data { data })) => stdout.push(&data),
            Ok(Some(ChannelMsg::ExtendedData { data, ext: 1 })) => stderr.push(&data),
            Ok(Some(ChannelMsg::ExitStatus { exit_status })) => exit_code = Some(exit_status),
            Ok(Some(ChannelMsg::ExitSignal { signal_name, .. })) => {
                exit_signal = Some(format!("{signal_name:?}"));
            }
            // 其他消息（含非标准错误流的扩展数据）不影响判定。
            Ok(Some(_)) => {}
            // 通道关闭：正常收尾，也可能是异常收尾——由是否有退出状态决定。
            Ok(None) => break,
            Err(_) => {
                timed_out = true;
                break;
            }
        }
    }

    // 收尾（失败不影响判定结果，忽略错误）。
    let _ = channel.close().await;
    let _ = session.disconnect(Disconnect::ByApplication, "", "").await;

    let truncated = stdout.truncated || stderr.truncated;
    let truncation_note = if truncated {
        format!(
            "；输出超过上限 {} 字节，仅保留尾部（原始输出未完整保留）",
            MAX_TAIL_BYTES
        )
    } else {
        String::new()
    };
    let duration_ms = elapsed_ms(started);

    // 规则 5：收到明确退出状态即为确定结果。
    if let Some(code) = exit_code {
        return ExecReport {
            status: map_exit_status(code),
            exit_code: Some(exit_code_to_i32(code)),
            stdout_tail: stdout.into_string(),
            stderr_tail: stderr.into_string(),
            duration_ms,
            truncated,
            note: format!("已收到退出状态 {code}：命令结束（退出状态为确定结果）{truncation_note}"),
        };
    }

    // 远端报告进程被信号终止：这是关于终止的明确报告，按 `failed` 记录（§3.1）。
    if let Some(signal) = exit_signal {
        return ExecReport {
            status: RuntimeExecutionStatus::Failed,
            exit_code: None,
            stdout_tail: stdout.into_string(),
            stderr_tail: stderr.into_string(),
            duration_ms,
            truncated,
            note: format!("远端报告进程被信号 {signal} 终止：判为失败{truncation_note}"),
        };
    }

    // 规则 4、6：没有退出状态时不得声称确定结果。
    let reason = if timed_out {
        "等待结果超时且未收到退出状态"
    } else {
        "通道关闭但未收到退出状态"
    };
    ExecReport {
        status: RuntimeExecutionStatus::Unknown,
        exit_code: None,
        stdout_tail: stdout.into_string(),
        stderr_tail: stderr.into_string(),
        duration_ms,
        truncated,
        note: format!("{reason}：{INDETERMINATE_ADVICE}{truncation_note}"),
    }
}

/// 明确退出状态到执行状态的映射（规则 5）。
fn map_exit_status(exit_code: u32) -> RuntimeExecutionStatus {
    if exit_code == 0 {
        RuntimeExecutionStatus::Completed
    } else {
        RuntimeExecutionStatus::Failed
    }
}

/// 退出码转 `i32`（协议字段为 `u32`；超出 `i32` 范围在实际协议中不会出现，取饱和值）。
fn exit_code_to_i32(exit_code: u32) -> i32 {
    i32::try_from(exit_code).unwrap_or(i32::MAX)
}

/// 耗时毫秒（饱和转换，避免极端情况下的截断）。
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// 距总时限还剩多少预算。返回零表示已耗尽：`tokio::time::timeout` 会立即到期，
/// 从而把该阶段判为超时，而不是再给一份完整预算。
fn remaining_budget(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// 明确失败（连接/认证/通道阶段）的报告：命令未发出，故可安全重试。
fn failure_report(started: Instant, note: String) -> ExecReport {
    ExecReport {
        status: RuntimeExecutionStatus::Failed,
        exit_code: None,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        duration_ms: elapsed_ms(started),
        truncated: false,
        note,
    }
}

/// 结果不确定（命令已发出）的报告：状态必须为 `unknown`（规则 4、6）。
fn indeterminate_report(
    started: Instant,
    stdout: TailBuffer,
    stderr: TailBuffer,
    note: String,
) -> ExecReport {
    ExecReport {
        status: RuntimeExecutionStatus::Unknown,
        exit_code: None,
        stdout_tail: stdout.into_string(),
        stderr_tail: stderr.into_string(),
        duration_ms: elapsed_ms(started),
        truncated: false,
        note,
    }
}

/// 尾部有界缓冲：只保留最后 [`MAX_TAIL_BYTES`] 字节，并记录是否发生截断。
#[derive(Debug, Default)]
struct TailBuffer {
    bytes: Vec<u8>,
    truncated: bool,
}

impl TailBuffer {
    /// 追加一段输出，超出上限时丢弃最早的部分并置截断标记。
    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= MAX_TAIL_BYTES {
            // 单块本身已达到上限：只保留其尾部；仅当确有字节被丢弃时才算截断。
            let (_, tail) = chunk.split_at(chunk.len() - MAX_TAIL_BYTES);
            self.truncated |= !self.bytes.is_empty() || chunk.len() > MAX_TAIL_BYTES;
            self.bytes.clear();
            self.bytes.extend_from_slice(tail);
            return;
        }
        let total = self.bytes.len() + chunk.len();
        if total > MAX_TAIL_BYTES {
            self.bytes.drain(..total - MAX_TAIL_BYTES);
            self.truncated = true;
        }
        self.bytes.extend_from_slice(chunk);
    }

    /// 有损解码为字符串（远端输出不保证是合法 UTF-8）。
    fn into_string(self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::future::Future;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use russh::keys::ssh_key::LineEnding;
    use russh::keys::{Algorithm, PrivateKey};
    use russh::server::{Auth, Msg, Server as _, Session as ServerSession};
    use russh::{Channel, ChannelId};
    use tokio::net::TcpListener;
    use whcode_domain::ErrorCode;

    /// 临时私钥文件名序号（并发测试之间不得冲突）。
    static KEY_SEQ: AtomicU64 = AtomicU64::new(0);

    /// 测试用临时私钥：析构时删除文件。
    struct TestKey {
        path: PathBuf,
    }

    impl TestKey {
        fn generate(tag: &str) -> Self {
            let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
                .expect("生成测试私钥应成功");
            let encoded = key
                .to_openssh(LineEnding::LF)
                .expect("编码 OpenSSH 私钥应成功");
            let seq = KEY_SEQ.fetch_add(1, Ordering::Relaxed);
            let mut path = std::env::temp_dir();
            path.push(format!("whcode-ssh-{tag}-{}-{seq}.key", std::process::id()));
            std::fs::write(&path, encoded.as_bytes()).expect("写入测试私钥应成功");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// 测试用临时信任文件路径（每测试唯一；文件本体按需创建）。
    fn trust_file() -> PathBuf {
        static TRUST_SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = TRUST_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "whcode-ssh-proto-trust-{}-{seq}.known_hosts",
            std::process::id()
        ))
    }

    /// 内存测试服务端行为。
    #[derive(Clone)]
    enum TestBehavior {
        /// 接受 exec 后回显固定输出并返回退出状态。
        Echo {
            stdout: Vec<u8>,
            stderr: Vec<u8>,
            exit_code: u32,
        },
        /// 接受 exec 但**不**给出任何结果：用于验证“命令已发出、结果不确定”。
        Silent,
        /// 接受 exec 后立即关闭通道但**不**发送退出状态：用于验证“异常断连”。
        CloseWithoutStatus,
    }

    struct TestSshServer {
        behavior: TestBehavior,
    }

    struct TestSshSession {
        behavior: TestBehavior,
    }

    impl russh::server::Server for TestSshServer {
        type Handler = TestSshSession;

        fn new_client(&mut self, _peer: Option<SocketAddr>) -> TestSshSession {
            TestSshSession {
                behavior: self.behavior.clone(),
            }
        }
    }

    impl russh::server::Handler for TestSshSession {
        type Error = russh::Error;

        async fn auth_publickey(
            &mut self,
            _user: &str,
            _key: russh::server::VerifiedPublicKey<'_>,
        ) -> Result<Auth, Self::Error> {
            Ok(Auth::Accept)
        }

        async fn channel_open_session(
            &mut self,
            _channel: Channel<Msg>,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut ServerSession,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn exec_request(
            &mut self,
            channel: ChannelId,
            _command: &[u8],
            session: &mut ServerSession,
        ) -> Result<(), Self::Error> {
            if let TestBehavior::Echo {
                stdout,
                stderr,
                exit_code,
            } = &self.behavior
            {
                session.channel_success(channel)?;
                if !stdout.is_empty() {
                    session.data(channel, stdout.clone())?;
                }
                if !stderr.is_empty() {
                    session.extended_data(channel, 1, stderr.clone())?;
                }
                session.exit_status_request(channel, *exit_code)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            if matches!(&self.behavior, TestBehavior::CloseWithoutStatus) {
                // 通道正常开启并确认 exec，然后直接结束：客户端会看到“关闭但无退出状态”。
                session.channel_success(channel)?;
                session.eof(channel)?;
                session.close(channel)?;
            }
            Ok(())
        }
    }

    /// 在进程内起一个最小 SSH 服务端，运行一段客户端操作；返回操作结果与服务端主机公钥
    /// （known_hosts 测试用指纹比对）。客户端私钥路径随回调注入。
    async fn run_against_test_server_with<F, T>(
        behavior: TestBehavior,
        client_ops: impl FnOnce(SocketAddr, russh::keys::PublicKey, PathBuf) -> F,
    ) -> (T, russh::keys::PublicKey)
    where
        F: Future<Output = T>,
    {
        let key = TestKey::generate("e2e");
        let host_key =
            PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("生成测试主机密钥");
        let host_public = host_key.public_key().clone();
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("绑定测试端口");
        let addr = listener.local_addr().expect("读取测试端口");

        let config = Arc::new(russh::server::Config {
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            keys: vec![host_key],
            inactivity_timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        });
        let mut server = TestSshServer { behavior };
        let running = server.run_on_socket(config, &listener);
        tokio::pin!(running);

        let outcome = tokio::select! {
            result = client_ops(addr, host_public.clone(), key.path().to_path_buf()) => result,
            _ = &mut running => panic!("测试服务端不应提前退出"),
        };

        running.handle().shutdown("测试结束".to_string());
        let drained = tokio::time::timeout(Duration::from_secs(5), &mut running).await;
        assert!(drained.is_ok(), "测试服务端应在收到关闭请求后退出");
        (outcome, host_public)
    }

    /// 单次 [`run_command`] 的便捷封装（多数既有测试的形态）。
    async fn run_against_test_server(
        behavior: TestBehavior,
        timeout_ms: u64,
    ) -> (Result<ExecReport, WhError>, Arc<CollectingExecRegistry>) {
        let trust_file = std::env::temp_dir().join(format!(
            "whcode-ssh-proto-trust-{}.known_hosts",
            std::process::id()
        ));
        let (outcome, _host) =
            run_against_test_server_with(behavior, |addr, _host_public, key_path| async move {
                let registry = Arc::new(CollectingExecRegistry::new());
                let target = SshTarget {
                    host: "127.0.0.1".to_string(),
                    port: addr.port(),
                    user: "tester".to_string(),
                    private_key_path: key_path,
                    command: "whcode proto-echo".to_string(),
                    allow_shell: false,
                    timeout_ms,
                    known_hosts_path: trust_file,
                };
                let result = run_command(&target, registry.as_ref()).await;
                (result, registry)
            })
            .await;
        outcome
    }

    #[test]
    fn exit_status_maps_to_completed_or_failed() {
        assert_eq!(map_exit_status(0), RuntimeExecutionStatus::Completed);
        assert_eq!(map_exit_status(1), RuntimeExecutionStatus::Failed);
        assert_eq!(map_exit_status(255), RuntimeExecutionStatus::Failed);
    }

    #[test]
    fn tail_buffer_keeps_tail_and_flags_truncation() {
        let mut buffer = TailBuffer::default();
        buffer.push(&vec![b'a'; MAX_TAIL_BYTES]);
        assert!(!buffer.truncated, "恰好达到上限不应视为截断");
        buffer.push(b"tail");
        assert!(buffer.truncated, "超出上限必须标记截断");
        let text = buffer.into_string();
        assert_eq!(text.len(), MAX_TAIL_BYTES);
        assert!(text.ends_with("tail"));

        let mut single = TailBuffer::default();
        single.push(&vec![b'z'; MAX_TAIL_BYTES + 128]);
        assert!(single.truncated);
        assert_eq!(single.into_string().len(), MAX_TAIL_BYTES);
    }

    #[tokio::test]
    async fn validation_rejects_invalid_targets_without_any_registry_call() {
        let key = TestKey::generate("validation");
        let missing = std::env::temp_dir().join("whcode-ssh-nonexistent-key");
        let base = SshTarget {
            host: "127.0.0.1".to_string(),
            port: 22,
            user: "tester".to_string(),
            private_key_path: key.path().to_path_buf(),
            command: "true".to_string(),
            // 本用例测“目标校验”，不是命令策略：显式打开 Shell 授权以隔离策略判定。
            allow_shell: true,
            timeout_ms: 1000,
            known_hosts_path: trust_file(),
        };
        let cases = [
            SshTarget {
                host: String::new(),
                ..base.clone()
            },
            SshTarget {
                host: "   ".to_string(),
                ..base.clone()
            },
            SshTarget {
                user: String::new(),
                ..base.clone()
            },
            SshTarget {
                command: String::new(),
                ..base.clone()
            },
            SshTarget {
                timeout_ms: 0,
                ..base.clone()
            },
            SshTarget {
                private_key_path: missing.clone(),
                ..base.clone()
            },
        ];

        for target in cases {
            let registry = CollectingExecRegistry::new();
            let error = run_command(&target, &registry)
                .await
                .expect_err("非法目标必须返回错误");
            assert_eq!(error.code, ErrorCode::InvalidState, "目标：{target:?}");
            assert!(
                registry.is_empty(),
                "校验失败不得发生任何登记副作用（既无意图也无结果）：{target:?}"
            );
        }
    }

    /// Check 97 客户端侧：默认入口只放行远端 `whcode`；任意系统 Shell 需显式授权。
    #[test]
    fn client_command_policy_allows_whcode_and_denies_shell_by_default() {
        let key = TestKey::generate("policy");
        let base = SshTarget {
            host: "127.0.0.1".to_string(),
            port: 22,
            user: "tester".to_string(),
            private_key_path: key.path().to_path_buf(),
            command: "whcode sessions".to_string(),
            allow_shell: false,
            timeout_ms: 1000,
            known_hosts_path: trust_file(),
        };
        for allowed in [
            "whcode sessions",
            "whcode.exe tasks",
            "/usr/local/bin/whcode session show S-1",
            "C:\\whcode\\whcode.exe tasks",
        ] {
            let target = SshTarget {
                command: allowed.to_string(),
                ..base.clone()
            };
            assert!(
                validate_command_policy(&target).is_ok(),
                "{allowed} 必须放行（默认入口允许远端 whcode）"
            );
        }
        for denied in [
            "sh",
            "bash -lc whoami",
            "powershell -Command Get-Process",
            "cmd /c dir",
            "./collect.sh",
        ] {
            let target = SshTarget {
                command: denied.to_string(),
                ..base.clone()
            };
            let error = validate_command_policy(&target).expect_err("默认必须拒绝任意 Shell");
            assert_eq!(error.code, ErrorCode::PermissionDenied, "命令：{denied}");
            assert!(!error.retryable, "权限拒绝不得标记为可重试");
        }
        let granted = SshTarget {
            command: "bash -lc whoami".to_string(),
            allow_shell: true,
            ..base.clone()
        };
        assert!(
            validate_command_policy(&granted).is_ok(),
            "显式 allow_shell 授权必须放行"
        );
    }

    /// 策略拒绝发生在意图登记之前：不得产生任何登记副作用。
    #[tokio::test]
    async fn shell_denied_by_default_leaves_registry_untouched() {
        let key = TestKey::generate("shell-denied");
        let target = SshTarget {
            host: "127.0.0.1".to_string(),
            port: 1,
            user: "tester".to_string(),
            private_key_path: key.path().to_path_buf(),
            command: "bash -lc whoami".to_string(),
            allow_shell: false,
            timeout_ms: 2000,
            known_hosts_path: trust_file(),
        };
        let registry = CollectingExecRegistry::new();
        let error = run_command(&target, &registry)
            .await
            .expect_err("默认必须拒绝任意 Shell");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(registry.is_empty(), "策略拒绝不得登记意图或结果");
    }

    /// 显式 Shell 授权必须进入意图登记（审计可追溯）。
    #[test]
    fn intent_records_explicit_shell_grant() {
        let key = TestKey::generate("grant");
        let target = SshTarget {
            host: "127.0.0.1".to_string(),
            port: 22,
            user: "tester".to_string(),
            private_key_path: key.path().to_path_buf(),
            command: "bash -lc whoami".to_string(),
            allow_shell: true,
            timeout_ms: 1000,
            known_hosts_path: trust_file(),
        };
        let registry = CollectingExecRegistry::new();
        registry.record_intent(&target).expect("意图登记");
        assert!(
            matches!(
                registry.calls()[0],
                RegistryCall::Intent {
                    allow_shell: true,
                    ..
                }
            ),
            "显式授权必须被登记：{:?}",
            registry.calls()
        );
    }

    #[tokio::test]
    async fn connection_failure_is_failed_and_records_intent_before_result() {
        let key = TestKey::generate("connfail");
        let registry = CollectingExecRegistry::new();
        let target = SshTarget {
            host: "127.0.0.1".to_string(),
            // 端口 1 上不会有监听者，连接必然被拒绝。
            port: 1,
            user: "tester".to_string(),
            private_key_path: key.path().to_path_buf(),
            command: "true".to_string(),
            allow_shell: true,
            timeout_ms: 2000,
            known_hosts_path: trust_file(),
        };

        let report = run_command(&target, &registry)
            .await
            .expect("连接失败必须返回报告而不是错误");
        assert_eq!(report.status, RuntimeExecutionStatus::Failed);
        assert_eq!(report.exit_code, None);
        assert!(report.note.contains("连接阶段"), "note：{}", report.note);

        let calls = registry.calls();
        assert_eq!(calls.len(), 2, "必须既有意图也有结果：{calls:?}");
        assert!(matches!(calls[0], RegistryCall::Intent { .. }));
        assert!(matches!(
            calls[1],
            RegistryCall::Result {
                status: RuntimeExecutionStatus::Failed,
                exit_code: None,
                ..
            }
        ));
    }

    struct FailingIntentRegistry;

    impl ExecRegistry for FailingIntentRegistry {
        fn record_intent(&self, _target: &SshTarget) -> Result<(), WhError> {
            Err(WhError::internal("意图登记失败（测试）"))
        }

        fn record_result(&self, _report: &ExecReport) -> Result<(), WhError> {
            Ok(())
        }
    }

    struct FailingResultRegistry;

    impl ExecRegistry for FailingResultRegistry {
        fn record_intent(&self, _target: &SshTarget) -> Result<(), WhError> {
            Ok(())
        }

        fn record_result(&self, _report: &ExecReport) -> Result<(), WhError> {
            Err(WhError::internal("结果登记失败（测试）"))
        }
    }

    #[tokio::test]
    async fn registry_failures_are_propagated() {
        let key = TestKey::generate("registry");
        let target = SshTarget {
            host: "127.0.0.1".to_string(),
            port: 1,
            user: "tester".to_string(),
            private_key_path: key.path().to_path_buf(),
            command: "true".to_string(),
            allow_shell: true,
            timeout_ms: 2000,
            known_hosts_path: trust_file(),
        };

        let error = run_command(&target, &FailingIntentRegistry)
            .await
            .expect_err("意图登记失败必须返回错误");
        assert_eq!(error.code, ErrorCode::Internal);

        let error = run_command(&target, &FailingResultRegistry)
            .await
            .expect_err("结果登记失败不得被视为成功");
        assert_eq!(error.code, ErrorCode::Internal);
    }

    #[tokio::test]
    async fn end_to_end_exec_reports_completed_with_output() {
        let (outcome, registry) = run_against_test_server(
            TestBehavior::Echo {
                stdout: b"whcode-proto-stdout\n".to_vec(),
                stderr: b"whcode-proto-stderr\n".to_vec(),
                exit_code: 0,
            },
            10_000,
        )
        .await;

        let report = outcome.expect("远端正常结束时必须返回报告");
        assert_eq!(report.status, RuntimeExecutionStatus::Completed);
        assert_eq!(report.exit_code, Some(0));
        assert!(
            report.stdout_tail.contains("whcode-proto-stdout"),
            "stdout 尾部：{}",
            report.stdout_tail
        );
        assert!(
            report.stderr_tail.contains("whcode-proto-stderr"),
            "stderr 尾部：{}",
            report.stderr_tail
        );
        assert!(!report.truncated);

        let calls = registry.calls();
        assert_eq!(calls.len(), 2, "必须既有意图也有结果：{calls:?}");
        assert!(matches!(calls[0], RegistryCall::Intent { .. }));
        assert!(matches!(
            calls[1],
            RegistryCall::Result {
                status: RuntimeExecutionStatus::Completed,
                exit_code: Some(0),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn end_to_end_nonzero_exit_is_failed() {
        let (outcome, _registry) = run_against_test_server(
            TestBehavior::Echo {
                stdout: Vec::new(),
                stderr: b"boom\n".to_vec(),
                exit_code: 7,
            },
            10_000,
        )
        .await;

        let report = outcome.expect("非零退出同样必须返回报告");
        assert_eq!(report.status, RuntimeExecutionStatus::Failed);
        assert_eq!(report.exit_code, Some(7));
        assert!(report.stderr_tail.contains("boom"));
    }

    #[tokio::test]
    async fn end_to_end_large_output_is_truncated_to_tail() {
        let marker = b"WHCODE-TAIL-END";
        let mut data = vec![b'a'; MAX_TAIL_BYTES + 4096];
        let marker_start = data.len() - marker.len();
        let (_, tail) = data.split_at_mut(marker_start);
        tail.copy_from_slice(marker);
        let expected_tail = data[data.len() - MAX_TAIL_BYTES..].to_vec();

        let (outcome, registry) = run_against_test_server(
            TestBehavior::Echo {
                stdout: data.clone(),
                stderr: Vec::new(),
                exit_code: 0,
            },
            20_000,
        )
        .await;

        let report = outcome.expect("大输出同样必须返回报告");
        assert_eq!(report.status, RuntimeExecutionStatus::Completed);
        assert_eq!(report.exit_code, Some(0));
        assert!(report.truncated, "超过上限必须标记截断");
        assert_eq!(report.stdout_tail.len(), MAX_TAIL_BYTES);
        assert_eq!(report.stdout_tail.as_bytes(), expected_tail.as_slice());
        assert!(report.stdout_tail.ends_with("WHCODE-TAIL-END"));
        assert!(
            report.note.contains("截断") || report.note.contains("仅保留尾部"),
            "note 必须写明截断事实：{}",
            report.note
        );
        assert_eq!(registry.len(), 2);
    }

    #[tokio::test]
    async fn command_without_exit_status_is_unknown_not_completed_or_failed() {
        let (outcome, registry) = run_against_test_server(TestBehavior::Silent, 300).await;

        let report = outcome.expect("结果不确定同样必须返回报告");
        assert_eq!(report.status, RuntimeExecutionStatus::Unknown);
        assert_ne!(report.status, RuntimeExecutionStatus::Completed);
        assert_ne!(report.status, RuntimeExecutionStatus::Failed);
        assert_eq!(report.exit_code, None);
        assert!(report.note.contains("必须核对"), "note：{}", report.note);
        assert!(
            report.note.contains("不得盲目重放"),
            "note：{}",
            report.note
        );
        assert_eq!(registry.calls().len(), 2, "不确定状态同样必须登记结果");
    }
    /// known_hosts 三分支（经真实客户端—服务端链路）：首次连接记录指纹（TOFU）、
    /// 二次连接命中、指纹不匹配结构化拒绝（ADR-005 阻断项结清的端到端证据）。
    #[tokio::test]
    async fn known_hosts_tofu_first_seen_then_trusted_then_mismatch() {
        let trust_path = trust_file();

        let ((first, second, third, after_first, after_second, after_third), host_public) =
            run_against_test_server_with(
                TestBehavior::Echo {
                    stdout: b"ok".to_vec(),
                    stderr: Vec::new(),
                    exit_code: 0,
                },
                |addr, _host_public, key_path| {
                    let trust_path = trust_path.clone();
                    async move {
                        let make_target = |known_hosts_path: PathBuf| SshTarget {
                            host: "127.0.0.1".to_string(),
                            port: addr.port(),
                            user: "tester".to_string(),
                            private_key_path: key_path.clone(),
                            command: "whcode proto-echo".to_string(),
                            allow_shell: false,
                            timeout_ms: 10_000,
                            known_hosts_path,
                        };

                        // 第一次：FirstSeen → 记录指纹并放行。
                        let first = run_command(
                            &make_target(trust_path.clone()),
                            &CollectingExecRegistry::new(),
                        )
                        .await
                        .expect("首次连接必须成功（TOFU 记录）");
                        let after_first =
                            std::fs::read_to_string(&trust_path).expect("读取信任文件");

                        // 第二次：Trusted → 放行。
                        let second = run_command(
                            &make_target(trust_path.clone()),
                            &CollectingExecRegistry::new(),
                        )
                        .await
                        .expect("第二次连接必须命中信任记录");
                        let after_second =
                            std::fs::read_to_string(&trust_path).expect("读取信任文件");

                        // 第三次：信任文件被人工写成攻击者指纹 → 结构化拒绝。
                        let attacker = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
                            .expect("生成攻击者密钥");
                        let attacker_entry = format!(
                            "[127.0.0.1]:{} {}
",
                            addr.port(),
                            crate::hostkey::fingerprint(attacker.public_key())
                        );
                        std::fs::write(&trust_path, attacker_entry.as_bytes())
                            .expect("写入攻击者指纹");
                        let third = run_command(
                            &make_target(trust_path.clone()),
                            &CollectingExecRegistry::new(),
                        )
                        .await
                        .expect("不匹配是报告不是 Err（协议规则 3）");
                        let after_third =
                            std::fs::read_to_string(&trust_path).expect("读取信任文件");
                        (first, second, third, after_first, after_second, after_third)
                    }
                },
            )
            .await;

        let server_fingerprint = fingerprint(&host_public);
        // 首次：成功且指纹已记录（TOFU）。
        assert_eq!(
            first.status,
            RuntimeExecutionStatus::Completed,
            "{}",
            first.note
        );
        assert!(
            after_first.contains(&server_fingerprint),
            "首次连接必须把服务端指纹写入信任文件：{after_first}"
        );
        // 第二次：成功且不重复记录。
        assert_eq!(second.status, RuntimeExecutionStatus::Completed);
        assert_eq!(
            after_first, after_second,
            "命中信任记录的连接不得改写信任文件"
        );
        assert_eq!(after_second.lines().count(), 1);
        // 第三次：拒绝 + 结构化 note + 信任文件保持人工记录（不被自动改写）。
        assert_eq!(third.status, RuntimeExecutionStatus::Failed);
        assert_eq!(third.exit_code, None);
        assert!(
            third.note.contains("不匹配") && third.note.contains("命令未发出"),
            "note 必须写明拒绝与未发出：{}",
            third.note
        );
        assert_eq!(after_third.lines().count(), 1, "不匹配不得追加记录");
        assert!(
            !after_third.contains(&server_fingerprint),
            "不匹配时信任文件保持人工写入的攻击者指纹，服务端指纹未被恢复/改写：{after_third}"
        );
    }

    /// Check 25／96／99 的**客户端侧**回环验收（FR-611、ADR-011、V-004）：一次测试覆盖
    /// 公钥认证、受限命令执行、输出上限截断、超时与断连四类行为。
    ///
    /// 边界（不得扩大解释）：服务端是进程内测试桩，本测试通过只证明客户端路径在该平台上
    /// **可运行**，不构成 Windows／FreeBSD／NetBSD／OpenBSD／DragonFly 任一平台的节点
    /// 连通性证据；V-004 仍属 M7，BSD 结论按 ADR-013 延后、不进入验收。
    #[tokio::test]
    async fn bsd_client_acceptance_loopback() {
        // —— 一、受限命令执行：默认入口只放行远端 whcode，任意系统 Shell 必须拒绝 ——
        let policy_key = TestKey::generate("bsd-policy");
        let base = SshTarget {
            host: "127.0.0.1".to_string(),
            port: 22,
            user: "tester".to_string(),
            private_key_path: policy_key.path().to_path_buf(),
            command: "whcode session list".to_string(),
            allow_shell: false,
            timeout_ms: 10_000,
            known_hosts_path: trust_file(),
        };
        assert!(
            validate_command_policy(&base).is_ok(),
            "受限入口必须放行远端 whcode 命令"
        );
        let denied = SshTarget {
            command: "sh -lc id".to_string(),
            ..base.clone()
        };
        let error = validate_command_policy(&denied).expect_err("任意系统 Shell 默认必须拒绝");
        assert_eq!(
            error.code,
            ErrorCode::PermissionDenied,
            "受限入口拒绝必须是权限拒绝而非其他错误"
        );

        // —— 二、公钥认证 + 受限命令执行 + 输出上限截断（同一条真实客户端链路）——
        // 测试服务端只实现公钥认证：执行成功即证明公钥认证在该平台通过。
        let marker = b"WHCODE-BSD-TAIL-END";
        let mut payload = vec![b'a'; MAX_TAIL_BYTES + 2048];
        let marker_start = payload.len() - marker.len();
        payload[marker_start..].copy_from_slice(marker);
        let expected_tail = payload[payload.len() - MAX_TAIL_BYTES..].to_vec();
        let exec_trust = trust_file();

        let ((report, calls, recorded_trust), host_public) = run_against_test_server_with(
            TestBehavior::Echo {
                stdout: payload,
                stderr: Vec::new(),
                exit_code: 0,
            },
            |addr, _host_public, key_path| {
                let exec_trust = exec_trust.clone();
                async move {
                    let registry = CollectingExecRegistry::new();
                    let target = SshTarget {
                        host: "127.0.0.1".to_string(),
                        port: addr.port(),
                        user: "tester".to_string(),
                        private_key_path: key_path,
                        command: "whcode session list".to_string(),
                        allow_shell: false,
                        timeout_ms: 30_000,
                        known_hosts_path: exec_trust.clone(),
                    };
                    let report = run_command(&target, &registry)
                        .await
                        .expect("回环执行必须返回报告而不是错误");
                    let recorded = std::fs::read_to_string(&exec_trust).unwrap_or_default();
                    (report, registry.calls(), recorded)
                }
            },
        )
        .await;

        assert_eq!(
            report.status,
            RuntimeExecutionStatus::Completed,
            "公钥认证 + 受限命令执行必须完成：{}",
            report.note
        );
        assert_eq!(report.exit_code, Some(0));
        assert!(
            report.truncated,
            "输出超过上限必须标记截断：{}",
            report.note
        );
        assert_eq!(
            report.stdout_tail.len(),
            MAX_TAIL_BYTES,
            "只保留尾部上限字节"
        );
        assert_eq!(
            report.stdout_tail.as_bytes(),
            expected_tail.as_slice(),
            "保留的必须是尾部而非头部"
        );
        assert!(report.stdout_tail.ends_with("WHCODE-BSD-TAIL-END"));
        assert!(
            report.note.contains("截断") || report.note.contains("仅保留尾部"),
            "note 必须写明截断事实：{}",
            report.note
        );
        assert!(
            recorded_trust.contains(&fingerprint(&host_public)),
            "主机密钥必须先经 TOFU 校验并记录，公钥认证才成立：{recorded_trust}"
        );
        assert_eq!(calls.len(), 2, "必须既有意图也有结果：{calls:?}");
        assert!(matches!(calls[0], RegistryCall::Intent { .. }));
        assert!(matches!(
            calls[1],
            RegistryCall::Result {
                status: RuntimeExecutionStatus::Completed,
                exit_code: Some(0),
                ..
            }
        ));

        // —— 三、超时：命令已发出，服务端不给任何结果（`unknown`，不得谎报）——
        // 预算取 3 s：需先完成连接与公钥认证，过小的预算会在平台上把“慢”误判为超时。
        let timeout_trust = trust_file();
        let ((timeout_report, timeout_calls), _) =
            run_against_test_server_with(TestBehavior::Silent, |addr, _host_public, key_path| {
                let timeout_trust = timeout_trust.clone();
                async move {
                    let registry = CollectingExecRegistry::new();
                    let target = SshTarget {
                        host: "127.0.0.1".to_string(),
                        port: addr.port(),
                        user: "tester".to_string(),
                        private_key_path: key_path,
                        command: "whcode session list".to_string(),
                        allow_shell: false,
                        timeout_ms: 3_000,
                        known_hosts_path: timeout_trust,
                    };
                    let report = run_command(&target, &registry)
                        .await
                        .expect("超时同样必须返回报告");
                    (report, registry.calls())
                }
            })
            .await;

        assert_eq!(
            timeout_report.status,
            RuntimeExecutionStatus::Unknown,
            "超时不得判为完成或失败：{}",
            timeout_report.note
        );
        assert_eq!(timeout_report.exit_code, None);
        assert!(
            timeout_report.note.contains("必须核对")
                && timeout_report.note.contains("不得盲目重放"),
            "超时说明必须要求核对、禁止盲目重放：{}",
            timeout_report.note
        );
        assert!(
            timeout_report.duration_ms >= 2_500,
            "超时必须实际消费预算而不是提前放弃：{} ms",
            timeout_report.duration_ms
        );
        assert_eq!(timeout_calls.len(), 2, "不确定状态同样必须登记结果");

        // —— 四、断连：服务端关闭通道但不发送退出状态（同样不得谎报）——
        let disconnect_trust = trust_file();
        let (disconnect_report, _) = run_against_test_server_with(
            TestBehavior::CloseWithoutStatus,
            |addr, _host_public, key_path| {
                let disconnect_trust = disconnect_trust.clone();
                async move {
                    let target = SshTarget {
                        host: "127.0.0.1".to_string(),
                        port: addr.port(),
                        user: "tester".to_string(),
                        private_key_path: key_path,
                        command: "whcode session list".to_string(),
                        allow_shell: false,
                        timeout_ms: 30_000,
                        known_hosts_path: disconnect_trust,
                    };
                    run_command(&target, &CollectingExecRegistry::new())
                        .await
                        .expect("断连同样必须返回报告")
                }
            },
        )
        .await;

        assert_eq!(
            disconnect_report.status,
            RuntimeExecutionStatus::Unknown,
            "无退出状态的断连不得判为完成或失败：{}",
            disconnect_report.note
        );
        assert_eq!(disconnect_report.exit_code, None);
        assert!(
            disconnect_report.note.contains("通道关闭"),
            "必须写明断连而非超时：{}",
            disconnect_report.note
        );
        assert!(
            disconnect_report.note.contains("必须核对")
                && disconnect_report.note.contains("不得盲目重放"),
            "断连说明必须要求核对、禁止盲目重放：{}",
            disconnect_report.note
        );

        // 临时信任文件清理（尽力而为，失败不影响断言结论）。
        for path in [&exec_trust, &timeout_trust, &disconnect_trust] {
            let _ = std::fs::remove_file(path);
        }
    }
}
