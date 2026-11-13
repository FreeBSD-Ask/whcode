//! # 文件前像存储（T289／FR-776／ADR-068）
//!
//! 每次成功的 `write_file`／`edit_file` 调用登记一条“改前内容”：
//!
//! - 文本文件且 ≤ [`MAX_PREIMAGE_BYTES`]：存内容；
//! - 超限或非 UTF-8：只登记 `skipped_reason`（不静默丢弃，恢复时可见“不可恢复”）。
//!
//! ## 有界性（资源铁律）
//!
//! 每会话最多 [`MAX_PREIMAGES_PER_SESSION`] 条、内容合计 ≤ [`MAX_PREIMAGE_TOTAL_BYTES`]；
//! 写入后按创建顺序裁剪最旧。裁剪只删前像行，不影响工作区文件与 `events`。
//!
//! ## 边界
//!
//! SQL 只在本 crate；文件系统读写由应用层承担（恢复是破坏族，见 `whcode-app::preimage`）。

use duckdb::{Connection, params};
use whcode_domain::{ErrorCode, WhError};

/// 单文件前像内容上限（字节）。超出不存内容，只登记跳过原因。
pub const MAX_PREIMAGE_BYTES: usize = 256 * 1024;

/// 每会话前像条数上限（含跳过登记）。
pub const MAX_PREIMAGES_PER_SESSION: usize = 50;

/// 每会话前像内容合计上限（字节）。
pub const MAX_PREIMAGE_TOTAL_BYTES: usize = 8 * 1024 * 1024;

/// 一条前像（读取视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreimageRow {
    /// 行 ID（`PI-<n>`）。
    pub id: String,
    /// 产生该前像的工具执行 ID（`TX-...`）。
    pub exec_id: String,
    /// 工作区内相对路径。
    pub path: String,
    /// 改前大小（字节；跳过行记录原始大小）。
    pub size_bytes: i64,
    /// 改前内容（跳过行与 `skipped_reason.is_some()` 互斥）。
    pub content: Option<String>,
    /// 跳过原因（`too_large`／`not_utf8`）；`None` 表示有内容。
    pub skipped_reason: Option<String>,
}

/// 登记一条前像（内容或跳过原因恰有一个）。
///
/// # Errors
///
/// `content` 与 `skipped_reason` 同时为空／同时给定时 `invalid_state`；会话不存在或写入
/// 失败时返回错误。裁剪失败按写入失败处理（前像表不得无界增长）。
pub fn record(
    conn: &Connection,
    session_id: &str,
    exec_id: &str,
    path: &str,
    size_bytes: i64,
    content: Option<&str>,
    skipped_reason: Option<&str>,
) -> Result<String, WhError> {
    if session_id.trim().is_empty() || exec_id.trim().is_empty() || path.trim().is_empty() {
        return Err(WhError::invalid_state(
            "文件前像需要非空会话、执行 ID 与路径",
        ));
    }
    match (content, skipped_reason) {
        (Some(_), None) | (None, Some(_)) => {}
        _ => {
            return Err(WhError::invalid_state(
                "文件前像必须且只能提供 content 或 skipped_reason 之一",
            ));
        }
    }
    if let Some(text) = content
        && text.len() > MAX_PREIMAGE_BYTES
    {
        return Err(WhError::invalid_state(format!(
            "前像内容 {} 字节超过单文件上限 {MAX_PREIMAGE_BYTES}；应以 skipped_reason 登记",
            text.len()
        )));
    }
    let next: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(CAST(SUBSTRING(id FROM 4) AS BIGINT)), 0) + 1
             FROM file_preimages",
            [],
            |row| row.get(0),
        )
        .map_err(|err| store_err("读取前像序号", err))?;
    let id = format!("PI-{next}");
    conn.execute(
        "INSERT INTO file_preimages (id, session_id, exec_id, path, size_bytes, content,
                                     skipped_reason, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, current_timestamp)",
        params![
            id,
            session_id,
            exec_id,
            path,
            size_bytes,
            content,
            skipped_reason
        ],
    )
    .map_err(|err| store_err("写入文件前像", err))?;
    prune(conn, session_id)?;
    Ok(id)
}

/// 按创建顺序裁剪到上限（条数＋内容字节）。
fn prune(conn: &Connection, session_id: &str) -> Result<(), WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(LENGTH(content), 0) FROM file_preimages
             WHERE session_id = ? ORDER BY created_at ASC, id ASC",
        )
        .map_err(|err| store_err("读取前像清单", err))?;
    let rows: Vec<(String, i64)> = stmt
        .query_map([session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|err| store_err("读取前像清单", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| store_err("读取前像清单", err))?;
    let mut drop_ids: Vec<String> = Vec::new();
    let mut count = rows.len();
    let mut total: i64 = rows.iter().map(|(_, len)| *len).sum();
    for (id, len) in &rows {
        if count <= MAX_PREIMAGES_PER_SESSION && total <= MAX_PREIMAGE_TOTAL_BYTES as i64 {
            break;
        }
        drop_ids.push(id.clone());
        count -= 1;
        total -= len;
    }
    for id in drop_ids {
        conn.execute("DELETE FROM file_preimages WHERE id = ?", [id])
            .map_err(|err| store_err("裁剪前像", err))?;
    }
    Ok(())
}

/// 列出会话前像（新→旧，`limit` 有界；含跳过登记）。
///
/// # Errors
///
/// `limit` 为 0、超过 [`MAX_PREIMAGES_PER_SESSION`] 或查询失败。
pub fn list(
    conn: &Connection,
    session_id: &str,
    limit: usize,
) -> Result<Vec<PreimageRow>, WhError> {
    if limit == 0 || limit > MAX_PREIMAGES_PER_SESSION {
        return Err(WhError::invalid_state(format!(
            "前像读取 limit 必须在 1..={MAX_PREIMAGES_PER_SESSION}"
        )));
    }
    let mut stmt = conn
        .prepare(
            "SELECT id, exec_id, path, size_bytes, content, skipped_reason
             FROM file_preimages WHERE session_id = ?
             ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .map_err(|err| store_err("读取前像", err))?;
    let rows = stmt
        .query_map(params![session_id, limit as i64], |row| {
            Ok(PreimageRow {
                id: row.get(0)?,
                exec_id: row.get(1)?,
                path: row.get(2)?,
                size_bytes: row.get(3)?,
                content: row.get(4)?,
                skipped_reason: row.get(5)?,
            })
        })
        .map_err(|err| store_err("读取前像", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| store_err("读取前像", err))?;
    Ok(rows)
}

/// 按执行 ID 取一组前像（同一次工具调用内的全部写；按登记顺序）。
///
/// # Errors
///
/// 查询失败。
pub fn by_exec(
    conn: &Connection,
    session_id: &str,
    exec_id: &str,
) -> Result<Vec<PreimageRow>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, exec_id, path, size_bytes, content, skipped_reason
             FROM file_preimages WHERE session_id = ? AND exec_id = ?
             ORDER BY created_at ASC, id ASC",
        )
        .map_err(|err| store_err("读取执行前像", err))?;
    let rows = stmt
        .query_map(params![session_id, exec_id], |row| {
            Ok(PreimageRow {
                id: row.get(0)?,
                exec_id: row.get(1)?,
                path: row.get(2)?,
                size_bytes: row.get(3)?,
                content: row.get(4)?,
                skipped_reason: row.get(5)?,
            })
        })
        .map_err(|err| store_err("读取执行前像", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| store_err("读取执行前像", err))?;
    Ok(rows)
}

fn store_err(context: &str, err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{context}失败：{err}"))
}
