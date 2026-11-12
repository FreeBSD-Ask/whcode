//! # 事件导出（T065 核心、FR-202/FR-205）
//!
//! 事件按持久化游标查询并导出为 JSON/JSONL（CUI 的 [`crate::replay`] 与
//! `whcode-cui::events_cmd` 复用本模块——前端禁止触库，查询必须经应用服务，
//! ADR-003/005、C-03）。
//!
//! **脱敏策略随导出注明**（FR-205）：导出对象携带 `trace_level` 与 `redacted`
//! 标记；`redacted`/`metadata` 级别下载荷经 [`whcode_llm::trace::sanitize`] 处理，
//! `off` 级别不导出载荷正文（`payload` 置空对象并标注）。`full` 级别原样导出，
//! 调用方须自行承担访问控制与警告（UI 警告属前端职责）。
//!
//! ## 三态分类（T283／FR-770／ADR-066）
//!
//! 每页事件附带其在**模型上下文**中的状态，判定复用模型自己的历史派生函数
//! [`crate::context::load_context`]（同一 fold surface，避免查询口径与模型口径分叉）：
//!
//! - `current`：该事件序号有对话消息孪生（`MSG-<seq>`），且该消息在当前上下文窗口内；
//! - `shadowed`：有孪生但不在窗口内（预算截断或被压缩摘要代表），原文仍可查；
//! - `log_only`：无孪生——纯审计/观测/控制事件，从不进入模型上下文。
//!
//! 窗口预算取 [`crate::turn::DEFAULT_TURN_BUDGET_TOKENS`]：视图回答“现在哪些历史还在
//! 模型上下文里”，不是对某次 turn 历史参数的复盘。

use std::collections::HashSet;

use duckdb::Connection;
use serde::Serialize;
use whcode_domain::{ErrorCode, TraceLevel, WhError};
use whcode_llm::trace::{self, IntegrityFlags};
use whcode_store::query::{self, EventRow};

use crate::authz::{Actor, Capability, Policy, require};

/// 导出单页上限（继承 store 分页上限）。
pub const EXPORT_PAGE_LIMIT: usize = 1000;

/// 事件在模型上下文中的三态（T283／FR-770／ADR-066）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventContextState {
    /// 当前上下文窗口内（会被发给模型）。
    Current,
    /// 有对话消息孪生但不在窗口内：被压缩摘要代表或预算截断遮蔽，原文仍可查。
    Shadowed,
    /// 无对话消息孪生：纯审计/观测/控制事件，从不进入模型上下文。
    LogOnly,
}

impl EventContextState {
    /// 稳定序列化名（CUI JSON、Web `/api/events` 与人类可读输出共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Shadowed => "shadowed",
            Self::LogOnly => "log_only",
        }
    }
}

/// 单条事件的三态标注（`seq` 与该页事件行一一对应）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventState {
    /// 事件序号。
    pub seq: i64,
    /// 上下文状态。
    pub state: EventContextState,
}

/// 三态计数（页内汇总；三值之和等于页内事件数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EventStateCounts {
    /// 当前上下文内的事件数。
    pub current: u64,
    /// 被遮蔽的事件数。
    pub shadowed: u64,
    /// 纯日志事件数。
    pub log_only: u64,
}

/// 事件导出对象（脱敏策略随对象注明，FR-205）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventsExport {
    /// 事件 schema 版本。
    pub schema_version: u32,
    /// 会话范围。
    pub session_id: String,
    /// 本次导出的追踪级别。
    pub trace_level: String,
    /// 是否已脱敏（`redacted` 级别为真）。
    pub redacted: bool,
    /// 完整性标记（FR-208：脱敏/截断痕迹可查询）。
    pub integrity: IntegrityFlags,
    /// 事件（载荷已按级别处理）。
    pub events: Vec<EventRow>,
    /// 每条事件的上下文三态（与 `events` 同序一一对应）。
    pub states: Vec<EventState>,
    /// 三态计数。
    pub state_counts: EventStateCounts,
}

fn invalid(context: &str) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.to_string())
}

/// 为一页事件标注上下文三态（T283／FR-770）。
///
/// 判定基准与模型同源：窗口取 [`crate::context::load_context`] 在默认 turn 预算下的
/// 结果（模型历史派生函数本身），孪生取 `MSG-<seq>` 的存在性。两者都不产生新的持久化
/// 状态——分类是派生视图，不做写入，故不会与折叠动作失同步。
fn classify_context_states(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    events: &[EventRow],
) -> Result<(Vec<EventState>, EventStateCounts), WhError> {
    if events.is_empty() {
        return Ok((Vec::new(), EventStateCounts::default()));
    }
    let window = crate::context::load_context(
        conn,
        policy,
        actor,
        session_id,
        crate::turn::DEFAULT_TURN_BUDGET_TOKENS,
    )?;
    let current: HashSet<String> = window.items.into_iter().map(|item| item.id).collect();

    let (lo, hi) = events.iter().fold((i64::MAX, i64::MIN), |(lo, hi), event| {
        (lo.min(event.seq), hi.max(event.seq))
    });
    let twins: HashSet<i64> = query::message_seqs_in_range(conn, session_id, lo, hi)?
        .into_iter()
        .collect();

    let mut counts = EventStateCounts::default();
    let mut states = Vec::with_capacity(events.len());
    for event in events {
        let state = if !twins.contains(&event.seq) {
            counts.log_only += 1;
            EventContextState::LogOnly
        } else if current.contains(&format!("MSG-{}", event.seq)) {
            counts.current += 1;
            EventContextState::Current
        } else {
            counts.shadowed += 1;
            EventContextState::Shadowed
        };
        states.push(EventState {
            seq: event.seq,
            state,
        });
    }
    Ok((states, counts))
}

/// 导出会话事件（FR-202/205）：按游标之后增量查询，载荷按追踪级别脱敏。
///
/// 会话存在性先于查询校验（Check 124）：不存在的会话返回 `not_found`，与
/// 存在但零事件的“正确的空”可区分。
///
/// # Errors
///
/// 权限拒绝、会话不存在、`limit` 越界或存储失败时返回 [`WhError`]。
pub fn events_export(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    after_seq: i64,
    limit: usize,
    level: TraceLevel,
) -> Result<EventsExport, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if whcode_store::get_session(conn, session_id)?.is_none() {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话，或核对会话 ID 后重试"),
        ));
    }
    if limit == 0 || limit > EXPORT_PAGE_LIMIT {
        return Err(invalid(&format!(
            "export limit 须在 1..={EXPORT_PAGE_LIMIT}，实际 {limit}"
        )));
    }
    let page = query::events_page(
        conn,
        session_id,
        whcode_store::Direction::Forward,
        Some(query::EventCursor { seq: after_seq }),
        limit,
    )?;

    let events = match level {
        TraceLevel::Full => page.items,
        TraceLevel::Redacted | TraceLevel::Metadata | TraceLevel::Off => page
            .items
            .into_iter()
            .map(|mut event| {
                let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json)
                    .map_err(|err| {
                        WhError::parse_error(format!("事件 {} 载荷损坏：{err}", event.id))
                    })?;
                event.payload_json = match level {
                    TraceLevel::Off => "{}".to_string(),
                    other => {
                        let sanitized = trace::sanitize(&payload, other)
                            .payload
                            .unwrap_or_else(|| "{}".into());
                        sanitized.to_string()
                    }
                };
                Ok(event)
            })
            .collect::<Result<Vec<EventRow>, WhError>>()?,
    };

    let (states, state_counts) = classify_context_states(conn, policy, actor, session_id, &events)?;

    Ok(EventsExport {
        schema_version: whcode_domain::SCHEMA_VERSION,
        session_id: session_id.to_string(),
        trace_level: level.as_str().to_string(),
        redacted: level == TraceLevel::Redacted,
        integrity: IntegrityFlags {
            redacted: level == TraceLevel::Redacted,
            missing_chunks: false,
            downsampled: false,
            persistence_failed: false,
        },
        events,
        states,
        state_counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::params;
    use whcode_store::{apply_migrations, create_session_with_id};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn seed_events(conn: &Connection, session_id: &str) {
        for (seq, payload) in [
            (1, r#"{"api_key":"sk-secret1","note":"正文"}"#),
            (2, r#"{"prompt":"普通载荷"}"#),
        ] {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}"), session_id, payload],
            )
            .expect("插入事件");
        }
    }

    #[test]
    fn export_annotates_trace_policy_and_redacts_at_redacted_level() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "导出", "active", 1_000).expect("创建");
        seed_events(&conn, "S-1");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let export = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Redacted,
        )
        .expect("导出");
        assert_eq!(
            export.trace_level, "redacted",
            "脱敏策略必须随导出注明（FR-205）"
        );
        assert!(export.redacted);
        assert!(export.integrity.redacted);
        let serialized = serde_json::to_string(&export).expect("序列化");
        assert!(
            !serialized.contains("sk-secret1"),
            "redacted 下秘密不得出现在导出"
        );
        assert!(serialized.contains("正文"), "普通内容保留");

        let full = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Full,
        )
        .expect("full 导出");
        let full_serialized = serde_json::to_string(&full).expect("序列化");
        assert!(full_serialized.contains("sk-secret1"), "full 原样导出");

        let off = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Off,
        )
        .expect("off 导出");
        assert!(off.events.iter().all(|event| event.payload_json == "{}"));
    }

    #[test]
    fn export_is_incremental_from_cursor_and_bounded() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "增量", "active", 1_000).expect("创建");
        seed_events(&conn, "S-1");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let first =
            events_export(&conn, &policy, &actor, "S-1", 0, 1, TraceLevel::Full).expect("第一页");
        assert_eq!(first.events.len(), 1);
        let second = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            1,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Full,
        )
        .expect("游标增量");
        assert_eq!(second.events.len(), 1);
        assert_eq!(second.events[0].seq, 2);

        let err = events_export(&conn, &policy, &actor, "S-1", 0, 0, TraceLevel::Full)
            .expect_err("limit=0");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT + 1,
            TraceLevel::Full,
        )
        .expect_err("超限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    /// Check 124：不存在的会话必须 `not_found`（含会话 ID 与可操作提示），
    /// 存在但零事件必须是成功的空——两个结果可区分。
    #[test]
    fn export_rejects_missing_session_and_allows_empty_existing_session() {
        let conn = migrated();
        create_session_with_id(&conn, "S-empty", "空会话", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let export = events_export(
            &conn,
            &policy,
            &actor,
            "S-empty",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Full,
        )
        .expect("存在但零事件是正确的空");
        assert_eq!(export.session_id, "S-empty");
        assert!(export.events.is_empty());

        let err = events_export(
            &conn,
            &policy,
            &actor,
            "S-missing",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Full,
        )
        .expect_err("不存在的会话必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        let message = err.to_string();
        assert!(message.contains("S-missing"), "错误须含会话 ID：{message}");
        assert!(
            message.contains("session list"),
            "错误须给出可操作提示：{message}"
        );
    }

    fn seed_mixed_history(conn: &Connection, session_id: &str) {
        // 事件 1—6：1／3／5 无消息孪生（纯日志），2／4／6 各有对话消息。
        for seq in 1..=6 {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST('{}' AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                params![seq, format!("E-{seq}"), session_id],
            )
            .expect("插入事件");
        }
        // MSG-2 在预算内；MSG-4 单条即超默认预算 → 其后（含 MSG-6）被截断遮蔽。
        for (seq, tokens) in [(2, 100i64), (4, 1_000_000), (6, 100)] {
            conn.execute(
                "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
                 VALUES (?, ?, 'user', ?, ?, current_timestamp, NULL)",
                params![
                    format!("MSG-{seq}"),
                    session_id,
                    format!("ref-{seq}"),
                    tokens
                ],
            )
            .expect("插入消息");
        }
    }

    /// T283／FR-770：三态判定复用模型上下文窗口——窗口内 `current`、窗口外
    /// `shadowed`、无孪生 `log_only`；标注与事件同序，计数之和等于页大小。
    #[test]
    fn states_classify_current_shadowed_and_log_only_against_the_model_window() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "三态", "active", 1_000).expect("创建");
        seed_mixed_history(&conn, "S-1");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let export = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Redacted,
        )
        .expect("导出");

        let observed: Vec<(i64, &str)> = export
            .states
            .iter()
            .map(|entry| (entry.seq, entry.state.as_str()))
            .collect();
        assert_eq!(
            observed,
            vec![
                (1, "log_only"),
                (2, "current"),
                (3, "log_only"),
                (4, "shadowed"),
                (5, "log_only"),
                (6, "shadowed"),
            ],
            "无孪生→log_only；窗口内→current；预算截断后→shadowed"
        );
        assert_eq!(
            export.states.len(),
            export.events.len(),
            "标注必须与事件行一一对应"
        );
        assert_eq!(
            (
                export.state_counts.current,
                export.state_counts.shadowed,
                export.state_counts.log_only
            ),
            (1, 2, 3),
            "三态计数"
        );
        assert_eq!(
            export.state_counts.current
                + export.state_counts.shadowed
                + export.state_counts.log_only,
            export.events.len() as u64,
            "计数之和必须等于页内事件数"
        );
        // 序列化名是稳定的机器可读标识（Web 与 JSON 导出共用）。
        let serialized = serde_json::to_string(&export).expect("序列化");
        assert!(serialized.contains(r#""state":"shadowed""#), "{serialized}");
    }

    /// 正确的空：无事件的会话状态集合为空、计数为零，不伪造标注；
    /// 分类是派生视图，不写库（事件/消息/context_windows 行数不变）。
    #[test]
    fn states_on_empty_session_are_empty_and_classification_never_writes() {
        let conn = migrated();
        create_session_with_id(&conn, "S-empty", "空", "active", 1_000).expect("创建");
        create_session_with_id(&conn, "S-1", "有历史", "active", 1_000).expect("创建");
        seed_mixed_history(&conn, "S-1");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let empty = events_export(
            &conn,
            &policy,
            &actor,
            "S-empty",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Redacted,
        )
        .expect("空会话");
        assert!(empty.states.is_empty());
        assert_eq!(empty.state_counts, EventStateCounts::default());

        let counts_before: (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM events), (SELECT COUNT(*) FROM messages),
                        (SELECT COUNT(*) FROM context_windows)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("行数");
        let _ = events_export(
            &conn,
            &policy,
            &actor,
            "S-1",
            0,
            EXPORT_PAGE_LIMIT,
            TraceLevel::Redacted,
        )
        .expect("有历史会话");
        let counts_after: (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM events), (SELECT COUNT(*) FROM messages),
                        (SELECT COUNT(*) FROM context_windows)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("行数");
        assert_eq!(counts_before, counts_after, "分类不得写入任何表");
    }
}
