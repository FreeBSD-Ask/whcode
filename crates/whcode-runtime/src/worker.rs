//! # 受控子进程 worker（T104、FR-705、§10.2）
//!
//! 超时、输出上限、取消（terminate）、输出尾部保留（FR-705）。
//! 子进程经 `std::process::Command`（ADR-008：主进程不得直接嵌入不受信任脚本）；
//! 超时经轮询 `try_wait`（std 无 wait_timeout）。
//!
//! 凭据隔离（§10.2、Check 144）：启动前统一剥离父进程与调用方显式设置中的
//! 凭据型环境变量（见 [`strip_credentials`]），子进程拿不到 API Key、令牌与
//! 代理套接字；剥离只记变量名、不记值。
//!
//! 边界：worker 执行由具体 Provider 调用（`provider.rs`）；`spawn` 前的进程组配置与
//! 进程树清理属 `whcode_platform`（FR-409，T109）。

use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use whcode_domain::{ErrorCode, WhError};

/// 默认输出上限（字节，FR-705）。
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// 默认并发子进程上限（资源纪律：进程级有界；取 `min(4, 可用并行度)`）。
pub const DEFAULT_MAX_CONCURRENT: usize = 4;

static DEFAULT_GATE: OnceLock<ConcurrencyGate> = OnceLock::new();

/// 进程级默认并发闸门（首次使用时按 `min(4, 可用并行度)` 初始化为 [`DEFAULT_MAX_CONCURRENT`]
/// 上限内的值；不创建线程）。
fn default_gate() -> &'static ConcurrencyGate {
    DEFAULT_GATE.get_or_init(|| {
        let cores = whcode_platform::available_physical_cores();
        let limit = whcode_platform::thread_budget(cores) as usize;
        ConcurrencyGate::new(limit.min(DEFAULT_MAX_CONCURRENT))
    })
}

/// 有界并发闸门（Mutex + Condvar；不创建线程、不预分配缓冲）。
#[derive(Debug)]
pub struct ConcurrencyGate {
    limit: usize,
    active: Mutex<usize>,
    released: Condvar,
}

impl ConcurrencyGate {
    /// 构造闸门；`limit` 为 0 时按 1 处理（不允许无限并发）。
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            active: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// 并发上限。
    #[must_use]
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// 当前占用的并发数。
    #[must_use]
    pub fn active(&self) -> usize {
        *self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 申请一个许可；达到上限时**阻塞等待**（调用方须保证在执行路径而非渲染路径）。
    pub fn acquire(&self) -> GatePermit<'_> {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        while *active >= self.limit {
            active = self
                .released
                .wait(active)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *active += 1;
        GatePermit { gate: self }
    }
}

/// 并发许可；`Drop` 时归还名额并唤醒一个等待者。
#[derive(Debug)]
pub struct GatePermit<'a> {
    gate: &'a ConcurrencyGate,
}

impl Drop for GatePermit<'_> {
    fn drop(&mut self) {
        let mut active = self
            .gate
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *active = active.saturating_sub(1);
        self.gate.released.notify_one();
    }
}

/// 凭据型环境变量名的子串标记（大写比较；命中即从子进程环境移除）。
///
/// 选择“名字特征”而不是枚举产品变量：新增服务（云厂商、模型提供商）无需改代码；
/// 代价是同名特征可能误删无害变量，因此只移除**子进程环境**，不影响本进程。
const CREDENTIAL_MARKERS: &[&str] = &[
    "KEY",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "COOKIE",
    "SIGNATURE",
];

/// 需要显式移除的凭据通道（名称不含上述标记，但同样不得透传）。
const CREDENTIAL_EXACT: &[&str] = &["SSH_AUTH_SOCK", "GPG_AGENT_INFO"];

/// 本程序自身前缀（配置与凭据不进入子进程）。
const APP_ENV_PREFIX: &str = "WHCODE_";

/// 判定环境变量名是否属于凭据类（纯函数，便于测试与审计）。
#[must_use]
pub fn is_credential_var(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with(APP_ENV_PREFIX)
        || CREDENTIAL_EXACT.contains(&upper.as_str())
        || CREDENTIAL_MARKERS
            .iter()
            .any(|marker| upper.contains(marker))
}

/// 从待启动命令中剥离凭据型环境变量，返回被移除的变量名（只记名，不记值）。
///
/// 同时覆盖两类来源：父进程继承的环境，以及调用方经 [`Command::env`] 显式设置的
/// 变量；两者都命中 [`is_credential_var`] 时移除。返回值已排序，供调用方记录审计。
pub fn strip_credentials(command: &mut Command) -> Vec<String> {
    let mut removed: Vec<String> = Vec::new();
    for (name, _) in std::env::vars_os() {
        if let Some(name) = name.to_str()
            && is_credential_var(name)
        {
            command.env_remove(name);
            removed.push(name.to_string());
        }
    }
    let explicit: Vec<String> = command
        .get_envs()
        .filter_map(|(name, _)| name.to_str().map(ToOwned::to_owned))
        .collect();
    for name in explicit {
        if is_credential_var(&name) {
            command.env_remove(&name);
            removed.push(name);
        }
    }
    removed.sort();
    removed.dedup();
    removed
}

/// 子进程默认继承的**最小环境白名单**（§10.2：默认继承最小化集合，而非完整继承
/// 主进程环境；Check 123 安全复查）。
///
/// 取舍：仅保留子进程可运行所需的系统级变量——可执行解析（`PATH`/`PATHEXT`）、
/// Windows 系统目录（`SystemRoot`/`SystemDrive`，python 与 Git Bash 启动必需）、
/// cmd 解析（`COMSPEC`）、临时目录（`TEMP`/`TMP`，`workdir_policy "temp"` 依赖）与
/// 并行度（`NUMBER_OF_PROCESSORS`）。其余一律不继承——配置、凭据、代理与本机
/// 个性化变量都不进子进程。失效条件：新运行时确需额外系统变量时，必须修改本
/// 白名单并在 ADR 记录原因，不得改回完整继承。
const MINIMAL_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SystemDrive",
    "COMSPEC",
    "TEMP",
    "TMP",
    "NUMBER_OF_PROCESSORS",
];

/// 把命令环境收敛为最小集合（§10.2），返回被丢弃的父环境变量个数（只记数不记值）。
///
/// 顺序敏感：先记录调用方显式设置的变量，再 [`Command::env_clear`]（禁用完整
/// 继承），然后按白名单从父环境回填、把调用方显式变量写回——后者随后由
/// [`strip_credentials`] 二次过滤（`execute_with_cancel` 中的固定次序）。
pub fn apply_minimal_env(command: &mut Command) -> usize {
    let explicit: Vec<(std::ffi::OsString, std::ffi::OsString)> = command
        .get_envs()
        .filter_map(|(name, value)| value.map(|value| (name.to_os_string(), value.to_os_string())))
        .collect();
    command.env_clear();
    let mut dropped = 0usize;
    for (name, value) in std::env::vars_os() {
        let Some(name_str) = name.to_str() else {
            dropped += 1;
            continue;
        };
        if MINIMAL_ENV_ALLOWLIST
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(name_str))
        {
            command.env(name, value);
        } else {
            dropped += 1;
        }
    }
    for (name, value) in explicit {
        command.env(name, value);
    }
    dropped
}

/// 子进程执行结果（输出尾部保留，FR-705）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkerResult {
    /// 退出码；被终止/超时时为 `None`。
    pub exit_code: Option<i32>,
    /// stdout 尾部（截断至上限）。
    pub stdout_tail: String,
    /// stderr 尾部（截断至上限，FR-705 保留）。
    pub stderr_tail: String,
    /// 是否超时被终止。
    pub timed_out: bool,
    /// 是否因取消请求被终止（FR-705：取消与超时可区分）。
    pub cancelled: bool,
}

fn tail(bytes: Vec<u8>, max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    let slice = &bytes[start..];
    String::from_utf8_lossy(slice).to_string()
}

/// 受控执行：子命令 + 超时 + 输出上限；超时经 `kill` 终止（进程树清理属平台层）。
///
/// 并发由进程级 [`ConcurrencyGate`] 约束（默认 `min(4, 可用并行度)`），不会为每个
/// 子进程创建线程；取消请求经 [`execute_with_cancel`] 的共享标志生效。
///
/// # Errors
///
/// 启动失败或超时错误返回 [`WhError`]；`stdout_tail`/`stderr_tail` 保留尾部。
pub fn execute(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> Result<WorkerResult, WhError> {
    execute_with_cancel(command, timeout, max_output, None)
}

/// 同 [`execute`]，并支持**协作式取消**：`cancel` 置位后在下一次轮询（≤10 ms）终止子进程。
///
/// 取消与超时的区别写入结果（`cancelled` / `timed_out`），调用方不得把两者混为一谈。
///
/// # Errors
///
/// 启动或读取输出失败时返回 [`WhError`]。
pub fn execute_with_cancel(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
    cancel: Option<&AtomicBool>,
) -> Result<WorkerResult, WhError> {
    // 资源上限：并发子进程闸门（有界；许可在函数返回时自动归还）。
    let _permit = default_gate().acquire();
    // 最小环境（§10.2）：默认继承最小化集合而非完整父环境；随后凭据剥离对
    // 调用方显式变量二次过滤。两者都只记数/记名，不记值。
    let _ = apply_minimal_env(command);
    let _ = strip_credentials(command);
    // 进程组（FR-409）：平台差异集中在 whcode-platform——Unix 让子进程自成进程组，
    // 终止时可按组回收子孙进程；Windows 为空操作（子进程树由 taskkill /T 处理）。
    whcode_platform::process::configure_child_process_group(command);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child: Child = command
        .spawn()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("启动子进程失败：{err}")))?;
    let started = Instant::now();
    loop {
        if cancel.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(WorkerResult {
                exit_code: None,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                timed_out: false,
                cancelled: true,
            });
        }
        match child
            .try_wait()
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?
        {
            Some(status) => {
                let output = child.wait_with_output().map_err(|err| {
                    WhError::new(ErrorCode::IoError, format!("读取输出失败：{err}"))
                })?;
                return Ok(WorkerResult {
                    exit_code: status.code(),
                    stdout_tail: tail(output.stdout, max_output),
                    stderr_tail: tail(output.stderr, max_output),
                    timed_out: false,
                    cancelled: false,
                });
            }
            None if started.elapsed() >= timeout => {
                // 超时：终止子进程（进程树清理属平台层 FR-409/T109）。
                let _ = child.kill();
                let _ = child.wait();
                return Ok(WorkerResult {
                    exit_code: None,
                    stdout_tail: String::new(),
                    stderr_tail: String::new(),
                    timed_out: true,
                    cancelled: false,
                });
            }
            None => {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// Check 104：输出上限触发——只保留尾部，不随输出增长。
    #[test]
    fn output_is_capped_to_tail() {
        let mut command = whcode_platform::shell_command("echo 0123456789abcdef0123456789abcdef");
        let result = execute(&mut command, Duration::from_secs(10), 8).expect("执行");
        assert!(
            result.stdout_tail.len() <= 8,
            "尾部不得超过上限：{:?}",
            result.stdout_tail
        );
        assert!(
            !result.stdout_tail.contains("0123"),
            "不得保留全部输出：{:?}",
            result.stdout_tail
        );
    }

    /// Check 104：取消在轮询间隔内生效，且与超时可区分（`cancelled` vs `timed_out`）。
    #[test]
    fn cancel_terminates_child_and_is_distinguishable_from_timeout() {
        let mut command = whcode_platform::sleep_command(30);
        let flag = Arc::new(AtomicBool::new(false));
        let worker_flag = Arc::clone(&flag);
        let started = Instant::now();
        let handle = std::thread::spawn(move || {
            execute_with_cancel(
                &mut command,
                Duration::from_secs(30),
                MAX_OUTPUT_BYTES,
                Some(worker_flag.as_ref()),
            )
        });
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::SeqCst);
        let result = handle.join().expect("等待线程").expect("执行");
        assert!(result.cancelled, "必须报告已取消");
        assert!(!result.timed_out, "取消不得混同为超时");
        assert_eq!(result.exit_code, None);
        // 本用例验证“取消可区分且有界生效”，不是延迟预算（延迟由 NFR 测量承担）。
        // 46 个 runtime 用例并行时，子进程回收线程可能被调度饥饿；10 s 上界曾在
        // 满载下实测 12.03 s。放宽到 20 s 只吸收调度抖动，不改变取消语义。
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "取消必须在有界时间内生效（实际 {:?}）",
            started.elapsed()
        );
    }

    /// Check 109／201（BSD 侧）：进程组终止后**父与孙进程均无残留**。
    ///
    /// 夹具 `sh -c 'sleep 30 & echo CHILD=$!; sleep 30'` 先打印孙进程 PID 再等待，因此
    /// 可以（a）做阳性对照——证明孙进程确实存在过；（b）整组终止后逐个 PID 用
    /// `kill -s 0` 探测，残留判据不依赖父进程“自称已清理”。
    ///
    /// 仅 Unix 运行：Windows 无进程组语义（`taskkill /T` 的残余验证由
    /// `tests/stability/test_process_cleanup.rs` 覆盖）；平台判定经 `whcode-platform`，
    /// 本 crate 不做平台条件编译（NFR-101）。
    #[test]
    fn terminate_tree_leaves_no_residual_in_process_group() {
        if whcode_platform::is_windows() {
            return;
        }
        let mut command = whcode_platform::shell_command("sleep 30 & echo CHILD=$!; sleep 30");
        whcode_platform::process::configure_child_process_group(&mut command);
        command.stdout(Stdio::piped());
        let mut child = command.spawn().expect("启动进程组");
        let parent = child.id();
        let stdout = child.stdout.take().expect("stdout 已管道化");
        let mut line = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .expect("读取 CHILD 行");
        let grandchild: u32 = line
            .trim()
            .strip_prefix("CHILD=")
            .unwrap_or_else(|| panic!("首个输出行应为 CHILD=<pid>，实际：{line:?}"))
            .parse()
            .expect("CHILD 后的 PID");
        assert!(
            whcode_platform::process_exists(grandchild).expect("探测孙进程"),
            "阳性对照：孙进程 {grandchild} 必须存在（否则残留判据无意义）"
        );

        whcode_platform::terminate_tree(parent).expect("终止进程组");
        let status = child.wait().expect("回收父进程");
        assert!(!status.success(), "父进程应被信号终止：{status:?}");

        assert!(
            wait_until_gone(parent, Duration::from_secs(10)),
            "父进程 {parent} 终止后仍存在（残留）"
        );
        assert!(
            wait_until_gone(grandchild, Duration::from_secs(10)),
            "孙进程 {grandchild} 未随进程组终止（残留）"
        );
    }

    /// 执行路径的进程组约定：`execute` 启动的子进程是**新进程组的组长**
    /// （`pgid == pid`），这是 `terminate_tree` 能用负 PID 覆盖其子孙的前提。
    ///
    /// 仅 Unix：Windows 无进程组语义，运行期跳过；平台判定经 `whcode-platform`。
    #[test]
    fn execute_puts_child_in_its_own_process_group() {
        if whcode_platform::is_windows() {
            return;
        }
        let mut command = whcode_platform::shell_command("echo PID=$$; ps -o pgid= -p $$");
        let result =
            execute(&mut command, Duration::from_secs(10), MAX_OUTPUT_BYTES).expect("执行");
        assert_eq!(result.exit_code, Some(0), "stderr：{}", result.stderr_tail);
        let mut pid = None;
        let mut pgid = None;
        for line in result.stdout_tail.lines() {
            let text = line.trim();
            if let Some(rest) = text.strip_prefix("PID=") {
                pid = rest.parse::<u32>().ok();
            } else if let Ok(value) = text.parse::<u32>() {
                pgid = Some(value);
            }
        }
        assert_eq!(
            pid, pgid,
            "子进程须自成进程组（pgid == pid），实际输出：{}",
            result.stdout_tail
        );
    }

    /// 有界轮询直到进程消失；探测失败直接失败——不得把错误当作“不存在”。
    fn wait_until_gone(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match whcode_platform::process_exists(pid) {
                Ok(false) => return true,
                Ok(true) => {}
                Err(err) => panic!("探测进程 {pid} 失败：{err}"),
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Check 104：并发闸门有界，许可在 Drop 时归还（资源上限的触发测试）。
    #[test]
    fn concurrency_gate_bounds_active_workers_and_releases_permits() {
        let gate = Arc::new(ConcurrencyGate::new(2));
        assert_eq!(gate.limit(), 2);
        let peak = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..6 {
            let gate = Arc::clone(&gate);
            let peak = Arc::clone(&peak);
            handles.push(std::thread::spawn(move || {
                let _permit = gate.acquire();
                peak.fetch_max(gate.active(), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
            }));
        }
        for handle in handles {
            handle.join().expect("等待线程");
        }
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "并发不得超过上限（实测峰值 {}）",
            peak.load(Ordering::SeqCst)
        );
        assert_eq!(gate.active(), 0, "许可必须全部归还");
        // limit=0 按 1 处理：不允许无限并发。
        assert_eq!(ConcurrencyGate::new(0).limit(), 1);
    }

    #[test]
    fn worker_executes_and_returns_exit_code() {
        let mut command = whcode_platform::shell_command("echo hello");
        let result =
            execute(&mut command, Duration::from_secs(10), MAX_OUTPUT_BYTES).expect("执行");
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout_tail.contains("hello"));
        assert!(!result.timed_out);
    }

    #[test]
    fn worker_preserves_stderr_tail_on_failure() {
        let mut command = whcode_platform::shell_command("exit 3");
        let result =
            execute(&mut command, Duration::from_secs(10), MAX_OUTPUT_BYTES).expect("执行");
        assert_eq!(result.exit_code, Some(3), "退出码保留");
        assert!(!result.timed_out);
    }

    #[test]
    fn worker_timeout_kills_process() {
        // 平台延时命令由 whcode-platform 提供（NFR-101：本 crate 不做平台分流）。
        let mut command = whcode_platform::sleep_command(30);
        let result =
            execute(&mut command, Duration::from_millis(200), MAX_OUTPUT_BYTES).expect("超时执行");
        assert!(result.timed_out, "超时必须终止");
        assert_eq!(result.exit_code, None);
    }

    /// 凭据特征判定：覆盖 API Key、令牌、口令、云厂商与代理通道，不误伤常规变量。
    #[test]
    fn credential_variable_detection_covers_common_channels() {
        for name in [
            "OPENAI_API_KEY",
            "WHCODE_LLM_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "DB_PASSWORD",
            "SSH_AUTH_SOCK",
            "GPG_AGENT_INFO",
            "ANTHROPIC_CREDENTIALS",
            "HTTP_COOKIE",
        ] {
            assert!(is_credential_var(name), "{name} 必须视为凭据");
        }
        for name in ["PATH", "TEMP", "HOME", "LANG", "PATHEXT", "SYSTEMROOT"] {
            assert!(!is_credential_var(name), "{name} 不得被剥离");
        }
    }

    /// Check 123（§10.2 最小环境）：收敛后命令环境＝白名单 ∩ 父环境 + 调用方显式变量。
    #[test]
    fn minimal_env_converges_to_allowlist_plus_explicit() {
        let mut command = Command::new("cmd");
        command.env("WHCODEX_EXPLICIT", "kept");
        let dropped = apply_minimal_env(&mut command);
        assert!(dropped > 0, "父环境必然存在非白名单变量（如 USERPROFILE）");
        let names: Vec<String> = command
            .get_envs()
            .filter_map(|(name, _)| name.to_str().map(String::from))
            .collect();
        for name in &names {
            assert!(
                MINIMAL_ENV_ALLOWLIST
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(name))
                    || name == "WHCODEX_EXPLICIT",
                "收敛后仅允许白名单或显式变量：{name}"
            );
        }
        assert!(
            names.iter().any(|name| name.eq_ignore_ascii_case("PATH")),
            "PATH 必须保留（可执行解析依赖）"
        );
        assert!(
            names.iter().any(|name| name == "WHCODEX_EXPLICIT"),
            "调用方显式变量在收敛后写回"
        );
    }

    /// Check 123（§10.2 最小环境）：真实子进程只看到最小集合——非白名单父变量
    /// （USERPROFILE 在测试环境必然存在）不继承，显式变量保留。
    ///
    /// **仅 Windows 执行**：夹具用 `cmd /C set` 列出子进程环境；BSD 无 `cmd`（实测），
    /// 在运行期跳过而不是用平台条件编译剔除（平台判定集中在 `whcode-platform`，
    /// NFR-101）。跨平台的最小环境收敛逻辑由
    /// [`minimal_env_converges_to_allowlist_plus_explicit`] 覆盖（不启动进程）。
    #[test]
    fn child_sees_only_allowlist_and_explicit_vars() {
        if !whcode_platform::is_windows() {
            return;
        }
        let mut command = Command::new("cmd");
        command.args(["/C", "set"]).env("WHCODEX_EXPLICIT", "kept");
        let result =
            execute(&mut command, Duration::from_secs(10), MAX_OUTPUT_BYTES).expect("执行");
        assert_eq!(result.exit_code, Some(0));
        let env = &result.stdout_tail;
        assert!(env.contains("WHCODEX_EXPLICIT=kept"), "显式变量保留：{env}");
        assert!(
            !env.contains("USERPROFILE="),
            "非白名单父变量（USERPROFILE）不得继承：{env}"
        );
        assert!(
            !env.contains("WHCODE_"),
            "WHCODE_ 前缀变量永不进入子进程：{env}"
        );
    }

    /// 子进程探针：父测试以 `--exact --ignored` 启动；看到凭据即退出码 7。
    #[test]
    #[ignore = "仅由 strip_credentials_removes_credentials_from_child（父测试）以子进程调用"]
    fn env_probe_child() {
        let visible = std::env::var("WHCODE_TEST_FAKE_TOKEN").is_ok();
        println!("probe-ran token={}", if visible { "yes" } else { "no" });
        std::process::exit(if visible { 7 } else { 0 });
    }

    /// Check 144：剥离后子进程看不到凭据；未剥离的阳性对照必须能看到（证明探针有效）。
    #[test]
    fn strip_credentials_removes_credentials_from_child() {
        let exe = std::env::current_exe().expect("测试可执行文件路径");
        // 返回（退出码，子进程 stdout）：stdout 用于证明探针确实执行，而不是“没匹配到用例”。
        let probe = |sanitize: bool| -> (Option<i32>, String) {
            let mut command = Command::new(&exe);
            command.args([
                "--exact",
                "worker::tests::env_probe_child",
                "--ignored",
                "--nocapture",
            ]);
            command.env("WHCODE_TEST_FAKE_TOKEN", "leak");
            if sanitize {
                let removed = strip_credentials(&mut command);
                assert!(
                    removed.iter().any(|name| name == "WHCODE_TEST_FAKE_TOKEN"),
                    "显式设置的凭据变量必须被移除：{removed:?}"
                );
                let result = execute(&mut command, Duration::from_secs(30), MAX_OUTPUT_BYTES)
                    .expect("探针执行");
                return (result.exit_code, result.stdout_tail);
            }
            // 阳性对照绕开 `execute` 的统一剥离，直接读原始退出码。
            let output = command.output().expect("探针执行");
            (
                output.status.code(),
                String::from_utf8_lossy(&output.stdout).to_string(),
            )
        };
        let (code, stdout) = probe(false);
        assert_eq!(code, Some(7), "阳性对照：未剥离时子进程可见凭据");
        assert!(
            stdout.contains("probe-ran token=yes"),
            "探针未执行：{stdout}"
        );
        let (code, stdout) = probe(true);
        assert_eq!(code, Some(0), "剥离后子进程不得看到凭据");
        assert!(
            stdout.contains("probe-ran token=no"),
            "探针未执行：{stdout}"
        );
    }
}
