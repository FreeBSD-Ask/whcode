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
pub mod catalogue;
pub mod paths;
pub mod plan;
pub mod present;
pub mod registry;
pub mod todo;

/// 工具输出正文上限（64 KiB；超限截断并显式标记，FR-728/R-05 对策）。
pub const MAX_TOOL_OUTPUT_BYTES: usize = 64 * 1024;

pub use builtins::{EDIT_FILE_TOOL, WRITE_FILE_TOOL, default_registry};
pub use paths::resolve_in_workspace;
pub use plan::{
    MAX_PLAN_GUIDANCE_CHARS, PLAN_GUIDANCE, PLAN_MODE_EXIT_RULE, PLAN_MODE_TOOL,
    plan_active_from_arguments,
};
pub use present::{
    MAX_DELIVERABLE_PATH_CHARS, MAX_DELIVERABLES_PER_CALL, PRESENT_DELIVERABLES_TOOL,
    entries_from_arguments as deliverable_entries_from_arguments,
};
pub use registry::{
    DEFAULT_TOOL_TIMEOUT_MS, MAX_TOOL_TIMEOUT_MS, ParamType, SideEffectScope, Tool, ToolDescriptor,
    ToolOutput, ToolParameter, ToolRegistry, effective_deadline, validate_arguments,
};
pub use todo::{MAX_TODO_CONTENT_CHARS, MAX_TODO_ITEMS, TODO_WRITE_TOOL, snapshot_from_arguments};
