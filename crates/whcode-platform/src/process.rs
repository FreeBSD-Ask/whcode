//! 子进程终止策略（FR-409、§10.2）。
//!
//! Windows 需要处理**子进程树**；BSD（以及类 Unix 平台）需要处理**进程组**、信号与子进程
//! 残留。策略在此集中声明，具体执行在 M8 的 worker 实现（T104/T109）中落地——本模块先
//! 固化"平台能承诺什么"的语义，避免实现层各自发明。

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
            note: "BSD：以进程组（setsid + killpg）终止；残留进程须由 M8 测试覆盖",
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
            note: "其他 Unix 平台：仅向直接子进程发信号，不承诺清理整棵树",
        }
    }
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

/// 终止子进程树并保留 stderr 尾部（T109、FR-409、§10.2）。
///
/// Windows：`taskkill /T /F /PID`（Job Object 语义的 std 等价路径，ADR-019 同口径
/// 的零依赖决策）；BSD：进程组语义属 M7 平台验证（ADR-013 延后），当前返回结构化
/// 错误不静默。`stderr_tail` 由调用方在终止前读取（worker 已保留尾部）。
///
/// # Errors
///
/// PID 无效或 taskkill 失败时返回 `io_error`（错误含平台说明）。
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
        let _ = pid;
        Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::IoError,
            "BSD/其他平台进程组终止属 M7 平台验证（ADR-013 延后）：不静默伪装成功",
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
}
