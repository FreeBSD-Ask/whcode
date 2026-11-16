//! # 学习捕获干预台账（P2.4／FR-794／ADR-084）
//!
//! 发现台账（[`crate::findings`]）记录“有一个缺口”；本模块记录“我们为阻止它再次发生做了
//! 什么”以及“干预是否真的有效”。四要件与 findings 同构必填——**目标模式**（针对什么复发）、
//! **措施**（做了什么）、**预期效果**（防住什么）、**验证路线**（怎么算生效）。
//!
//! **outcome 由 store 自动判定，不接受人工指定**（ADR-084 决策 4 的防自证）：
//! 登记时记 `recurrences_before`（该模式下全部发现的复发计数总和），`conclude` 时重算
//! 同口径的 `recurrences_after`——相等判 `effective`，否则 `not_effective`。人工在结论里
//! 写“有效”不算数，计数算数。
//!
//! 有界性：四要件各 ≤ [`MAX_INTERVENTION_TEXT_CHARS`]、模式 ≤ [`MAX_INTERVENTION_PATTERN_CHARS`]、
//! 列表一次 ≤ [`MAX_INTERVENTION_ROWS`] 条（超出由调用方分页，不静默截断）。
//!
//! 边界：本模块只读写 `interventions` 表；权限与字符串枚举归一在 `whcode-app`（与 findings
//! 同口径）；复发计数来自 `findings.recurrence_count`，按 `pattern` 精确匹配聚合。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

/// 模式归类字符上限（与发现标题同量级：一句话说清针对什么）。
pub const MAX_INTERVENTION_PATTERN_CHARS: usize = 200;

/// 四要件单段文本的字符上限。
pub const MAX_INTERVENTION_TEXT_CHARS: usize = 2000;

/// 单次列表返回条数上限。
pub const MAX_INTERVENTION_ROWS: usize = 200;

/// 干预状态（封闭两态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterventionStatus {
    /// 已登记、验证窗口进行中。
    Active,
    /// 已结论（`outcome` 与 `concluded_at` 落定）。
    Concluded,
}

impl InterventionStatus {
    /// 库内稳定名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Concluded => "concluded",
        }
    }

    /// 解析稳定名（未知取值返回 `None`，由调用方决定报错口径）。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "active" => Some(Self::Active),
            "concluded" => Some(Self::Concluded),
            _ => None,
        }
    }
}

/// 干预结论（自动判定，人工不可指定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterventionOutcome {
    /// 验证窗口内同模式复发计数无增长。
    Effective,
    /// 窗口内仍有复发——干预未生效，必须显式可见。
    NotEffective,
}

impl InterventionOutcome {
    /// 库内稳定名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Effective => "effective",
            Self::NotEffective => "not-effective",
        }
    }

    /// 解析稳定名。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "effective" => Some(Self::Effective),
            "not-effective" => Some(Self::NotEffective),
            _ => None,
        }
    }
}

/// 新干预的登记请求（四要件均须非空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIntervention {
    /// 关联会话（可选：流程级干预可能没有会话归属）。
    pub session_id: Option<String>,
    /// 目标复发模式（与 `findings.pattern` 精确匹配聚合）。
    pub pattern: String,
    /// 措施：做了什么。
    pub action: String,
    /// 预期效果：防住什么。
    pub expected_effect: String,
    /// 验证路线：怎么算生效。
    pub verification: String,
    /// 关联首发发现（可选）。
    pub target_finding_id: Option<String>,
}

/// 一条干预（读出的完整行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intervention {
    /// `INT-<序号>`。
    pub id: String,
    /// 关联会话。
    pub session_id: Option<String>,
    /// 目标复发模式。
    pub pattern: String,
    /// 措施。
    pub action: String,
    /// 预期效果。
    pub expected_effect: String,
    /// 验证路线。
    pub verification: String,
    /// 状态。
    pub status: InterventionStatus,
    /// 结论（仅 concluded 有；自动判定）。
    pub outcome: Option<InterventionOutcome>,
    /// 关联首发发现。
    pub target_finding_id: Option<String>,
    /// 登记时的复发计数基线（该模式总和）。
    pub recurrences_before: u64,
    /// 结论时复算的复发计数（结论后才有）。
    pub recurrences_after: Option<u64>,
    /// 乐观锁版本（每次迁移 +1）。
    pub version: i64,
    /// 登记时间（Unix 毫秒）。
    pub created_at_ms: u64,
    /// 最近更新时间。
    pub updated_at_ms: u64,
    /// 结论时间（其余状态为 `None`）。
    pub concluded_at_ms: Option<u64>,
}

const INTERVENTION_COLUMNS: &str = "id, session_id, pattern, action, expected_effect, verification, \
     status, outcome, target_finding_id, recurrences_before, recurrences_after, version, \
     epoch_ms(created_at), epoch_ms(updated_at), epoch_ms(concluded_at)";

fn intervention_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!("interventions {context} 失败：{err}"),
    )
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::invalid_state(message.into())
}

fn text_within(value: &str, name: &str, limit: usize) -> Result<(), WhError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!(
            "intervention 的 {name} 不得为空：四要件缺一不可（模式／措施／预期效果／验证路线）"
        )));
    }
    if trimmed.chars().count() > limit {
        return Err(invalid(format!(
            "intervention 的 {name} 超过 {limit} 字符上限"
        )));
    }
    Ok(())
}

/// 该模式下全部发现的复发计数总和（登记与结论共用同一口径，ADR-084 决策 4）。
fn recurrence_total(conn: &Connection, pattern: &str) -> Result<u64, WhError> {
    let total: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(recurrence_count), 0) FROM findings WHERE pattern = ?",
            duckdb::params![pattern],
            |row| row.get(0),
        )
        .map_err(|err| intervention_err("聚合复发计数", err))?;
    u64::try_from(total).map_err(|_| {
        WhError::new(
            ErrorCode::ParseError,
            format!("findings.recurrence_count 聚合为负数（{total}）：库内容漂移，需人工核对"),
        )
    })
}

fn row_to_intervention(row: &duckdb::Row<'_>) -> Result<Intervention, WhError> {
    let status_text: String = row
        .get(6)
        .map_err(|err| intervention_err("读取状态", err))?;
    let status = InterventionStatus::parse(&status_text).ok_or_else(|| {
        WhError::new(
            ErrorCode::ParseError,
            format!("interventions.status 出现未知取值 `{status_text}`：库内容漂移，需人工核对"),
        )
    })?;
    let outcome_text: Option<String> = row
        .get(7)
        .map_err(|err| intervention_err("读取结论", err))?;
    let outcome = outcome_text
        .as_deref()
        .map(|text| {
            InterventionOutcome::parse(text).ok_or_else(|| {
                WhError::new(
                    ErrorCode::ParseError,
                    format!("interventions.outcome 出现未知取值 `{text}`：库内容漂移，需人工核对"),
                )
            })
        })
        .transpose()?;
    let recurrences_before: i64 = row
        .get(9)
        .map_err(|err| intervention_err("读取复发基线", err))?;
    let recurrences_after: Option<i64> = row
        .get(10)
        .map_err(|err| intervention_err("读取复发复算", err))?;
    let created_at_ms: i64 = row
        .get(12)
        .map_err(|err| intervention_err("读取创建时间", err))?;
    let updated_at_ms: i64 = row
        .get(13)
        .map_err(|err| intervention_err("读取更新时间", err))?;
    let concluded_at_ms: Option<i64> = row
        .get(14)
        .map_err(|err| intervention_err("读取结论时间", err))?;
    if recurrences_before < 0 {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!("recurrences_before 为负数（{recurrences_before}）：库内容漂移，需人工核对"),
        ));
    }
    Ok(Intervention {
        id: row.get(0).map_err(|err| intervention_err("读取 id", err))?,
        session_id: row
            .get(1)
            .map_err(|err| intervention_err("读取会话", err))?,
        pattern: row
            .get(2)
            .map_err(|err| intervention_err("读取模式", err))?,
        action: row
            .get(3)
            .map_err(|err| intervention_err("读取措施", err))?,
        expected_effect: row
            .get(4)
            .map_err(|err| intervention_err("读取预期效果", err))?,
        verification: row
            .get(5)
            .map_err(|err| intervention_err("读取验证路线", err))?,
        status,
        outcome,
        target_finding_id: row
            .get(8)
            .map_err(|err| intervention_err("读取关联发现", err))?,
        recurrences_before: u64::try_from(recurrences_before).unwrap_or(0),
        recurrences_after: recurrences_after.and_then(|value| u64::try_from(value).ok()),
        version: row
            .get(11)
            .map_err(|err| intervention_err("读取版本", err))?,
        created_at_ms: u64::try_from(created_at_ms).unwrap_or(0),
        updated_at_ms: u64::try_from(updated_at_ms).unwrap_or(0),
        concluded_at_ms: concluded_at_ms.and_then(|value| u64::try_from(value).ok()),
    })
}

/// 登记一条干预（四要件全部校验后写入；`recurrences_before` 取当前同模式复发总和；版本从 1 开始）。
///
/// # Errors
///
/// 任一要件为空或超长（`invalid_state`）、写入失败（`io_error`）时返回 [`WhError`]。
pub fn create_intervention(
    conn: &Connection,
    new: &NewIntervention,
    now_ms: u64,
) -> Result<Intervention, WhError> {
    text_within(&new.pattern, "目标模式", MAX_INTERVENTION_PATTERN_CHARS)?;
    text_within(&new.action, "措施", MAX_INTERVENTION_TEXT_CHARS)?;
    text_within(
        &new.expected_effect,
        "预期效果",
        MAX_INTERVENTION_TEXT_CHARS,
    )?;
    text_within(&new.verification, "验证路线", MAX_INTERVENTION_TEXT_CHARS)?;
    if new
        .target_finding_id
        .as_deref()
        .is_some_and(|finding| finding.trim().is_empty())
    {
        return Err(invalid(
            "intervention 的关联发现不得为空白字符串：要么不填，要么给可解析的 finding id",
        ));
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let before = recurrence_total(conn, new.pattern.trim())?;
    let before_i64 = i64::try_from(before).map_err(|_| invalid("复发基线超出 i64 范围"))?;
    let id: String = conn
        .query_row(
            "INSERT INTO interventions
                (id, session_id, pattern, action, expected_effect, verification, status,
                 target_finding_id, recurrences_before, version, created_at, updated_at)
             VALUES ('INT-' || nextval('whcode_intervention_id_seq'), ?, ?, ?, ?, ?,
                     'active', ?, ?, 1, epoch_ms(?), epoch_ms(?))
             RETURNING id",
            duckdb::params![
                new.session_id.as_deref(),
                new.pattern.trim(),
                new.action.trim(),
                new.expected_effect.trim(),
                new.verification.trim(),
                new.target_finding_id.as_deref(),
                before_i64,
                now,
                now,
            ],
            |row| row.get(0),
        )
        .map_err(|err| intervention_err("登记", err))?;
    get_intervention(conn, &id)?.ok_or_else(|| {
        WhError::new(
            ErrorCode::Internal,
            format!("intervention {id} 登记后读回失败"),
        )
    })
}

/// 按 id 读取一条干预；不存在返回 `None`（正确的空）。
///
/// # Errors
///
/// 查询失败或库内出现未知枚举值时返回 [`WhError`]。
pub fn get_intervention(conn: &Connection, id: &str) -> Result<Option<Intervention>, WhError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {INTERVENTION_COLUMNS} FROM interventions WHERE id = ?"
        ))
        .map_err(|err| intervention_err("准备查询", err))?;
    let mut rows = stmt
        .query(duckdb::params![id])
        .map_err(|err| intervention_err("查询单条", err))?;
    match rows
        .next()
        .map_err(|err| intervention_err("遍历单条", err))?
    {
        Some(row) => Ok(Some(row_to_intervention(row)?)),
        None => Ok(None),
    }
}

/// 列出干预（可按状态与模式过滤，`None`＝全部；按创建时间倒序；上限 [`MAX_INTERVENTION_ROWS`]）。
///
/// # Errors
///
/// `limit` 为 0、查询失败或库内出现未知枚举值时返回 [`WhError`]。
pub fn list_interventions(
    conn: &Connection,
    status: Option<InterventionStatus>,
    pattern: Option<&str>,
    limit: usize,
) -> Result<Vec<Intervention>, WhError> {
    if limit == 0 {
        return Err(invalid("list_interventions 的 limit 必须大于 0"));
    }
    let limit_i64 = i64::try_from(limit.min(MAX_INTERVENTION_ROWS))
        .map_err(|_| invalid("limit 超出 i64 范围"))?;
    let mut clauses: Vec<&str> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    if status.is_some() {
        clauses.push("status = ?");
    }
    if pattern.is_some() {
        clauses.push("pattern = ?");
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT {INTERVENTION_COLUMNS} FROM interventions{where_sql} \
         ORDER BY created_at DESC, id DESC LIMIT ?"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| intervention_err("准备列表", err))?;
    if let Some(status) = status {
        params.push(status.as_str().to_string());
    }
    if let Some(pattern) = pattern {
        params.push(pattern.to_string());
    }
    params.push(limit_i64.to_string());
    let param_refs: Vec<&str> = params.iter().map(String::as_str).collect();
    let mut rows = stmt
        .query(duckdb::params_from_iter(param_refs))
        .map_err(|err| intervention_err("列表查询", err))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|err| intervention_err("遍历列表", err))?
    {
        out.push(row_to_intervention(row)?);
    }
    Ok(out)
}

/// 按会话列出干预（台账视图用；`limit` 夹取到 [`MAX_INTERVENTION_ROWS`]）。
///
/// # Errors
///
/// `limit` 为 0、查询失败或库内出现未知枚举值时返回 [`WhError`]。
pub fn list_interventions_for_session(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<Intervention>, WhError> {
    if limit == 0 {
        return Err(invalid(
            "list_interventions_for_session 的 limit 必须大于 0",
        ));
    }
    let limit_i64 = i64::try_from(limit.min(MAX_INTERVENTION_ROWS))
        .map_err(|_| invalid("limit 超出 i64 范围"))?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {INTERVENTION_COLUMNS} FROM interventions WHERE session_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        ))
        .map_err(|err| intervention_err("准备按会话查询", err))?;
    let mut rows = stmt
        .query(duckdb::params![session_id, limit_i64])
        .map_err(|err| intervention_err("按会话查询", err))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|err| intervention_err("遍历会话干预", err))?
    {
        out.push(row_to_intervention(row)?);
    }
    Ok(out)
}

/// 结论一条干预（乐观锁）：`recurrences_after` 按 [`recurrence_total`] 重算，`outcome`
/// 自动判定——窗口内同模式复发计数无增长即 `effective`，否则 `not_effective`。人工不得
/// 指定结论（ADR-084 决策 4 的防自证）。
///
/// # Errors
///
/// 记录不存在（`not_found`）、版本冲突（`invalid_state`）、已结论（`invalid_state`）或
/// 写入失败（`io_error`）时返回 [`WhError`]。
pub fn conclude_intervention(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    now_ms: u64,
) -> Result<Intervention, WhError> {
    let current = get_intervention(conn, id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("intervention {id} 不存在")))?;
    if current.version != expected_version {
        return Err(invalid(format!(
            "intervention {id} 版本冲突：期望版本 {expected_version}，当前 {}——先重读再提交（乐观锁）",
            current.version
        )));
    }
    if current.status != InterventionStatus::Active {
        return Err(invalid(format!(
            "intervention {id} 已于此前结论（{}）：结论是终态，重开须另立新干预",
            current.outcome.map_or("?", |outcome| outcome.as_str())
        )));
    }
    let after = recurrence_total(conn, &current.pattern)?;
    let outcome = if after == current.recurrences_before {
        InterventionOutcome::Effective
    } else {
        InterventionOutcome::NotEffective
    };
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let after_i64 = i64::try_from(after).map_err(|_| invalid("复发复算超出 i64 范围"))?;
    let changed = conn
        .execute(
            "UPDATE interventions
                SET status = 'concluded', outcome = ?, recurrences_after = ?,
                    concluded_at = epoch_ms(?), version = version + 1, updated_at = epoch_ms(?)
              WHERE id = ? AND version = ?",
            duckdb::params![outcome.as_str(), after_i64, now, now, id, expected_version],
        )
        .map_err(|err| intervention_err("结论", err))?;
    if changed == 0 {
        let actual = get_intervention(conn, id)?.map_or(-1, |item| item.version);
        return Err(invalid(format!(
            "intervention {id} 版本冲突：期望版本 {expected_version}，当前 {actual}——先重读再提交（乐观锁）"
        )));
    }
    get_intervention(conn, id)?.ok_or_else(|| {
        WhError::new(
            ErrorCode::Internal,
            format!("intervention {id} 结论后读回失败"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{
        FindingStatus, NewFinding, create_finding, record_finding_recurrence, transition_finding,
    };
    use crate::schema::apply_migrations;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn new_intervention(pattern: &str) -> NewIntervention {
        NewIntervention {
            session_id: Some("S-1".to_string()),
            pattern: pattern.to_string(),
            action: "为测试目录加共享夹具，禁止各自硬编码分隔符".to_string(),
            expected_effect: "同类断言不再因平台差异反复失败".to_string(),
            verification: "下一个批次的三次 CI 运行全绿".to_string(),
            target_finding_id: None,
        }
    }

    fn fixed_finding_with_pattern(conn: &Connection, pattern: &str) -> String {
        let finding = create_finding(
            conn,
            &NewFinding {
                session_id: Some("S-1".to_string()),
                title: "测试断言硬编码反斜杠".to_string(),
                gap: "FreeBSD 上目录断言失败".to_string(),
                impact: "CI 红且误判为代码回归".to_string(),
                owner_fix: "改为归一比较".to_string(),
                verification: "三平台 CI 全绿".to_string(),
                severity: crate::findings::FindingSeverity::Medium,
            },
            1_000,
        )
        .expect("登记发现");
        // 修复声明完成时归类模式（复发聚合的锚点）。
        transition_finding(
            conn,
            &finding.id,
            finding.version,
            FindingStatus::Fixed,
            None,
            2_000,
        )
        .expect("修复");
        conn.execute(
            "UPDATE findings SET pattern = ? WHERE id = ?",
            duckdb::params![pattern, finding.id],
        )
        .expect("回填归类");
        finding.id
    }

    #[test]
    fn create_validates_four_fields_and_bounds() {
        let conn = fixture();
        let mut new = new_intervention("测试断言硬编码分隔符");
        new.action = "   ".to_string();
        let err = create_intervention(&conn, &new, 1_000).expect_err("空措施必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        new.action = "x".repeat(MAX_INTERVENTION_TEXT_CHARS + 1);
        let err = create_intervention(&conn, &new, 1_000).expect_err("超长必须拒绝");
        assert!(
            err.message.contains("超过"),
            "消息须写明上限：{}",
            err.message
        );
        let mut with_blank_finding = new_intervention("模式");
        with_blank_finding.target_finding_id = Some("   ".to_string());
        let err = create_intervention(&conn, &with_blank_finding, 1_000)
            .expect_err("空白关联发现必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn recurrences_before_is_pattern_total_at_registration() {
        let conn = fixture();
        let pattern = "测试断言硬编码分隔符";
        let id_a = fixed_finding_with_pattern(&conn, pattern);
        // 两轮“修复→复发”循环：复发只发生在 fixed 声明之后（产品语义），每轮先修回 fixed。
        record_finding_recurrence(&conn, &id_a, 2, None, 3_000).expect("复发 1");
        transition_finding(&conn, &id_a, 3, FindingStatus::Fixed, None, 3_200).expect("再修");
        record_finding_recurrence(&conn, &id_a, 4, None, 3_500).expect("复发 2");
        let item = create_intervention(&conn, &new_intervention(pattern), 4_000).expect("登记");
        assert_eq!(item.recurrences_before, 2, "基线＝该模式当前复发总和");
        assert_eq!(item.status, InterventionStatus::Active);
        // 无匹配模式的干预：基线 0（正确的零，预防性干预合法）。
        let preventive = create_intervention(&conn, &new_intervention("尚未出现过的模式"), 5_000)
            .expect("预防性干预");
        assert_eq!(preventive.recurrences_before, 0);
    }

    #[test]
    fn conclude_outcome_is_automatic_not_human() {
        let conn = fixture();
        let pattern = "测试断言硬编码分隔符";
        let id_a = fixed_finding_with_pattern(&conn, pattern);
        record_finding_recurrence(&conn, &id_a, 2, None, 3_000).expect("复发");
        let item = create_intervention(&conn, &new_intervention(pattern), 4_000).expect("登记");
        let concluded = conclude_intervention(&conn, &item.id, item.version, 5_000).expect("结论");
        assert_eq!(
            concluded.outcome,
            Some(InterventionOutcome::Effective),
            "窗口内无新复发：自动判有效"
        );
        assert_eq!(concluded.recurrences_after, Some(1), "复算值与基线一致");
        // 结论后再来一轮“修复→复发”（finding 此刻 open，先修回 fixed），再立新干预：
        // 结论前又复发一次，复算超过基线即 not-effective（人工不可改判）。
        transition_finding(&conn, &id_a, 3, FindingStatus::Fixed, None, 5_500).expect("再修");
        record_finding_recurrence(&conn, &id_a, 4, None, 6_000).expect("复发 2");
        let second = create_intervention(&conn, &new_intervention(pattern), 7_000).expect("再登记");
        transition_finding(&conn, &id_a, 5, FindingStatus::Fixed, None, 7_200).expect("三修");
        record_finding_recurrence(&conn, &id_a, 6, None, 7_500).expect("复发 3");
        let failed =
            conclude_intervention(&conn, &second.id, second.version, 8_000).expect("结论 2");
        assert_eq!(
            failed.outcome,
            Some(InterventionOutcome::NotEffective),
            "窗口内复发增长：必须显式判未生效"
        );
        // 结论是终态。
        let err = conclude_intervention(&conn, &second.id, failed.version, 9_000)
            .expect_err("重复结论必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn version_conflict_and_filters_are_explicit() {
        let conn = fixture();
        let item = create_intervention(&conn, &new_intervention("模式 A"), 1_000).expect("登记");
        create_intervention(&conn, &new_intervention("模式 B"), 2_000).expect("登记 2");
        let err = conclude_intervention(&conn, &item.id, item.version + 1, 3_000)
            .expect_err("版本过期必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.message.contains("版本冲突"),
            "消息须给出期望与实际：{}",
            err.message
        );
        let active = list_interventions(&conn, Some(InterventionStatus::Active), None, 10)
            .expect("按状态过滤");
        assert_eq!(active.len(), 2);
        let by_pattern = list_interventions(&conn, None, Some("模式 A"), 10).expect("按模式过滤");
        assert_eq!(by_pattern.len(), 1);
        assert_eq!(by_pattern[0].pattern, "模式 A");
        let err = list_interventions(&conn, None, None, 0).expect_err("limit=0 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(get_intervention(&conn, "INT-9999").expect("查询").is_none());
    }

    /// 无过滤列表（无 WHERE 分支）、超上限夹取、按会话范围与缺失会话的正确空。
    #[test]
    fn unfiltered_list_session_scope_and_clamp() {
        let conn = fixture();
        let first = create_intervention(&conn, &new_intervention("模式 A"), 1_000).expect("登记");
        let mut other = new_intervention("模式 B");
        other.session_id = Some("S-2".to_string());
        create_intervention(&conn, &other, 2_000).expect("登记 S-2");

        let all = list_interventions(&conn, None, None, 10).expect("无过滤列表");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].pattern, "模式 B", "登记时间倒序");
        let clamped = list_interventions(&conn, None, None, usize::MAX).expect("超上限夹取");
        assert_eq!(clamped.len(), 2);

        let scoped = list_interventions_for_session(&conn, "S-1", 10).expect("按会话");
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].id, first.id);
        assert!(
            list_interventions_for_session(&conn, "S-404", 10)
                .expect("无记录")
                .is_empty(),
            "无记录＝正确的空"
        );
        assert_eq!(
            list_interventions_for_session(&conn, "S-1", 0)
                .expect_err("limit=0 必须拒绝")
                .code,
            ErrorCode::InvalidState
        );
    }

    /// 对不存在的干预下结论：`not_found`，不是版本冲突也不是静默成功。
    #[test]
    fn conclude_missing_id_is_not_found() {
        let conn = fixture();
        assert_eq!(
            conclude_intervention(&conn, "INT-9999", 1, 1_000)
                .expect_err("不存在必须报错")
                .code,
            ErrorCode::NotFound
        );
    }

    /// 库内容漂移：未知状态／结论码与负基线按 `ParseError` 显式报错。
    #[test]
    fn row_parsing_rejects_drifted_columns() {
        let conn = fixture();
        let item = create_intervention(&conn, &new_intervention("漂移"), 1_000).expect("登记");
        conn.execute(
            "UPDATE interventions SET status = 'paused' WHERE id = ?",
            duckdb::params![&item.id],
        )
        .expect("注入状态漂移");
        assert_eq!(
            list_interventions(&conn, None, None, 10)
                .expect_err("状态漂移必须报错")
                .code,
            ErrorCode::ParseError
        );
        conn.execute(
            "UPDATE interventions SET status = 'concluded', outcome = 'maybe' WHERE id = ?",
            duckdb::params![&item.id],
        )
        .expect("注入结论漂移");
        assert_eq!(
            list_interventions(&conn, None, None, 10)
                .expect_err("结论漂移必须报错")
                .code,
            ErrorCode::ParseError
        );
        conn.execute(
            "UPDATE interventions SET status = 'active', outcome = NULL, \
             recurrences_before = -1 WHERE id = ?",
            duckdb::params![&item.id],
        )
        .expect("注入负基线");
        assert_eq!(
            list_interventions(&conn, None, None, 10)
                .expect_err("负基线必须报错")
                .code,
            ErrorCode::ParseError
        );
    }
}
