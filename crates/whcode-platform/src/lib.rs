//! # whcode-platform
//!
//! 操作系统差异集中层（NFR-101）：全部 `cfg(target_os)` 平台分支**必须**收敛于本 crate，
//! 其他 crate 通过本模块的稳定 API 获得平台行为。平台依赖与可选功能以 Cargo feature
//! 隔离（NFR-102）。
//!
//! 当前覆盖：进程终止策略（Windows 子进程树 / BSD-类进程组）、路径与资源探测、
//! 终端能力接口（骨架，M0/T024 接线实际探测）、用户级字体注册（FR-717）。

pub mod fonts;
pub mod paths;
pub mod process;
pub mod resources;
pub mod term;

pub use paths::{default_temp_dir, workspace_config_dir, workspace_temp_dir};
pub use process::{
    TerminationPolicy, child_pids, is_windows, process_exists, shell_command, sleep_command,
    terminate_tree, termination_policy_for,
};
pub use resources::{
    ProcessStats, ResourceLimits, available_physical_cores, current_rss_bytes,
    memory_limit_defaults, process_rss_bytes, process_stats, thread_budget,
};
pub use term::{is_window_minimized, stdin_is_interactive, terminal_size};
