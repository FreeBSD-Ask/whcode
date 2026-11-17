//! # 任务充分性计算报告（FR-795／ADR-085）
//!
//! 存一次计算的结论与反例明细：阶段、可判定片段结论、逐条计数与反例 JSON。计算本身
//! 是纯函数（`whcode_domain::coherence`）；本模块只负责持久化与读取。
//!
//! 边界（ADR-018）：SQL 只在本 crate；封闭枚举用 `whcode-domain` 的类型，不在库里
//! 拼写字符串。反例明细是半结构化 JSON，按 ADR-036 用 VARIANT：写入
//! `CAST(? AS JSON)::VARIANT`，读取 `CAST(CAST(col AS JSON) AS VARCHAR)`。
//!
//! 有界性：要求文本 ≤ [`MAX_COHERENCE_REQUIREMENT_CHARS`]、单次列表 ≤
//! [`MAX_COHERENCE_RUN_ROWS`] 条；超限拒绝，不静默截断。

use duckdb::Connection;
use whcode_domain::{CoherencePhase, ErrorCode, Verdict, WhError};

/// 要求文本字符上限（与领域层单段文本上限同量级）。
pub const MAX_COHERENCE_REQUIREMENT_CHARS: usize = 512;

/// 单次查询返回的条目上限（超出由调用方分页）。
pub const MAX_COHERENCE_RUN_ROWS: usize = 50;

const COLUMNS: &str = "id, session_id, phase, verdict, requirement, total, counterexamples, \
                       unknowns, CAST(CAST(counterexamples_json AS JSON) AS VARCHAR), \
                       CAST(epoch_ms(created_at) AS BIGINT)";

/// 新计算的登记请求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewCoherenceRun<'a> {
    /// 关联会话（可选：仓库级或装配期计算没有会话归属）。
    pub session_id: Option<&'a str>,
    /// 阶段。
    pub phase: CoherencePhase,
    /// 可判定片段结论。
    pub verdict: Verdict,
    /// 要求文本（已由应用层裁剪与校验）。
    pub requirement: &'a str,
    /// 逐条规则总数。
    pub total: u32,
    /// 反例条数。
    pub counterexamples: u32,
    /// 未知条数（不计反例，也不计通过）。
    pub unknowns: u32,
    /// 反例明细（JSON 文本；必须可解析为 JSON 数组）。
    pub counterexamples_json: &'a str,
}

/// 一条已持久化的计算报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoherenceRun {
    /// `COH-<序号>`。
    pub id: String,
    /// 关联会话。
    pub session_id: Option<String>,
    /// 阶段。
    pub phase: CoherencePhase,
    /// 可判定片段结论。
    pub verdict: Verdict,
    /// 要求文本。
    pub requirement: String,
    /// 逐条规则总数。
    pub total: u32,
    /// 反例条数。
    pub counterexamples: u32,
    /// 未知条数。
    pub unknowns: u32,
    /// 反例明细（JSON 文本，已由读取链路转回）。
    pub counterexamples_json: String,
    /// 创建时间（Unix 毫秒）。
    pub created_at_ms: i64,
}

fn run_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!("coherence_runs {context} 失败：{err}"),
    )
}

fn invalid(message: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.to_string())
}

fn to_u32(value: i64, label: &str) -> Result<u32, WhError> {
    u32::try_from(value)
        .map_err(|_| WhError::new(ErrorCode::ParseError, format!("{label} 越界：{value}")))
}

/// 登记一次计算，返回 `COH-<序号>`。
///
/// # Errors
///
/// 要求文本超限、反例明细不是 JSON 数组、计数与明细不一致或写入失败时返回 [`WhError`]。
pub fn insert_run(
    conn: &Connection,
    new: NewCoherenceRun<'_>,
    now_ms: u64,
) -> Result<String, WhError> {
    if new.requirement.trim().is_empty() {
        return Err(invalid("要求文本为空：不得把空要求登记为一次计算"));
    }
    if new.requirement.chars().count() > MAX_COHERENCE_REQUIREMENT_CHARS {
        return Err(invalid("要求文本超出长度上限"));
    }
    let parsed: serde_json::Value = serde_json::from_str(new.counterexamples_json)
        .map_err(|err| invalid(&format!("反例明细不是合法 JSON：{err}")))?;
    let Some(items) = parsed.as_array() else {
        return Err(invalid("反例明细必须是 JSON 数组"));
    };
    let declared = usize::try_from(new.counterexamples).map_err(|_| invalid("反例计数超出范围"))?;
    if items.len() != declared {
        return Err(invalid("反例计数与明细条数不一致"));
    }
    if new.total < new.counterexamples {
        return Err(invalid("规则总数小于反例条数"));
    }
    let now_ms = i64::try_from(now_ms).map_err(|_| invalid("时间戳超出 i64 范围"))?;
    let id: String = conn
        .query_row(
            "INSERT INTO coherence_runs \
             (id, session_id, phase, verdict, requirement, total, counterexamples, unknowns, \
              counterexamples_json, created_at) \
             VALUES ('COH-' || nextval('whcode_coherence_run_id_seq'), ?, ?, ?, ?, ?, ?, ?, \
                     CAST(? AS JSON)::VARIANT, epoch_ms(?)) \
             RETURNING id",
            duckdb::params![
                new.session_id,
                new.phase.as_str(),
                new.verdict.as_str(),
                new.requirement.trim(),
                i64::from(new.total),
                i64::from(new.counterexamples),
                i64::from(new.unknowns),
                new.counterexamples_json,
                now_ms,
            ],
            |row| row.get(0),
        )
        .map_err(|err| run_err("写入", err))?;
    Ok(id)
}

fn row_to_run(row: &duckdb::Row<'_>) -> Result<CoherenceRun, WhError> {
    let phase_text: String = row.get(2).map_err(|err| run_err("读取阶段", err))?;
    let phase = CoherencePhase::parse(&phase_text).ok_or_else(|| {
        WhError::new(
            ErrorCode::ParseError,
            format!("库内阶段未知：{phase_text}；允许 pre／mid／end"),
        )
    })?;
    let verdict_text: String = row.get(3).map_err(|err| run_err("读取结论", err))?;
    let verdict = match verdict_text.as_str() {
        "satisfied" => Verdict::Satisfied,
        "counterexample" => Verdict::Counterexample,
        other => {
            return Err(WhError::new(
                ErrorCode::ParseError,
                format!("库内结论未知：{other}；允许 satisfied／counterexample"),
            ));
        }
    };
    let total: i64 = row.get(5).map_err(|err| run_err("读取总数", err))?;
    let counterexamples: i64 = row.get(6).map_err(|err| run_err("读取反例数", err))?;
    let unknowns: i64 = row.get(7).map_err(|err| run_err("读取未知数", err))?;
    Ok(CoherenceRun {
        id: row.get(0).map_err(|err| run_err("读取 id", err))?,
        session_id: row.get(1).map_err(|err| run_err("读取会话", err))?,
        phase,
        verdict,
        requirement: row.get(4).map_err(|err| run_err("读取要求", err))?,
        total: to_u32(total, "规则总数")?,
        counterexamples: to_u32(counterexamples, "反例数")?,
        unknowns: to_u32(unknowns, "未知数")?,
        counterexamples_json: row.get(8).map_err(|err| run_err("读取反例明细", err))?,
        created_at_ms: row.get(9).map_err(|err| run_err("读取时间", err))?,
    })
}

/// 读取指定会话最近一次计算；无记录返回 `None`（正确的空）。
///
/// # Errors
///
/// 查询失败或库内枚举值未知时返回 [`WhError`]。
pub fn latest_run(conn: &Connection, session_id: &str) -> Result<Option<CoherenceRun>, WhError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM coherence_runs WHERE session_id = ? \
             ORDER BY created_at DESC, id DESC LIMIT 1"
        ))
        .map_err(|err| run_err("准备最近一次查询", err))?;
    let mut rows = stmt
        .query(duckdb::params![session_id])
        .map_err(|err| run_err("查询最近一次", err))?;
    match rows.next().map_err(|err| run_err("遍历最近一次", err))? {
        Some(row) => Ok(Some(row_to_run(row)?)),
        None => Ok(None),
    }
}

/// 列出最近的计算（按时间倒序，上限 [`MAX_COHERENCE_RUN_ROWS`]）。
///
/// # Errors
///
/// `limit` 为 0、查询失败或库内枚举值未知时返回 [`WhError`]。
pub fn list_runs(conn: &Connection, limit: usize) -> Result<Vec<CoherenceRun>, WhError> {
    if limit == 0 {
        return Err(invalid("list_runs 的 limit 必须大于 0"));
    }
    let limit = limit.min(MAX_COHERENCE_RUN_ROWS);
    let limit_i64 = i64::try_from(limit).map_err(|_| invalid("limit 超出 i64 范围"))?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM coherence_runs ORDER BY created_at DESC, id DESC LIMIT ?"
        ))
        .map_err(|err| run_err("准备列表", err))?;
    let mut rows = stmt
        .query(duckdb::params![limit_i64])
        .map_err(|err| run_err("列表查询", err))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|err| run_err("遍历列表", err))? {
        out.push(row_to_run(row)?);
    }
    Ok(out)
}

/// 会话当前任务（未完成的最近一条）：标题与验收条件 JSON 文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTaskBrief {
    /// 任务标题（作为“要求”）。
    pub title: String,
    /// 验收条件 JSON 文本（作为“检查点”来源）。
    pub acceptance_json: String,
}

/// 读取会话当前任务（`status` 不是 completed／failed／aborted 的最近一条）。
///
/// 无任务返回 `None`（正确的空：没有可审计的三元组，不伪造通过）。
///
/// # Errors
///
/// 查询失败时返回 [`WhError`]。
pub fn session_task(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SessionTaskBrief>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT title, CAST(CAST(acceptance_json AS JSON) AS VARCHAR) FROM tasks \
             WHERE session_id = ? AND status NOT IN ('completed', 'failed', 'aborted') \
             ORDER BY updated_at DESC, id DESC LIMIT 1",
        )
        .map_err(|err| run_err("准备当前任务查询", err))?;
    let mut rows = stmt
        .query(duckdb::params![session_id])
        .map_err(|err| run_err("查询当前任务", err))?;
    match rows.next().map_err(|err| run_err("遍历当前任务", err))? {
        Some(row) => Ok(Some(SessionTaskBrief {
            title: row.get(0).map_err(|err| run_err("读取任务标题", err))?,
            acceptance_json: row.get(1).map_err(|err| run_err("读取验收条件", err))?,
        })),
        None => Ok(None),
    }
}

/// 会话已开始的 turn 数（`TurnStarted` 事件计数）。
///
/// # Errors
///
/// 查询失败时返回 [`WhError`]。
pub fn session_turn_count(conn: &Connection, session_id: &str) -> Result<u32, WhError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id = ? AND kind = 'TurnStarted'",
            duckdb::params![session_id],
            |row| row.get(0),
        )
        .map_err(|err| run_err("统计 turn 数", err))?;
    u32::try_from(count)
        .map_err(|_| WhError::new(ErrorCode::ParseError, format!("turn 数越界：{count}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        crate::schema::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn request<'a>(phase: CoherencePhase, verdict: Verdict) -> NewCoherenceRun<'a> {
        NewCoherenceRun {
            session_id: Some("S-1"),
            phase,
            verdict,
            requirement: "补齐导出",
            total: 10,
            counterexamples: 0,
            unknowns: 1,
            counterexamples_json: "[]",
        }
    }

    #[test]
    fn insert_and_read_latest_run() {
        let conn = migrated();
        let id = insert_run(
            &conn,
            request(CoherencePhase::Pre, Verdict::Satisfied),
            1_700_000_000_000,
        )
        .expect("写入");
        assert!(id.starts_with("COH-"));
        let run = latest_run(&conn, "S-1").expect("查询").expect("存在");
        assert_eq!(run.phase, CoherencePhase::Pre);
        assert_eq!(run.verdict, Verdict::Satisfied);
        assert_eq!(run.total, 10);
        assert_eq!(run.unknowns, 1);
        assert_eq!(run.counterexamples_json, "[]");
    }

    #[test]
    fn latest_run_is_absent_for_unknown_session() {
        let conn = migrated();
        assert!(latest_run(&conn, "S-none").expect("查询").is_none());
    }

    #[test]
    fn counterexample_count_must_match_detail_length() {
        let conn = migrated();
        let mut new = request(CoherencePhase::Pre, Verdict::Counterexample);
        new.counterexamples = 2;
        new.counterexamples_json = r#"[{"rule":"SFR-03"}]"#;
        let err = insert_run(&conn, new, 1).expect_err("计数不一致必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn non_json_detail_is_rejected() {
        let conn = migrated();
        let mut new = request(CoherencePhase::Pre, Verdict::Satisfied);
        new.counterexamples_json = "not-json";
        let err = insert_run(&conn, new, 1).expect_err("非法 JSON 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn empty_requirement_is_rejected() {
        let conn = migrated();
        let mut new = request(CoherencePhase::Pre, Verdict::Satisfied);
        new.requirement = "  ";
        let err = insert_run(&conn, new, 1).expect_err("空要求必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn list_runs_is_bounded_and_ordered() {
        let conn = migrated();
        insert_run(&conn, request(CoherencePhase::Pre, Verdict::Satisfied), 1).expect("第一条");
        insert_run(&conn, request(CoherencePhase::Mid, Verdict::Satisfied), 2).expect("第二条");
        let runs = list_runs(&conn, 10).expect("列表");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].phase, CoherencePhase::Mid);
        assert!(list_runs(&conn, 0).is_err());
    }

    #[test]
    fn requirement_over_limit_is_rejected() {
        let conn = migrated();
        let long = "x".repeat(MAX_COHERENCE_REQUIREMENT_CHARS + 1);
        let mut new = request(CoherencePhase::Pre, Verdict::Satisfied);
        new.requirement = &long;
        let err = insert_run(&conn, new, 1).expect_err("超长要求必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn total_smaller_than_counterexamples_is_rejected() {
        let conn = migrated();
        let mut new = request(CoherencePhase::Pre, Verdict::Counterexample);
        new.total = 0;
        new.counterexamples = 1;
        new.counterexamples_json = r#"[{"rule":"SFR-03"}]"#;
        let err = insert_run(&conn, new, 1).expect_err("总数小于反例数必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn session_less_run_is_allowed_and_readable_by_list() {
        let conn = migrated();
        let mut new = request(CoherencePhase::End, Verdict::Satisfied);
        new.session_id = None;
        insert_run(&conn, new, 5).expect("仓库级计算");
        let runs = list_runs(&conn, 5).expect("列表");
        assert_eq!(runs.len(), 1);
        assert!(runs[0].session_id.is_none());
    }

    #[test]
    fn drifted_phase_is_parse_error() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO coherence_runs (id, session_id, phase, verdict, requirement, total, \
             counterexamples, unknowns, counterexamples_json) \
             VALUES ('COH-BAD-PHASE', 'S-1', 'audit', 'satisfied', '要求', 1, 0, 0, \
             CAST('[]' AS JSON)::VARIANT)",
            [],
        )
        .expect("写入漂移行");
        let err = latest_run(&conn, "S-1").expect_err("未知阶段必须报错");
        assert_eq!(err.code, ErrorCode::ParseError);
    }

    #[test]
    fn drifted_verdict_is_parse_error() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO coherence_runs (id, session_id, phase, verdict, requirement, total, \
             counterexamples, unknowns, counterexamples_json) \
             VALUES ('COH-BAD-VERDICT', 'S-1', 'pre', 'green', '要求', 1, 0, 0, \
             CAST('[]' AS JSON)::VARIANT)",
            [],
        )
        .expect("写入漂移行");
        let err = latest_run(&conn, "S-1").expect_err("未知结论必须报错");
        assert_eq!(err.code, ErrorCode::ParseError);
    }

    #[test]
    fn detail_must_be_json_array() {
        let conn = migrated();
        let mut new = request(CoherencePhase::Pre, Verdict::Satisfied);
        new.counterexamples_json = r#"{"rule":"SFR-03"}"#;
        let err = insert_run(&conn, new, 1).expect_err("JSON 对象不是数组必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn counterexample_verdict_round_trips() {
        let conn = migrated();
        let mut new = request(CoherencePhase::Pre, Verdict::Counterexample);
        new.counterexamples = 1;
        new.total = 10;
        new.counterexamples_json = r#"[{"rule":"SFR-03","evidence":"缺口登记待补"}]"#;
        insert_run(&conn, new, 7).expect("写入反例结论");
        let run = latest_run(&conn, "S-1").expect("查询").expect("存在");
        assert_eq!(run.verdict, Verdict::Counterexample);
        assert_eq!(run.counterexamples, 1);
    }

    #[test]
    fn session_task_prefers_latest_unfinished() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status) \
             VALUES ('S-T', '任务会话', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("建会话");
        for (id, acceptance, updated) in [
            ("T-OLD", r#"["旧判据"]"#, "2025-01-01 00:00:00"),
            ("T-NEW", r#"["新判据"]"#, "2025-06-01 00:00:00"),
        ] {
            conn.execute(
                "INSERT INTO tasks (id, session_id, title, stage, status, parent_id, \
                 acceptance_json, retry_count, updated_at) \
                 VALUES (?, 'S-T', '补齐导出', 'specify', 'pending', NULL, \
                 CAST(? AS JSON)::VARIANT, 0, CAST(? AS TIMESTAMP))",
                duckdb::params![id, acceptance, updated],
            )
            .expect("建任务");
        }
        let brief = session_task(&conn, "S-T").expect("读取").expect("有任务");
        assert_eq!(brief.title, "补齐导出");
        assert_eq!(brief.acceptance_json, r#"["新判据"]"#);
        assert!(session_task(&conn, "S-none").expect("读取空").is_none());
    }

    #[test]
    fn session_turn_count_counts_only_turn_started() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status) \
             VALUES ('S-C', '计数会话', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("建会话");
        assert_eq!(session_turn_count(&conn, "S-C").expect("空计数"), 0);
        for (seq, kind) in [(1, "TurnStarted"), (2, "TurnFinished"), (3, "TurnStarted")] {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id, \
                 kind, payload_json, ts, persistence_level) \
                 VALUES (?, ?, 'S-C', NULL, NULL, NULL, ?, CAST('{}' AS JSON)::VARIANT, \
                 current_timestamp, 'redacted')",
                duckdb::params![seq, format!("EV-{seq}"), kind],
            )
            .expect("写事件");
        }
        assert_eq!(session_turn_count(&conn, "S-C").expect("计数"), 2);
    }
}
