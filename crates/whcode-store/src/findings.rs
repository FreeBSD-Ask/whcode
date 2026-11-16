//! # 评审发现台账（P2.2／FR-786／ADR-080）
//!
//! 把“评审发现的缺口”从散落的文档段落变成可查询、可加锁的记录。四要件必填——
//! **缺口**（观察到什么不对）、**有界影响**（最坏后果与范围）、**最小修复**（owner 能做的
//! 最小动作）、**验证路线**（怎么证明修好了）。缺任一项的登记会被拒绝：只有缺口的抱怨
//! 不可执行，只有修复的想法无法复核。
//!
//! **修复乐观锁**（ADR-080 决策 3）：每次状态迁移必须带 `expected_version`；更新语句
//! 带 `WHERE version = ?`，影响行数为 0 时区分“记录不存在”（`not_found`）与“版本冲突”
//! （`invalid_state`，消息给出期望与实际版本）。并发两人改同一条发现时，后到者得到冲突，
//! 不会静默覆盖前者的结论。
//!
//! 有界性：标题 ≤ [`MAX_FINDING_TITLE_CHARS`]、四要件各 ≤ [`MAX_FINDING_TEXT_CHARS`]、
//! 列表一次 ≤ [`MAX_FINDING_ROWS`] 条（超出由调用方分页或收窄条件，不静默截断）。
//!
//! 学习捕获扩展（P2.4／FR-794／ADR-084）：`pattern`（可选归类）、`recurrence_count`
//! （显式登记的复发次数）与 `dismiss_reason`（dismissed 迁移必填的封闭原因码，负面控制）。
//! 复发是**显式动作**（[`record_finding_recurrence`]）：只有 `fixed` 状态可登记——修复尚未
//! 声明完成的重开不计复发，防“重开即复发”的口径漂移。
//!
//! 边界：本模块只读写 `findings` 表；权限、字符串枚举解析与默认值归一在 `whcode-app::findings`。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

/// 标题字符上限。
pub const MAX_FINDING_TITLE_CHARS: usize = 200;

/// 四要件单段文本的字符上限。
pub const MAX_FINDING_TEXT_CHARS: usize = 2000;

/// 单次列表返回条数上限。
pub const MAX_FINDING_ROWS: usize = 200;

/// 发现状态（库内稳定名与枚举一一对应）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingStatus {
    /// 已登记、未开始修。
    Open,
    /// 正在修。
    Fixing,
    /// 已修（`fixed_at` 落时间）。
    Fixed,
    /// 判定不修（显式记录原因写在 `owner_fix` 或 `verification` 里）。
    Dismissed,
}

impl FindingStatus {
    /// 库内稳定名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Fixing => "fixing",
            Self::Fixed => "fixed",
            Self::Dismissed => "dismissed",
        }
    }

    /// 解析稳定名（未知取值返回 `None`，由调用方决定报错口径）。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "open" => Some(Self::Open),
            "fixing" => Some(Self::Fixing),
            "fixed" => Some(Self::Fixed),
            "dismissed" => Some(Self::Dismissed),
            _ => None,
        }
    }

    /// 是否允许从 `self` 迁移到 `next`（ADR-080 决策 4 的表；同态不算迁移）。
    #[must_use]
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Open, Self::Fixing | Self::Fixed | Self::Dismissed)
                | (Self::Fixing, Self::Fixed | Self::Dismissed | Self::Open)
                | (Self::Fixed, Self::Open)
                | (Self::Dismissed, Self::Open)
        )
    }
}

/// dismissed 的封闭原因码（P2.4／ADR-084 决策 3，ADR-080 决策 4 修订）：负面控制——
/// “不修”必须回答为什么，不接受无理由的关闭。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DismissReason {
    /// 按设计如此（行为即规格）。
    ByDesign,
    /// 已被后续方案取代。
    Superseded,
    /// 与既有发现重复。
    Duplicate,
    /// 代价超出收益（写明取舍）。
    Cost,
    /// 暂不处理（显式放弃，而非遗忘）。
    Wontfix,
}

impl DismissReason {
    /// 库内稳定名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ByDesign => "by-design",
            Self::Superseded => "superseded",
            Self::Duplicate => "duplicate",
            Self::Cost => "cost",
            Self::Wontfix => "wontfix",
        }
    }

    /// 解析稳定名。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "by-design" => Some(Self::ByDesign),
            "superseded" => Some(Self::Superseded),
            "duplicate" => Some(Self::Duplicate),
            "cost" => Some(Self::Cost),
            "wontfix" => Some(Self::Wontfix),
            _ => None,
        }
    }
}

/// 严重度（只三档：更多档位是假精度，ADR-080 决策 5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    /// 低：不影响正确性。
    Low,
    /// 中：影响可用性或可维护性。
    Medium,
    /// 高：安全、数据完整性或门禁失效。
    High,
}

impl FindingSeverity {
    /// 库内稳定名。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// 解析稳定名。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// 新发现的登记请求（四要件均须非空）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewFinding {
    /// 关联会话（可选：跨会话的工程发现没有会话归属）。
    pub session_id: Option<String>,
    /// 标题（一句话说明是什么缺口）。
    pub title: String,
    /// 缺口：观察到什么不对。
    pub gap: String,
    /// 有界影响：最坏后果与范围。
    pub impact: String,
    /// 最小修复：owner 能执行的最小动作。
    pub owner_fix: String,
    /// 验证路线：怎么证明修好了。
    pub verification: String,
    /// 严重度。
    pub severity: FindingSeverity,
}

/// 一条发现（读出的完整行）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// `FND-<序号>`。
    pub id: String,
    /// 关联会话。
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
    /// 状态。
    pub status: FindingStatus,
    /// 严重度。
    pub severity: FindingSeverity,
    /// 乐观锁版本（每次迁移 +1）。
    pub version: i64,
    /// 登记时间（Unix 毫秒）。
    pub created_at_ms: u64,
    /// 最近迁移时间。
    pub updated_at_ms: u64,
    /// 标记为 fixed 的时间（其余状态为 `None`）。
    pub fixed_at_ms: Option<u64>,
    /// 复发模式归类（可选；登记复发时提供即写回）。
    pub pattern: Option<String>,
    /// 显式登记的复发次数（0＝未复发过）。
    pub recurrence_count: u64,
    /// dismissed 原因码（仅在 dismissed 迁移时写入；历史行为 `None`）。
    pub dismiss_reason: Option<DismissReason>,
}

const FINDING_COLUMNS: &str = "id, session_id, title, gap, impact, owner_fix, verification, \
     status, severity, version, epoch_ms(created_at), epoch_ms(updated_at), epoch_ms(fixed_at), \n     pattern, recurrence_count, dismiss_reason";

fn finding_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!("findings {context} 失败：{err}"),
    )
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::invalid_state(message.into())
}

fn text_within(value: &str, name: &str, limit: usize) -> Result<(), WhError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!(
            "finding 的 {name} 不得为空：四要件缺一不可（缺口／影响／最小修复／验证路线）"
        )));
    }
    if trimmed.chars().count() > limit {
        return Err(invalid(format!("finding 的 {name} 超过 {limit} 字符上限")));
    }
    Ok(())
}

fn row_to_finding(row: &duckdb::Row<'_>) -> Result<Finding, WhError> {
    let status_text: String = row.get(7).map_err(|err| finding_err("读取状态", err))?;
    let severity_text: String = row.get(8).map_err(|err| finding_err("读取严重度", err))?;
    let status = FindingStatus::parse(&status_text).ok_or_else(|| {
        WhError::new(
            ErrorCode::ParseError,
            format!("findings.status 出现未知取值 `{status_text}`：库内容漂移，需人工核对"),
        )
    })?;
    let severity = FindingSeverity::parse(&severity_text).ok_or_else(|| {
        WhError::new(
            ErrorCode::ParseError,
            format!("findings.severity 出现未知取值 `{severity_text}`：库内容漂移，需人工核对"),
        )
    })?;
    let created_at_ms: i64 = row
        .get(10)
        .map_err(|err| finding_err("读取创建时间", err))?;
    let updated_at_ms: i64 = row
        .get(11)
        .map_err(|err| finding_err("读取更新时间", err))?;
    let fixed_at_ms: Option<i64> = row
        .get(12)
        .map_err(|err| finding_err("读取修复时间", err))?;
    let pattern: Option<String> = row
        .get(13)
        .map_err(|err| finding_err("读取复发模式", err))?;
    let recurrence_count: i64 = row
        .get(14)
        .map_err(|err| finding_err("读取复发计数", err))?;
    let dismiss_text: Option<String> = row
        .get(15)
        .map_err(|err| finding_err("读取关闭原因", err))?;
    let dismiss_reason = dismiss_text
        .as_deref()
        .map(|text| {
            DismissReason::parse(text).ok_or_else(|| {
                WhError::new(
                    ErrorCode::ParseError,
                    format!(
                        "findings.dismiss_reason 出现未知取值 `{text}`：库内容漂移，需人工核对"
                    ),
                )
            })
        })
        .transpose()?;
    if recurrence_count < 0 {
        return Err(WhError::new(
            ErrorCode::ParseError,
            format!(
                "findings.recurrence_count 为负数（{recurrence_count}）：库内容漂移，需人工核对"
            ),
        ));
    }
    Ok(Finding {
        id: row.get(0).map_err(|err| finding_err("读取 id", err))?,
        session_id: row.get(1).map_err(|err| finding_err("读取会话", err))?,
        title: row.get(2).map_err(|err| finding_err("读取标题", err))?,
        gap: row.get(3).map_err(|err| finding_err("读取缺口", err))?,
        impact: row.get(4).map_err(|err| finding_err("读取影响", err))?,
        owner_fix: row.get(5).map_err(|err| finding_err("读取最小修复", err))?,
        verification: row.get(6).map_err(|err| finding_err("读取验证路线", err))?,
        status,
        severity,
        version: row.get(9).map_err(|err| finding_err("读取版本", err))?,
        created_at_ms: u64::try_from(created_at_ms).unwrap_or(0),
        updated_at_ms: u64::try_from(updated_at_ms).unwrap_or(0),
        fixed_at_ms: fixed_at_ms.and_then(|value| u64::try_from(value).ok()),
        pattern,
        recurrence_count: u64::try_from(recurrence_count).unwrap_or(0),
        dismiss_reason,
    })
}

/// 登记一条发现（四要件全部校验后写入；版本从 1 开始）。
///
/// # Errors
///
/// 任一要件为空或超长（`invalid_state`）、写入失败（`io_error`）时返回 [`WhError`]。
pub fn create_finding(
    conn: &Connection,
    new: &NewFinding,
    now_ms: u64,
) -> Result<Finding, WhError> {
    text_within(&new.title, "标题", MAX_FINDING_TITLE_CHARS)?;
    text_within(&new.gap, "缺口", MAX_FINDING_TEXT_CHARS)?;
    text_within(&new.impact, "有界影响", MAX_FINDING_TEXT_CHARS)?;
    text_within(&new.owner_fix, "最小修复", MAX_FINDING_TEXT_CHARS)?;
    text_within(&new.verification, "验证路线", MAX_FINDING_TEXT_CHARS)?;
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let id: String = conn
        .query_row(
            "INSERT INTO findings
                (id, session_id, title, gap, impact, owner_fix, verification, status, severity,
                 version, created_at, updated_at)
             VALUES ('FND-' || nextval('whcode_finding_id_seq'), ?, ?, ?, ?, ?, ?, 'open', ?,
                     1, epoch_ms(?), epoch_ms(?))
             RETURNING id",
            duckdb::params![
                new.session_id.as_deref(),
                new.title.trim(),
                new.gap.trim(),
                new.impact.trim(),
                new.owner_fix.trim(),
                new.verification.trim(),
                new.severity.as_str(),
                now,
                now,
            ],
            |row| row.get(0),
        )
        .map_err(|err| finding_err("登记", err))?;
    get_finding(conn, &id)?
        .ok_or_else(|| WhError::new(ErrorCode::Internal, format!("finding {id} 登记后读回失败")))
}

/// 按 id 读取一条发现；不存在返回 `None`（正确的空）。
///
/// # Errors
///
/// 查询失败或库内出现未知枚举值时返回 [`WhError`]。
pub fn get_finding(conn: &Connection, id: &str) -> Result<Option<Finding>, WhError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {FINDING_COLUMNS} FROM findings WHERE id = ?"
        ))
        .map_err(|err| finding_err("准备查询", err))?;
    let mut rows = stmt
        .query(duckdb::params![id])
        .map_err(|err| finding_err("查询单条", err))?;
    match rows.next().map_err(|err| finding_err("遍历单条", err))? {
        Some(row) => Ok(Some(row_to_finding(row)?)),
        None => Ok(None),
    }
}

/// 列出发（可按状态过滤，`None`＝全部；按创建时间倒序；上限 [`MAX_FINDING_ROWS`]）。
///
/// # Errors
///
/// `limit` 为 0、查询失败或库内出现未知枚举值时返回 [`WhError`]。
pub fn list_findings(
    conn: &Connection,
    status: Option<FindingStatus>,
    limit: usize,
) -> Result<Vec<Finding>, WhError> {
    if limit == 0 {
        return Err(invalid("list_findings 的 limit 必须大于 0"));
    }
    let limit = limit.min(MAX_FINDING_ROWS);
    let limit_i64 = i64::try_from(limit).map_err(|_| invalid("limit 超出 i64 范围"))?;
    let sql = match status {
        Some(_) => format!(
            "SELECT {FINDING_COLUMNS} FROM findings WHERE status = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        ),
        None => format!(
            "SELECT {FINDING_COLUMNS} FROM findings ORDER BY created_at DESC, id DESC LIMIT ?"
        ),
    };
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|err| finding_err("准备列表", err))?;
    let mut rows = match status {
        Some(status) => stmt.query(duckdb::params![status.as_str(), limit_i64]),
        None => stmt.query(duckdb::params![limit_i64]),
    }
    .map_err(|err| finding_err("列表查询", err))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|err| finding_err("遍历列表", err))? {
        out.push(row_to_finding(row)?);
    }
    Ok(out)
}

/// 迁移状态（乐观锁：`expected_version` 必须与当前版本一致）。
///
/// # Errors
///
/// 记录不存在（`not_found`）、版本冲突（`invalid_state`，消息含期望与实际版本）、
/// 非法迁移（`invalid_state`）或写入失败（`io_error`）时返回 [`WhError`]。
pub fn transition_finding(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    next: FindingStatus,
    dismiss_reason: Option<DismissReason>,
    now_ms: u64,
) -> Result<Finding, WhError> {
    let current = get_finding(conn, id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("finding {id} 不存在")))?;
    if current.version != expected_version {
        return Err(version_conflict(id, expected_version, current.version));
    }
    if !current.status.can_transition_to(next) {
        return Err(invalid(format!(
            "finding {id} 不允许从 {} 迁移到 {}（ADR-080 决策 4 的迁移表）",
            current.status.as_str(),
            next.as_str()
        )));
    }
    // 负面控制（ADR-084 决策 3，ADR-080 决策 4 修订）：dismissed 必须回答“为什么不修”，
    // 且原因码只允许出现在 dismissed 迁移上（其余迁移带码属调用方口径错误）。
    match (next, dismiss_reason) {
        (FindingStatus::Dismissed, None) => {
            return Err(invalid(
                "dismissed 迁移必须带原因码（by-design／superseded／duplicate／cost／wontfix 之一）",
            ));
        }
        (FindingStatus::Dismissed, Some(_))
        | (FindingStatus::Open | FindingStatus::Fixing | FindingStatus::Fixed, None) => {}
        (_, Some(_)) => {
            return Err(invalid(
                "仅 dismissed 迁移可携带原因码：其余迁移与原因码不匹配",
            ));
        }
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let reason_text = dismiss_reason.map(|reason| reason.as_str().to_string());
    let changed = conn
        .execute(
            "UPDATE findings
                SET status = ?, version = version + 1, updated_at = epoch_ms(?),
                    fixed_at = CASE WHEN ? = 'fixed' THEN epoch_ms(?) ELSE fixed_at END,
                    dismiss_reason = CASE WHEN ? = 'dismissed' THEN ? ELSE dismiss_reason END
              WHERE id = ? AND version = ?",
            duckdb::params![
                next.as_str(),
                now,
                next.as_str(),
                now,
                next.as_str(),
                reason_text,
                id,
                expected_version,
            ],
        )
        .map_err(|err| finding_err("状态迁移", err))?;
    if changed == 0 {
        // 并发窗口：读取后、更新前被别人改过。重读一次给出实际版本。
        let actual = get_finding(conn, id)?.map_or(-1, |finding| finding.version);
        return Err(version_conflict(id, expected_version, actual));
    }
    get_finding(conn, id)?
        .ok_or_else(|| WhError::new(ErrorCode::Internal, format!("finding {id} 迁移后读回失败")))
}

/// 登记一次复发（P2.4／FR-794／ADR-084 决策 2）：显式动作，语义＝“修复声明完成后，
/// 同一问题再次出现”。仅 `fixed` 状态可登记：状态回 `open`（复用 ADR-080 的 fixed→open
/// 迁移）、`recurrence_count` +1；提供 `pattern` 时写回归类（已有归类时不传则保留）。
/// 乐观锁与 [`transition_finding`] 同款。
///
/// # Errors
///
/// 记录不存在（`not_found`）、版本冲突（`invalid_state`）、状态不是 `fixed`
/// （`invalid_state`）、pattern 超长（`invalid_state`）或写入失败（`io_error`）时返回
/// [`WhError`]。
pub fn record_finding_recurrence(
    conn: &Connection,
    id: &str,
    expected_version: i64,
    pattern: Option<&str>,
    now_ms: u64,
) -> Result<Finding, WhError> {
    let current = get_finding(conn, id)?
        .ok_or_else(|| WhError::new(ErrorCode::NotFound, format!("finding {id} 不存在")))?;
    if current.version != expected_version {
        return Err(version_conflict(id, expected_version, current.version));
    }
    if current.status != FindingStatus::Fixed {
        return Err(invalid(format!(
            "finding {id} 当前为 {}：仅 fixed 状态可登记复发——修复尚未声明完成的重开不计复发",
            current.status.as_str()
        )));
    }
    if let Some(pattern) = pattern {
        text_within(pattern, "复发模式归类", MAX_FINDING_TITLE_CHARS)?;
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;
    let changed = conn
        .execute(
            "UPDATE findings
                SET status = 'open', version = version + 1, updated_at = epoch_ms(?),
                    recurrence_count = recurrence_count + 1,
                    pattern = COALESCE(?, pattern)
              WHERE id = ? AND version = ?",
            duckdb::params![now, pattern, id, expected_version],
        )
        .map_err(|err| finding_err("登记复发", err))?;
    if changed == 0 {
        let actual = get_finding(conn, id)?.map_or(-1, |finding| finding.version);
        return Err(version_conflict(id, expected_version, actual));
    }
    get_finding(conn, id)?.ok_or_else(|| {
        WhError::new(
            ErrorCode::Internal,
            format!("finding {id} 复发登记后读回失败"),
        )
    })
}

/// 按会话列出发（台账视图用；`limit` 夹取到 [`MAX_FINDING_ROWS`]）。
///
/// # Errors
///
/// `limit` 为 0、查询失败或库内出现未知枚举值时返回 [`WhError`]。
pub fn list_findings_for_session(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<Finding>, WhError> {
    if limit == 0 {
        return Err(invalid("list_findings_for_session 的 limit 必须大于 0"));
    }
    let limit_i64 =
        i64::try_from(limit.min(MAX_FINDING_ROWS)).map_err(|_| invalid("limit 超出 i64 范围"))?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {FINDING_COLUMNS} FROM findings WHERE session_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT ?"
        ))
        .map_err(|err| finding_err("准备按会话查询", err))?;
    let mut rows = stmt
        .query(duckdb::params![session_id, limit_i64])
        .map_err(|err| finding_err("按会话查询", err))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|err| finding_err("遍历会话发现", err))?
    {
        out.push(row_to_finding(row)?);
    }
    Ok(out)
}

fn version_conflict(id: &str, expected: i64, actual: i64) -> WhError {
    invalid(format!(
        "finding {id} 版本冲突：期望版本 {expected}，当前 {actual}——先重读再提交（乐观锁，ADR-080 决策 3）"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn new_finding(title: &str) -> NewFinding {
        NewFinding {
            session_id: Some("S-1".to_string()),
            title: title.to_string(),
            gap: "评审发现 X 未覆盖失败路径".to_string(),
            impact: "失败路径静默成功，用户误以为已交付".to_string(),
            owner_fix: "补失败路径断言".to_string(),
            verification: "cargo test -p whcode-app <用例名> 变红后转绿".to_string(),
            severity: FindingSeverity::Medium,
        }
    }

    /// 四要件齐备才写入；缺一即拒绝（错误消息点出缺哪一项）。
    #[test]
    fn create_requires_all_four_elements() {
        let conn = fixture();
        let finding = create_finding(&conn, &new_finding("X 缺失败路径"), 1_000).expect("登记");
        assert_eq!(finding.status, FindingStatus::Open);
        assert_eq!(finding.version, 1);
        assert_eq!(finding.severity, FindingSeverity::Medium);
        assert!(finding.id.starts_with("FND-"), "{}", finding.id);

        for field in ["gap", "impact", "owner_fix", "verification"] {
            let mut blank = new_finding("空要件");
            match field {
                "gap" => blank.gap = "   ".to_string(),
                "impact" => blank.impact = String::new(),
                "owner_fix" => blank.owner_fix = "\t".to_string(),
                _ => blank.verification = " ".to_string(),
            }
            let err = create_finding(&conn, &blank, 1_000).expect_err("缺要件必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState);
            assert!(err.message.contains("四要件"), "{}", err.message);
        }

        let mut long = new_finding("x");
        long.owner_fix = "长".repeat(MAX_FINDING_TEXT_CHARS + 1);
        assert_eq!(
            create_finding(&conn, &long, 1_000)
                .expect_err("超长必须拒绝")
                .code,
            ErrorCode::InvalidState
        );
    }

    /// 乐观锁：过期版本被拒绝，消息给出期望与实际版本；最新版本可成功。
    #[test]
    fn transition_rejects_stale_version() {
        let conn = fixture();
        let finding = create_finding(&conn, &new_finding("并发"), 1_000).expect("登记");
        let fixing = transition_finding(&conn, &finding.id, 1, FindingStatus::Fixing, None, 2_000)
            .expect("开始修");
        assert_eq!(fixing.version, 2);

        let err = transition_finding(&conn, &finding.id, 1, FindingStatus::Fixed, None, 3_000)
            .expect_err("过期版本必须冲突");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("版本冲突"), "{}", err.message);
        assert!(err.message.contains("期望版本 1"), "{}", err.message);
        assert!(err.message.contains("当前 2"), "{}", err.message);

        let fixed = transition_finding(&conn, &finding.id, 2, FindingStatus::Fixed, None, 3_000)
            .expect("按最新版本修完");
        assert_eq!(fixed.status, FindingStatus::Fixed);
        assert_eq!(fixed.version, 3);
        assert_eq!(fixed.fixed_at_ms, Some(3_000), "fixed 必须落修复时间");
    }

    /// 非法迁移被拒绝（fixed→fixing），且不改版本（失败不消耗乐观锁）。
    #[test]
    fn illegal_transition_is_rejected_without_bumping_version() {
        let conn = fixture();
        let finding = create_finding(&conn, &new_finding("迁移表"), 1_000).expect("登记");
        transition_finding(&conn, &finding.id, 1, FindingStatus::Fixed, None, 2_000)
            .expect("直接修完");
        let err = transition_finding(&conn, &finding.id, 2, FindingStatus::Fixing, None, 3_000)
            .expect_err("fixed→fixing 非法");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("迁移表"), "{}", err.message);
        let reread = get_finding(&conn, &finding.id)
            .expect("读回")
            .expect("存在");
        assert_eq!(reread.version, 2, "失败不得消耗版本");
        assert_eq!(reread.status, FindingStatus::Fixed);
    }

    /// 列表：状态过滤、创建时间倒序、limit 生效且 0 被拒绝。
    #[test]
    fn list_filters_by_status_and_orders_newest_first() {
        let conn = fixture();
        for index in 0..3 {
            let mut item = new_finding(&format!("第 {index} 条"));
            item.severity = FindingSeverity::Low;
            create_finding(&conn, &item, 1_000 + index).expect("登记");
        }
        let all = list_findings(&conn, None, 10).expect("全部");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].title, "第 2 条", "最新登记在前");

        let open = list_findings(&conn, Some(FindingStatus::Open), 10).expect("open");
        assert_eq!(open.len(), 3);
        let dismissed =
            list_findings(&conn, Some(FindingStatus::Dismissed), 10).expect("dismissed");
        assert!(dismissed.is_empty(), "无记录＝正确的空");

        assert_eq!(list_findings(&conn, None, 2).expect("limit").len(), 2);
        assert_eq!(
            list_findings(&conn, None, 0).expect_err("limit=0").code,
            ErrorCode::InvalidState
        );
    }

    /// 按会话过滤：只返回该会话的记录（跨会话记录不混入）。
    #[test]
    fn session_filter_excludes_other_sessions() {
        let conn = fixture();
        let mut other = new_finding("别的会话");
        other.session_id = Some("S-2".to_string());
        create_finding(&conn, &new_finding("本会话"), 1_000).expect("登记 S-1");
        create_finding(&conn, &other, 2_000).expect("登记 S-2");
        let rows = list_findings_for_session(&conn, "S-1", 10).expect("按会话");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "本会话");
        assert!(
            list_findings_for_session(&conn, "S-404", 10)
                .expect("无记录")
                .is_empty()
        );
        assert_eq!(
            list_findings_for_session(&conn, "S-1", 0)
                .expect_err("limit=0")
                .code,
            ErrorCode::InvalidState
        );
    }

    /// 未知状态迁移目标与不存在的 id：显式错误，不静默。
    #[test]
    fn unknown_id_and_missing_record_are_explicit() {
        let conn = fixture();
        let err = transition_finding(&conn, "FND-9999", 1, FindingStatus::Fixed, None, 1_000)
            .expect_err("不存在必须报错");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(get_finding(&conn, "FND-9999").expect("查询").is_none());
        assert_eq!(FindingStatus::parse("done"), None, "未知状态名不静默映射");
    }

    /// dismissed 的负面控制：必须带封闭原因码，且原因码不得出现在其他迁移上。
    #[test]
    fn dismiss_requires_reason_and_isolates_reason_to_dismissed() {
        let conn = fixture();
        let finding = create_finding(&conn, &new_finding("负面控制"), 1_000).expect("登记");
        let err = transition_finding(&conn, &finding.id, 1, FindingStatus::Dismissed, None, 2_000)
            .expect_err("dismissed 无原因码必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("原因码"), "{}", err.message);

        let dismissed = transition_finding(
            &conn,
            &finding.id,
            1,
            FindingStatus::Dismissed,
            Some(DismissReason::ByDesign),
            2_500,
        )
        .expect("带原因码关闭");
        assert_eq!(dismissed.status, FindingStatus::Dismissed);
        assert_eq!(dismissed.dismiss_reason, Some(DismissReason::ByDesign));

        let other = create_finding(&conn, &new_finding("原因码越界"), 3_000).expect("登记");
        let err = transition_finding(
            &conn,
            &other.id,
            1,
            FindingStatus::Fixing,
            Some(DismissReason::Cost),
            3_500,
        )
        .expect_err("非 dismissed 迁移带原因码必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("仅 dismissed"), "{}", err.message);
    }

    /// 三个枚举的稳定名与解析：全变体往返一致，未知值返回 `None`（不静默映射）。
    #[test]
    fn enum_parsers_round_trip_and_reject_unknown() {
        for status in [
            FindingStatus::Open,
            FindingStatus::Fixing,
            FindingStatus::Fixed,
            FindingStatus::Dismissed,
        ] {
            assert_eq!(FindingStatus::parse(status.as_str()), Some(status));
        }
        for reason in [
            DismissReason::ByDesign,
            DismissReason::Superseded,
            DismissReason::Duplicate,
            DismissReason::Cost,
            DismissReason::Wontfix,
        ] {
            assert_eq!(DismissReason::parse(reason.as_str()), Some(reason));
        }
        for severity in [
            FindingSeverity::Low,
            FindingSeverity::Medium,
            FindingSeverity::High,
        ] {
            assert_eq!(FindingSeverity::parse(severity.as_str()), Some(severity));
        }
        assert_eq!(DismissReason::parse("nope"), None);
        assert_eq!(FindingSeverity::parse("critical"), None);
    }

    /// 复发登记的前置条件：仅 fixed 可登记；版本过期、模式超长与缺失 id 都显式拒绝；
    /// 不传模式时保留既有归类（`COALESCE` 语义）。
    #[test]
    fn recurrence_requires_fixed_state_and_preserves_pattern() {
        let conn = fixture();
        let finding = create_finding(&conn, &new_finding("复发前置"), 1_000).expect("登记");
        let err = record_finding_recurrence(&conn, &finding.id, 1, None, 2_000)
            .expect_err("open 状态不得登记复发");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("仅 fixed"), "{}", err.message);

        assert_eq!(
            record_finding_recurrence(&conn, "FND-9999", 1, None, 2_000)
                .expect_err("不存在必须报错")
                .code,
            ErrorCode::NotFound
        );

        transition_finding(&conn, &finding.id, 1, FindingStatus::Fixed, None, 3_000).expect("修复");
        assert_eq!(
            record_finding_recurrence(&conn, &finding.id, 1, None, 3_500)
                .expect_err("版本过期必须拒绝")
                .code,
            ErrorCode::InvalidState
        );
        let too_long = "x".repeat(MAX_FINDING_TITLE_CHARS + 1);
        assert_eq!(
            record_finding_recurrence(&conn, &finding.id, 2, Some(&too_long), 3_800)
                .expect_err("模式超长必须拒绝")
                .code,
            ErrorCode::InvalidState
        );

        let recurred =
            record_finding_recurrence(&conn, &finding.id, 2, Some("平台差异断言"), 4_000)
                .expect("登记复发");
        assert_eq!(recurred.status, FindingStatus::Open);
        assert_eq!(recurred.recurrence_count, 1);
        assert_eq!(recurred.pattern.as_deref(), Some("平台差异断言"));

        transition_finding(
            &conn,
            &finding.id,
            recurred.version,
            FindingStatus::Fixed,
            None,
            5_000,
        )
        .expect("再修");
        let again =
            record_finding_recurrence(&conn, &finding.id, recurred.version + 1, None, 6_000)
                .expect("二次复发");
        assert_eq!(again.recurrence_count, 2);
        assert_eq!(
            again.pattern.as_deref(),
            Some("平台差异断言"),
            "不传模式时保留既有归类"
        );
    }

    /// 库内容漂移：未知状态／严重度／原因码与负计数按 `ParseError` 显式报错。
    #[test]
    fn row_parsing_rejects_drifted_columns() {
        let conn = fixture();
        let finding = create_finding(&conn, &new_finding("漂移"), 1_000).expect("登记");
        conn.execute(
            "UPDATE findings SET status = 'yanked' WHERE id = ?",
            duckdb::params![&finding.id],
        )
        .expect("注入状态漂移");
        assert_eq!(
            list_findings(&conn, None, 10)
                .expect_err("状态漂移必须报错")
                .code,
            ErrorCode::ParseError
        );
        conn.execute(
            "UPDATE findings SET status = 'open', severity = 'critical' WHERE id = ?",
            duckdb::params![&finding.id],
        )
        .expect("注入严重度漂移");
        assert_eq!(
            list_findings(&conn, None, 10)
                .expect_err("严重度漂移必须报错")
                .code,
            ErrorCode::ParseError
        );
        conn.execute(
            "UPDATE findings SET severity = 'medium', dismiss_reason = 'meh' WHERE id = ?",
            duckdb::params![&finding.id],
        )
        .expect("注入原因码漂移");
        assert_eq!(
            list_findings(&conn, None, 10)
                .expect_err("原因码漂移必须报错")
                .code,
            ErrorCode::ParseError
        );
        conn.execute(
            "UPDATE findings SET dismiss_reason = NULL, recurrence_count = -1 WHERE id = ?",
            duckdb::params![&finding.id],
        )
        .expect("注入负计数");
        assert_eq!(
            list_findings(&conn, None, 10)
                .expect_err("负计数必须报错")
                .code,
            ErrorCode::ParseError
        );
    }
}
