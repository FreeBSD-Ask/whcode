//! # 会话分叉存储原语（T282／FR-769／ADR-065）
//!
//! 分叉＝在源会话的一个 **turn 结束边界**上创建新会话，并把该边界之前的对话历史复制到
//! 新会话。原会话不被修改；载荷、时间戳与持久化级别原样保留，只有 `seq`／`id` 与
//! `session_id` 重新分配。
//!
//! ## 复制范围（events 与 messages 双表）
//!
//! `append_turn` 把 turn 的事件与消息写入 `events`／`messages` 两表，且**共用一条全局
//! 序号**（`EV-<seq>`／`MSG-<seq>` 同源递增）。因此对话历史的前缀由两表共同构成，分叉
//! 必须同时复制：
//!
//! - **事件**：`seq <= at_seq` 的全部事件；
//! - **消息**：`MSG-<n>` 序号 **小于源会话中下一个事件的 seq** 的全部消息。turn 的
//!   尾随消息（最终助手回复等）与 `TurnFinished` 同批落表且排在事件之后，所以"下一个
//!   事件之前"恰好把边界轮的消息收进来、又不会带入下一轮写在 `TurnStarted` 之后的
//!   用户消息；边界是最后一个 turn 时没有下一个事件，取全部剩余消息。
//!   `at_seq = 0`（空分叉）不复制任何消息。
//! - id 不符合 `MSG-<n>` 形态的消息（外部夹具）不参与计数与复制，与 [`crate::turn`]
//!   的序号口径一致。
//!
//! ## 边界与有界性
//!
//! - **只接受 turn 结束事件作为分叉点**：`at_seq = 0` 表示空分叉；`at_seq > 0` 时该
//!   `seq` 的事件必须是 `TurnFinished`，落在 turn 内一律 `InvalidState`——半截 turn
//!   不能成为新会话的起点。
//! - **有界**：继承事件数与继承消息数各自不超过 [`MAX_FORK_EVENTS`]（2000）；超限拒绝
//!   而不是截断，避免新会话静默丢失历史。
//! - **幂等键**：复制事件的 `origin_event_id` 记录源事件 ID，与目标 `session_id` 组成
//!   唯一索引（`idx_events_origin`）；同一目标会话重复写入同一源事件会被数据库拒绝
//!   （事务内不会发生，索引用于防止将来增量修复路径重复复制）。消息不带 origin 键：
//!   复制只发生在单事务内，消息的源身份由事件的 `origin_event_id` 链间接可溯。
//! - **单事务**：会话行、事件与消息在同一事务内提交；失败即回滚，不留下半个分叉。
//! - **序号基线**：新序号从 `GREATEST(全局事件最大 seq, 全局消息最大序号)` 之后分配
//!   （与 [`crate::turn::append_turn`] 同口径），保持"序号即物理先后"的全局不变量，
//!   未来的分叉边界判定不因此失真。
//!
//! 边界：SQL 只在本模块；权限判定与标题默认值属 `whcode-app`。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, EventKind, WhError};

use crate::directory::{self, SessionRow};

/// 单次分叉最多继承的事件数（含 turn 边界事件）。
pub const MAX_FORK_EVENTS: usize = 2_000;

/// 分叉结果（新建会话的元数据 + 复制统计）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ForkOutcome {
    /// 新会话行。
    pub session: SessionRow,
    /// 源会话 ID。
    pub source_session: String,
    /// 分叉点（源会话序号；0 表示空分叉）。
    pub forked_at_seq: i64,
    /// 实际继承的事件数。
    pub inherited_events: usize,
    /// 实际继承的消息数。
    pub inherited_messages: usize,
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, message.into())
}

fn store_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}：{err}"))
}

/// 在 `source` 的 `at_seq` 边界分叉出新会话。
///
/// `title` 是最终标题（默认值由应用层决定）；空标题拒绝。
///
/// # Errors
///
/// 源会话不存在→`not_found`；分叉点不是 turn 结束、越界、继承事件或消息超限、标题为空→
/// `invalid_state`；数据库失败→`io_error`（事务回滚）。
pub fn fork_session(
    conn: &mut Connection,
    source: &str,
    at_seq: i64,
    title: &str,
    now_ms: u64,
) -> Result<ForkOutcome, WhError> {
    if title.trim().is_empty() {
        return Err(invalid("分叉会话标题不得为空"));
    }
    if at_seq < 0 {
        return Err(invalid(format!("分叉序号不得为负：{at_seq}")));
    }
    let now = i64::try_from(now_ms).map_err(|_| invalid("now_ms 超出可表示范围"))?;

    let source_row = directory::get_session(conn, source)?.ok_or_else(|| {
        WhError::new(
            ErrorCode::NotFound,
            format!("源会话不存在：{source}；用 session list 查看现存会话"),
        )
    })?;

    let max_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?",
            params![source],
            |row| row.get(0),
        )
        .map_err(|err| store_err("读取源会话最新事件", err))?;
    if at_seq > max_seq {
        return Err(invalid(format!(
            "分叉序号 {at_seq} 超过源会话最新事件 {max_seq}"
        )));
    }
    if at_seq > 0 {
        let kind: String = conn
            .query_row(
                "SELECT kind FROM events WHERE session_id = ? AND seq = ?",
                params![source, at_seq],
                |row| row.get(0),
            )
            .map_err(|err| match err {
                duckdb::Error::QueryReturnedNoRows => invalid(format!(
                    "源会话 {source} 在 seq {at_seq} 没有事件；分叉点必须是 turn 结束边界"
                )),
                other => store_err("读取分叉点事件", other),
            })?;
        if kind != EventKind::TurnFinished.as_str() {
            return Err(invalid(format!(
                "分叉点必须是 turn 结束事件：seq {at_seq} 是 {kind}；请选择 TurnFinished 事件后重试"
            )));
        }
    }

    let inherited: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id = ? AND seq <= ?",
            params![source, at_seq],
            |row| row.get(0),
        )
        .map_err(|err| store_err("统计继承事件", err))?;
    let inherited = usize::try_from(inherited).map_err(|_| invalid("继承事件数超出范围"))?;
    if inherited > MAX_FORK_EVENTS {
        return Err(invalid(format!(
            "分叉将继承 {inherited} 条事件，超过上限 {MAX_FORK_EVENTS}；请选择更早的边界"
        )));
    }

    // 消息复制上限＝源会话中下一个事件的 seq（边界轮的尾随消息排在该事件之前；
    // 空分叉不复制消息；边界是最后一个 turn 时取全部剩余消息）。
    let message_limit: i64 = if at_seq == 0 {
        0
    } else {
        conn.query_row(
            "SELECT COALESCE(MIN(seq), 9223372036854775807) FROM events
             WHERE session_id = ? AND seq > ?",
            params![source, at_seq],
            |row| row.get(0),
        )
        .map_err(|err| store_err("读取分叉点之后的下一个事件", err))?
    };
    let inherited_messages: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages
             WHERE session_id = ? AND id LIKE 'MSG-%'
               AND CAST(SUBSTRING(id FROM 5) AS INTEGER) < ?",
            params![source, message_limit],
            |row| row.get(0),
        )
        .map_err(|err| store_err("统计继承消息", err))?;
    let inherited_messages =
        usize::try_from(inherited_messages).map_err(|_| invalid("继承消息数超出范围"))?;
    if inherited_messages > MAX_FORK_EVENTS {
        return Err(invalid(format!(
            "分叉将继承 {inherited_messages} 条消息，超过上限 {MAX_FORK_EVENTS}；请选择更早的边界"
        )));
    }

    let staged: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM events WHERE session_id = ? AND seq <= ? ORDER BY seq ASC")
            .map_err(|err| store_err("读取待复制事件", err))?;
        let rows = stmt
            .query_map(params![source, at_seq], |row| row.get::<_, String>(0))
            .map_err(|err| store_err("读取待复制事件", err))?;
        rows.collect::<duckdb::Result<Vec<_>>>()
            .map_err(|err| store_err("读取待复制事件", err))?
    };
    if staged.len() != inherited {
        return Err(invalid(format!(
            "待复制事件 {}/{} 与计数不一致；分叉已中止",
            staged.len(),
            inherited
        )));
    }
    let staged_messages: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM messages
                 WHERE session_id = ? AND id LIKE 'MSG-%'
                   AND CAST(SUBSTRING(id FROM 5) AS INTEGER) < ?
                 ORDER BY CAST(SUBSTRING(id FROM 5) AS INTEGER) ASC",
            )
            .map_err(|err| store_err("读取待复制消息", err))?;
        let rows = stmt
            .query_map(params![source, message_limit], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|err| store_err("读取待复制消息", err))?;
        rows.collect::<duckdb::Result<Vec<_>>>()
            .map_err(|err| store_err("读取待复制消息", err))?
    };
    if staged_messages.len() != inherited_messages {
        return Err(invalid(format!(
            "待复制消息 {}/{} 与计数不一致；分叉已中止",
            staged_messages.len(),
            inherited_messages
        )));
    }

    let tx = conn
        .transaction()
        .map_err(|err| store_err("开始分叉事务", err))?;
    let inherited_i64 = i64::try_from(inherited).map_err(|_| invalid("继承事件数超出范围"))?;
    let new_id: String = tx
        .query_row(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status,
                                   stage, parent_id, forked_at_seq, inherited_event_count)
             VALUES ('S-' || nextval('whcode_session_id_seq'), ?, epoch_ms(?), epoch_ms(?), ?,
                     FALSE, 'active', ?, ?, ?, ?)
             RETURNING id",
            params![
                title.trim(),
                now,
                now,
                source_row.model,
                source_row.stage,
                source,
                at_seq,
                inherited_i64,
            ],
            |row| row.get(0),
        )
        .map_err(|err| store_err("创建分叉会话", err))?;

    // 序号基线取 events 与 messages 双表最大值（与 append_turn 同口径），保持
    // "序号即物理先后"的全局不变量。
    let mut base: i64 = tx
        .query_row(
            "SELECT GREATEST(\
             (SELECT COALESCE(MAX(seq), 0) FROM events), \
             (SELECT COALESCE(MAX(CAST(SUBSTRING(id FROM 5) AS INTEGER)), 0) \
              FROM messages WHERE id LIKE 'MSG-%'))",
            [],
            |row| row.get(0),
        )
        .map_err(|err| store_err("读取事件序号", err))?;
    for origin in &staged {
        base += 1;
        let new_seq = base;
        let inserted = tx
            .execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level, origin_event_id)
                 SELECT ?, ?, ?, task_id, request_id, tool_call_id, kind, payload_json, ts,
                        persistence_level, id
                 FROM events WHERE id = ?",
                params![new_seq, format!("EV-{new_seq}"), new_id, origin],
            )
            .map_err(|err| store_err("复制分叉事件", err))?;
        if inserted != 1 {
            return Err(invalid(format!(
                "源事件 {origin} 未复制（期望 1 行，实际 {inserted}）；分叉已回滚"
            )));
        }
    }
    for origin in &staged_messages {
        base += 1;
        let new_seq = base;
        let inserted = tx
            .execute(
                "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
                 SELECT ?, ?, role, content_ref, token_count, ts, parent_id
                 FROM messages WHERE id = ?",
                params![format!("MSG-{new_seq}"), new_id, origin],
            )
            .map_err(|err| store_err("复制分叉消息", err))?;
        if inserted != 1 {
            return Err(invalid(format!(
                "源消息 {origin} 未复制（期望 1 行，实际 {inserted}）；分叉已回滚"
            )));
        }
    }
    tx.commit().map_err(|err| store_err("提交分叉事务", err))?;

    Ok(ForkOutcome {
        session: SessionRow {
            id: new_id,
            title: title.trim().to_string(),
            created_at_ms: now,
            last_active_at_ms: now,
            model: source_row.model,
            archived: false,
            status: "active".to_string(),
            stage: source_row.stage,
        },
        source_session: source.to_string(),
        forked_at_seq: at_seq,
        inherited_events: inherited,
        inherited_messages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;
    use crate::turn::{TurnEventDraft, TurnMessageDraft};

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-9001', '源会话', epoch_ms(1000), epoch_ms(1000), 'test-model', FALSE, 'active', 'implement')",
            [],
        )
        .expect("源会话");
        conn
    }

    fn event(conn: &Connection, seq: i64, kind: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-9001', NULL, 'req-1', NULL, ?, CAST(? AS JSON)::VARIANT,
                     epoch_ms(?), 'redacted')",
            params![
                seq,
                format!("EV-{seq}"),
                kind,
                format!("{{\"seq\":{seq}}}"),
                1000 + seq,
            ],
        )
        .expect("事件");
    }

    fn message(conn: &Connection, seq: i64, role: &str, content: &str) {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, 'S-9001', ?, ?, 0, epoch_ms(?), NULL)",
            params![format!("MSG-{seq}"), role, content, 1000 + seq],
        )
        .expect("消息");
    }

    fn count(conn: &Connection, sql: &str, session: &str) -> i64 {
        conn.query_row(sql, params![session], |row| row.get(0))
            .expect("计数")
    }

    #[test]
    fn fork_copies_events_and_boundary_turn_messages_and_keeps_source_intact() {
        let mut conn = db();
        // 全局序号跨两表递增：turn1 事件 1—3、尾随消息 4；turn2 从事件 5 开始。
        event(&conn, 1, "TurnStarted");
        event(&conn, 2, "StreamingChunk");
        event(&conn, 3, "TurnFinished");
        message(&conn, 4, "assistant", "回复一");
        event(&conn, 5, "TurnStarted");
        message(&conn, 6, "user", "问题二");
        event(&conn, 7, "TurnFinished");
        message(&conn, 8, "assistant", "回复二");

        let outcome = fork_session(&mut conn, "S-9001", 3, "分叉", 2000).expect("分叉");
        assert_eq!(outcome.inherited_events, 3);
        assert_eq!(outcome.inherited_messages, 1, "边界轮尾随消息一并继承");
        let copied_events: i64 = count(
            &conn,
            "SELECT COUNT(*) FROM events WHERE session_id = ?",
            &outcome.session.id,
        );
        assert_eq!(copied_events, 3);
        let copied_messages: i64 = count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE session_id = ?",
            &outcome.session.id,
        );
        assert_eq!(copied_messages, 1);
        let content: String = conn
            .query_row(
                "SELECT content_ref FROM messages WHERE session_id = ?",
                params![outcome.session.id],
                |row| row.get(0),
            )
            .expect("继承消息正文");
        assert_eq!(content, "回复一");
        let source_events: i64 = count(
            &conn,
            "SELECT COUNT(*) FROM events WHERE session_id = ?",
            "S-9001",
        );
        let source_messages: i64 = count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE session_id = ?",
            "S-9001",
        );
        assert_eq!((source_events, source_messages), (5, 3), "源会话不被修改");
        let origin: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ? AND origin_event_id IS NOT NULL",
                params![outcome.session.id],
                |row| row.get(0),
            )
            .expect("幂等键计数");
        assert_eq!(origin, 3);

        // 边界是最后一个 turn：没有下一个事件，全部剩余消息随分叉继承。
        let last = fork_session(&mut conn, "S-9001", 7, "末尾分叉", 3000).expect("末尾分叉");
        assert_eq!(last.inherited_events, 5);
        assert_eq!(last.inherited_messages, 3);
    }

    #[test]
    fn fork_rejects_mid_turn_boundary_and_unknown_source() {
        let mut conn = db();
        event(&conn, 1, "TurnStarted");
        event(&conn, 2, "StreamingChunk");
        event(&conn, 3, "TurnFinished");
        let err = fork_session(&mut conn, "S-9001", 2, "半截", 2000).expect_err("turn 内必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = fork_session(&mut conn, "S-404", 0, "缺失", 2000).expect_err("缺失源");
        assert_eq!(err.code, ErrorCode::NotFound);
        let err = fork_session(&mut conn, "S-9001", 9, "越界", 2000).expect_err("越界");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = fork_session(&mut conn, "S-9001", 0, "", 2000).expect_err("空标题");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn fork_at_zero_creates_empty_child_and_enforces_caps() {
        let mut conn = db();
        message(&conn, 1, "user", "先于任何事件的消息不随空分叉继承");
        let empty = fork_session(&mut conn, "S-9001", 0, "空分叉", 2000).expect("空分叉");
        assert_eq!(empty.inherited_events, 0);
        assert_eq!(empty.inherited_messages, 0);
        let count_events: i64 = count(
            &conn,
            "SELECT COUNT(*) FROM events WHERE session_id = ?",
            &empty.session.id,
        );
        let count_messages: i64 = count(
            &conn,
            "SELECT COUNT(*) FROM messages WHERE session_id = ?",
            &empty.session.id,
        );
        assert_eq!((count_events, count_messages), (0, 0));

        // 事件上限：继承 2001 条事件即拒绝。
        for seq in 1..=(MAX_FORK_EVENTS as i64 + 1) {
            let kind = if seq == MAX_FORK_EVENTS as i64 + 1 {
                "TurnFinished"
            } else {
                "StreamingChunk"
            };
            event(&conn, seq, kind);
        }
        event(&conn, MAX_FORK_EVENTS as i64 + 2, "TurnFinished");
        let err = fork_session(
            &mut conn,
            "S-9001",
            MAX_FORK_EVENTS as i64 + 2,
            "事件超限",
            2000,
        )
        .expect_err("超过事件上限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);

        // 消息上限：1 条边界事件 + 2001 条消息即拒绝。
        let mut conn2 = db();
        event(&conn2, 1, "TurnFinished");
        for seq in 2..=(MAX_FORK_EVENTS as i64 + 2) {
            message(&conn2, seq, "assistant", "填充");
        }
        let err = fork_session(&mut conn2, "S-9001", 1, "消息超限", 2000)
            .expect_err("超过消息上限必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }

    #[test]
    fn fork_allocates_from_shared_counter_and_keeps_global_order_invariant() {
        let mut conn = db();
        event(&conn, 1, "TurnFinished");
        // 尾随消息序号高于全部事件：基线必须取双表最大值。
        message(&conn, 5, "assistant", "尾部一");
        message(&conn, 6, "assistant", "尾部二");
        let outcome = fork_session(&mut conn, "S-9001", 1, "计数器", 2000).expect("分叉");
        assert_eq!(outcome.inherited_events, 1);
        assert_eq!(outcome.inherited_messages, 2);
        let child_event_seq: i64 = conn
            .query_row(
                "SELECT MIN(seq) FROM events WHERE session_id = ?",
                params![outcome.session.id],
                |row| row.get(0),
            )
            .expect("子会话事件序号");
        assert!(
            child_event_seq > 6,
            "新事件序号必须大于全局消息最大序号（实际 {child_event_seq}）"
        );
        // 分叉之后 append_turn 的双源序号仍不冲突：向子会话追加一轮不报错。
        let draft = vec![TurnEventDraft {
            kind: EventKind::TurnStarted,
            request_id: Some("req-fork".to_string()),
            tool_call_id: None,
            payload: serde_json::json!({"kind": "TurnStarted"}),
        }];
        let report = crate::turn::append_turn(
            &mut conn,
            &outcome.session.id,
            &draft,
            &[TurnMessageDraft {
                role: "user",
                content: "分叉后第一问".to_string(),
                token_count: Some(8),
                parent_id: None,
            }],
        )
        .expect("分叉后追加 turn");
        assert_eq!(report.seqs.len(), 2, "事件与消息各占一个序号");
    }
}
