//! # whcode-runtime
//!
//! 运行时 provider、payload 管理与 worker 子进程协议（§4.7、ADR-008）。
//!
//! 已交付（M8，T100—T113）：
//! - [`provider`]：RuntimeProvider 接口（FR-701 六要素）；
//! - [`capability`]：能力等级 Full/Restricted/ShellOnly（FR-702）与运行时声明；
//! - [`payload`]：payload 独立分发校验（来源/版本/哈希/平台，FR-703/D-07）；
//! - [`worker`]：受控子进程（超时/输出上限/退出码/stderr 尾部，FR-705）；
//! - [`checkpoint`]：执行前检查点四要素（FR-706/ADR-007）与触发策略（FR-405）；
//! - [`cache`]：重复执行缓存仅显式启用（FR-707）；
//! - [`errors`]：能力不足结构化错误与未启用运行时检出（FR-708/710、V-306）；
//! - [`providers`]：js/python/gitbash/bsdshell 四 Provider（诚实能力门控，
//!   D-08/D-09、§4.7.1）。
//!
//! **当前状态：默认不启用任何运行时（FR-704：`default = []` 即构建期表达）。**
//! `js` 无内置引擎（D-08 诚实不可用）；`python` 经系统解释器子进程执行；
//! `gitbash` payload 校验已交付（V-304）；`bsdshell` 属 M7 延后。
//!
//! 硬约束：
//! - 所有脚本在**受控子进程**中执行；主进程不得直接嵌入不受信任脚本执行（ADR-008）；
//! - 未启用运行时不加载动态库、不占端口、不注册可执行工具（FR-704）——`default = []`
//!   即为此约束的构建期表达；
//! - 能力等级 `Full`/`Restricted`/`ShellOnly` 必须在界面与门禁中可见（FR-702）；
//! - 命名边界：Boa ≠ 完整 Node.js；RustPython ≠ 完整 Python；Windows Git Bash 与
//!   BSD Shell 是不同能力（§4.7.1）。
//!
//! Provider 契约见 `specs/001-whcode-harness/contracts/runtime-protocol.md`。
//! feature 门控（T103/FR-704）：未启用运行时不加载动态库、不占端口、不注册工具——
//! 由 `default = []` feature 表达，启用须通过能力测试。

pub mod cache;
pub mod capability;
pub mod checkpoint;
pub mod errors;
pub mod gitbash;
pub mod js;
pub mod payload;
pub mod provider;
pub mod providers;
pub mod python;
pub mod worker;

pub use capability::{Capability, RuntimeDeclaration};
pub use checkpoint::{
    Checkpoint, CheckpointPolicy, CheckpointState, CheckpointTrigger, retain_recent,
};
pub use errors::{UnavailableReason, requires_unavailable_runtime, runtime_unavailable};
pub use payload::{PayloadMeta, SourceAllowlist, fnv64, validate};
pub use provider::{RuntimeProvider, Script, ensure_available};
pub use providers::{BsdShellProvider, GitBashProvider, JsProvider, PythonProvider};
pub use worker::{
    ConcurrencyGate, DEFAULT_MAX_CONCURRENT, GatePermit, MAX_OUTPUT_BYTES, WorkerResult, execute,
    execute_with_cancel,
};
