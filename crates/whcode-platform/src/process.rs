//! 子进程终止策略与执行（FR-409、§10.2）。
//!
//! Windows 需要处理**子进程树**（`taskkill /T`）；BSD（以及类 Unix 平台）需要处理
//! **进程组**、信号与子进程残留（`setpgid` + 向负 PID 发 TERM/KILL）。平台承诺与执行都
//! 收敛在此（NFR-101）：[`configure_child_process_group`] 在 `spawn` 前建组，
//! [`terminate_tree`] 终止整棵树，[`process_exists`] 提供逐 PID 的残留探测。
//!
//! 平台结论边界（ADR-013）：Windows 是唯一验收目标；BSD 侧的运行证据只能由 BSD CI
//! 取得，未取得前不得把编译通过当作平台结论。

use std::fmt;

/// 终止策略：声明一个平台清理子进程的方式与保证边界。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminationPolicy {
    /// 策略是否保证清理整棵子进程树。
    pub kills_tree: bool,
    /// 是否依赖进程组（`killpg` / `setsid`）语义。
    pub uses_process_group: bool,
    /// 平台说明（写入文档与错误消息；不得声称未实现的能力）。
    pub note: &'static str,
}

impl fmt::Display for TerminationPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.note)
    }
}

/// 返回当前 `cfg(target_os)` 对应的终止策略。
///
/// 观测事实（本轮实测）：本项目 Windows 验收环境为 `x86_64-pc-windows-msvc`
/// （rustc 1.99.1、MSVC 14.44.35207、Windows SDK 10.0.26100.0），按 D-01 修订
/// **不使用任何 GNU 工具链**。四大 BSD 的策略声明在对应平台编译时生效；其运行验证属
/// M0/M7（V-004），未验证前 `note` 只描述计划语义，不构成能力承诺。
#[must_use]
pub const fn termination_policy_for() -> TerminationPolicy {
    #[cfg(target_os = "windows")]
    {
        TerminationPolicy {
            kills_tree: true,
            uses_process_group: false,
            note: "Windows：经 Job Object / taskkill /T 终止子进程树；超时与信号路径在 M8 验证",
        }
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        TerminationPolicy {
            kills_tree: true,
            uses_process_group: true,
            note: "BSD：以进程组终止（setpgid(0,0) + kill -s TERM/KILL 到负 PID）；运行证据只能由 BSD CI 取得，未取得前不作平台结论",
        }
    }
    #[cfg(not(any(
        target_os = "windows",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    {
        TerminationPolicy {
            kills_tree: false,
            uses_process_group: true,
            note: "其他 Unix 平台：已实现进程组终止（setpgid + kill 到负 PID），但非验收目标、未取得运行证据，故不承诺清理整棵树",
        }
    }
}

/// 当前宿主是否为 Windows（平台判定集中点，NFR-101）。
///
/// 供需要在**运行期**选择夹具、路径或外部命令、但不应自行写平台条件编译的调用方使用
/// （例如测试：Windows 内建的 `cmd` 在 BSD 不存在，须运行期跳过而不是编译期剔除）。
/// 产品逻辑应优先调用本 crate 的具体能力函数，不要用本函数分支出两套平台实现。
#[must_use]
pub const fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

/// 平台 Shell 命令构造（cfg 集中于本 crate，NFR-101）：
/// Windows 用 `cmd /C`，类 Unix 用 `sh -c`——测试与其他 crate 经此构造，不自行分支。
#[must_use]
pub fn shell_command(script: &str) -> std::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", script]);
        command
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", script]);
        command
    }
}

/// 平台延时命令（超时类用例共用，cfg 集中于本 crate）：Windows 用 `ping` 轮询，
/// 类 Unix 用 `sleep`。
///
/// 秒级粒度，`0` 按 1 秒处理；Windows 的 `ping -n N` 实际墙钟约 `N-1` 秒，
/// 调用方只应把它当作“明显长于被测超时”的延时，不得用于精确计时。
#[must_use]
pub fn sleep_command(seconds: u64) -> std::process::Command {
    let seconds = seconds.max(1);
    #[cfg(target_os = "windows")]
    {
        shell_command(&format!("ping -n {seconds} 127.0.0.1 > NUL"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        shell_command(&format!("sleep {seconds}"))
    }
}

/// 让即将启动的子进程成为**新进程组的组长**（FR-409、§10.2）。
///
/// Unix 经 `std::os::unix::process::CommandExt::process_group(0)`（std 对 `setpgid(0, 0)`
/// 的封装，不引入 libc 依赖）；此后 [`terminate_tree`] 可用负 PID 向整组发信号，父进程
/// 退出不会让孙进程逃逸。Windows 无进程组语义，子进程树由 `taskkill /T` 处理，
/// 本函数为空操作。
///
/// 调用点：`whcode-runtime` 的 worker 在 `spawn` 之前调用（平台差异只在此 crate，
/// NFR-101）。失效条件：子进程自行 `setsid` 或再次 `setpgid` 脱离后，组终止不再覆盖它。
pub fn configure_child_process_group(command: &mut std::process::Command) {
    #[cfg(target_os = "windows")]
    {
        // Windows：零操作；参数绑定保留以表明签名跨平台一致（不是遗漏）。
        let _ = command;
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

/// 进程组终止的默认宽限：TERM 送达后等待整组退出的上限，超时升级为 KILL。
#[cfg(not(target_os = "windows"))]
const GROUP_TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// 组存活探测的轮询间隔（有界轮询，不创建线程）。
#[cfg(not(target_os = "windows"))]
const GROUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// `kill` 的参数：`-s <信号> -- -<PGID>`。
///
/// `--` 必须出现在负 PID 之前，否则 `-1234` 会被 `kill` 当作信号编号或未知选项，
/// 而不是“整个进程组”这一操作数。参数经 `Command::args` 以 argv 直传，不经过 shell，
/// 因而不存在命令拼接与注入面。
#[cfg(not(target_os = "windows"))]
fn kill_group_args(signal: &str, pgid: u32) -> [String; 4] {
    [
        "-s".to_string(),
        signal.to_string(),
        "--".to_string(),
        format!("-{pgid}"),
    ]
}

/// 判定 `kill` 的失败是否只是“目标不存在”（ESRCH）。
///
/// 只把这一情形视为幂等成功；权限（EPERM）、用法错误等其他失败一律向上报告，不猜测。
/// 判据是 `kill` 工具 stderr 的英文措辞——验收环境为 C locale；若被本地化，则按失败上报。
#[cfg(not(target_os = "windows"))]
fn is_no_such_process(stderr: &str) -> bool {
    stderr.to_ascii_lowercase().contains("no such process")
}

/// 向进程组发信号（TERM／KILL／0）。目标不存在时视为幂等成功。
///
/// # Errors
///
/// `kill` 不可用、信号被系统拒绝（权限、用法）时返回 `io_error`，消息附 stderr 尾部。
#[cfg(not(target_os = "windows"))]
fn signal_process_group(pgid: u32, signal: &str) -> Result<(), whcode_domain::WhError> {
    let output = std::process::Command::new("kill")
        .args(kill_group_args(signal, pgid))
        .output()
        .map_err(|err| {
            whcode_domain::WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!(
                    "无法执行系统 kill（进程组 {pgid} 未被终止）：{err}；\
                     请确认 PATH 包含系统 kill 命令"
                ),
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if is_no_such_process(&stderr) {
        return Ok(());
    }
    Err(whcode_domain::WhError::new(
        whcode_domain::ErrorCode::IoError,
        format!(
            "向进程组 {pgid} 发送 {signal} 失败（退出码 {}，stderr：{stderr}）；\
             请确认该组仍属当前用户会话，或改为逐个 PID 回收",
            output.status.code().unwrap_or(-1)
        ),
    ))
}

/// 进程组是否仍有成员（僵尸进程仍占用该组，故计为存活）。
#[cfg(not(target_os = "windows"))]
fn process_group_alive(pgid: u32) -> bool {
    std::process::Command::new("kill")
        .args(kill_group_args("0", pgid))
        .output()
        .is_ok_and(|output| output.status.success())
}

/// 有界轮询等待整组退出（不创建线程、不预分配缓冲）。
#[cfg(not(target_os = "windows"))]
fn wait_process_group_gone(pgid: u32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if !process_group_alive(pgid) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(GROUP_POLL_INTERVAL);
    }
}

/// 校验可作为进程组目标的 PID。
///
/// `0` 在 `kill` 语义中代表“调用者所在进程组”（会误杀自身所在组），`> i32::MAX`
/// 超出 `pid_t` 可表示范围而必然被 `kill` 拒绝——两者都提前报错并给出有效范围。
#[cfg(not(target_os = "windows"))]
fn ensure_group_target(pid: u32) -> Result<(), whcode_domain::WhError> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::IoError,
            format!(
                "PID {pid} 不能作为进程组目标（有效范围 1..={}）：PID 0 表示调用者自身进程组",
                i32::MAX
            ),
        ));
    }
    Ok(())
}

/// 终止**进程组**：先发 TERM，宽限期内未退出再发 KILL（FR-409、§10.2）。
///
/// Unix：`kill -s TERM/KILL -- -<pid>`，要求调用方启动时已把该进程建成组长
/// （见 [`configure_child_process_group`]）。Windows 无进程组语义，本函数转 `taskkill /T`。
///
/// 幂等：目标进程组已不存在时不报错（“无残留”已是目标状态）；权限、参数等其他失败
/// 返回 `io_error`，消息附 stderr 与可操作建议。KILL 送达即返回——若组内只剩已终止
/// 但尚未被回收的僵尸进程，组仍然“存在”，残留判据属于调用方回收（`wait`）之后的
/// 逐 PID 探测（Check 201）。
///
/// # Errors
///
/// PID 非法、`kill`／`taskkill` 不可用或信号被系统拒绝时返回 `io_error`。
pub fn term_process_group(pid: u32) -> Result<(), whcode_domain::WhError> {
    #[cfg(target_os = "windows")]
    {
        terminate_tree(pid)
    }
    #[cfg(not(target_os = "windows"))]
    {
        ensure_group_target(pid)?;
        signal_process_group(pid, "TERM")?;
        if wait_process_group_gone(pid, GROUP_TERM_GRACE) {
            return Ok(());
        }
        signal_process_group(pid, "KILL")
    }
}

/// 终止子进程树并保留 stderr 尾部（T109、FR-409、§10.2）。
///
/// Windows：`taskkill /T /F /PID`（Job Object 语义的 std 等价路径，ADR-019 同口径
/// 的零依赖决策）；Unix（含 BSD）：按进程组终止，见 [`term_process_group`]——前提是
/// 启动前调用过 [`configure_child_process_group`]。`stderr_tail` 由调用方在终止前读取
/// （worker 已保留尾部）。
///
/// # Errors
///
/// PID 无效、`taskkill`／`kill` 失败时返回 `io_error`（错误含平台说明与 stderr 尾部）。
pub fn terminate_tree(pid: u32) -> Result<(), whcode_domain::WhError> {
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output()
            .map_err(|err| {
                whcode_domain::WhError::new(
                    whcode_domain::ErrorCode::IoError,
                    format!("taskkill 失败：{err}"),
                )
            })?;
        if output.status.success() {
            Ok(())
        } else {
            Err(whcode_domain::WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!(
                    "taskkill 非零退出：{}（stderr 尾部：{}）",
                    output.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        term_process_group(pid)
    }
}

/// 判断进程是否仍存在（Check 201：终止后零残留验证）。
///
/// Windows 经 `tasklist` 查询；Unix（含 BSD）经 `kill -s 0 -- <pid>` 探测：只有明确的
/// “No such process”才判为不存在，权限等其他失败按错误上报——调用方不得把 `Err`
/// 当作“进程不存在”。
///
/// # Errors
///
/// 查询命令不可用、PID 非法或失败原因无法判定时返回 `io_error`。
pub fn process_exists(pid: u32) -> Result<bool, whcode_domain::WhError> {
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map_err(|err| {
                whcode_domain::WhError::new(
                    whcode_domain::ErrorCode::IoError,
                    format!("tasklist 不可用：{err}"),
                )
            })?;
        let text = String::from_utf8_lossy(&output.stdout);
        // CSV 首个数据行形如 `"image","1234","Console","1","12,345 K"`；
        // 无匹配时 tasklist 输出本地化提示（无 PID 列）。
        Ok(text.lines().any(|line| {
            line.split(',')
                .nth(1)
                .is_some_and(|field| field.trim_matches('"') == pid.to_string())
        }))
    }
    #[cfg(not(target_os = "windows"))]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(whcode_domain::WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!("PID {pid} 不是有效进程号（有效范围 1..={}）", i32::MAX),
            ));
        }
        let target = pid.to_string();
        let output = std::process::Command::new("kill")
            .args(["-s", "0", "--", target.as_str()])
            .output()
            .map_err(|err| {
                whcode_domain::WhError::new(
                    whcode_domain::ErrorCode::IoError,
                    format!(
                        "无法执行系统 kill 探测进程 {pid}（不视为“不存在”）：{err}；\
                         请确认 PATH 包含系统 kill 命令"
                    ),
                )
            })?;
        if output.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if is_no_such_process(&stderr) {
            return Ok(false);
        }
        Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::IoError,
            format!(
                "探测进程 {pid} 失败（退出码 {}，stderr：{stderr}）：\
                 无法区分“不存在”与“无权限”，不猜测",
                output.status.code().unwrap_or(-1)
            ),
        ))
    }
}

/// 列出某进程的**直接**子进程 PID（Check 201：零残留验证）。
///
/// Windows 经 PowerShell `Get-CimInstance Win32_Process` 查询；其他平台返回结构化
/// 错误（未实现，不伪装）。返回值可能为空（进程尚无子进程）。
///
/// # Errors
///
/// 查询工具不可用、输出无法解析时返回 `io_error`。
pub fn child_pids(pid: u32) -> Result<Vec<u32>, whcode_domain::WhError> {
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "Get-CimInstance Win32_Process -Filter 'ParentProcessId={pid}' | Select-Object -ExpandProperty ProcessId"
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|err| {
                whcode_domain::WhError::new(
                    whcode_domain::ErrorCode::IoError,
                    format!("powershell 不可用：{err}"),
                )
            })?;
        if !output.status.success() {
            return Err(whcode_domain::WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!(
                    "子进程查询失败：{}（stderr 尾部：{}）",
                    output.status.code().unwrap_or(-1),
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut pids: Vec<u32> = text
            .split_whitespace()
            .filter_map(|token| token.trim().parse::<u32>().ok())
            .collect();
        pids.sort_unstable();
        pids.dedup();
        Ok(pids)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = pid;
        Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::IoError,
            "子进程枚举在非 Windows 平台未实现（BSD 进程组验证属 M7，ADR-013 延后）：不静默伪装结论",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_matches_target_platform() {
        let policy = termination_policy_for();
        if cfg!(target_os = "windows") {
            assert!(policy.kills_tree);
            assert!(!policy.uses_process_group);
            assert!(policy.note.contains("Windows"));
        } else if cfg!(any(
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        )) {
            assert!(policy.kills_tree && policy.uses_process_group);
        } else {
            assert!(!policy.kills_tree);
        }
    }

    /// T109：Windows 上 taskkill 路径可用（对无效 PID 返回结构化错误不 panic）。
    #[test]
    fn terminate_tree_reports_invalid_pid_as_error() {
        let result = terminate_tree(u32::MAX);
        // 本机（Windows）：taskkill 对无效 PID 非零退出 → Err；其他平台：延后错误。
        assert!(result.is_err(), "无效 PID 必须返回错误而非 panic");
        if cfg!(target_os = "windows") {
            let err = result.expect_err("无效 PID");
            assert!(err.to_string().contains("taskkill"), "{err}");
        }
    }

    /// Check 201：自进程存在性可查询（其他平台返回“延后”错误而非伪结论）。
    #[test]
    fn process_exists_probes_current_process() {
        match process_exists(std::process::id()) {
            Ok(exists) => assert!(exists, "本进程必须存在"),
            Err(err) => assert!(err.message.contains("延后"), "非实现错误须写明延后：{err}"),
        }
    }

    /// 命令构造：`--` 必须排在负 PID 之前，否则 `-4321` 会被 `kill` 当作信号编号或选项。
    ///
    /// Windows 无 `kill`（走 `taskkill`），该路径在 Windows 编译期剔除。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn kill_group_args_terminate_option_parsing_before_negative_pid() {
        assert_eq!(kill_group_args("TERM", 4321).join(" "), "-s TERM -- -4321");
        assert_eq!(kill_group_args("KILL", 4321).join(" "), "-s KILL -- -4321");
        assert_eq!(kill_group_args("0", 99).join(" "), "-s 0 -- -99");
    }

    /// 进程组目标校验（纯逻辑）：0（调用者自身进程组）与超出 `pid_t` 的值必须被拒绝。
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn process_group_target_rejects_zero_and_overflow() {
        for pid in [0u32, i32::MAX as u32 + 1, u32::MAX] {
            assert!(ensure_group_target(pid).is_err(), "PID {pid} 必须被拒绝");
        }
        assert!(
            ensure_group_target(1).is_ok(),
            "PID 1 是合法数值（是否可信号取决于权限）"
        );
    }

    /// Check 109／201（平台侧）：配置进程组后仍能正常启动，且整树终止后无残留。
    ///
    /// 两个分支在各自平台真实执行：Windows 走 `taskkill /T`，Unix 走 `kill` 到负 PID。
    #[test]
    fn configured_child_is_terminable_and_leaves_no_residue() {
        let mut command = sleep_command(30);
        configure_child_process_group(&mut command);
        let mut child = command.spawn().expect("配置进程组后启动");
        let pid = child.id();
        assert!(
            process_exists(pid).expect("查询子进程"),
            "子进程 {pid} 启动后必须存在（阳性对照）"
        );
        terminate_tree(pid).expect("终止子进程树/组");
        let _ = child.wait();
        assert!(
            wait_until_gone(pid, std::time::Duration::from_secs(10)),
            "父进程 {pid} 终止后不应存在（残留）"
        );
    }

    /// 有界轮询直到进程消失；探测失败直接 panic——不得把 `Err` 当作“不存在”。
    fn wait_until_gone(pid: u32, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match process_exists(pid) {
                Ok(false) => return true,
                Ok(true) => {}
                Err(err) => panic!("探测进程 {pid} 失败：{err}"),
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// Check 201：子进程枚举能看到刚启动的子进程，终止后消失。
    ///
    /// CI 环境（GitHub Windows runner）PowerShell 启动可达数秒；5 秒延时子进程可能在
    /// 首次枚举完成前已退出。用 30 秒延时并在 30 秒窗口内轮询，避免把“枚举慢”误判为
    /// “未列出子进程”。
    #[test]
    fn child_pids_lists_spawned_child_and_lifecycle_is_observable() {
        let mut child = sleep_command(30).spawn().expect("启动子进程");
        let pid = child.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let pids = loop {
            match child_pids(std::process::id()) {
                Ok(list) => {
                    if list.contains(&pid) {
                        break list;
                    }
                    if std::time::Instant::now() >= deadline {
                        break list;
                    }
                }
                Err(err) => {
                    assert!(err.message.contains("延后"), "非实现错误须写明延后：{err}");
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        };
        assert!(pids.contains(&pid), "应列出子进程 {pid}：{pids:?}");
        assert!(
            process_exists(pid).expect("查询子进程"),
            "子进程 {pid} 应存在"
        );
        let _ = child.kill();
        let _ = child.wait();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut gone = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if !process_exists(pid).unwrap_or(true) {
                gone = true;
                break;
            }
        }
        assert!(gone, "子进程 {pid} 终止后不应存在");
    }
}
