//! # turn 事件与消息轮次落表（T208、FR-722、ADR-044/ADR-048）
//!
//! turn 循环把**关键事件**（请求开始/增量/完成/失败、工具调用开始/结束、上下文状态、
//! 运行时终止）与**消息轮次**（user/assistant/tool）在**同一事务**内追加进 `events`/
//! `messages` 表——任一行失败整体回滚，不留半程状态（与 [`crate::checkpoints::task_transition`]
//! 同法：`&mut Connection` + 事务 + `MAX(seq)+1` 序号分配）。
//!
//! 口径：
//! - 事件 `kind` 用 domain [`EventKind`] 稳定名（任务书 §8.3）；
//! - `payload` 走 `CAST(? AS JSON)::VARIANT`（ADR-036），只带标识与计数、**不含正文与
//!   提示词**（FR-203）；超 [`MAX_PAYLOAD_BYTES`] 直接拒绝（R-05，不静默截断）；
//! - `persistence_level` 固定 [`PersistenceLevel::Redacted`]（FR-203 默认）；
//! - 消息 `content_ref` 直存正文（≤ [`MAX_TURN_MESSAGE_BYTES`]；无内容表 API 的现状
//!   已在 plan-p0-1 §6.6 标注，超限拒绝不截断）；
//! - 消息 `id` 取 `MSG-<seq>`、事件 `id` 取 `EV-<seq>`——同一 seq 池分配，全局唯一。

use duckdb::Connection;
use serde_json::Value;
use whcode_domain::{EventKind, MAX_PAYLOAD_BYTES, PersistenceLevel, WhError};

/// 单条 turn 消息的正文上限（字节；plan-p0-1 §6.6 的直存上限）。
pub const MAX_TURN_MESSAGE_BYTES: usize = 1 << 20;

/// 待落表事件（turn 循环产出）。
#[derive(Debug, Clone)]
pub struct TurnEventDraft {
    /// 事件类型（稳定名落库）。
    pub kind: EventKind,
    /// 所属 LLM 请求。
    pub request_id: Option<String>,
    /// 所属工具调用（工具事件用）。
    pub tool_call_id: Option<String>,
    /// 载荷（标识与计数，不含正文）。
    pub payload: Value,
}

/// 待落表消息轮次。
#[derive(Debug, Clone)]
pub struct TurnMessageDraft {
    /// 角色（`user`/`assistant`/`tool`；system 不落 messages，FR-722）。
    pub role: &'static str,
    /// 正文（直存 `content_ref`，≤ [`MAX_TURN_MESSAGE_BYTES`]）。
    pub content: String,
    /// token 计量（可空；估算值由调用方给）。
    pub token_count: Option<i64>,
    /// 父消息（可空）。
    pub parent_id: Option<String>,
}

/// 追加结果：本次事务分配到的 seq（事件与消息混排，升序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendTurnReport {
    /// 分配的 seq 列表（与入参行序一致）。
    pub seqs: Vec<i64>,
}

fn turn_err(stage: &str, err: duckdb::Error) -> WhError {
    WhError::new(
        whcode_domain::ErrorCode::IoError,
        format!("turn 落表失败（{stage}）：{err}"),
    )
}

fn invalid(message: &str) -> WhError {
    WhError::invalid_state(message.to_string())
}

/// 把 turn 的事件与消息轮次在同一事务内追加进 `events`/`messages` 表。
///
/// 事务内步骤：校验入参（会话非空、角色合法、载荷与正文上限）→ 取 `MAX(seq)` 逐行
/// 递增分配 → 事件行（`EV-<seq>`）与消息行（`MSG-<seq>`）写入 → 提交。任一行失败
/// 整体回滚。
///
/// # Errors
///
/// 会话为空、角色非法、载荷超 [`MAX_PAYLOAD_BYTES`]、正文超 [`MAX_TURN_MESSAGE_BYTES`]
/// 或数据库读写失败时返回 [`WhError`]。
pub fn append_turn(
    conn: &mut Connection,
    session_id: &str,
    events: &[TurnEventDraft],
    messages: &[TurnMessageDraft],
) -> Result<AppendTurnReport, WhError> {
    let session = session_id.trim();
    if session.is_empty() {
        return Err(invalid("turn 落表需要非空 session ID"));
    }
    for event in events {
        let payload_bytes = serde_json::to_vec(&event.payload)
            .map_err(|err| WhError::parse_error(format!("turn 事件载荷序列化失败：{err}")))?;
        if payload_bytes.len() > MAX_PAYLOAD_BYTES {
            return Err(invalid(&format!(
                "turn 事件载荷超过 {MAX_PAYLOAD_BYTES} 字节上限：须改走内容表/引用（R-05）"
            )));
        }
    }
    for message in messages {
        match message.role {
            "user" | "assistant" | "tool" => {}
            other => {
                return Err(invalid(&format!(
                    "turn 消息角色非法：{other}（FR-722 口径）"
                )));
            }
        }
        if message.content.len() > MAX_TURN_MESSAGE_BYTES {
            return Err(invalid(&format!(
                "turn 消息正文 {} 字节超过 {} 字节上限：须改走引用并留标记（R-05），不静默截断",
                message.content.len(),
                MAX_TURN_MESSAGE_BYTES
            )));
        }
    }

    let tx = conn
        .transaction()
        .map_err(|err| turn_err("开始事务", err))?;
    let base_seq: i64 = tx
        .query_row(
            "SELECT GREATEST(\
             (SELECT COALESCE(MAX(seq), 0) FROM events), \
             (SELECT COALESCE(MAX(CAST(SUBSTRING(id FROM 5) AS INTEGER)), 0) \
              FROM messages WHERE id LIKE 'MSG-%'))",
            [],
            |row| row.get(0),
        )
        .map_err(|err| turn_err("分配序号（events+messages 双源）", err))?;
    let mut seq = base_seq;
    let mut seqs = Vec::with_capacity(events.len() + messages.len());

    for event in events {
        seq += 1;
        tx.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, ?, ?, ?, CAST(? AS JSON)::VARIANT, current_timestamp, ?)",
            duckdb::params![
                seq,
                format!("EV-{seq}"),
                session,
                event.request_id,
                event.tool_call_id,
                event.kind.as_str(),
                serde_json::to_string(&event.payload).map_err(|err| {
                    WhError::parse_error(format!("turn 事件载荷序列化失败：{err}"))
                })?,
                PersistenceLevel::Redacted.to_string(),
            ],
        )
        .map_err(|err| turn_err("写入 turn 事件", err))?;
        seqs.push(seq);
    }
    for message in messages {
        seq += 1;
        tx.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, ?, ?, ?, current_timestamp, ?)",
            duckdb::params![
                format!("MSG-{seq}"),
                session,
                message.role,
                message.content,
                message.token_count,
                message.parent_id,
            ],
        )
        .map_err(|err| turn_err("写入 turn 消息", err))?;
        seqs.push(seq);
    }

    tx.commit().map_err(|err| turn_err("提交事务", err))?;
    Ok(AppendTurnReport { seqs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use whcode_domain::ErrorCode;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        crate::apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn appends_events_and_messages_in_one_transaction_with_monotonic_seqs() {
        let mut conn = migrated();
        let report = append_turn(
            &mut conn,
            "S-turn",
            &[
                TurnEventDraft {
                    kind: EventKind::RequestStarted,
                    request_id: Some("llm-1".to_string()),
                    tool_call_id: None,
                    payload: json!({ "turn_id": "turn-S-turn-1", "iteration": 0 }),
                },
                TurnEventDraft {
                    kind: EventKind::RuntimeTerminated,
                    request_id: Some("llm-1".to_string()),
                    tool_call_id: None,
                    payload: json!({ "termination_reason": "model_finished" }),
                },
            ],
            &[
                TurnMessageDraft {
                    role: "user",
                    content: "你好".to_string(),
                    token_count: Some(1),
                    parent_id: None,
                },
                TurnMessageDraft {
                    role: "assistant",
                    content: "回复".to_string(),
                    token_count: Some(1),
                    parent_id: None,
                },
            ],
        )
        .expect("落表成功");
        assert_eq!(report.seqs.len(), 4);

        let (event_count, message_count): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM events WHERE session_id = 'S-turn'),
                        (SELECT COUNT(*) FROM messages WHERE session_id = 'S-turn')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("计数");
        assert_eq!((event_count, message_count), (2, 2));
        // 事件 kind 用稳定名、载荷可按 JSON 读回。
        let kind: String = conn
            .query_row(
                "SELECT kind FROM events WHERE seq = ?",
                [report.seqs[0]],
                |row| row.get(0),
            )
            .expect("kind");
        assert_eq!(kind, "RequestStarted");
        let turn_id: String = conn
            .query_row(
                "SELECT CAST(CAST(payload_json AS JSON)->>'$.turn_id' AS VARCHAR)
                 FROM events WHERE seq = ?",
                [report.seqs[0]],
                |row| row.get(0),
            )
            .expect("payload");
        assert_eq!(turn_id, "turn-S-turn-1");
        // 消息正文直存 content_ref。
        let content: String = conn
            .query_row(
                "SELECT content_ref FROM messages WHERE id = ?",
                [format!("MSG-{}", report.seqs[2])],
                |row| row.get(0),
            )
            .expect("消息");
        assert_eq!(content, "你好");
    }

    #[test]
    fn rejects_empty_session_bad_role_and_oversized_payload() {
        let mut conn = migrated();
        let err = append_turn(&mut conn, "  ", &[], &[]).expect_err("空会话必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err = append_turn(
            &mut conn,
            "S-x",
            &[],
            &[TurnMessageDraft {
                role: "system",
                content: "s".to_string(),
                token_count: None,
                parent_id: None,
            }],
        )
        .expect_err("system 不落 messages（FR-722）");
        assert!(err.message.contains("角色非法"), "{err}");

        let oversized = "x".repeat(MAX_PAYLOAD_BYTES + 1);
        let err = append_turn(
            &mut conn,
            "S-x",
            &[TurnEventDraft {
                kind: EventKind::StreamingChunk,
                request_id: None,
                tool_call_id: None,
                payload: json!({ "blob": oversized }),
            }],
            &[],
        )
        .expect_err("超限载荷必须拒绝（R-05）");
        assert!(err.message.contains("上限"), "{err}");
    }

    #[test]
    fn rollback_leaves_no_partial_rows() {
        let mut conn = migrated();
        append_turn(
            &mut conn,
            "S-r",
            &[TurnEventDraft {
                kind: EventKind::RequestStarted,
                request_id: None,
                tool_call_id: None,
                payload: json!({ "n": 1 }),
            }],
            &[],
        )
        .expect("首批成功");
        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("计数");
        // 第二批：事件合法、消息正文超限——整体回滚，事件不得落表。
        let err = append_turn(
            &mut conn,
            "S-r",
            &[TurnEventDraft {
                kind: EventKind::RequestCompleted,
                request_id: None,
                tool_call_id: None,
                payload: json!({ "n": 2 }),
            }],
            &[TurnMessageDraft {
                role: "assistant",
                content: "y".repeat(MAX_TURN_MESSAGE_BYTES + 1),
                token_count: None,
                parent_id: None,
            }],
        );
        assert!(err.is_err(), "超限必须失败");
        let after: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(before, after, "回滚不留半程事件");
    }
}
