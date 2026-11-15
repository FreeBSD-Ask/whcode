//! # 发现台账命令（P2.2／FR-786／ADR-080）
//!
//! 命令编排：权限 → 字符串枚举解析 → 默认值与上限归一 → 存储原语（[`whcode_store::findings`]）。
//! **无 SQL**（ADR-018）。四要件的非空与长度校验在 store 层（与表约束同一处），本层只把
//! “用户/模型给的字符串”收敛成类型化输入，并把未知取值变成显式错误。
//!
//! 权限：登记与状态迁移按写类（`SessionCreate`）判定；列表按读类（`SessionRead`）。
//! 发现台账跨会话（`session_id` 可空），但写入仍是会话内动作——与待办/交付物同口径。

use duckdb::Connection;
use whcode_domain::WhError;
use whcode_store::findings::{
    self, Finding, FindingSeverity, FindingStatus, MAX_FINDING_ROWS, NewFinding,
};

use crate::authz::{Actor, Capability, Policy, require};

/// 列表默认返回条数。
pub const DEFAULT_FINDING_LIMIT: usize = 50;

/// 登记请求（字符串形态；枚举解析在本层）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingDraft {
    /// 关联会话（可空）。
    pub session_id: Option<String>,
    /// 标题。
    pub title: String,
    /// 缺口。
    pub gap: String,
    /// 有界影响。
    pub impact: String,
    /// 最小修复。
    pub owner_fix: String,
    /// 验证路线。
    pub verification: String,
    /// 严重度稳定名（`low`／`medium`／`high`）。
    pub severity: String,
}

/// 登记一条发现。
///
/// # Errors
///
/// 缺少 `SessionCreate` 权限（`permission_denied`）、严重度未知、四要件缺失或超长
/// （`invalid_state`）、写入失败（`io_error`）时返回 [`WhError`]。
pub fn record_finding(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    draft: &FindingDraft,
    now_ms: u64,
) -> Result<Finding, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let severity = FindingSeverity::parse(draft.severity.trim()).ok_or_else(|| {
        WhError::invalid_state(format!(
            "未知发现严重度 `{}`：允许 low／medium／high（ADR-080 决策 5：只三档）",
            draft.severity
        ))
    })?;
    findings::create_finding(
        conn,
        &NewFinding {
            session_id: draft
                .session_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            title: draft.title.clone(),
            gap: draft.gap.clone(),
            impact: draft.impact.clone(),
            owner_fix: draft.owner_fix.clone(),
            verification: draft.verification.clone(),
            severity,
        },
        now_ms,
    )
}

/// 列出发现（可按状态过滤；`limit` 缺省 [`DEFAULT_FINDING_LIMIT`]，上限
/// [`MAX_FINDING_ROWS`]）。
///
/// # Errors
///
/// 缺少 `SessionRead` 权限、状态名未知或 `limit` 为 0 时返回 [`WhError`]。
pub fn list_finding_rows(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    status: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<Finding>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let status = match status.map(str::trim).filter(|text| !text.is_empty()) {
        None => None,
        Some(text) => Some(FindingStatus::parse(text).ok_or_else(|| {
            WhError::invalid_state(format!(
                "未知发现状态 `{text}`：允许 open／fixing／fixed／dismissed"
            ))
        })?),
    };
    let limit = limit.unwrap_or(DEFAULT_FINDING_LIMIT);
    if limit == 0 {
        return Err(WhError::invalid_state("finding 列表 limit 必须大于 0"));
    }
    findings::list_findings(conn, status, limit.min(MAX_FINDING_ROWS))
}

/// 按会话列出发（台账视图的数据源；`limit` 缺省 [`DEFAULT_FINDING_LIMIT`]）。
///
/// # Errors
///
/// 缺少 `SessionRead` 权限或 `limit` 为 0 时返回 [`WhError`]。
pub fn list_session_findings(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    limit: Option<usize>,
) -> Result<Vec<Finding>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let limit = limit.unwrap_or(DEFAULT_FINDING_LIMIT);
    if limit == 0 {
        return Err(WhError::invalid_state("finding 列表 limit 必须大于 0"));
    }
    findings::list_findings_for_session(conn, session_id, limit)
}

/// 迁移发现状态（乐观锁：`expected_version` 与当前版本不一致即冲突）。
///
/// # Errors
///
/// 缺少 `SessionCreate` 权限、状态名未知、版本冲突、非法迁移或记录不存在时返回 [`WhError`]。
pub fn move_finding(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    id: &str,
    expected_version: i64,
    next_status: &str,
    now_ms: u64,
) -> Result<Finding, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    let next = FindingStatus::parse(next_status.trim()).ok_or_else(|| {
        WhError::invalid_state(format!(
            "未知发现状态 `{next_status}`：允许 open／fixing／fixed／dismissed"
        ))
    })?;
    findings::transition_finding(conn, id.trim(), expected_version, next, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_domain::ErrorCode;
    use whcode_store::schema::apply_migrations;

    use crate::authz::{Actor, DefaultPolicy, actor_with_all_capabilities};

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn draft() -> FindingDraft {
        FindingDraft {
            session_id: Some("S-1".to_string()),
            title: "缺失败路径".to_string(),
            gap: "评审发现失败路径未覆盖".to_string(),
            impact: "失败被当成成功".to_string(),
            owner_fix: "补断言".to_string(),
            verification: "cargo test 变红后转绿".to_string(),
            severity: "high".to_string(),
        }
    }

    /// 登记 → 列表 → 迁移：正常路径，严重度与状态字符串收敛成枚举。
    #[test]
    fn record_list_and_move_roundtrip() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        let finding = record_finding(&conn, &DefaultPolicy, &actor, &draft(), 1_000).expect("登记");
        assert_eq!(finding.severity, FindingSeverity::High);
        assert_eq!(finding.status, FindingStatus::Open);

        let rows =
            list_finding_rows(&conn, &DefaultPolicy, &actor, Some("open"), None).expect("列表");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, finding.id);

        let moved = move_finding(
            &conn,
            &DefaultPolicy,
            &actor,
            &finding.id,
            finding.version,
            "fixed",
            2_000,
        )
        .expect("迁移");
        assert_eq!(moved.status, FindingStatus::Fixed);
        assert_eq!(moved.fixed_at_ms, Some(2_000));
    }

    /// 未知严重度/状态：显式错误，不静默取默认值。
    #[test]
    fn unknown_severity_and_status_are_rejected() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        let mut bad = draft();
        bad.severity = "critical".to_string();
        let err =
            record_finding(&conn, &DefaultPolicy, &actor, &bad, 1_000).expect_err("未知严重度");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("low"), "{}", err.message);

        let err = list_finding_rows(&conn, &DefaultPolicy, &actor, Some("done"), None)
            .expect_err("未知状态");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    /// 权限：无能力者不能登记、不能列表。
    #[test]
    fn permissions_are_enforced() {
        let conn = fixture();
        let actor = Actor::new("u", []);
        let err = record_finding(&conn, &DefaultPolicy, &actor, &draft(), 1_000)
            .expect_err("无 SessionCreate 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        let err = list_finding_rows(&conn, &DefaultPolicy, &actor, None, None)
            .expect_err("无 SessionRead 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }

    /// limit 缺省、上限夹取与 0 拒绝。
    #[test]
    fn limit_defaults_clamps_and_rejects_zero() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        record_finding(&conn, &DefaultPolicy, &actor, &draft(), 1_000).expect("登记");
        assert_eq!(
            list_finding_rows(&conn, &DefaultPolicy, &actor, None, None)
                .expect("缺省")
                .len(),
            1
        );
        assert_eq!(
            list_finding_rows(&conn, &DefaultPolicy, &actor, None, Some(usize::MAX))
                .expect("上限夹取")
                .len(),
            1
        );
        assert_eq!(
            list_finding_rows(&conn, &DefaultPolicy, &actor, None, Some(0))
                .expect_err("0 拒绝")
                .code,
            ErrorCode::InvalidState
        );
    }
}
