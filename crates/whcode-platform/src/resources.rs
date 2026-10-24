//! 资源约束默认值（§5.4、FR-705）。
//!
//! 数值是**可配置基线**，不是架构定理（§0.3-6）：`memory_limit` 仅约束 DuckDB 缓冲
//! 管理器，不代表进程 RSS 上限；`threads` 初始不高于 `min(4, 可用物理核数)`。

/// DuckDB 缓冲管理器默认内存上限（字节）：192 MiB。
pub const DUCKDB_MEMORY_LIMIT_DEFAULT: u64 = 192 * 1024 * 1024;
/// DuckDB 缓冲管理器候选最大内存上限（字节）：384 MiB。
pub const DUCKDB_MEMORY_LIMIT_MAX_CANDIDATE: u64 = 384 * 1024 * 1024;

/// 数据库相关的资源默认值（§5.4 表格）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// DuckDB `memory_limit`（字节）。
    pub duckdb_memory_limit: u64,
    /// DuckDB `threads` 初始值。
    pub duckdb_threads: u32,
}

/// 返回数据库资源默认值。`available_physical_cores` 为探测到的可用物理核数。
///
/// # Panics
///
/// `available_physical_cores == 0` 时 panic——核数探测失败属于启动期致命配置错误。
#[must_use]
pub fn memory_limit_defaults(available_physical_cores: u32) -> ResourceLimits {
    assert!(available_physical_cores > 0, "物理核数探测值必须大于 0");
    ResourceLimits {
        duckdb_memory_limit: DUCKDB_MEMORY_LIMIT_DEFAULT,
        duckdb_threads: available_physical_cores.min(4),
    }
}

/// 通用工作线程预算：`min(4, cores)`（§5.4 `threads` 行）。
#[must_use]
pub fn thread_budget(available_physical_cores: u32) -> u32 {
    available_physical_cores.clamp(1, 4)
}

/// 探测可用并行度。
///
/// 说明：`std::thread::available_parallelism` 返回的是**可用并行度**（受 cgroup 配额、
/// CPU 亲和性等影响），并非严格物理核数。§5.4 的 `min(4, 物理核)` 基线在此以可用并行度
/// 实现；M0 基线报告（V-006）必须记录实际取值与探测方式的偏差，不得把该值当作物理核数
/// 写进性能结论。
#[must_use]
pub fn available_physical_cores() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

/// 进程当前工作集（RSS）字节数（Check 221 内存可观测性；NFR-201—204）。
///
/// Windows 经 `tasklist /FO CSV` 读取 “Mem Usage” 列；其他平台返回结构化错误
/// （BSD/Linux 的 RSS 读取属 M7，ADR-013 延后）——调用方**不得**把 `Err` 当作 0。
///
/// # Errors
///
/// 查询工具不可用、PID 不存在或输出无法解析时返回 [`WhError`]
/// （`io_error` / `not_found` / `parse_error`）。
pub fn process_rss_bytes(pid: u32) -> Result<u64, whcode_domain::WhError> {
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .map_err(|err| whcode_domain::WhError::io("查询进程工作集", &err))?;
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            // 形如 `"whcode.exe","1234","Console","1","12,345 K"`
            let fields: Vec<&str> = line.split("\",\"").collect();
            if fields.len() < 5 || fields[1] != pid.to_string() {
                continue;
            }
            let digits: String = fields[4].chars().filter(char::is_ascii_digit).collect();
            let kilobytes: u64 = digits.parse().map_err(|_| {
                whcode_domain::WhError::new(
                    whcode_domain::ErrorCode::ParseError,
                    format!("无法解析 tasklist 的内存列：{line}"),
                )
            })?;
            return Ok(kilobytes.saturating_mul(1024));
        }
        Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::NotFound,
            format!("进程不存在或无可读工作集（PID {pid}）"),
        ))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = pid;
        Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::IoError,
            "进程工作集读取在非 Windows 平台未实现（BSD/Linux RSS 属 M7，ADR-013 延后）：不返回 0 伪装成功",
        ))
    }
}

/// 当前进程工作集（RSS）字节数。
///
/// # Errors
///
/// 同 [`process_rss_bytes`]。
pub fn current_rss_bytes() -> Result<u64, whcode_domain::WhError> {
    process_rss_bytes(std::process::id())
}

/// 进程资源快照（CPU 累计时间、线程数、句柄数；NFR-211、§11.5 长跑采样）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessStats {
    /// 进程启动以来的累计 CPU 时间（秒，用户态 + 内核态）。
    pub cpu_seconds: f64,
    /// 当前线程数。
    pub threads: u32,
    /// 当前句柄数。
    pub handles: u32,
}

/// 读取指定进程的资源快照。
///
/// Windows 经 PowerShell `Get-Process` 读取；其他平台返回结构化错误（与
/// [`process_rss_bytes`] 同口径，不得把 `Err` 当作 0）。
///
/// # Errors
///
/// 进程不存在、查询工具不可用或输出无法解析时返回 [`whcode_domain::WhError`]。
pub fn process_stats(pid: u32) -> Result<ProcessStats, whcode_domain::WhError> {
    #[cfg(target_os = "windows")]
    {
        let script = format!(
            "$p = Get-Process -Id {pid} -ErrorAction Stop; \
             \"$($p.CPU)|$($p.Threads.Count)|$($p.HandleCount)\""
        );
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|err| whcode_domain::WhError::io("查询进程资源", &err))?;
        if !output.status.success() {
            return Err(whcode_domain::WhError::new(
                whcode_domain::ErrorCode::NotFound,
                format!(
                    "进程不存在或资源不可读（PID {pid}）：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let mut fields = line.trim().split('|');
        let cpu = fields
            .next()
            .and_then(|value| value.trim().parse::<f64>().ok());
        let threads = fields
            .next()
            .and_then(|value| value.trim().parse::<u32>().ok());
        let handles = fields
            .next()
            .and_then(|value| value.trim().parse::<u32>().ok());
        match (cpu, threads, handles) {
            (Some(cpu_seconds), Some(threads), Some(handles)) => Ok(ProcessStats {
                cpu_seconds,
                threads,
                handles,
            }),
            _ => Err(whcode_domain::WhError::parse_error(format!(
                "无法解析进程资源输出（PID {pid}）：{line}"
            ))),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = pid;
        Err(whcode_domain::WhError::new(
            whcode_domain::ErrorCode::IoError,
            "进程资源读取在非 Windows 平台未实现（BSD/Linux 属 M7，ADR-013 延后）：不返回 0 伪装成功",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_defaults_match_task_book() {
        let limits = memory_limit_defaults(8);
        assert_eq!(limits.duckdb_memory_limit, 192 * 1024 * 1024);
        assert_eq!(limits.duckdb_threads, 4);
        assert!(limits.duckdb_memory_limit < DUCKDB_MEMORY_LIMIT_MAX_CANDIDATE);
    }

    #[test]
    fn threads_capped_at_four_even_on_big_machines() {
        assert_eq!(thread_budget(64), 4);
        assert_eq!(thread_budget(2), 2);
        assert_eq!(thread_budget(1), 1);
    }

    #[test]
    fn single_core_machine_gets_one_thread() {
        assert_eq!(memory_limit_defaults(1).duckdb_threads, 1);
    }

    #[test]
    #[should_panic(expected = "大于 0")]
    fn zero_cores_is_fatal() {
        let _ = memory_limit_defaults(0);
    }

    /// Check 221：自进程 RSS 可读，量级合理；不存在的 PID 必须报错而不是返回 0。
    #[test]
    fn rss_probe_reads_current_process_and_rejects_missing_pid() {
        match current_rss_bytes() {
            Ok(bytes) => {
                assert!(
                    bytes > 1024 * 1024,
                    "工作集应大于 1 MiB（实测 {bytes} 字节）"
                );
                assert!(
                    bytes < 1024_u64.pow(4),
                    "工作集不可能超过 1 TiB（实测 {bytes} 字节）"
                );
            }
            Err(err) => assert!(err.message.contains("延后"), "非实现错误须写明延后：{err}"),
        }
        let missing = process_rss_bytes(u32::MAX);
        assert!(missing.is_err(), "不存在的 PID 必须返回错误（不得返回 0）");
    }

    /// §11.5 长跑采样：当前进程的 CPU／线程／句柄可读；不存在的 PID 必须报错。
    #[test]
    fn process_stats_reads_current_process_and_rejects_missing_pid() {
        match process_stats(std::process::id()) {
            Ok(stats) => {
                assert!(stats.cpu_seconds >= 0.0, "CPU 累计时间不得为负");
                assert!(stats.threads >= 1, "进程至少有一个线程");
                assert!(stats.handles >= 1, "进程至少有一个句柄");
            }
            Err(err) => assert!(err.message.contains("延后"), "非实现错误须写明延后：{err}"),
        }
        let missing = process_stats(u32::MAX);
        assert!(missing.is_err(), "不存在的 PID 必须返回错误（不得返回 0）");
    }
}
