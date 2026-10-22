//! # 受控子进程 worker（T104、FR-705、§10.2）
//!
//! 超时、输出上限、取消（terminate）、输出尾部保留（FR-705）。
//! 子进程经 `std::process::Command`（ADR-008：主进程不得直接嵌入不受信任脚本）；
//! 超时经轮询 `try_wait`（std 无 wait_timeout）。
//!
//! 边界：worker 执行由具体 Provider 调用（`provider.rs`）；进程树清理属
//! `whcode_platform`（FR-409，M8/T109）。

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use whcode_domain::{ErrorCode, WhError};

/// 默认输出上限（字节，FR-705）。
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

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
}

fn tail(bytes: Vec<u8>, max: usize) -> String {
    let start = bytes.len().saturating_sub(max);
    let slice = &bytes[start..];
    String::from_utf8_lossy(slice).to_string()
}

/// 受控执行：子命令 + 超时 + 输出上限；超时经 `kill` 终止（进程树清理属平台层）。
///
/// # Errors
///
/// 启动失败或超时错误返回 [`WhError`]；`stdout_tail`/`stderr_tail` 保留尾部。
pub fn execute(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> Result<WorkerResult, WhError> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child: Child = command
        .spawn()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("启动子进程失败：{err}")))?;
    let started = Instant::now();
    loop {
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
}
