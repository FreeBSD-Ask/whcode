//! # SDD 门禁命令面（T394／FR-301—314、ADR-089 批 5）
//!
//! `spec.validate`／`spec.analyze`／`spec.converge`／`spec.next`／
//! `spec.force-advance` 的应用服务编排：工作区 `specs/` 产物装载 →
//! [`whcode_speckit::parse_artifact`] 解析 → 门禁判定。四端同源
//! （FR-314：TUI／CUI／Web 显示同一份阶段状态、门禁结果与产物版本）。
//!
//! ## 产物布局（v1 约定，ADR-089 已知限制）
//!
//! 平面目录 `<workspace>/specs/`：`constitution.md`、`spec.md`、`plan.md`、
//! `tasks.md`。缺失文件是**显式状态**（`present = false`），不是解析错误；
//! 自定义工作流配置文件装载不在本批（内置 `default_workflow` 承载 FR-302 的
//! 版本化配置）。
//!
//! ## 门禁判定来源（FR-313 禁止推断）
//!
//! [`TransitionContext`] 的三个输入全部来自解析产物，不从历史文本推断：
//! 未解决高优澄清项来自 `spec.md` 的 `[priority: high]` 未勾选澄清；
//! Analyze 通过性来自 [`analyze`] 的五类缺陷检出；Constitution 满足性 v1
//! 只验存在性（深度校验待 ADR，见模块文档与 ADR-089 限制 3）。

use std::path::Path;

use duckdb::Connection;
use whcode_domain::{AuditAction, AuditRecord, ErrorCode, WhError};
use whcode_speckit::parser::parse_artifact;
use whcode_speckit::workflow::{SddStage, default_workflow};
use whcode_speckit::{
    AcceptanceResult, AcceptanceStatus, IdRegistry, ParseReport, analyze, converge,
};
use whcode_store::directory::{self, SessionRow};
use whcode_store::tasks::{TaskRow, tasks_of_session};

use crate::approval::StoreAuditSink;
use crate::authz::{Actor, Capability, Policy, require};
use crate::gates::{TransitionContext, force_advance, request_transition};

/// 工作区 SDD 产物原文（`None`＝文件缺失，显式状态）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceArtifacts {
    /// Constitution（FR-301）。
    pub constitution: Option<String>,
    /// 需求基线。
    pub spec: Option<String>,
    /// 技术方案。
    pub plan: Option<String>,
    /// 任务分解。
    pub tasks: Option<String>,
}

impl WorkspaceArtifacts {
    /// 从 `<workspace>/specs/` 装载四产物（缺失即 `None`，不报错）。
    ///
    /// # Errors
    ///
    /// 目录存在但文件不可读（`io_error`）时返回 [`WhError`]。
    pub fn load(workspace: &Path) -> Result<Self, WhError> {
        let read = |name: &str| -> Result<Option<String>, WhError> {
            let path = workspace.join("specs").join(name);
            if !path.exists() {
                return Ok(None);
            }
            std::fs::read_to_string(&path).map(Some).map_err(|err| {
                WhError::new(
                    ErrorCode::IoError,
                    format!("读取 {} 失败：{err}", path.display()),
                )
            })
        };
        Ok(Self {
            constitution: read("constitution.md")?,
            spec: read("spec.md")?,
            plan: read("plan.md")?,
            tasks: read("tasks.md")?,
        })
    }
}

/// 单产物解析视图（`validate` 输出的一行；四端同源）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArtifactParseView {
    /// 产物稳定名（与工作流 `required_artifacts` 同一口径）。
    pub name: &'static str,
    /// 文件是否存在。
    pub present: bool,
    /// 解析是否干净（无未解析段落／重复 ID／未识别字段，FR-309）。
    pub clean: bool,
    /// 异常摘要（纠偏错误，原文保留在产物文件本身）。
    pub anomalies: Vec<String>,
    /// 解析出的需求数。
    pub requirements: usize,
    /// 解析出的任务数。
    pub tasks: usize,
    /// 未解决高优澄清项数。
    pub unresolved_high_priority: usize,
}

/// `spec.validate` 输出（FR-302/303：必需产物存在且解析干净）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidateView {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 会话。
    pub session_id: String,
    /// 当前阶段（稳定名）。
    pub stage: String,
    /// 本阶段必需产物的解析视图。
    pub artifacts: Vec<ArtifactParseView>,
    /// 总判定（全部必需产物 present 且 clean）。
    pub passed: bool,
}

/// `spec.analyze` 输出（FR-306：五类缺陷检出 + 门禁结果）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AnalyzeView {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 遗漏需求（无任务覆盖）。
    pub missing_requirements: Vec<String>,
    /// 孤儿任务（无需求与计划引用）。
    pub orphan_tasks: Vec<String>,
    /// 无效引用（任务, 引用）。
    pub invalid_refs: Vec<(String, String)>,
    /// 循环依赖（环上任务链）。
    pub cycles: Vec<Vec<String>>,
    /// 覆盖不足（无验收条件的任务）。
    pub uncovered_acceptance: Vec<String>,
    /// 总判定（五类皆空）。
    pub passed: bool,
}

/// `spec.converge` 输出（FR-310/311：逐条验收条件判定 + 差异报告）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConvergeView {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 会话。
    pub session_id: String,
    /// pass 条件数。
    pub passed: u64,
    /// fail 条件数。
    pub failed: u64,
    /// not_tested（含 not_applicable）条件数。
    pub not_tested: u64,
    /// 差异行（任务、条件、差距；不以口头通过结项）。
    pub differences: Vec<ConvergeDifference>,
    /// 总判定（零差异）。
    pub converged: bool,
}

/// 差异报告一行（FR-310/311）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ConvergeDifference {
    /// 任务 ID。
    pub task_id: String,
    /// 验收条件。
    pub criterion: String,
    /// 状态稳定名。
    pub status: String,
    /// 差距说明。
    pub gap: String,
}

fn invalid(message: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.to_string())
}

/// 解析单个产物原文；空产物按“无条目”处理（不报错——validate 报缺失）。
fn parse_optional(text: Option<&str>) -> ParseReport {
    match text {
        Some(text) => parse_artifact(text),
        None => ParseReport::default(),
    }
}

/// 汇总解析三产物为逐产物视图（constitution 只验存在性，见模块文档）。
fn parse_views(artifacts: &WorkspaceArtifacts) -> Vec<ArtifactParseView> {
    let mut views = Vec::new();
    let constitution = artifacts.constitution.as_deref();
    views.push(ArtifactParseView {
        name: "constitution",
        present: constitution.is_some(),
        clean: true,
        anomalies: Vec::new(),
        requirements: 0,
        tasks: 0,
        unresolved_high_priority: 0,
    });
    for (name, text) in [
        ("spec", artifacts.spec.as_deref()),
        ("plan", artifacts.plan.as_deref()),
        ("tasks", artifacts.tasks.as_deref()),
    ] {
        let report = parse_optional(text);
        let unresolved_high_priority = report
            .clarifications
            .iter()
            .filter(|entry| entry.high_priority && !entry.resolved)
            .count();
        views.push(ArtifactParseView {
            name,
            present: text.is_some(),
            clean: report.is_clean(),
            anomalies: report.anomaly_summary(),
            requirements: report.requirements.len(),
            tasks: report.tasks.len(),
            unresolved_high_priority,
        });
    }
    views
}

/// 校验当前阶段必需产物（FR-302/303；`spec.validate`，T394）。
///
/// # Errors
///
/// 权限拒绝、会话不存在或存储失败时返回 [`WhError`]。
pub fn validate(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    workspace: &Path,
) -> Result<ValidateView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let session = directory::get_session(conn, session_id)?.ok_or_else(|| not_found(session_id))?;
    let workflow = default_workflow();
    let stage = SddStage::parse(&session.stage)?;
    let required = workflow
        .stage(stage)
        .map(|definition| definition.required_artifacts)
        .unwrap_or(&[]);
    let views = parse_views(&WorkspaceArtifacts::load(workspace)?);
    let artifacts: Vec<ArtifactParseView> = views
        .into_iter()
        .filter(|view| required.contains(&view.name))
        .collect();
    let passed = artifacts.iter().all(|view| view.present && view.clean);
    Ok(ValidateView {
        schema_version: whcode_domain::SCHEMA_VERSION,
        session_id: session_id.to_string(),
        stage: stage.as_str().to_string(),
        artifacts,
        passed,
    })
}

/// 执行 Analyze（FR-306；`spec.analyze`，T394）。
///
/// 需求与澄清来自 `spec.md`、任务来自 `tasks.md`；`plan.md` 的决策引用经
/// `IdRegistry` 一并登记（无效引用检出含 plan 引用）。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn analyze_view(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    workspace: &Path,
) -> Result<AnalyzeView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let session = directory::get_session(conn, session_id)?.ok_or_else(|| not_found(session_id))?;
    let _ = session;
    let artifacts = WorkspaceArtifacts::load(workspace)?;
    let spec = parse_optional(artifacts.spec.as_deref());
    let plan = parse_optional(artifacts.plan.as_deref());
    let tasks_report = parse_optional(artifacts.tasks.as_deref());

    // 三产物条目合并（同一解析器、同一 ID 口径；plan.md 的任务/澄清段一并计入）。
    let mut requirements = spec.requirements.clone();
    requirements.extend(plan.requirements);
    let mut tasks = tasks_report.tasks.clone();
    tasks.extend(plan.tasks);
    let mut clarifications = spec.clarifications;
    clarifications.extend(plan.clarifications);
    clarifications.extend(tasks_report.clarifications);

    let mut registry = IdRegistry::new();
    for id in requirements
        .iter()
        .map(|entry| entry.id.clone())
        .chain(tasks.iter().map(|entry| entry.id.clone()))
    {
        registry
            .register(&id)
            .map_err(|err| invalid(&format!("产物 ID 注册失败：{err}")))?;
    }
    let report = analyze(&requirements, &tasks, &clarifications, &registry);
    let passed = report.passed();
    Ok(AnalyzeView {
        schema_version: whcode_domain::SCHEMA_VERSION,
        missing_requirements: report.missing_requirements,
        orphan_tasks: report.orphan_tasks,
        invalid_refs: report.invalid_refs,
        cycles: report.cycles,
        uncovered_acceptance: report.uncovered_acceptance,
        passed,
    })
}

/// 汇总会话任务的验收判定（FR-310/311；`spec.converge`，T394）。
///
/// 判定口径与 [`crate::overview`] 的 diff 视图同源（任务行状态 → 三态映射、
/// 证据引用只指向任务行事实）；本命令为只读计算，差异报告每次重算
/// （ADR-089 已知限制：报告不落新表，持久化呈现由 diff 视图与事件流承载）。
///
/// # Errors
///
/// 权限拒绝、会话不存在或存储失败时返回 [`WhError`]。
pub fn converge_view(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<ConvergeView, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(not_found(session_id));
    }
    let rows: Vec<TaskRow> = tasks_of_session(conn, session_id, 10_000)?;
    let mut view = ConvergeView {
        schema_version: whcode_domain::SCHEMA_VERSION,
        session_id: session_id.to_string(),
        passed: 0,
        failed: 0,
        not_tested: 0,
        differences: Vec::new(),
        converged: true,
    };
    for row in rows {
        let criteria: Vec<String> = match serde_json::from_str(&row.acceptance_json) {
            Ok(criteria) => criteria,
            Err(err) => {
                view.failed += 1;
                view.converged = false;
                view.differences.push(ConvergeDifference {
                    task_id: row.id.clone(),
                    criterion: "acceptance_json".to_string(),
                    status: "fail".to_string(),
                    gap: format!("验收条件 JSON 损坏：{err}"),
                });
                continue;
            }
        };
        let status = match row.status {
            whcode_domain::TaskStatus::Completed => AcceptanceStatus::Pass,
            whcode_domain::TaskStatus::Failed | whcode_domain::TaskStatus::Aborted => {
                AcceptanceStatus::Fail
            }
            _ => AcceptanceStatus::NotTested,
        };
        let evidence_ref = match status {
            AcceptanceStatus::Pass => Some(format!("task:{}:completed", row.id)),
            AcceptanceStatus::Fail => Some(format!("task:{}:{}", row.id, row.status.as_str())),
            _ => None,
        };
        let results: Vec<AcceptanceResult> = criteria
            .iter()
            .map(|criterion| AcceptanceResult {
                criterion: criterion.clone(),
                status,
                evidence_ref: evidence_ref.clone(),
            })
            .collect();
        let report = match converge(&row.id, &criteria, &results) {
            Ok(report) => report,
            Err(err) => {
                view.failed += 1;
                view.converged = false;
                view.differences.push(ConvergeDifference {
                    task_id: row.id.clone(),
                    criterion: "acceptance_json".to_string(),
                    status: "fail".to_string(),
                    gap: err.to_string(),
                });
                continue;
            }
        };
        for result in &report.results {
            match result.status {
                AcceptanceStatus::Pass => view.passed += 1,
                AcceptanceStatus::Fail => view.failed += 1,
                AcceptanceStatus::NotTested | AcceptanceStatus::NotApplicable => {
                    view.not_tested += 1;
                }
            }
        }
        for difference in report.differences {
            view.converged = false;
            view.differences.push(ConvergeDifference {
                task_id: difference.task_id,
                criterion: difference.criterion,
                status: difference.status.as_str().to_string(),
                gap: difference.gap,
            });
        }
    }
    Ok(view)
}

/// 请求阶段推进（FR-307/308；`spec.next`，T394）。
///
/// `to` 缺省沿状态机成功转换推进；门禁输入（澄清／Analyze／Constitution）
/// 由解析产物显式提供。非法转换与门禁失败保持 `gate_failed`／`invalid_state`
/// 语义（与 [`crate::gates`] 一致）。
///
/// # Errors
///
/// 权限拒绝、会话不存在、无后继阶段、门禁未通过或存储失败时返回 [`WhError`]。
pub fn next(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    workspace: &Path,
    to: Option<&str>,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SpecAdvance)?;
    let session = directory::get_session(conn, session_id)?.ok_or_else(|| not_found(session_id))?;
    let workflow = default_workflow();
    let from = SddStage::parse(&session.stage)?;
    let target = match to {
        Some(to) => SddStage::parse(to)?,
        None => workflow.next_stage(from).ok_or_else(|| {
            invalid(&format!(
                "阶段 {from} 无后继（状态机终态）；用 spec force-advance 显式放行或核对状态机"
            ))
        })?,
    };
    let artifacts = WorkspaceArtifacts::load(workspace)?;
    let context = transition_context(&artifacts)?;
    request_transition(conn, policy, actor, session_id, &workflow, target, &context)
}

/// 人工强制越过门禁（FR-312；`spec.force-advance`，T394）。
///
/// `reason` 必填；审计含 actor／reason／跳过的门禁／产物版本／时间戳
/// （审计失败时转换不得生效）。
///
/// # Errors
///
/// 权限拒绝、`reason` 为空、非法转换或审计失败时返回 [`WhError`]。
#[allow(clippy::too_many_arguments)]
pub fn force_advance_view(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    workspace: &Path,
    to: &str,
    reason: &str,
    now_ms: u64,
) -> Result<SessionRow, WhError> {
    require(policy, actor, Capability::SpecAdvance)?;
    let workflow = default_workflow();
    let to = SddStage::parse(to)?;
    let artifact_version = artifact_version(workspace)?;
    let command = crate::gates::ForceAdvance {
        session_id,
        to,
        reason,
        artifact_version,
    };
    let sink = StoreAuditSink::new(conn);
    force_advance(conn, policy, actor, &workflow, &command, &sink, now_ms)
}

/// 门禁输入从解析产物显式提供（FR-313 禁止推断）。
fn transition_context(artifacts: &WorkspaceArtifacts) -> Result<TransitionContext, WhError> {
    let spec = parse_optional(artifacts.spec.as_deref());
    let tasks_report = parse_optional(artifacts.tasks.as_deref());
    let mut unresolved_high_priority: Vec<String> = spec
        .clarifications
        .iter()
        .filter(|entry| entry.high_priority && !entry.resolved)
        .map(|entry| entry.id.clone())
        .collect();
    unresolved_high_priority.extend(
        tasks_report
            .clarifications
            .iter()
            .filter(|entry| entry.high_priority && !entry.resolved)
            .map(|entry| entry.id.clone()),
    );
    // Analyze 通过性：对当前产物集跑同一五类缺陷检出（与 spec.analyze 同一 fold）。
    let analysis = analyze_view_of(&spec, &tasks_report, artifacts)?;
    let constitution_passed = artifacts.constitution.is_some();
    Ok(TransitionContext {
        unresolved_high_priority,
        analyze_passed: analysis,
        constitution_passed,
    })
}

/// Analyze 通过性（内部：不经权限与会话查询的纯产物计算）。
fn analyze_view_of(
    spec: &ParseReport,
    tasks_report: &ParseReport,
    artifacts: &WorkspaceArtifacts,
) -> Result<bool, WhError> {
    let plan = parse_optional(artifacts.plan.as_deref());
    let mut requirements = spec.requirements.clone();
    requirements.extend(plan.requirements);
    let mut tasks = tasks_report.tasks.clone();
    tasks.extend(plan.tasks);
    let mut clarifications = spec.clarifications.clone();
    clarifications.extend(plan.clarifications);
    clarifications.extend(tasks_report.clarifications.clone());
    let mut registry = IdRegistry::new();
    for id in requirements
        .iter()
        .map(|entry| entry.id.clone())
        .chain(tasks.iter().map(|entry| entry.id.clone()))
    {
        registry
            .register(&id)
            .map_err(|err| invalid(&format!("产物 ID 注册失败：{err}")))?;
    }
    Ok(analyze(&requirements, &tasks, &clarifications, &registry).passed())
}

/// 产物版本（FR-312 审计要素）：四产物字节数之和取模（可复现、不含时间）。
fn artifact_version(workspace: &Path) -> Result<u32, WhError> {
    let artifacts = WorkspaceArtifacts::load(workspace)?;
    let mut total: u64 = 0;
    for text in [
        artifacts.constitution.as_deref(),
        artifacts.spec.as_deref(),
        artifacts.plan.as_deref(),
        artifacts.tasks.as_deref(),
    ] {
        total += u64::try_from(text.map_or(0, str::len)).unwrap_or(0);
    }
    Ok(u32::try_from(total % 4_096).unwrap_or(0))
}

fn not_found(session_id: &str) -> WhError {
    WhError::new(
        ErrorCode::NotFound,
        format!("会话不存在：{session_id}；用 session list 查看现存会话"),
    )
}

/// 审计动作引用（供 force-advance 审计行文）；本模块不直接写审计，
/// 引入只为类型可见性说明。
const _: AuditAction = AuditAction::GateOverride;

#[allow(dead_code)]
fn audit_record_shape() -> AuditRecord {
    unreachable!("形状说明用：审计由 gates::force_advance 经 AuditSink 写入")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use std::fs;
    use whcode_store::{apply_migrations, create_session_with_id};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn workspace_with(files: &[(&str, &str)]) -> std::path::PathBuf {
        // 原子计数命名：并行测试不得共享临时目录（同名互删曾是竞态源）。
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("whcode-spec-{}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("specs")).expect("建 specs 目录");
        for (name, content) in files {
            fs::write(dir.join("specs").join(name), content).expect("写产物");
        }
        dir
    }

    /// 解析器真实格式：`- **REQ-xxx**: 文本`／`- [x] **CLAR-xxx**: 文本 [priority: high]`；
    /// 稳定 ID 前缀白名单 REQ-/T-/DEC-/CLAR-（FR-304）。
    const SPEC_MD: &str = "# Spec\n\n## Requirements\n\n- **REQ-001**: 需求甲\n\n## Clarifications\n\n- [x] **CLAR-001**: 已解决 [priority: high]\n";
    /// 任务注解经 `(refs: …)`／`(acceptance: …)` 传入（annotations 键集）。
    const TASKS_MD: &str =
        "# Tasks\n\n## Tasks\n\n- [ ] T-001: 任务甲 (refs: REQ-001) (acceptance: 存在可执行断言)\n";

    /// T394／validate：必需产物缺失或解析不干净判 fail；齐备且干净判 pass。
    #[test]
    fn validate_checks_required_artifacts_for_current_stage() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "门禁", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        // specify 阶段必需 spec.md：缺失 → fail，且显式 present=false。
        let empty = workspace_with(&[]);
        let view = validate(&conn, &policy, &actor, "S-1", &empty).expect("校验");
        assert_eq!(view.stage, "specify");
        assert!(!view.passed);
        assert!(
            view.artifacts
                .iter()
                .any(|a| a.name == "spec" && !a.present)
        );

        // 提供干净 spec.md → pass。
        let full = workspace_with(&[("spec.md", SPEC_MD)]);
        let view = validate(&conn, &policy, &actor, "S-1", &full).expect("校验");
        assert!(view.passed, "{view:?}");

        // 产物含未知段落（解析不静默丢弃，FR-309）→ fail。
        let dirty = workspace_with(&[("spec.md", "# Spec\n\n## 未知段\n\n内容行\n")]);
        let view = validate(&conn, &policy, &actor, "S-1", &dirty).expect("校验");
        assert!(!view.passed, "解析不干净必须 fail");
        let _ = fs::remove_dir_all(&empty);
        let _ = fs::remove_dir_all(&full);
        let _ = fs::remove_dir_all(&dirty);
    }

    /// T394／analyze：遗漏需求与孤儿任务全检出（V-203 形状的单元级对应）。
    #[test]
    fn analyze_detects_missing_requirements_and_orphan_tasks() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "分析", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        // T-001 只引用 REQ-001；REQ-002 无任务覆盖；T-002 无引用（孤儿）。
        let spec = "# Spec\n\n## Requirements\n\n- **REQ-001**: 需求甲\n- **REQ-002**: 需求乙\n";
        let tasks = "# Tasks\n\n## Tasks\n\n- [ ] T-001: 任务甲 (refs: REQ-001) (acceptance: 断言)\n- [ ] T-002: 孤儿任务 (acceptance: 断言)\n";
        let dir = workspace_with(&[("spec.md", spec), ("tasks.md", tasks)]);
        let view = analyze_view(&conn, &policy, &actor, "S-1", &dir).expect("分析");
        assert!(view.missing_requirements.contains(&"REQ-002".to_string()));
        assert!(view.orphan_tasks.contains(&"T-002".to_string()));
        assert!(!view.passed, "五类缺陷必须判不通过");
        let _ = fs::remove_dir_all(&dir);
    }

    /// T394／next：未解决高优澄清阻止 Plan（V-202）；解析干净则放行。
    #[test]
    fn next_blocked_by_unresolved_clarifications_then_advances() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "推进", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let blocked_spec =
            "# Spec\n\n## Clarifications\n\n- [ ] **CLAR-001**: 未解决 [priority: high]\n";
        let dir = workspace_with(&[("spec.md", blocked_spec), ("tasks.md", TASKS_MD)]);
        // specify → clarify（无门禁），clarify → plan 触发 V-202 高优澄清门禁。
        next(&conn, &policy, &actor, "S-1", &dir, None).expect("先推进到 clarify");
        let err = next(&conn, &policy, &actor, "S-1", &dir, Some("plan"))
            .expect_err("高优澄清必须阻止 Plan");
        assert_eq!(err.code, ErrorCode::GateFailed);
        let _ = fs::remove_dir_all(&dir);
    }

    /// T394／converge：completed 通过、failed 进差异（V-205 不得静默完成）。
    #[test]
    fn converge_reports_differences_for_failed_criteria() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "收敛", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        whcode_store::insert_task(
            &conn,
            &whcode_store::tasks::NewTask {
                id: "T-C",
                session_id: "S-1",
                title: "完成",
                stage: "implement",
                parent_id: None,
                acceptance_json: "[\"条件甲\"]",
            },
            1,
        )
        .expect("建任务");
        whcode_store::update_task_status(&conn, "T-C", whcode_domain::TaskStatus::Completed, 0, 1)
            .expect("完成");
        whcode_store::insert_task(
            &conn,
            &whcode_store::tasks::NewTask {
                id: "T-F",
                session_id: "S-1",
                title: "失败",
                stage: "implement",
                parent_id: None,
                acceptance_json: "[\"条件乙\"]",
            },
            2,
        )
        .expect("建任务");
        whcode_store::update_task_status(&conn, "T-F", whcode_domain::TaskStatus::Failed, 0, 2)
            .expect("失败");

        let view = converge_view(&conn, &policy, &actor, "S-1").expect("收敛");
        assert_eq!(view.passed, 1);
        assert_eq!(view.failed, 1);
        assert!(!view.converged, "有失败必须不收敛");
        assert!(
            view.differences
                .iter()
                .any(|d| d.task_id == "T-F" && d.status == "fail")
        );
    }

    /// T394／force-advance：缺 reason 拒绝；带 reason 落审计并推进（FR-312）。
    #[test]
    fn force_advance_requires_reason_and_audits() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "放行", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let dir = workspace_with(&[("spec.md", SPEC_MD)]);

        let err = force_advance_view(&conn, &policy, &actor, "S-1", &dir, "clarify", "  ", 1)
            .expect_err("空 reason 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let row = force_advance_view(
            &conn,
            &policy,
            &actor,
            "S-1",
            &dir,
            "clarify",
            "用户显式放行",
            1,
        )
        .expect("放行");
        assert_eq!(row.stage, "clarify");
        let audits: i64 = conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'gate_override'",
                [],
                |r| r.get(0),
            )
            .expect("审计计数");
        assert_eq!(audits, 1, "强制放行必须落一条审计");
        let _ = fs::remove_dir_all(&dir);
    }

    /// T394：权限拒绝（SpecAdvance）与缺失会话的 `not_found` 可区分。
    #[test]
    fn spec_commands_require_capability_and_existing_session() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "权限", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let weak = Actor::new("weak", []);
        let dir = workspace_with(&[("spec.md", SPEC_MD)]);

        let err =
            next(&conn, &policy, &weak, "S-1", &dir, None).expect_err("无 SpecAdvance 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);

        let err = validate(
            &conn,
            &policy,
            &actor_with_all_capabilities("t"),
            "S-404",
            &dir,
        )
        .expect_err("缺会话必须 not_found");
        assert_eq!(err.code, ErrorCode::NotFound);
        let _ = fs::remove_dir_all(&dir);
    }
}
