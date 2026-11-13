//! # 计划模式状态存储原语（T287／FR-774／ADR-067）
//!
//! `PlanModeChanged` 是**整值替换**的 log-only 事件：最新一条即当前状态（`active`）。
//! 本模块只提供“落一条状态事件”的原语；读取走 [`crate::query::latest_event_of_kind`]，
//! 状态派生与权限判定在应用层（`whcode-app::plan`）。
//!
//! 边界：序号沿用全局 `events` 序号（与 `append_turn`、分叉同一不变量），单条插入、
//! 无批量累积；载荷由调用方给定时长有界（三个短字段）。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 记录一次计划模式状态（整值替换；`source` 为 `user`／`model`）。
///
/// 返回新事件的 `(seq, id)`，便于调用方回填与审计对照。
///
/// # Errors
///
/// `source` 非 `user`／`model`、`session_id` 为空或数据库写入失败时返回错误
/// （`session` 外键缺失由数据库拒绝，不静默伪造成功）。
pub fn record_mode(
    conn: &Connection,
    session_id: &str,
    active: bool,
    source: &str,
    tool_call_id: Option<&str>,
) -> Result<(i64, String), WhError> {
    if session_id.trim().is_empty() {
        return Err(WhError::invalid_state("计划模式状态需要非空会话 ID"));
    }
    if source != "user" && source != "model" {
        return Err(WhError::invalid_state(format!(
            "计划模式来源只能是 user 或 model，实际 {source}"
        )));
    }
    let mut payload = serde_json::json!({ "active": active, "source": source });
    if let Some(tool_call_id) = tool_call_id {
        payload["tool_call_id"] = serde_json::Value::String(tool_call_id.to_string());
    }
    let seq: i64 = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM events", [], |row| {
            row.get(0)
        })
        .map_err(|err| store_err("读取事件序号", err))?;
    let id = format!("EV-{seq}");
    conn.execute(
        "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                             kind, payload_json, ts, persistence_level)
         VALUES (?, ?, ?, NULL, NULL, ?, 'PlanModeChanged', CAST(? AS JSON)::VARIANT,
                 current_timestamp, 'redacted')",
        params![seq, id, session_id, tool_call_id, payload.to_string()],
    )
    .map_err(|err| store_err("写入计划模式状态", err))?;
    Ok((seq, id))
}

/// 读取会话当前计划模式状态（最新一条 `PlanModeChanged`）；无记录返回 `None`。
///
/// # Errors
///
/// 数据库查询失败或载荷损坏时返回错误（`None` 只表示从未写入，是正确的空）。
pub fn current_mode(conn: &Connection, session_id: &str) -> Result<Option<(i64, bool)>, WhError> {
    let Some((seq, payload)) =
        crate::query::latest_event_of_kind(conn, session_id, "PlanModeChanged")?
    else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&payload)
        .map_err(|err| WhError::parse_error(format!("计划模式事件载荷损坏：{err}")))?;
    let active = value
        .get("active")
        .and_then(serde_json::Value::as_bool)
        .ok_or_else(|| {
            WhError::new(
                ErrorCode::InvalidState,
                "计划模式事件缺少 active 布尔值（状态不可判定）",
            )
        })?;
    Ok(Some((seq, active)))
}

fn store_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}
