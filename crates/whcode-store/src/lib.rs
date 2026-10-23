//! # whcode-store
//!
//! DuckDB schema、迁移、单写者服务、读取连接、事务、清理与导出（§4.5、§7）。
//!
//! **当前状态：M1 存储基座起步。** DuckDB 原生依赖已按 T022 引入，采用**官方预编译动态库**
//! （非源码构建）：由 `ci/fetch-duckdb.py` 拉取并校验 SHA256，构建期经 `DUCKDB_LIB_DIR`
//! 指向 `vendor/duckdb/<version>/`（不入库）。平台结论见 `docs/adr/ADR-DB-BUILD.md`。
//!
//! 已交付：
//! - [`smoke`]：M0 数据库冒烟（创建 / 事务写读 / 关闭重启 / 迁移，T022、V-002）；
//! - [`writer`]：事件写者引擎（幂等键 / 批次阈值 / 关键事件确认 / 错误传播，T026、V-005）；
//! - [`schema`]：14 表版本化 schema 与可重复迁移（T032/T033；v2/v3 查询索引；FR-501）；
//! - [`service`]：写者服务（单写线程 + 有界通道 + 同步确认，T034、FR-502/503、ADR-017）；
//! - [`query`]：稳定全序 + 双向游标分页（T035、FR-504、§7.2）；
//! - [`indexes`]：必需索引注册表与完整性检查（T036、§7.2）；
//! - [`overview`]：会话概览预聚合与重建（T037、FR-506、§7.3-8）；
//! - [`monitor`]：数据库统计、表规模、WAL 与写者队列监控（T038、FR-507）；
//! - [`purge`]：会话归档、按时间清理与 TTL，含预览/确认/审计/活跃保护（T039、FR-508/509）；
//! - [`export`]：工作区导出/导入，清单、分块校验和与兼容/冲突检查（T040、FR-508、§7.4）；
//! - [`compact`]：复制—校验—原子切换与四项空间统计（T041、FR-510、R-09/D-10）；
//! - [`wal`]：WAL 阈值策略、显式与空闲窗口 checkpoint（T042、§5.4）；
//! - [`directory`] / [`tasks`]：会话目录与任务投影存储原语（M2/T046—T052 支撑、ADR-018）；
//! - [`tool_exec`]：工具执行“意图—执行—登记”与不确定恢复（T114、FR-409/410、ADR-007）。
//!
//! M1 存储基座（T032—T042）已在本 crate 交付；跨进程/跨连接的验收集成测试
//! （T043—T045）位于仓库 `tests/integration/`（whcode-tests）。
//!
//! 边界（ADR-002）：**唯一**接受应用数据库写命令的组件；其他 crate 不得自行打开写连接。
//! SQL 只在本 crate 内出现（ADR-018）；`whcode-app` 经本 crate 公共 API 编排命令。

pub mod compact;
pub mod directory;
pub mod export;
pub mod indexes;
pub mod monitor;
pub mod overview;
pub mod purge;
pub mod query;
pub mod schema;
pub mod service;
pub mod smoke;
pub mod tasks;
pub mod tool_exec;
pub mod wal;
pub mod writer;

/// 测试临时目录（仅测试构建；见模块文档）。
#[cfg(test)]
mod test_util;

pub use compact::{CompactReport, SpaceReport, capture_space, compact_database};
pub use directory::{
    MAX_LIST_LIMIT, SessionRow, create_session, create_session_with_id, event_cursor, get_session,
    list_sessions, rename_session, touch_session, update_stage,
};
pub use export::{
    EXCLUDED_TABLES, EXPORT_FORMAT, EXPORT_FORMAT_VERSION, EXPORT_TABLES, ExportFile,
    ExportManifest, ExportReport, ImportReport, ROWS_PER_CHUNK, export_workspace, import_workspace,
};
pub use indexes::{
    REQUIRED_INDEXES, RequiredIndex, installed_indexes, missing_indexes, verify_indexes,
};
pub use monitor::{
    DatabaseStats, MonitorReport, PlanRow, QueryPlan, TableStat, WalStatus, WriterStats, collect,
    database_stats, plan_query, table_stats, wal_status,
};
pub use overview::{
    OVERVIEW_EVENTUAL_FIELDS, OVERVIEW_STRONG_FIELDS, SessionOverview, compute_overview,
    load_overview, rebuild_all_overviews, rebuild_overview,
};
pub use purge::{
    DEFAULT_ACTIVE_WINDOW_MS, IDS_PER_QUERY, OnProtected, PurgeCounts, PurgeCriteria,
    PurgeCriterion, PurgePreview, PurgeProtection, PurgeResult, execute_purge, preview_purge,
    set_archived,
};
pub use query::{
    Direction, EventCursor, EventPage, EventRow, MAX_PAGE_SIZE, MessageCursor, MessagePage,
    MessageRow, events_page, latest_seq, messages_page,
};
pub use schema::{STORE_SCHEMA_VERSION, TABLE_COUNT, apply_migrations, checksum, schema_version};
pub use service::WriterService;
pub use smoke::{SMOKE_STEPS, SmokeReport, SmokeStep, engine_version};
pub use tasks::{
    ArtifactRefRow, CheckpointRow, MAX_ACTIVE_TASKS, MAX_DETAIL_CHECKPOINTS, MAX_RECENT_EVENTS,
    TaskRow, active_tasks, get_task, insert_task, recent_task_events, stage_distribution,
    status_distribution, task_artifacts, task_checkpoints, update_task_status,
};
pub use tool_exec::{
    MAX_RECOVERED, RecoveredExec, RecoveryReport, ToolExecRow, UNCERTAIN_NEXT_ACTION, load,
    mark_running, record_intent, record_result, recover_uncertain,
};
pub use wal::{
    CheckpointReason, CheckpointReport, EffectiveWalSettings, WalPolicy, apply_wal_policy,
    run_checkpoint, should_checkpoint_on_idle,
};
pub use writer::{EventWriter, WriteOutcome, WriterConfig};
