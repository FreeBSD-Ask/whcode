//! # 会话归档、按时间清理与 TTL（T039、FR-508/509、§10.3、V-107/SC-007）
//!
//! 清理以**会话为根**：删除一个会话意味着删除它的全部从属行——`tasks`、`checkpoints` 与
//! `runtime_executions`（经 `task_id` 关联）、`events`、`messages`、`stage_artifacts`、
//! `context_windows`、`session_overview`，最后是 `sessions` 本行。以下内容**不在**清理范围：
//!
//! - `audit`：审计不随普通会话 TTL 自动清除（data-model.md §1、§10.3）；
//! - `workspace_meta` / `runtime_state` / `schema_migrations`：全局状态；
//! - `tool_executions`：无会话关联（经 `tool_call_id`），生命周期由工具执行流程管理。
//!
//! ## FR-509 四项机制
//!
//! 1. **预览**：[`preview_purge`] 只读地冻结清理集（会话清单 + 各表行数），不写库；
//!    预览输出与实际删除集合一致由冻结清单保证（Check 39）。
//! 2. **确认**：[`execute_purge`] 要求 `confirm == true`，否则拒绝执行；CUI 交互确认由
//!    上层（T086）承担，本层不做隐式默认。
//! 3. **审计**：删除与 `audit` 行（`db_purge`）在**同一事务**内提交；审计写失败则整个
//!    清理回滚（domain `AuditSink` 契约：审计失败不得视为操作成功）。
//! 4. **活跃会话保护**：两种机制叠加——调用方显式传入的 [`PurgeProtection::protect_ids`]
//!    （应用层打开的会话）与近期活跃窗口 [`PurgeProtection::active_window_ms`]（默认
//!    24 小时）。命中保护的会话按 [`PurgeProtection::on_protected`] 跳过（默认）或整体
//!    拒绝。保护在**预览与执行两个时点各检查一次**：预览后新变活跃或新加入保护名单的
//!    会话在执行时被跳过并在结果中报告（Check 208）。
//!
//! ## 一致性边界
//!
//! 行数统计是**预览时点**的读数；执行时以 `DELETE` 实际行数为准并原样返回。会话**集合**
//! 由冻结清单保证；执行时活跃保护复检导致的跳过会让实际删除集合小于预览集合，跳过项
//! 一律在 [`PurgeResult::skipped_protected`] 中列出，不静默。
//!
//! ## TTL 与活跃窗口的交互
//!
//! 活跃窗口优先于时间判据：若 TTL/`before` 的截止点落在活跃窗口内（默认 24 小时），
//! 全部候选都会因"近期活跃"受保护，清理不会删除任何会话——这是保护机制的正确行为，
//! 不是缺陷。需要更激进的 TTL 时，调用方应显式调小 `active_window_ms`，或以显式
//! `protect_ids` 名单替代窗口保护；两种机制都改变不了 `audit` 不随 TTL 清除的约束。
//!
//! 时间基点 `now_ms` 由调用方显式传入（可测试、无隐式系统时钟）；TTL 截止时间在预览时
//! 解析并冻结进 [`PurgePreview`]。
//!
//! 边界：本模块只接受 [`duckdb::Connection`]，不打开连接；权限判定属应用层（§10.2），
//! 这里只保证存储侧的确认与审计不可绕过。

use std::collections::{BTreeMap, BTreeSet};

use duckdb::{Connection, params, params_from_iter};
use serde::Serialize;
use whcode_domain::{AuditAction, ErrorCode, WhError};

use crate::schema::checksum;

/// 一次 IN 查询/删除的最大占位符数量（与写者分块一致，避免超长参数列表）。
pub const IDS_PER_QUERY: usize = 256;

/// 默认活跃窗口：`last_active_at` 距 `now_ms` 不足该时长的会话视为活跃，受保护。
pub const DEFAULT_ACTIVE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// 审计 `detail_json` 中会话清单的条目上限；超出部分截断并标记 `truncated`。
const AUDIT_SESSION_LIST_CAP: usize = 100;

/// 清理判据：按绝对时间点、按 TTL（相对执行时刻）或按会话 ID 清单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum PurgeCriterion {
    /// 清理 `last_active_at` 早于该 Unix 毫秒时间戳的会话（`whcode db purge --before`）。
    Before {
        /// Unix 毫秒截止时间（不含）。
        ts_ms: i64,
    },
    /// 清理 `last_active_at` 早于 `now_ms - ttl` 的会话。
    Ttl {
        /// TTL 时长（秒），不得为负。
        seconds: i64,
    },
    /// 清理清单中的会话（`session delete` 命令；活跃保护与审计全链同样生效）。
    Sessions {
        /// 目标会话 ID（去重后生效）。
        ids: Vec<String>,
    },
}

impl PurgeCriterion {
    /// 稳定模式名（入审计与展示）。
    #[must_use]
    pub const fn mode(&self) -> &'static str {
        match self {
            Self::Before { .. } => "before",
            Self::Ttl { .. } => "ttl",
            Self::Sessions { .. } => "sessions",
        }
    }
}

/// 清理判据与范围过滤。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgeCriteria {
    /// 判据。
    pub criterion: PurgeCriterion,
    /// 为真时只考虑 `archived = TRUE` 的会话（先归档、后清理的工作流；仅时间判据生效）。
    pub archived_only: bool,
}

impl PurgeCriteria {
    /// 按绝对时间点清理。
    #[must_use]
    pub const fn before(ts_ms: i64) -> Self {
        Self {
            criterion: PurgeCriterion::Before { ts_ms },
            archived_only: false,
        }
    }

    /// 按 TTL 清理。
    #[must_use]
    pub const fn ttl(seconds: i64) -> Self {
        Self {
            criterion: PurgeCriterion::Ttl { seconds },
            archived_only: false,
        }
    }

    /// 按会话 ID 清单清理（会话删除命令的存储路径）。
    #[must_use]
    pub fn sessions<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            criterion: PurgeCriterion::Sessions {
                ids: ids.into_iter().map(Into::into).collect(),
            },
            archived_only: false,
        }
    }

    /// 只清理已归档会话。
    #[must_use]
    pub const fn archived_only(mut self) -> Self {
        self.archived_only = true;
        self
    }
}

/// 活跃会话命中保护时的处置策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OnProtected {
    /// 跳过受保护会话，清理其余（默认；Check 39 允许“拒绝或跳过”两种语义）。
    Skip,
    /// 存在受保护会话时整体拒绝，不做部分清理。
    Abort,
}

/// 活跃会话保护策略（FR-509）。
///
/// 默认值可见、可改：`protect_ids` 为空、活跃窗口 24 小时、命中即跳过。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgeProtection {
    /// 显式受保护的会话 ID（应用层当前打开的会话）。
    pub protect_ids: Vec<String>,
    /// 近期活跃窗口（毫秒）；`last_active_at >= now_ms - active_window_ms` 视为活跃。
    pub active_window_ms: i64,
    /// 命中保护时的处置。
    pub on_protected: OnProtected,
}

impl Default for PurgeProtection {
    fn default() -> Self {
        Self {
            protect_ids: Vec::new(),
            active_window_ms: DEFAULT_ACTIVE_WINDOW_MS,
            on_protected: OnProtected::Skip,
        }
    }
}

/// 各表实际/预估删除行数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct PurgeCounts {
    /// `sessions` 行数。
    pub sessions: i64,
    /// `tasks` 行数。
    pub tasks: i64,
    /// `checkpoints` 行数（经 `task_id` 关联）。
    pub checkpoints: i64,
    /// `runtime_executions` 行数（经 `task_id` 关联）。
    pub runtime_executions: i64,
    /// `stage_artifacts` 行数。
    pub stage_artifacts: i64,
    /// `events` 行数（仅 `session_id` 非空的行；全局事件不受影响）。
    pub events: i64,
    /// `messages` 行数。
    pub messages: i64,
    /// `context_windows` 行数。
    pub context_windows: i64,
    /// `session_overview` 行数。
    pub session_overview: i64,
}

/// 清理预览（也是执行的冻结计划）。
///
/// 只能由 [`preview_purge`] 产生；执行按 `selected_sessions` 冻结清单进行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgePreview {
    /// 冻结的判据与范围过滤。
    pub criteria: PurgeCriteria,
    /// 冻结的截止时间（Unix 毫秒）：`Before` 为判据本身，`Ttl` 为预览时解析值。
    pub cutoff_ms: i64,
    /// 将被清理的会话 ID（已排除预览时点受保护的会话）。
    pub selected_sessions: Vec<String>,
    /// 预览时点因活跃保护被跳过的会话 ID（排序去重）。
    pub skipped_protected: Vec<String>,
    /// 预览时点各表行数（执行时以实际删除行数为准）。
    pub counts: PurgeCounts,
}

/// 清理执行结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PurgeResult {
    /// 实际删除的会话 ID。
    pub deleted_sessions: Vec<String>,
    /// 全部因活跃保护被跳过的会话 ID（预览时点 + 执行时点复检，排序去重）。
    pub skipped_protected: Vec<String>,
    /// 实际删除行数。
    pub deleted: PurgeCounts,
}

fn db_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

fn rows_i64(rows: usize) -> i64 {
    i64::try_from(rows).unwrap_or(i64::MAX)
}

/// 把调用方传入的 `now_ms` 校验为 `i64`（`epoch_ms` 参数范围）。
fn ms_i64(now_ms: u64) -> Result<i64, WhError> {
    i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围（须为 0..=i64::MAX 毫秒）"))
}

/// 解析判据为冻结截止时间；TTL 为负时拒绝。
///
/// `Sessions` 判据无时间语义，截止点取执行时刻（活跃窗口保护仍按候选的
/// `last_active_at` 独立评估）。
fn resolve_cutoff(criteria: &PurgeCriteria, now_ms: i64) -> Result<i64, WhError> {
    match criteria.criterion {
        PurgeCriterion::Before { ts_ms } => Ok(ts_ms),
        PurgeCriterion::Sessions { .. } => Ok(now_ms),
        PurgeCriterion::Ttl { seconds } => {
            if seconds < 0 {
                return Err(invalid("TTL 不能为负"));
            }
            Ok(now_ms.saturating_sub(seconds.saturating_mul(1000)))
        }
    }
}

/// 把会话清单转成审计用 JSON；超过上限时截断并标记。
fn session_list_json(ids: &[String]) -> serde_json::Value {
    if ids.len() <= AUDIT_SESSION_LIST_CAP {
        serde_json::json!(ids)
    } else {
        serde_json::json!({
            "first": &ids[..AUDIT_SESSION_LIST_CAP],
            "truncated": true,
            "total": ids.len(),
        })
    }
}

/// 分块统计 `table.column` 命中 `ids` 的行数（表/列名为 crate 内常量，非用户输入）。
fn count_by_ids(
    conn: &Connection,
    table: &str,
    column: &str,
    ids: &[String],
) -> Result<i64, WhError> {
    let mut total = 0i64;
    for chunk in ids.chunks(IDS_PER_QUERY) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!("SELECT count(*) FROM {table} WHERE {column} IN ({placeholders})");
        let rows: i64 = conn
            .query_row(&sql, params_from_iter(chunk.iter()), |row| row.get(0))
            .map_err(|err| db_err(&format!("统计 {table} 行数"), err))?;
        total += rows;
    }
    Ok(total)
}

/// 分块删除 `table.column` 命中 `ids` 的行，返回实际删除行数。
fn delete_by_ids(
    conn: &Connection,
    table: &str,
    column: &str,
    ids: &[String],
) -> Result<usize, WhError> {
    let mut total = 0usize;
    for chunk in ids.chunks(IDS_PER_QUERY) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!("DELETE FROM {table} WHERE {column} IN ({placeholders})");
        let rows = conn
            .execute(&sql, params_from_iter(chunk.iter()))
            .map_err(|err| db_err(&format!("删除 {table} 行"), err))?;
        total += rows;
    }
    Ok(total)
}

/// 汇总清理会话集合涉及的 `tasks.id`（经 `session_id` 关联，排序去重）。
fn task_ids_for_sessions(
    conn: &Connection,
    session_ids: &[String],
) -> Result<Vec<String>, WhError> {
    let mut ids = BTreeSet::new();
    for chunk in session_ids.chunks(IDS_PER_QUERY) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!("SELECT id FROM tasks WHERE session_id IN ({placeholders}) ORDER BY id");
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|err| db_err("查询任务清单", err))?;
        let rows = stmt
            .query_map(params_from_iter(chunk.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| db_err("查询任务清单", err))?;
        for row in rows {
            ids.insert(row.map_err(|err| db_err("查询任务清单", err))?);
        }
    }
    Ok(ids.into_iter().collect())
}

/// 在会话集合中找出显式受保护的 ID（排序去重）。
fn explicitly_protected(selected: &[String], protect_ids: &[String]) -> Vec<String> {
    let protect: BTreeSet<&str> = protect_ids.iter().map(String::as_str).collect();
    selected
        .iter()
        .filter(|id| protect.contains(id.as_str()))
        .cloned()
        .collect()
}

/// 全库近期活跃（`last_active_at >= now_ms - window`）的会话 ID；`sessions` 为元数据级
/// 小表，直接取回后在内存中与清理集求交，避免异构参数绑定。
fn recently_active_ids(
    conn: &Connection,
    now_ms: i64,
    active_window_ms: i64,
) -> Result<Vec<String>, WhError> {
    let window_start = now_ms.saturating_sub(active_window_ms);
    let mut stmt = conn
        .prepare("SELECT id FROM sessions WHERE last_active_at >= epoch_ms(?) ORDER BY id")
        .map_err(|err| db_err("查询近期活跃会话", err))?;
    let rows = stmt
        .query_map(params![window_start], |row| row.get::<_, String>(0))
        .map_err(|err| db_err("查询近期活跃会话", err))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|err| db_err("查询近期活跃会话", err))?);
    }
    Ok(ids)
}

/// 合并两批受保护会话：排序去重。
fn merge_protected(a: &[String], b: Vec<String>) -> Vec<String> {
    let mut merged: BTreeSet<String> = a.iter().cloned().collect();
    merged.extend(b);
    merged.into_iter().collect()
}

/// 归档或取消归档指定会话（FR-508）。
///
/// 全部 ID 必须存在，否则返回 `not_found` 且不做任何修改（全有或全无）；更新在单一
/// 事务内完成。归档是可逆的元数据标记，不删除数据，不写 `db_purge` 审计。
///
/// # Errors
///
/// 会话不存在、数据库查询或更新失败时返回 [`WhError`]。
pub fn set_archived(
    conn: &mut Connection,
    session_ids: &[&str],
    archived: bool,
) -> Result<usize, WhError> {
    if session_ids.is_empty() {
        return Ok(0);
    }
    let unique: Vec<String> = session_ids
        .iter()
        .map(|id| (*id).to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let tx = conn
        .transaction()
        .map_err(|err| db_err("开始归档事务", err))?;

    let mut found = BTreeSet::new();
    for chunk in unique.chunks(IDS_PER_QUERY) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!("SELECT id FROM sessions WHERE id IN ({placeholders}) ORDER BY id");
        let mut stmt = tx
            .prepare(&sql)
            .map_err(|err| db_err("核对会话存在性", err))?;
        let rows = stmt
            .query_map(params_from_iter(chunk.iter()), |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| db_err("核对会话存在性", err))?;
        for row in rows {
            found.insert(row.map_err(|err| db_err("核对会话存在性", err))?);
        }
    }
    let missing: Vec<String> = unique
        .iter()
        .filter(|id| !found.contains(*id))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在，归档未执行：{}", missing.join(", ")),
        ));
    }

    let mut updated = 0usize;
    for chunk in unique.chunks(IDS_PER_QUERY) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let flag_sql = if archived { "TRUE" } else { "FALSE" };
        let sql = format!("UPDATE sessions SET archived = {flag_sql} WHERE id IN ({placeholders})");
        let rows = tx
            .execute(sql.as_str(), params_from_iter(chunk.iter()))
            .map_err(|err| db_err("更新归档标记", err))?;
        updated += rows;
    }
    tx.commit().map_err(|err| db_err("提交归档事务", err))?;
    Ok(updated)
}

/// 预览清理（FR-509 第 1 项）：只读地冻结清理集，不写库。
///
/// 预览时点执行一次活跃会话保护检查；命中保护的会话进入 `skipped_protected`，
/// [`OnProtected::Abort`] 策略下直接拒绝。
///
/// # Errors
///
/// 判据非法（TTL 为负、活跃窗口为负）、`on_protected = Abort` 且命中保护、或数据库
/// 查询失败时返回 [`WhError`]。
pub fn preview_purge(
    conn: &Connection,
    criteria: &PurgeCriteria,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgePreview, WhError> {
    if protection.active_window_ms < 0 {
        return Err(invalid("活跃窗口不能为负"));
    }
    let now = ms_i64(now_ms)?;
    let cutoff = resolve_cutoff(criteria, now)?;

    // 候选查询：时间判据按范围选择；Sessions 判据按 ID 分块取（重复 ID 去重）。
    let mut candidates: BTreeMap<String, i64> = BTreeMap::new();
    match &criteria.criterion {
        PurgeCriterion::Before { .. } | PurgeCriterion::Ttl { .. } => {
            let archived_filter = if criteria.archived_only {
                " AND archived"
            } else {
                ""
            };
            let sql = format!(
                "SELECT id, epoch_us(last_active_at) FROM sessions
                 WHERE last_active_at < epoch_ms(?){archived_filter} ORDER BY id"
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|err| db_err("查询清理候选会话", err))?;
            let rows = stmt
                .query_map(params![cutoff], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|err| db_err("查询清理候选会话", err))?;
            for row in rows {
                let (id, active_us) = row.map_err(|err| db_err("读取清理候选会话", err))?;
                candidates.insert(id, active_us);
            }
        }
        PurgeCriterion::Sessions { ids } => {
            for chunk in ids.chunks(IDS_PER_QUERY) {
                let placeholders = vec!["?"; chunk.len()].join(", ");
                let sql = format!(
                    "SELECT id, epoch_us(last_active_at) FROM sessions
                     WHERE id IN ({placeholders}) ORDER BY id"
                );
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|err| db_err("查询清理候选会话", err))?;
                let rows = stmt
                    .query_map(params_from_iter(chunk.iter()), |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(|err| db_err("查询清理候选会话", err))?;
                for row in rows {
                    let (id, active_us) = row.map_err(|err| db_err("读取清理候选会话", err))?;
                    candidates.insert(id, active_us);
                }
            }
        }
    }

    let window_start = now.saturating_sub(protection.active_window_ms);
    let mut selected = Vec::new();
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    let mut conflicts: Vec<String> = Vec::new();
    for (id, active_us) in &candidates {
        let active_ms = active_us.div_euclid(1000);
        let protected = protection.protect_ids.iter().any(|p| p == id) || active_ms >= window_start;
        if !protected {
            selected.push(id.clone());
        } else {
            skipped.insert(id.clone());
            conflicts.push(id.clone());
        }
    }

    if protection.on_protected == OnProtected::Abort && !conflicts.is_empty() {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "活跃会话保护策略为拒绝执行，以下会话受保护：{}",
                conflicts.join(", ")
            ),
        ));
    }

    let task_ids = task_ids_for_sessions(conn, &selected)?;
    let counts = PurgeCounts {
        sessions: rows_i64(selected.len()),
        tasks: count_by_ids(conn, "tasks", "session_id", &selected)?,
        checkpoints: count_by_ids(conn, "checkpoints", "task_id", &task_ids)?,
        runtime_executions: count_by_ids(conn, "runtime_executions", "task_id", &task_ids)?,
        stage_artifacts: count_by_ids(conn, "stage_artifacts", "session_id", &selected)?,
        events: count_by_ids(conn, "events", "session_id", &selected)?,
        messages: count_by_ids(conn, "messages", "session_id", &selected)?,
        context_windows: count_by_ids(conn, "context_windows", "session_id", &selected)?,
        session_overview: count_by_ids(conn, "session_overview", "session_id", &selected)?,
    };

    Ok(PurgePreview {
        criteria: criteria.clone(),
        cutoff_ms: cutoff,
        selected_sessions: selected,
        skipped_protected: skipped.into_iter().collect(),
        counts,
    })
}

/// 执行清理（FR-509 第 2—4 项）：确认、执行时点活跃复检、事务内删除并写审计。
///
/// 行为约定：
/// - `confirm` 必须为 `true`，否则拒绝（`invalid_state`），不发生任何写操作；
/// - 执行时点**重新**检查活跃保护（显式名单 + 近期活跃窗口），预览后新受保护的会话
///   被跳过并在结果中报告；[`OnProtected::Abort`] 下整体拒绝；
/// - 删除与 `db_purge` 审计行在同一事务提交；任一步失败全部回滚；
/// - 空清理集仍是已发生的操作，同样写审计（计数为零）。
///
/// # Errors
///
/// 未确认、执行时点命中 `Abort` 保护、或数据库读写失败时返回 [`WhError`]。
pub fn execute_purge(
    conn: &mut Connection,
    preview: &PurgePreview,
    confirm: bool,
    actor: &str,
    protection: &PurgeProtection,
    now_ms: u64,
) -> Result<PurgeResult, WhError> {
    if !confirm {
        return Err(invalid(
            "清理必须显式确认（预览后由调用方确认执行），未执行任何删除",
        ));
    }
    if protection.active_window_ms < 0 {
        return Err(invalid("活跃窗口不能为负"));
    }
    let now = ms_i64(now_ms)?;

    let re_skipped = {
        let mut skipped = explicitly_protected(&preview.selected_sessions, &protection.protect_ids);
        let active: BTreeSet<String> = recently_active_ids(conn, now, protection.active_window_ms)?
            .into_iter()
            .collect();
        skipped.extend(
            preview
                .selected_sessions
                .iter()
                .filter(|id| active.contains(*id))
                .cloned(),
        );
        skipped.sort();
        skipped.dedup();
        skipped
    };

    if protection.on_protected == OnProtected::Abort && !re_skipped.is_empty() {
        return Err(WhError::new(
            ErrorCode::InvalidState,
            format!(
                "活跃会话保护策略为拒绝执行，以下会话受保护：{}",
                re_skipped.join(", ")
            ),
        ));
    }

    let protected: BTreeSet<String> = re_skipped.iter().cloned().collect();
    let final_sessions: Vec<String> = preview
        .selected_sessions
        .iter()
        .filter(|id| !protected.contains(*id))
        .cloned()
        .collect();
    let merged_skipped = merge_protected(&preview.skipped_protected, re_skipped);

    let tx = conn
        .transaction()
        .map_err(|err| db_err("开始清理事务", err))?;

    let task_ids = task_ids_for_sessions(&tx, &final_sessions)?;
    let deleted = PurgeCounts {
        checkpoints: rows_i64(delete_by_ids(&tx, "checkpoints", "task_id", &task_ids)?),
        runtime_executions: rows_i64(delete_by_ids(
            &tx,
            "runtime_executions",
            "task_id",
            &task_ids,
        )?),
        tasks: rows_i64(delete_by_ids(&tx, "tasks", "session_id", &final_sessions)?),
        stage_artifacts: rows_i64(delete_by_ids(
            &tx,
            "stage_artifacts",
            "session_id",
            &final_sessions,
        )?),
        events: rows_i64(delete_by_ids(&tx, "events", "session_id", &final_sessions)?),
        messages: rows_i64(delete_by_ids(
            &tx,
            "messages",
            "session_id",
            &final_sessions,
        )?),
        context_windows: rows_i64(delete_by_ids(
            &tx,
            "context_windows",
            "session_id",
            &final_sessions,
        )?),
        session_overview: rows_i64(delete_by_ids(
            &tx,
            "session_overview",
            "session_id",
            &final_sessions,
        )?),
        sessions: rows_i64(delete_by_ids(&tx, "sessions", "id", &final_sessions)?),
    };

    let audit_id = format!("purge-{}-{}", now, checksum(&final_sessions.join(",")));
    let detail = serde_json::json!({
        "criteria": {
            "mode": preview.criteria.criterion.mode(),
            "archived_only": preview.criteria.archived_only,
        },
        "cutoff_ms": preview.cutoff_ms,
        "deleted_sessions": final_sessions.len(),
        "sessions": session_list_json(&final_sessions),
        "skipped_protected": merged_skipped,
        "counts": deleted,
    });
    let target = match &preview.criteria.criterion {
        PurgeCriterion::Sessions { ids } => {
            format!("sessions by id ({}) [sessions]", ids.len())
        }
        _ => format!(
            "sessions last_active_at < epoch_ms({}) [{}]",
            preview.cutoff_ms,
            preview.criteria.criterion.mode()
        ),
    };
    tx.execute(
        "INSERT INTO audit (id, actor, action, target, detail_json, ts)
         VALUES (?, ?, ?, ?, ?, current_timestamp)",
        params![
            audit_id,
            actor,
            AuditAction::DbPurge.as_str(),
            target,
            detail.to_string(),
        ],
    )
    .map_err(|err| db_err("写入清理审计", err))?;

    tx.commit().map_err(|err| db_err("提交清理事务", err))?;

    Ok(PurgeResult {
        deleted_sessions: final_sessions,
        skipped_protected: merged_skipped,
        deleted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_700_000_100_000;
    const HOUR_MS: i64 = 60 * 60 * 1000;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        crate::schema::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_session(conn: &Connection, id: &str, last_active_ms: i64, archived: bool) {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
             VALUES (?, ?, epoch_ms(?), epoch_ms(?), NULL, ?, 'idle')",
            params![
                id,
                format!("会话 {id}"),
                last_active_ms,
                last_active_ms,
                archived
            ],
        )
        .expect("插入会话");
    }

    fn insert_task(conn: &Connection, id: &str, session_id: &str) {
        conn.execute(
            "INSERT INTO tasks (id, session_id, title, stage, status, parent_id,
                                acceptance_json, retry_count, updated_at)
             VALUES (?, ?, ?, 'implement', 'running', NULL, '[]', 0, current_timestamp)",
            params![id, session_id, format!("任务 {id}")],
        )
        .expect("插入任务");
    }

    fn insert_event(conn: &Connection, seq: i64, session_id: Option<&str>) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, 'SessionStarted', '{}',
                     current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}"), session_id],
        )
        .expect("插入事件");
    }

    fn insert_message(conn: &Connection, id: &str, session_id: &str) {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, 'user', ?, NULL, current_timestamp, NULL)",
            params![id, session_id, format!("ref-{id}")],
        )
        .expect("插入消息");
    }

    fn insert_checkpoint(conn: &Connection, id: &str, task_id: &str) {
        conn.execute(
            "INSERT INTO checkpoints (id, task_id, step, delta_json, message_cursor,
                                      artifact_refs_json, ts)
             VALUES (?, ?, 1, '{}', NULL, '[]', current_timestamp)",
            params![id, task_id],
        )
        .expect("插入检查点");
    }

    fn insert_runtime_execution(conn: &Connection, id: &str, task_id: &str) {
        conn.execute(
            "INSERT INTO runtime_executions (id, task_id, runtime, script_hash, input_hash,
                                             runtime_fingerprint, state, exit_code,
                                             duration_ms, peak_mb, stdout_ref, stderr_ref, ts)
             VALUES (?, ?, 'native', 'h', 'i', 'f', 'completed', 0, 1, 1, NULL, NULL,
                     current_timestamp)",
            params![id, task_id],
        )
        .expect("插入运行时执行记录");
    }

    fn insert_stage_artifact(conn: &Connection, id: &str, session_id: &str) {
        conn.execute(
            "INSERT INTO stage_artifacts (id, session_id, task_id, stage, artifact_type,
                                          content, refs_json, parser_version, validated,
                                          version, created_at)
             VALUES (?, ?, NULL, 'plan', 'spec', 'c', '[]', 'v1', FALSE, 1, current_timestamp)",
            params![id, session_id],
        )
        .expect("插入阶段产物");
    }

    fn insert_context_window(conn: &Connection, id: &str, session_id: &str) {
        conn.execute(
            "INSERT INTO context_windows (id, session_id, window_start_seq, window_end_seq,
                                          total_tokens, summary_ref, created_at)
             VALUES (?, ?, 1, 2, 10, NULL, current_timestamp)",
            params![id, session_id],
        )
        .expect("插入上下文窗口");
    }

    fn insert_session_overview(conn: &Connection, session_id: &str) {
        conn.execute(
            "INSERT INTO session_overview (session_id, by_stage_json, total, done, blocked,
                                           refreshed_at)
             VALUES (?, '{}', 0, 0, 0, current_timestamp)",
            params![session_id],
        )
        .expect("插入会话概览");
    }

    fn insert_audit(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO audit (id, actor, action, target, detail_json, ts)
             VALUES (?, 'tester', 'gate_override', 'gate:clarify', '{}', current_timestamp)",
            params![id],
        )
        .expect("插入审计行");
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect("计数")
    }

    fn seed_session_tree(conn: &Connection, session_id: &str, last_active_ms: i64) {
        insert_session(conn, session_id, last_active_ms, false);
        for seq in 1..=3 {
            insert_event(conn, seq, Some(session_id));
        }
        insert_message(conn, "M-1", session_id);
        insert_message(conn, "M-2", session_id);
        insert_task(conn, "T-1", session_id);
        insert_task(conn, "T-2", session_id);
        insert_checkpoint(conn, "C-1", "T-1");
        insert_checkpoint(conn, "C-2", "T-2");
        insert_runtime_execution(conn, "R-1", "T-1");
        insert_stage_artifact(conn, "A-1", session_id);
        insert_context_window(conn, "W-1", session_id);
        insert_session_overview(conn, session_id);
    }

    fn audit_rows(conn: &Connection) -> i64 {
        count(conn, "SELECT count(*) FROM audit")
    }

    #[test]
    fn preview_selects_by_time_and_reports_protected() {
        let conn = migrated();
        seed_session_tree(&conn, "S-old", NOW_MS - 48 * HOUR_MS);
        insert_session(&conn, "S-recent", NOW_MS - HOUR_MS, false);
        insert_session(&conn, "S-prot", NOW_MS - 48 * HOUR_MS, false);

        let protection = PurgeProtection {
            protect_ids: vec!["S-prot".to_string()],
            ..PurgeProtection::default()
        };
        let preview = preview_purge(
            &conn,
            &PurgeCriteria::before(NOW_MS),
            &protection,
            u64::try_from(NOW_MS).expect("NOW_MS"),
        )
        .expect("预览清理");

        assert_eq!(preview.selected_sessions, vec!["S-old".to_string()]);
        assert_eq!(
            preview.skipped_protected,
            vec!["S-prot".to_string(), "S-recent".to_string()],
            "显式保护与近期活跃都必须跳过且排序输出"
        );
        assert_eq!(preview.counts.sessions, 1);
        assert_eq!(preview.counts.events, 3);
        assert_eq!(preview.counts.messages, 2);
        assert_eq!(preview.counts.tasks, 2);
        assert_eq!(preview.cutoff_ms, NOW_MS);
        assert_eq!(audit_rows(&conn), 0, "预览是只读的，不得写审计");
    }

    #[test]
    fn preview_set_matches_executed_deletions() {
        let conn = migrated();
        seed_session_tree(&conn, "S-old", NOW_MS - 48 * HOUR_MS);
        insert_session(&conn, "S-recent", NOW_MS - HOUR_MS, false);
        insert_session(&conn, "S-prot", NOW_MS - 48 * HOUR_MS, false);
        insert_event(&conn, 10, Some("S-recent"));
        insert_event(&conn, 11, Some("S-prot"));
        insert_event(&conn, 100, None);
        insert_audit(&conn, "A-pre");

        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");
        let protection = PurgeProtection {
            protect_ids: vec!["S-prot".to_string()],
            ..PurgeProtection::default()
        };
        let preview = preview_purge(&conn, &PurgeCriteria::before(NOW_MS), &protection, now_ms)
            .expect("预览清理");

        let mut conn = conn;
        let result = execute_purge(&mut conn, &preview, true, "tester", &protection, now_ms)
            .expect("执行清理");

        assert_eq!(
            result.deleted_sessions, preview.selected_sessions,
            "实际删除集合必须与预览一致"
        );
        assert_eq!(
            result.skipped_protected, preview.skipped_protected,
            "跳过集合必须与预览一致"
        );
        assert_eq!(result.deleted, preview.counts, "实际删除行数必须与预览一致");
        assert_eq!(
            result.deleted,
            PurgeCounts {
                sessions: 1,
                tasks: 2,
                checkpoints: 2,
                runtime_executions: 1,
                stage_artifacts: 1,
                events: 3,
                messages: 2,
                context_windows: 1,
                session_overview: 1,
            }
        );

        assert_eq!(count(&conn, "SELECT count(*) FROM sessions"), 2);
        assert_eq!(
            count(&conn, "SELECT count(*) FROM events"),
            3,
            "仅剩 S-recent/S-prot 与全局事件"
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM events WHERE session_id IS NULL"
            ),
            1,
            "全局事件（session_id 为空）不得被清理"
        );
        assert_eq!(count(&conn, "SELECT count(*) FROM messages"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM tasks"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM checkpoints"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM runtime_executions"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM stage_artifacts"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM context_windows"), 0);
        assert_eq!(count(&conn, "SELECT count(*) FROM session_overview"), 0);
        assert_eq!(audit_rows(&conn), 2, "原有审计行保留，另新增一条 db_purge");

        let (action, target, detail): (String, String, String) = conn
            .query_row(
                "SELECT action, target, detail_json FROM audit
                 WHERE id LIKE 'purge-%' ORDER BY id LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("读取清理审计行");
        assert_eq!(action, "db_purge");
        assert!(target.contains("before"), "target 须含判据模式：{target}");
        let detail: serde_json::Value = serde_json::from_str(&detail).expect("审计 detail 可解析");
        assert_eq!(detail["cutoff_ms"], NOW_MS);
        assert_eq!(detail["deleted_sessions"], 1);
        assert_eq!(detail["counts"]["events"], 3);
        assert_eq!(
            detail["skipped_protected"],
            serde_json::json!(["S-prot", "S-recent"])
        );
    }

    #[test]
    fn execute_requires_explicit_confirmation() {
        let conn = migrated();
        seed_session_tree(&conn, "S-old", NOW_MS - 48 * HOUR_MS);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");
        let preview = preview_purge(
            &conn,
            &PurgeCriteria::before(NOW_MS),
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("预览清理");

        let mut exec_conn = conn;
        let err = execute_purge(
            &mut exec_conn,
            &preview,
            false,
            "tester",
            &PurgeProtection::default(),
            now_ms,
        )
        .expect_err("未确认必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable);

        assert_eq!(count(&exec_conn, "SELECT count(*) FROM sessions"), 1);
        assert_eq!(count(&exec_conn, "SELECT count(*) FROM events"), 3);
        assert_eq!(audit_rows(&exec_conn), 0, "拒绝执行不得写审计");
    }

    #[test]
    fn abort_policy_rejects_at_preview_and_execute() {
        let conn = migrated();
        seed_session_tree(&conn, "S-old", NOW_MS - 48 * HOUR_MS);
        insert_session(&conn, "S-prot", NOW_MS - 48 * HOUR_MS, false);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");

        let protection = PurgeProtection {
            protect_ids: vec!["S-prot".to_string()],
            on_protected: OnProtected::Abort,
            ..PurgeProtection::default()
        };
        let err = preview_purge(&conn, &PurgeCriteria::before(NOW_MS), &protection, now_ms)
            .expect_err("预览阶段命中 Abort 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.to_string().contains("S-prot"));
        assert_eq!(audit_rows(&conn), 0);

        // 预览时无保护，执行时保护名单变化：执行阶段同样拒绝。
        let preview = preview_purge(
            &conn,
            &PurgeCriteria::before(NOW_MS),
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("无保护预览");
        let mut exec_conn = conn;
        let protection = PurgeProtection {
            protect_ids: vec!["S-old".to_string()],
            on_protected: OnProtected::Abort,
            ..PurgeProtection::default()
        };
        let err = execute_purge(
            &mut exec_conn,
            &preview,
            true,
            "tester",
            &protection,
            now_ms,
        )
        .expect_err("执行阶段命中 Abort 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert_eq!(count(&exec_conn, "SELECT count(*) FROM sessions"), 2);
        assert_eq!(audit_rows(&exec_conn), 0);
    }

    #[test]
    fn ttl_criterion_boundary_and_negative_rejected() {
        let conn = migrated();
        let cutoff = NOW_MS - HOUR_MS;
        insert_session(&conn, "S-at-cutoff", cutoff, false);
        insert_session(&conn, "S-before", cutoff - 1, false);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");
        // TTL 截止点在默认 24h 活跃窗口内时全体候选都会被保护；此处用 30 分钟窗口
        // 让 1 小时前的候选按时间判据参与清理（交互语义见模块文档）。
        let protection = PurgeProtection {
            active_window_ms: HOUR_MS / 2,
            ..PurgeProtection::default()
        };

        let preview = preview_purge(&conn, &PurgeCriteria::ttl(3_600), &protection, now_ms)
            .expect("TTL 预览");
        assert_eq!(preview.cutoff_ms, cutoff);
        assert_eq!(preview.selected_sessions, vec!["S-before".to_string()]);
        assert_eq!(preview.criteria.criterion.mode(), "ttl");

        let err = preview_purge(&conn, &PurgeCriteria::ttl(-1), &protection, now_ms)
            .expect_err("负 TTL 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn ttl_within_default_active_window_protects_everything() {
        let conn = migrated();
        // 比 TTL 截止点早 1 ms：是合法候选，但仍在默认 24h 活跃窗口内，受保护。
        insert_session(&conn, "S-old", NOW_MS - HOUR_MS - 1, false);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");

        let preview = preview_purge(
            &conn,
            &PurgeCriteria::ttl(3_600),
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("默认窗口下的 TTL 预览");
        assert!(
            preview.selected_sessions.is_empty(),
            "TTL 截止点落在默认活跃窗口内时，候选一律受保护"
        );
        assert_eq!(preview.skipped_protected, vec!["S-old".to_string()]);
    }

    #[test]
    fn archived_only_limits_selection() {
        let conn = migrated();
        insert_session(&conn, "S-arch-old", NOW_MS - 48 * HOUR_MS, true);
        insert_session(&conn, "S-old", NOW_MS - 48 * HOUR_MS, false);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");

        let preview = preview_purge(
            &conn,
            &PurgeCriteria::before(NOW_MS).archived_only(),
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("archived_only 预览");
        assert_eq!(preview.selected_sessions, vec!["S-arch-old".to_string()]);

        let preview = preview_purge(
            &conn,
            &PurgeCriteria::before(NOW_MS),
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("全量预览");
        assert_eq!(
            preview.selected_sessions,
            vec!["S-arch-old".to_string(), "S-old".to_string()]
        );
    }

    #[test]
    fn execute_rechecks_protection_after_preview() {
        let conn = migrated();
        seed_session_tree(&conn, "S-old", NOW_MS - 48 * HOUR_MS);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");
        let protection = PurgeProtection::default();

        let preview = preview_purge(&conn, &PurgeCriteria::before(NOW_MS), &protection, now_ms)
            .expect("预览清理");
        assert_eq!(preview.selected_sessions.len(), 1);

        conn.execute(
            "UPDATE sessions SET last_active_at = epoch_ms(?) WHERE id = 'S-old'",
            params![NOW_MS - HOUR_MS],
        )
        .expect("会话在预览后重新活跃");

        let mut exec_conn = conn;
        let result = execute_purge(
            &mut exec_conn,
            &preview,
            true,
            "tester",
            &protection,
            now_ms,
        )
        .expect("执行清理（跳过新受保护会话）");
        assert!(
            result.deleted_sessions.is_empty(),
            "预览后新变活跃的会话必须在执行时被跳过"
        );
        assert_eq!(result.skipped_protected, vec!["S-old".to_string()]);
        assert_eq!(result.deleted.sessions, 0);
        assert_eq!(count(&exec_conn, "SELECT count(*) FROM sessions"), 1);
        assert_eq!(count(&exec_conn, "SELECT count(*) FROM events"), 3);
        assert_eq!(audit_rows(&exec_conn), 1, "空清理同样写审计");
    }

    #[test]
    fn set_archived_flips_flag_and_missing_ids_rejected() {
        let conn = migrated();
        insert_session(&conn, "S-1", NOW_MS - 48 * HOUR_MS, false);
        insert_session(&conn, "S-2", NOW_MS - 48 * HOUR_MS, false);
        let mut exec_conn = conn;

        let updated = set_archived(&mut exec_conn, &["S-1", "S-2"], true).expect("归档");
        assert_eq!(updated, 2);
        let archived: i64 = exec_conn
            .query_row("SELECT count(*) FROM sessions WHERE archived", [], |row| {
                row.get(0)
            })
            .expect("统计已归档");
        assert_eq!(archived, 2);

        let err = set_archived(&mut exec_conn, &["S-1", "S-missing"], false)
            .expect_err("缺失会话必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.to_string().contains("S-missing"));
        let archived: i64 = exec_conn
            .query_row("SELECT count(*) FROM sessions WHERE archived", [], |row| {
                row.get(0)
            })
            .expect("统计已归档");
        assert_eq!(archived, 2, "失败路径不得部分更新");

        let updated = set_archived(&mut exec_conn, &["S-1", "S-2"], false).expect("取消归档");
        assert_eq!(updated, 2);
        let updated = set_archived(&mut exec_conn, &[], true).expect("空清单为无操作");
        assert_eq!(updated, 0);
    }

    #[test]
    fn sessions_criterion_selects_by_id_with_protection() {
        let conn = migrated();
        insert_session(&conn, "S-old", NOW_MS - 48 * HOUR_MS, false);
        insert_session(&conn, "S-prot", NOW_MS - 48 * HOUR_MS, false);
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");
        let protection = PurgeProtection {
            protect_ids: vec!["S-prot".to_string()],
            ..PurgeProtection::default()
        };

        let preview = preview_purge(
            &conn,
            &PurgeCriteria::sessions(["S-old", "S-prot", "S-missing"]),
            &protection,
            now_ms,
        )
        .expect("按 ID 预览");
        assert_eq!(
            preview.selected_sessions,
            vec!["S-old".to_string()],
            "受保护与不存在的 ID 不得进入清理集"
        );
        assert_eq!(preview.skipped_protected, vec!["S-prot".to_string()]);
        assert_eq!(preview.criteria.criterion.mode(), "sessions");

        let mut conn = conn;
        let result =
            execute_purge(&mut conn, &preview, true, "tester", &protection, now_ms).expect("执行");
        assert_eq!(result.deleted_sessions, vec!["S-old".to_string()]);
        assert_eq!(count(&conn, "SELECT count(*) FROM sessions"), 1);

        let (action, target): (String, String) = conn
            .query_row(
                "SELECT action, target FROM audit WHERE id LIKE 'purge-%'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("审计行");
        assert_eq!(action, "db_purge");
        assert!(target.contains("by id"), "target 须含按 ID 模式：{target}");
    }

    #[test]
    fn empty_selection_executes_noop_and_audits() {
        let conn = migrated();
        let now_ms = u64::try_from(NOW_MS).expect("NOW_MS");
        let preview = preview_purge(
            &conn,
            &PurgeCriteria::before(NOW_MS),
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("空预览");
        assert!(preview.selected_sessions.is_empty());
        assert_eq!(preview.counts, PurgeCounts::default());

        let mut exec_conn = conn;
        let result = execute_purge(
            &mut exec_conn,
            &preview,
            true,
            "tester",
            &PurgeProtection::default(),
            now_ms,
        )
        .expect("空清理执行");
        assert!(result.deleted_sessions.is_empty());
        assert_eq!(result.deleted, PurgeCounts::default());
        assert_eq!(audit_rows(&exec_conn), 1, "空清理同样写审计（计数为零）");
    }
}
