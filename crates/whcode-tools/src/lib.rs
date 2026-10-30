//! # whcode-tools（ADR-048：P0-2 工具系统）
//!
//! 工具注册表、输入校验、工作区路径绑定与内建工具六件套（FR-725—726、FR-729）。
//!
//! 分层边界（`test_layering`）：本 crate 只依赖 `whcode-domain`（叶子）；文件操作用
//! `std::fs`（无平台条件编译，平台差异归 `whcode-platform`）；**不做持久化**——
//! 工具执行的三步登记协议（prepared→running→终态 + `recover_uncertain`，§0.3-8 不承诺
//! exactly-once；FR-410 unknown 语义）由 `whcode-app` 经 `whcode-store` 公共 API 落库
//! （ADR-018：SQL 只在 store；登记接线属 T215）。
//!
//! 有界性（NFR-215）：[`MAX_TOOL_OUTPUT_BYTES`] 输出上限、读文件 1 MiB 上限
//! （[`builtins::MAX_READ_BYTES`]）、遍历目录数与结果条目上限；全部同步执行，不新增线程。

pub mod builtins;
pub mod paths;
pub mod registry;

/// 工具输出正文上限（64 KiB；超限截断并显式标记，FR-728/R-05 对策）。
pub const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;

pub use builtins::default_registry;
pub use registry::{
    ParamType, SideEffectScope, Tool, ToolDescriptor, ToolOutput, ToolParameter, ToolRegistry,
    validate_arguments,
};
