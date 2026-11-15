//! # 对话剪枝边界存储原语（T288／FR-775／ADR-068）
//!
//! `SessionRewound` 是**只增不改**的边界事件：最新一条给出 `(target_seq, rewind_seq)`，
//! 派生层据此判定历史有效区（`s <= target || s > rewind_seq`）。本模块只做边界校验、
//! 事件写入与读取；过滤在应用层（`whcode-app::rewind`／`context`）。
//!
//! 边界：目标必须是 `0` 或本会话一条 `TurnFinished` 的 `seq`（ADR-065 同口径：不接受半截
//! turn）；回退到当前末尾是无操作，显式拒绝。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 事件类型原文（与 `whcode_domain::EventKind::SessionRewound` 的稳定名一致）。
pub const REWIND_EVENT_KIND: &str = "SessionRewound";

/// 记录一次剪枝边界（`requested_by` 为 `user`／`model`）。
///
/// 返回新事件的 `(seq, id)`。
///
/// # Errors
///
/// 目标不是合法 turn 边界、目标不早于当前末尾、来源非法、会话不存在或写入失败。
pub fn record_rewind(
    conn: &Connection,
    session_id: &str,
    target_seq: i64,
    requested_by: &str,
) -> Result<(i64, String), WhError> {
    if session_id.trim().is_empty() {
        return Err(WhError::invalid_state("对话剪枝需要非空会话 ID"));
    }
    if requested_by != "user" && requested_by != "model" {
        return Err(WhError::invalid_state(format!(
            "对话剪枝来源只能是 user 或 model，实际 {requested_by}"
        )));
    }
    if target_seq < 0 {
        return Err(WhError::invalid_state("对话剪枝目标序号不得为负"));
    }
    let max_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|err| store_err("读取会话末尾序号", err))?;
    if max_seq == 0 {
        return Err(WhError::new(
            ErrorCode::NotFound,
            format!("会话 {session_id} 没有任何事件，无可回退的历史"),
        ));
    }
    if target_seq >= max_seq {
        return Err(WhError::invalid_state(format!(
            "回退目标 {target_seq} 不早于当前末尾 {max_seq}：回退到末尾是无操作；\
             请选择更早的 TurnFinished 序号"
        )));
    }
    if target_seq > 0 {
        let kind: Option<String> = conn
            .query_row(
                "SELECT kind FROM events WHERE session_id = ? AND seq = ?",
                params![session_id, target_seq],
                |row| row.get(0),
            )
            .ok();
        match kind.as_deref() {
            Some("TurnFinished") => {}
            Some(other) => {
                return Err(WhError::invalid_state(format!(
                    "回退目标 {target_seq} 是 {other} 事件；边界必须落在 turn 结束\
                     （TurnFinished）或 0（空回退）——半截 turn 不得成为回退点"
                )));
            }
            None => {
                return Err(WhError::invalid_state(format!(
                    "回退目标 {target_seq} 不在本会话事件中；请用 events list 查看可用边界"
                )));
            }
        }
    }
    let seq: i64 = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM events", [], |row| {
            row.get(0)
        })
        .map_err(|err| store_err("读取事件序号", err))?;
    let id = format!("EV-{seq}");
    let payload = serde_json::json!({ "target_seq": target_seq, "requested_by": requested_by });
    conn.execute(
        "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                             kind, payload_json, ts, persistence_level)
         VALUES (?, ?, ?, NULL, NULL, NULL, ?, CAST(? AS JSON)::VARIANT,
                 current_timestamp, 'redacted')",
        params![seq, id, session_id, REWIND_EVENT_KIND, payload.to_string()],
    )
    .map_err(|err| store_err("写入对话剪枝边界", err))?;
    Ok((seq, id))
}

/// 读取最新一条剪枝边界（`(target_seq, rewind_seq)`）；无记录返回 `None`（正确的空）。
///
/// # Errors
///
/// 查询失败或载荷损坏（缺少 `target_seq`）时返回错误——不得静默按“未回退”处理。
pub fn latest_rewind(conn: &Connection, session_id: &str) -> Result<Option<(i64, i64)>, WhError> {
    let Some((seq, payload)) =
        crate::query::latest_event_of_kind(conn, session_id, REWIND_EVENT_KIND)?
    else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|err| WhError::parse_error(format!("对话剪枝事件载荷损坏：{err}")))?;
    let target = value
        .get("target_seq")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| {
            WhError::new(
                ErrorCode::InvalidState,
                "对话剪枝事件缺少 target_seq（边界不可判定）",
            )
        })?;
    Ok(Some((target, seq)))
}

fn store_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-1', '剪枝', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'implement')",
            [],
        )
        .expect("插入会话");
        conn
    }

    fn insert_event(conn: &Connection, seq: i64, kind: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-1', ?, CAST('{}' AS JSON)::VARIANT, current_timestamp, 'redacted')",
            params![seq, format!("EV-{seq}"), kind],
        )
        .expect("插入事件");
    }

    /// 每类非法边界都显式报错：空会话、来源、负序号、空历史、末尾、半截 turn、缺口。
    #[test]
    fn invalid_boundaries_are_explicit() {
        let conn = fixture();
        for (session, target, by, code, why) in [
            ("  ", 1, "user", ErrorCode::InvalidState, "空会话"),
            ("S-1", 1, "robot", ErrorCode::InvalidState, "来源非法"),
            ("S-1", -1, "model", ErrorCode::InvalidState, "负序号"),
            ("S-1", 1, "user", ErrorCode::NotFound, "无事件"),
        ] {
            let err = record_rewind(&conn, session, target, by).expect_err(why);
            assert_eq!(err.code, code, "{why}: {}", err.message);
        }

        // 1 为 turn 边界、2 为缺口、3 为半截 turn、4 为末尾。
        insert_event(&conn, 1, "TurnFinished");
        insert_event(&conn, 3, "UserMessage");
        insert_event(&conn, 4, "TurnFinished");
        let err = record_rewind(&conn, "S-1", 4, "user").expect_err("回退到末尾");
        assert!(err.message.contains("不早于当前末尾"), "{}", err.message);
        let err = record_rewind(&conn, "S-1", 3, "user").expect_err("半截 turn");
        assert!(err.message.contains("TurnFinished"), "{}", err.message);
        let err = record_rewind(&conn, "S-1", 2, "user").expect_err("序号缺口");
        assert!(err.message.contains("不在本会话事件中"), "{}", err.message);

        // 合法边界：写入 SessionRewound 且可读回；无记录的会话返回正确的空。
        let (seq, id) = record_rewind(&conn, "S-1", 1, "model").expect("回退到 1");
        assert_eq!(seq, 5);
        assert_eq!(id, "EV-5");
        assert_eq!(latest_rewind(&conn, "S-1").expect("最新"), Some((1, 5)));
        assert_eq!(latest_rewind(&conn, "S-NONE").expect("无记录"), None);
    }

    /// 载荷缺 `target_seq` 必须显式报错，不得静默当作“未回退”。
    #[test]
    fn corrupted_rewind_payload_is_explicit() {
        let conn = fixture();
        conn.execute(
            "INSERT INTO events (seq, id, session_id, kind, payload_json, ts, persistence_level)
             VALUES (1, 'EV-1', 'S-1', ?,
                     CAST('{\"x\":1}' AS JSON)::VARIANT, current_timestamp, 'redacted')",
            params![REWIND_EVENT_KIND],
        )
        .expect("插入坏载荷");
        let err = latest_rewind(&conn, "S-1").expect_err("缺 target_seq");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("target_seq"), "{}", err.message);
    }
}
