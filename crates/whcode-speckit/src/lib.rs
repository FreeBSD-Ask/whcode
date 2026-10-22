//! # whcode-speckit
//!
//! SDD 工作流配置、Markdown 解析、产物校验、双向追溯与门禁（§4.3）。
//!
//! 已交付（M4，T068—T077）：
//! - [`workflow`]：版本化工作流配置与阶段定义（应用层状态机，FR-302）；
//! - [`parser`]：Markdown 解析器（未解析段落/重复 ID/未识别字段报告，FR-309）与
//!   纠偏重试（默认 ≤2 次，仍失败 `blocked` 并保留原文，R-06）；
//! - [`constitution`]：Constitution 创建/导入、版本记录与变更审计（FR-301）；
//! - [`ids`]：需求/任务稳定 ID 与引用校验（FR-304）；
//! - [`trace`]：双向追溯（任务 ↔ 需求/决策，断链可检出，FR-305）；
//! - [`analyze`]：五类缺陷检出（遗漏需求/孤儿任务/无效引用/循环依赖/覆盖不足，FR-306）；
//! - [`converge`]：验收条件四态判定与差异报告（FR-310/311，不静默标完成）。
//!
//! 边界：产物默认保留可读 Markdown，内部解析为版本化结构；**不得**假定上游 Spec Kit
//! 某一版本的命令、模板或插件是稳定 API（§4.3）；解析失败必须保留原文（R-06）。
//! 门禁判定结果类型见 `whcode_domain::gate`；阶段转换由应用服务执行（FR-308、T074）。

pub mod analyze;
pub mod constitution;
pub mod converge;
pub mod ids;
pub mod parser;
pub mod trace;
pub mod workflow;

pub use analyze::{AnalyzeReport, analyze};
pub use constitution::{Constitution, ConstitutionArticle};
pub use converge::{AcceptanceResult, AcceptanceStatus, ConvergeReport, Difference, converge};
pub use ids::{ID_PREFIXES, IdRegistry, normalize_id};
pub use parser::{
    Clarification, DEFAULT_CORRECTION_RETRIES, PARSER_VERSION, ParseBlocked, ParseReport,
    ParsedRequirement, ParsedTask, ensure_non_empty, parse_artifact, parse_with_retry,
};
pub use trace::Traceability;
pub use workflow::{SddStage, StageDefinition, WORKFLOW_VERSION, WorkflowConfig, default_workflow};
