//! # 学习捕获干预台账命令（P2.4／FR-794／ADR-084）
//!
//! 命令编排：权限 → 字符串枚举解析 → 默认值与上限归一 → 存储原语
//! （[`whcode_store::interventions`]）。**无 SQL**（ADR-018）。四要件的非空与长度校验在
//! store 层（与表约束同一处），本层只把“用户/模型给的字符串”收敛成类型化输入。
//!
//! **结论不可人工指定**（ADR-084 决策 4 的防自证）：[`conclude_intervention`] 只传 id 与
//! 乐观锁版本，`outcome` 由 store 按 before／after 复发计数自动判定——人工在结论里写
//! “有效”不算数，计数算数。
//!
//! 权限：登记与结论按写类（`SessionCreate`）；列表按读类（`SessionRead`）。干预台账跨
//! 会话（`session_id` 可空），但写入仍是会话内动作——与发现台账同口径。

use duckdb::Connection;
use whcode_domain::WhError;
use whcode_store::interventions::{
    self, Intervention, InterventionStatus, MAX_INTERVENTION_ROWS, NewIntervention,
};

use crate::authz::{Actor, Capability, Policy, require};

/// 列表默认返回条数。
pub const DEFAULT_INTERVENTION_LIMIT: usize = 50;

/// 登记请求（字符串形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterventionDraft {
    /// 关联会话（可空）。
    pub session_id: Option<String>,
    /// 目标复发模式（与 `findings.pattern` 精确匹配聚合）。
    pub pattern: String,
    /// 措施：做了什么。
    pub action: String,
    /// 预期效果：防住什么。
    pub expected_effect: String,
    /// 验证路线：怎么算生效。
    pub verification: String,
    /// 关联首发发现（可空；`FND-<n>`）。
    pub target_finding_id: Option<String>,
}

/// 登记一条干预。
///
/// # Errors
///
/// 缺少 `SessionCreate` 权限（`permission_denied`）、四要件缺失或超长（`invalid_state`）、
/// 写入失败（`io_error`）时返回 [`WhError`]。
pub fn record_intervention(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    draft: &InterventionDraft,
    now_ms: u64,
) -> Result<Intervention, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    interventions::create_intervention(
        conn,
        &NewIntervention {
            session_id: draft
                .session_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
            pattern: draft.pattern.clone(),
            action: draft.action.clone(),
            expected_effect: draft.expected_effect.clone(),
            verification: draft.verification.clone(),
            target_finding_id: draft
                .target_finding_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
        },
        now_ms,
    )
}

/// 列出干预（可按状态与模式过滤；`limit` 缺省 [`DEFAULT_INTERVENTION_LIMIT`]，上限
/// [`MAX_INTERVENTION_ROWS`]）。
///
/// # Errors
///
/// 缺少 `SessionRead` 权限、状态名未知或 `limit` 为 0 时返回 [`WhError`]。
pub fn list_intervention_rows(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    status: Option<&str>,
    pattern: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<Intervention>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let status = match status.map(str::trim).filter(|text| !text.is_empty()) {
        None => None,
        Some(text) => Some(InterventionStatus::parse(text).ok_or_else(|| {
            WhError::invalid_state(format!("未知干预状态 `{text}`：允许 active／concluded"))
        })?),
    };
    let limit = limit.unwrap_or(DEFAULT_INTERVENTION_LIMIT);
    if limit == 0 {
        return Err(WhError::invalid_state("intervention 列表 limit 必须大于 0"));
    }
    interventions::list_interventions(
        conn,
        status,
        pattern.map(str::trim).filter(|text| !text.is_empty()),
        limit.min(MAX_INTERVENTION_ROWS),
    )
}

/// 按会话列出干预（台账视图的数据源；`limit` 缺省 [`DEFAULT_INTERVENTION_LIMIT`]）。
///
/// # Errors
///
/// 缺少 `SessionRead` 权限、会话不存在或 `limit` 为 0 时返回 [`WhError`]。
pub fn list_session_interventions(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    limit: Option<usize>,
) -> Result<Vec<Intervention>, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let limit = limit.unwrap_or(DEFAULT_INTERVENTION_LIMIT);
    if limit == 0 {
        return Err(WhError::invalid_state("intervention 列表 limit 必须大于 0"));
    }
    // 会话不存在与“该会话没有干预”必须可区分：前者 `not_found`，后者是正确的空
    // （与发现／待办／交付物同口径，台账视图与 Web `/api/ledger` 依赖这一区分）。
    if whcode_store::get_session(conn, session_id.trim())?.is_none() {
        return Err(WhError::new(
            whcode_domain::ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话"),
        ));
    }
    interventions::list_interventions_for_session(conn, session_id.trim(), limit)
}

/// 结论一条干预（乐观锁；`outcome` 由 store 自动判定，本层不接收结论参数）。
///
/// # Errors
///
/// 缺少 `SessionCreate` 权限、记录不存在、版本冲突或已结论时返回 [`WhError`]。
pub fn conclude_intervention(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    id: &str,
    expected_version: i64,
    now_ms: u64,
) -> Result<Intervention, WhError> {
    require(policy, actor, Capability::SessionCreate)?;
    interventions::conclude_intervention(conn, id.trim(), expected_version, now_ms)
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

    fn draft() -> InterventionDraft {
        InterventionDraft {
            session_id: Some("S-1".to_string()),
            pattern: "测试断言硬编码分隔符".to_string(),
            action: "共享夹具做归一比较".to_string(),
            expected_effect: "同类断言不再因平台差异失败".to_string(),
            verification: "三平台 CI 全绿".to_string(),
            target_finding_id: None,
        }
    }

    /// 登记 → 列表 → 结论：正常路径，outcome 由 store 自动判定（本层不传结论）。
    #[test]
    fn record_list_and_conclude_roundtrip() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        let item =
            record_intervention(&conn, &DefaultPolicy, &actor, &draft(), 1_000).expect("登记");
        assert_eq!(item.status, InterventionStatus::Active);

        let rows =
            list_intervention_rows(&conn, &DefaultPolicy, &actor, Some("active"), None, None)
                .expect("列表");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, item.id);

        let concluded =
            conclude_intervention(&conn, &DefaultPolicy, &actor, &item.id, item.version, 2_000)
                .expect("结论");
        assert_eq!(concluded.status, InterventionStatus::Concluded);
        assert_eq!(
            concluded.outcome,
            Some(whcode_store::InterventionOutcome::Effective),
            "窗口内无新复发：自动判有效"
        );
        assert_eq!(concluded.concluded_at_ms, Some(2_000));
    }

    /// 未知状态：显式错误；limit 语义与 findings 同款。
    #[test]
    fn unknown_status_and_limit_are_rejected() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        let err = list_intervention_rows(&conn, &DefaultPolicy, &actor, Some("done"), None, None)
            .expect_err("未知状态");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("active"), "{}", err.message);
        let err = list_intervention_rows(&conn, &DefaultPolicy, &actor, None, None, Some(0))
            .expect_err("0 拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    /// 按会话列干预：会话不存在返回 `not_found`，不是空列表。
    #[test]
    fn session_listing_distinguishes_missing_session_from_empty() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        whcode_store::create_session_with_id(&conn, "S-1", "干预会话", "active", 1_000)
            .expect("创建会话");
        record_intervention(&conn, &DefaultPolicy, &actor, &draft(), 1_000).expect("登记");
        assert_eq!(
            list_session_interventions(&conn, &DefaultPolicy, &actor, "S-1", None)
                .expect("列表")
                .len(),
            1
        );
        whcode_store::create_session_with_id(&conn, "S-E", "空会话", "active", 1_000)
            .expect("创建空会话");
        assert!(
            list_session_interventions(&conn, &DefaultPolicy, &actor, "S-E", None)
                .expect("空会话是正确的空")
                .is_empty()
        );
        assert_eq!(
            list_session_interventions(&conn, &DefaultPolicy, &actor, "S-404", None)
                .expect_err("会话不存在")
                .code,
            ErrorCode::NotFound
        );
    }

    /// 权限：无能力者不能登记、不能结论、不能列表。
    #[test]
    fn permissions_are_enforced() {
        let conn = fixture();
        let actor = Actor::new("u", []);
        let err = record_intervention(&conn, &DefaultPolicy, &actor, &draft(), 1_000)
            .expect_err("无 SessionCreate 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        let err = list_intervention_rows(&conn, &DefaultPolicy, &actor, None, None, None)
            .expect_err("无 SessionRead 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        let err = conclude_intervention(&conn, &DefaultPolicy, &actor, "INT-1", 1, 1_000)
            .expect_err("无 SessionCreate 必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
    }

    /// 结论不可人工指定：本层 API 不接收结论参数（防自证，ADR-084 决策 4）。
    #[test]
    fn conclusion_takes_no_human_verdict() {
        let conn = fixture();
        let actor = actor_with_all_capabilities("tester");
        let item =
            record_intervention(&conn, &DefaultPolicy, &actor, &draft(), 1_000).expect("登记");
        // 编译面即防自证：conclude_intervention 无 outcome 参数（见函数签名）；
        // 运行面断言结论只能来自 before／after 聚合。
        let concluded =
            conclude_intervention(&conn, &DefaultPolicy, &actor, &item.id, item.version, 2_000)
                .expect("结论");
        assert!(matches!(
            concluded.outcome,
            Some(whcode_store::InterventionOutcome::Effective)
        ));
    }
}
