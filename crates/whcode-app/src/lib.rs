//! # whcode-app
//!
//! 统一应用服务：命令处理、权限检查、用例编排与状态快照生成。
//!
//! 边界（ADR-003/005/018）：TUI、CUI、Web、SSH **只**经本层的命令／快照／事件访问
//! 状态，不得直接触库；本 crate 不依赖任何前端，且**不内嵌 SQL**——存储访问一律经
//! [`whcode-store`] 的公共 API（ADR-018）。
//!
//! 已交付（M3，T060/T061/T065/T066）：
//! - [`context`]：上下文加载器契约（`items + next_cursor + consumed_budget +
//!   truncated_reason`，有界迭代，FR-105）与压缩摘要指针（压缩后原文仍可查，FR-107）；
//! - [`replay`]：按持久化游标的补拉与重连（FR-202/605）；
//! - [`events`]：事件导出（脱敏策略随导出注明，FR-205；CUI 复用——前端禁止触库）。
//!
//! 已交付（M2，T046—T052）：
//! - [`authz`]：能力模型与统一拒绝语义（`permission_denied`、不可重试，FR-604）；
//! - [`session`]：会话命令 new/list/show/rename/archive/delete（FR-101）与会话切换
//!   的有界加载（元数据 + 概览 + 有界尾部 + 游标，FR-102、V-101）；
//! - [`overview`]：任务总览——阶段分布、完成度、阻塞、活跃任务（FR-103，强一致）；
//! - [`task`]：任务详情五类信息（FR-104）与 retry/abort 状态机（FR-401/408）；
//! - [`snapshot`]：四端同源状态快照（FR-314；M2 未覆盖字段显式标注，见模块文档）；
//! - [`queue`]：有界队列与满载语义（T015，基础设施）。
//!
//! 待交付：SDD 门禁编排（M4，T068+）。

pub mod authz;
pub mod commands;
pub mod context;
pub mod events;
pub mod fonts;
pub mod gates;
pub mod maintenance;
pub mod overview;
pub mod queue;
pub mod replay;
pub mod residency;
pub mod session;
pub mod snapshot;
pub mod stage_role;
pub mod surface;
pub mod task;
pub mod view_source;
pub mod views;

pub use authz::{Actor, Capability, DefaultPolicy, Policy, actor_with_all_capabilities, require};
pub use commands::{CommandExecutor, CommandOutcome, FrontendCommand, execute_command};
pub use context::{
    CONTEXT_PAGE_SIZE, ContextCompression, ContextItem, ContextWindow,
    DEFAULT_CONTEXT_BUDGET_TOKENS, REASON_BUDGET_EXHAUSTED, SummaryPointer, compress_context,
    load_context, load_summary_pointer,
};
pub use events::{EventsExport, events_export};
pub use gates::{ForceAdvance, TransitionContext, force_advance, request_transition};
pub use maintenance::{
    PurgeCounts, PurgeCriteria, PurgePreview, PurgeProtection, PurgeResult, purge_execute,
    purge_preview,
};
pub use overview::{TaskOverview, task_overview};
pub use queue::BoundedQueue;
pub use replay::{REPLAY_PAGE_LIMIT, ReplayPage, replay_since};
pub use residency::{ResidencyManager, ResidencyState, UnloadedSession, idle_unload_due};
pub use session::{
    LIST_SESSIONS_LIMIT, NewSession, SWITCH_TAIL_LIMIT, SessionView, delete_session, list_sessions,
    new_session, preview_delete_session, rename_session, set_session_archived, show_session,
    switch_session,
};
pub use snapshot::{StateSnapshot, build_snapshot};
pub use stage_role::{StageRole, role_for};
pub use task::{TaskDetail, abort_task, retry_task, task_detail};
pub use views::{purge_counts_value, session_value, task_value};
// 会话行类型（门面 `CommandOutcome::Session` 的载荷；前端据此渲染，无需依赖 store）。
pub use whcode_store::SessionRow;
