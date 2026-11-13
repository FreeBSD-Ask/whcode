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
