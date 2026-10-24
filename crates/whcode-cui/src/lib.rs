//! # whcode-cui
//!
//! 命令行界面：稳定命令树、明确退出码、版本化 JSON 输出（FR-602、§8.1）。
//!
//! 已交付（M3，T062/T065）：
//! - [`context_cmd`]：预算视图（当前预算 / 装入来源 / 截断原因，FR-108）；
//! - [`events_cmd`]：事件 JSON/JSONL 查询与导出，脱敏策略随导出注明（FR-205）。
//!
//! 契约（contracts/cui-cli.md）：所有 JSON 输出必须含 `schema_version`；错误对象必须含
//! `code`/`message`/`retryable`/`details`（类型见 `whcode_domain::error`）。不得暴露任意
//! SQL；`db explain` 只跑只读查询或预定义模板。
//!
//! 注意：本模块是应用服务的**展示包装**（前端禁止触库，ADR-003/005、C-03）；
//! clap 命令树属 M5（T084）；`--json` 信封与退出码属 T085；破坏性命令
//! dry-run/确认属 T086。

pub mod cli;
pub mod confirm;
pub mod context_cmd;
pub mod events_cmd;
pub mod output;
pub mod spec_cmd;
pub mod surface_map;

/// 测试夹具用迁移 SQL（include_str，不引入 store 依赖——分层门禁禁止前端触库）。
#[cfg(test)]
pub const TEST_MIGRATIONS: &str = concat!(
    include_str!("../../whcode-store/migrations/0001_initial.sql"),
    include_str!("../../whcode-store/migrations/0002_event_trace_indexes.sql"),
    include_str!("../../whcode-store/migrations/0003_query_indexes.sql"),
    include_str!("../../whcode-store/migrations/0004_session_id_sequence.sql"),
    include_str!("../../whcode-store/migrations/0005_session_stage.sql"),
    include_str!("../../whcode-store/migrations/0006_events_variant.sql"),
);
