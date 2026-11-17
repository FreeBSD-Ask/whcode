//! # 任务充分性计算的应用服务（FR-795／ADR-085）
//!
//! 把“要求—检查点—手段”三元组的可判定充分性计算接进产品：`pre` 在执行前判定，
//! `mid`／`end` 只记录。判定本身在 [`whcode_domain::coherence`]（纯函数），持久化在
//! [`whcode_store::coherence`]；本模块负责权限、输入构造与视图封装。
//!
//! 语义边界（ADR-085 §7）：
//!
//! - **不判语义**：意图是否正确、方案是否最优恒为 `unknown`，由用户确认；
//! - **无任务不伪造**：会话没有未完成 tasks 时返回 `None`（正确的空），不返回“通过”；
//! - **失败不静默**：读取或写入失败一律上抛，`pre` 路径按失败处理（不得把审计失败当放行）。

use duckdb::Connection;
use whcode_domain::{
    CoherencePhase, ErrorCode, MAX_SUFFICIENCY_TEXT_CHARS, SufficiencyInput, SufficiencyReport,
    Verdict, WhError, evaluate_sufficiency,
};
use whcode_store::{CoherenceRun, NewCoherenceRun, coherence as store};

use crate::authz::{Actor, Capability, Policy, require};

/// 一次计算的对外视图（四端同源；字段名即 JSON 键名）。
#[derive(Debug, Clone, PartialEq)]
pub struct CoherenceView {
    /// 报告编号（`COH-<序号>`；`None`＝尚未落库的内存报告）。
    pub id: Option<String>,
    /// 会话。
    pub session_id: Option<String>,
    /// 阶段。
    pub phase: CoherencePhase,
    /// 可判定片段结论。
    pub verdict: Verdict,
    /// 要求文本。
    pub requirement: String,
    /// 规则总数。
    pub total: u32,
    /// 反例条数。
    pub counterexamples: u32,
    /// 未知条数（不计反例，也不计通过）。
    pub unknowns: u32,
    /// 反例明细。
    pub details: Vec<serde_json::Value>,
    /// 创建时间（Unix 毫秒）。
    pub created_at_ms: i64,
}

fn invalid(message: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.to_string())
}

/// 反例明细的 JSON 文本（写入前序列化，长度由条数与规则文本上界决定）。
fn details_json(report: &SufficiencyReport) -> String {
    let items: Vec<serde_json::Value> = report
        .counterexamples()
        .map(|item| {
            serde_json::json!({
                "rule": item.rule.id(),
                "tier": item.rule.tier(),
                "detail": item.detail,
            })
        })
        .collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
}

fn to_view(run: &CoherenceRun) -> CoherenceView {
    let details = serde_json::from_str::<Vec<serde_json::Value>>(&run.counterexamples_json)
        .unwrap_or_default();
    CoherenceView {
        id: Some(run.id.clone()),
        session_id: run.session_id.clone(),
        phase: run.phase,
        verdict: run.verdict,
        requirement: run.requirement.clone(),
        total: run.total,
        counterexamples: run.counterexamples,
        unknowns: run.unknowns,
        details,
        created_at_ms: run.created_at_ms,
    }
}

/// 计算并登记一次充分性审计。
///
/// `session_id` 可为 `None`（仓库级或装配期计算）。要求文本超上限即拒绝，不截断。
///
/// # Errors
///
/// 缺 `session.create` 权限（`permission_denied`）、要求为空或超长（`invalid_state`）、
/// 写入失败（`io_error`）时返回 [`WhError`]。
pub fn audit(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: Option<&str>,
    phase: CoherencePhase,
    input: &SufficiencyInput<'_>,
    now_ms: u64,
) -> Result<CoherenceView, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    if input.requirement.chars().count() > MAX_SUFFICIENCY_TEXT_CHARS {
        return Err(invalid("要求文本超出长度上限"));
    }
    let report = evaluate_sufficiency(phase, input);
    let json = details_json(&report);
    let id = store::insert_run(
        conn,
        NewCoherenceRun {
            session_id,
            phase: report.phase,
            verdict: report.verdict,
            requirement: input.requirement.trim(),
            total: u32::try_from(report.outcomes.len()).map_err(|_| invalid("规则条数越界"))?,
            counterexamples: u32::try_from(report.counterexamples().count())
                .map_err(|_| invalid("反例条数越界"))?,
            unknowns: u32::try_from(report.unknowns().count())
                .map_err(|_| invalid("未知条数越界"))?,
            counterexamples_json: &json,
        },
        now_ms,
    )?;
    Ok(CoherenceView {
        id: Some(id),
        session_id: session_id.map(str::to_string),
        phase: report.phase,
        verdict: report.verdict,
        requirement: input.requirement.trim().to_string(),
        total: u32::try_from(report.outcomes.len()).map_err(|_| invalid("规则条数越界"))?,
        counterexamples: u32::try_from(report.counterexamples().count())
            .map_err(|_| invalid("反例条数越界"))?,
        unknowns: u32::try_from(report.unknowns().count()).map_err(|_| invalid("未知条数越界"))?,
        details: serde_json::from_str(&json).unwrap_or_default(),
        created_at_ms: i64::try_from(now_ms).map_err(|_| invalid("时间戳越界"))?,
    })
}

/// 读取会话最近一次计算；从未计算过返回 `None`（正确的空）。
///
/// # Errors
///
/// 缺 `session.read` 权限、会话不存在（`not_found`）或读取失败时返回 [`WhError`]。
pub fn latest(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<Option<CoherenceView>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    Ok(store::latest_run(conn, session_id)?.as_ref().map(to_view))
}

/// 用会话当前任务构造三元组并计算（tools 为可用手段集合）。
///
/// 返回 `None` 表示会话没有未完成任务——**正确的空**，不判为通过也不判为失败。
///
/// # Errors
///
/// 权限不足、任务读取或写入失败时返回 [`WhError`]。
pub fn audit_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    phase: CoherencePhase,
    available_means: &[&str],
    now_ms: u64,
) -> Result<Option<CoherenceView>, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let Some(task) = store::session_task(conn, session_id)? else {
        return Ok(None);
    };
    let acceptance: serde_json::Value = serde_json::from_str(&task.acceptance_json)
        .map_err(|err| WhError::parse_error(format!("验收条件不是合法 JSON：{err}")))?;
    let owned: Vec<String> = acceptance
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let checkpoints: Vec<&str> = owned.iter().map(String::as_str).collect();
    let means: Vec<&str> = if available_means.is_empty() {
        Vec::new()
    } else {
        available_means.to_vec()
    };
    let input = SufficiencyInput {
        requirement: task.title.trim(),
        checkpoints: &checkpoints,
        means: &means,
        available_means,
        budget_mb: None,
    };
    Ok(Some(audit(
        conn,
        policy,
        actor,
        Some(session_id),
        phase,
        &input,
        now_ms,
    )?))
}

/// `pre` 阶段闸门：有反例即返回错误（调用方据此拒绝执行）。
///
/// 返回 `Ok(None)` 表示没有可审计的三元组（会话无未完成任务）。审计过程本身失败
/// 一律上抛，不按“通过”处理。
///
/// # Errors
///
/// 存在反例时返回 `invalid_state`，消息列出反例条数与报告编号；权限与读写错误原样上抛。
pub fn preflight(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    available_means: &[&str],
    now_ms: u64,
) -> Result<Option<CoherenceView>, WhError> {
    let view = audit_session(
        conn,
        policy,
        actor,
        session_id,
        CoherencePhase::Pre,
        available_means,
        now_ms,
    )?;
    if let Some(report) = &view
        && report.verdict == Verdict::Counterexample
    {
        let rules: Vec<&str> = report
            .details
            .iter()
            .filter_map(|item| item.get("rule").and_then(serde_json::Value::as_str))
            .collect();
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "充分性前置未通过：{} 条反例（规则 {}，报告 {}）；补齐检查点或手段后再执行",
                report.counterexamples,
                rules.join("、"),
                report.id.clone().unwrap_or_default(),
            ),
        ));
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn session(conn: &Connection, id: &str) {
        whcode_store::create_session_with_id(conn, id, "测试会话", "active", 1_000)
            .expect("建会话");
    }

    fn task(conn: &Connection, session_id: &str, title: &str, acceptance: &str) {
        let id = format!("T-{session_id}");
        whcode_store::insert_task(
            conn,
            &whcode_store::tasks::NewTask {
                id: &id,
                session_id,
                title,
                stage: "specify",
                parent_id: None,
                acceptance_json: acceptance,
            },
            1_000,
        )
        .expect("建任务");
    }

    fn input<'a>(requirement: &'a str, checkpoints: &'a [&'a str]) -> SufficiencyInput<'a> {
        SufficiencyInput {
            requirement,
            checkpoints,
            means: &["shell"],
            available_means: &["shell", "read_file"],
            budget_mb: Some(96),
        }
    }

    #[test]
    fn audit_records_and_reads_back() {
        let conn = migrated();
        session(&conn, "S-1");
        let actor = actor_with_all_capabilities("tester");
        let view = audit(
            &conn,
            &DefaultPolicy,
            &actor,
            Some("S-1"),
            CoherencePhase::Pre,
            &input("补齐导出", &["导出含 schema_version"]),
            1_700_000_000_000,
        )
        .expect("计算");
        assert_eq!(view.verdict, Verdict::Satisfied);
        assert_eq!(view.counterexamples, 0);
        assert!(view.unknowns > 0, "语义项应记为未知");
        let latest = latest(&conn, &DefaultPolicy, &actor, "S-1")
            .expect("读取")
            .expect("存在");
        assert_eq!(latest.id, view.id);
        assert_eq!(latest.requirement, "补齐导出");
    }

    #[test]
    fn audit_requires_create_capability() {
        let conn = migrated();
        let actor = Actor::new("limited", Vec::new());
        let err = audit(
            &conn,
            &DefaultPolicy,
            &actor,
            None,
            CoherencePhase::Pre,
            &input("补齐导出", &["有判据"]),
            1,
        )
        .expect_err("缺权限必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }

    #[test]
    fn latest_rejects_unknown_session() {
        let conn = migrated();
        let actor = actor_with_all_capabilities("tester");
        let err = latest(&conn, &DefaultPolicy, &actor, "S-none").expect_err("未知会话必须报错");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn session_without_task_is_correct_empty() {
        let conn = migrated();
        session(&conn, "S-2");
        let actor = actor_with_all_capabilities("tester");
        let view = audit_session(
            &conn,
            &DefaultPolicy,
            &actor,
            "S-2",
            CoherencePhase::Pre,
            &["shell"],
            1,
        )
        .expect("读取任务");
        assert!(view.is_none(), "无任务时不伪造通过");
        assert!(
            preflight(&conn, &DefaultPolicy, &actor, "S-2", &["shell"], 1)
                .expect("闸门")
                .is_none()
        );
    }

    #[test]
    fn task_without_checkpoint_blocks_preflight() {
        let conn = migrated();
        session(&conn, "S-3");
        task(&conn, "S-3", "补齐导出", "[]");
        let actor = actor_with_all_capabilities("tester");
        let err = preflight(&conn, &DefaultPolicy, &actor, "S-3", &["shell"], 1)
            .expect_err("缺检查点必须阻断");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("SFR-03"), "消息要指名规则：{err}");
    }

    #[test]
    fn task_with_checkpoint_and_tools_passes() {
        let conn = migrated();
        session(&conn, "S-4");
        task(&conn, "S-4", "补齐导出", r#"["导出含 schema_version"]"#);
        let actor = actor_with_all_capabilities("tester");
        let view = preflight(&conn, &DefaultPolicy, &actor, "S-4", &["shell"], 1)
            .expect("闸门")
            .expect("有任务");
        assert_eq!(view.verdict, Verdict::Satisfied);
    }

    #[test]
    fn audit_flags_unavailable_means() {
        let conn = migrated();
        session(&conn, "S-5");
        let actor = actor_with_all_capabilities("tester");
        let draft = SufficiencyInput {
            requirement: "补齐导出",
            checkpoints: &["有判据"],
            means: &["not_registered"],
            available_means: &["shell"],
            budget_mb: Some(96),
        };
        let view = audit(
            &conn,
            &DefaultPolicy,
            &actor,
            Some("S-5"),
            CoherencePhase::Pre,
            &draft,
            1,
        )
        .expect("计算");
        assert_eq!(view.verdict, Verdict::Counterexample);
        assert!(
            view.details
                .iter()
                .any(|item| item.get("rule").and_then(serde_json::Value::as_str) == Some("SFR-07")),
            "反例明细要指名 SFR-07：{:?}",
            view.details
        );
    }
}
