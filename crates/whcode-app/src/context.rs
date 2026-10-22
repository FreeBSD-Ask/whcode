//! # 上下文加载与压缩摘要指针（T060/T061、FR-105/107/108/505、§4.1）
//!
//! [`load_context`] 按**预算**有界迭代会话历史（`messages` 稳定游标 `(ts, id)`，
//! FR-505/FR-106），返回契约四元组：`items + next_cursor + consumed_budget +
//! truncated_reason`（FR-105、§4.1）——不把整个历史一次性载入内存（FR-105 禁止）。
//!
//! [`compress_context`] 记录压缩摘要指针（`context_windows` 表：窗口边界 +
//! `summary_ref`）；**不删除任何原始历史**——压缩后原始历史仍可完整查询（FR-107）。
//! 摘要正文由 LLM 调用生成（属后续接入），本模块只记录指针。
//!
//! 一致性口径：`token_count` 为空的消息按 0 计入预算（历史数据无 token 计量时
//! 不阻塞加载；偏差已记录）。装入来源固定为 `messages`（FR-108）。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};
use whcode_store::query::{self, MessageCursor};

use crate::authz::{Actor, Capability, Policy, require};

/// 默认上下文预算（token 数；state-snapshot 契约示例值）。
pub const DEFAULT_CONTEXT_BUDGET_TOKENS: usize = 128_000;

/// 单页迭代上限（继承 store 分页上限，有界）。
pub const CONTEXT_PAGE_SIZE: usize = 1000;

/// 上下文条目（`content_ref` 引用化，不载入正文，R-05）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    /// 消息 ID。
    pub id: String,
    /// 角色。
    pub role: String,
    /// 正文引用（大正文走内容表/引用）。
    pub content_ref: String,
    /// token 数（历史数据可空，按 0 计）。
    pub token_count: usize,
    /// 时间（Unix 微秒）。
    pub ts_us: i64,
}

/// 上下文加载结果（契约四元组，FR-105）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextWindow {
    /// 装入的条目（按时间升序）。
    pub items: Vec<ContextItem>,
    /// 继续装入的游标；`None` 表示历史已到头。
    pub next_cursor: Option<MessageCursor>,
    /// 本次装入消耗的预算（token 数，空计量按 0）。
    pub consumed_budget: usize,
    /// 截断原因；`None` 表示未截断（FR-108：截断必须可查原因）。
    pub truncated_reason: Option<&'static str>,
    /// 装入来源（FR-108；M3 固定为 `messages`）。
    pub sources: Vec<&'static str>,
}

/// 预算耗尽的截断原因（FR-108 可查）。
pub const REASON_BUDGET_EXHAUSTED: &str = "budget_exhausted";

fn invalid(context: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, context.into())
}

/// 按预算有界加载上下文（FR-105）。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn load_context(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    budget_tokens: usize,
) -> Result<ContextWindow, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let mut items = Vec::new();
    let mut consumed = 0usize;
    let mut truncated = false;
    let mut cursor: Option<MessageCursor> = None;
    let mut last_cursor: Option<MessageCursor> = None;

    loop {
        let page = query::messages_page(
            conn,
            session_id,
            whcode_store::Direction::Forward,
            cursor,
            CONTEXT_PAGE_SIZE,
        )?;
        for message in &page.items {
            let tokens = message.token_count.unwrap_or(0).max(0) as usize;
            if consumed + tokens > budget_tokens {
                truncated = true;
                break;
            }
            consumed += tokens;
            items.push(ContextItem {
                id: message.id.clone(),
                role: message.role.clone(),
                content_ref: message.content_ref.clone(),
                token_count: tokens,
                ts_us: message.ts_us,
            });
            last_cursor = Some(MessageCursor {
                ts_us: message.ts_us,
                id: message.id.clone(),
            });
        }
        if truncated || page.next.is_none() {
            break;
        }
        cursor = page.next;
    }

    Ok(ContextWindow {
        next_cursor: if truncated { last_cursor } else { None },
        items,
        consumed_budget: consumed,
        truncated_reason: truncated.then_some(REASON_BUDGET_EXHAUSTED),
        sources: vec!["messages"],
    })
}

/// 压缩摘要指针（`context_windows` 表行投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryPointer {
    /// 指针行 ID。
    pub id: String,
    /// 所属会话。
    pub session_id: String,
    /// 窗口起点 `seq`。
    pub window_start_seq: i64,
    /// 窗口终点 `seq`。
    pub window_end_seq: i64,
    /// 窗口内 token 总数。
    pub total_tokens: i64,
    /// 摘要引用（正文由 LLM 生成，属后续接入）。
    pub summary_ref: String,
}

/// 压缩参数（窗口边界 + 摘要引用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCompression<'a> {
    /// 所属会话。
    pub session_id: &'a str,
    /// 窗口起点 `seq`。
    pub window_start_seq: i64,
    /// 窗口终点 `seq`。
    pub window_end_seq: i64,
    /// 窗口内 token 总数。
    pub total_tokens: i64,
    /// 摘要引用。
    pub summary_ref: &'a str,
}

/// 记录压缩摘要指针（T061、FR-107）：**不删除原始历史**，压缩后原文仍可查。
///
/// # Errors
///
/// 权限拒绝、边界非法或存储失败时返回 [`WhError`]。
pub fn compress_context(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    compression: &ContextCompression<'_>,
) -> Result<SummaryPointer, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if compression.window_end_seq < compression.window_start_seq {
        return Err(invalid(format!(
            "上下文窗口边界非法：start {} > end {}（FR-107 校验）",
            compression.window_start_seq, compression.window_end_seq
        )));
    }
    if compression.summary_ref.trim().is_empty() {
        return Err(invalid("summary_ref 不得为空：压缩指针必须可定位摘要"));
    }
    let session_id = compression.session_id;
    let window_start_seq = compression.window_start_seq;
    let window_end_seq = compression.window_end_seq;
    let total_tokens = compression.total_tokens;
    let summary_ref = compression.summary_ref;
    let id = format!("CW-{session_id}-{window_end_seq}");
    conn.execute(
        "INSERT INTO context_windows (id, session_id, window_start_seq, window_end_seq,
                                      total_tokens, summary_ref, created_at)
         VALUES (?, ?, ?, ?, ?, ?, current_timestamp)",
        duckdb::params![
            id,
            session_id,
            window_start_seq,
            window_end_seq,
            total_tokens,
            summary_ref
        ],
    )
    .map_err(|err| WhError::new(ErrorCode::IoError, format!("写入压缩指针失败：{err}")))?;
    Ok(SummaryPointer {
        id,
        session_id: session_id.to_string(),
        window_start_seq,
        window_end_seq,
        total_tokens,
        summary_ref: summary_ref.to_string(),
    })
}

/// 读取会话的最新压缩指针；不存在返回 `None`。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn load_summary_pointer(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<SummaryPointer>, WhError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, window_start_seq, window_end_seq, total_tokens, summary_ref
             FROM context_windows WHERE session_id = ?
             ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("查询压缩指针失败：{err}")))?;
    let mut rows = stmt
        .query(duckdb::params![session_id])
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("查询压缩指针失败：{err}")))?;
    let Some(row) = rows
        .next()
        .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?
    else {
        return Ok(None);
    };
    Ok(Some(SummaryPointer {
        id: row
            .get(0)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?,
        session_id: row
            .get(1)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?,
        window_start_seq: row
            .get(2)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?,
        window_end_seq: row
            .get(3)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?,
        total_tokens: row
            .get(4)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?,
        summary_ref: row
            .get(5)
            .map_err(|err| WhError::new(ErrorCode::IoError, format!("{err}")))?,
    }))
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

    fn seed_messages(conn: &Connection, session_id: &str, count: usize, tokens_each: usize) {
        for index in 0..count {
            conn.execute(
                "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
                 VALUES (?, ?, 'user', ?, ?, current_timestamp, NULL)",
                params![
                    format!("M-{index}"),
                    session_id,
                    format!("ref-{index}"),
                    tokens_each as i64
                ],
            )
            .expect("插入消息");
        }
    }

    #[test]
    fn load_context_returns_contract_tuple_and_reports_truncation_reason() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "上下文", "active", 1_000).expect("创建");
        seed_messages(&conn, "S-1", 10, 100);

        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        // 预算 350：装入 3 条（300），第 4 条会超 → 截断原因 budget_exhausted。
        let window = load_context(&conn, &policy, &actor, "S-1", 350).expect("加载");
        assert_eq!(window.items.len(), 3);
        assert_eq!(window.consumed_budget, 300);
        assert_eq!(window.truncated_reason, Some(REASON_BUDGET_EXHAUSTED));
        let cursor = window.next_cursor.expect("续装游标");
        assert_eq!(cursor.id, "M-2", "游标指向最后装入条目");

        // 全量预算：10 条全部装入后到头（无截断）。
        let remaining = load_context(&conn, &policy, &actor, "S-1", 1_000_000).expect("全量");
        assert_eq!(remaining.items.len(), 10);
        assert_eq!(remaining.truncated_reason, None);
        assert!(remaining.next_cursor.is_none());
        assert_eq!(remaining.sources, vec!["messages"]);

        // items 引用化：不携带正文。
        assert_eq!(remaining.items[0].content_ref, "ref-0");
    }

    #[test]
    fn empty_session_loads_nothing_and_permission_is_enforced() {
        let conn = migrated();
        create_session_with_id(&conn, "S-empty", "空", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let window = load_context(&conn, &policy, &actor, "S-empty", 1_000).expect("空加载");
        assert!(window.items.is_empty());
        assert_eq!(window.truncated_reason, None);
        assert_eq!(window.consumed_budget, 0);

        let err = load_context(
            &conn,
            &policy,
            &crate::authz::Actor::new("无能力", []),
            "S-empty",
            1_000,
        )
        .expect_err("无能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }

    #[test]
    fn compress_records_pointer_without_deleting_history() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "压缩", "active", 1_000).expect("创建");
        seed_messages(&conn, "S-1", 5, 100);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let pointer = compress_context(
            &conn,
            &policy,
            &actor,
            &ContextCompression {
                session_id: "S-1",
                window_start_seq: 1,
                window_end_seq: 5,
                total_tokens: 500,
                summary_ref: "summary:A-1",
            },
        )
        .expect("记录指针");
        assert_eq!(pointer.window_start_seq, 1);
        assert_eq!(pointer.summary_ref, "summary:A-1");

        let loaded = load_summary_pointer(&conn, "S-1")
            .expect("读取指针")
            .expect("存在");
        assert_eq!(loaded.id, pointer.id);
        assert_eq!(loaded.total_tokens, 500);

        // 压缩后原始历史仍可完整查询（FR-107）。
        let window = load_context(&conn, &policy, &actor, "S-1", 1_000_000).expect("原文可查");
        assert_eq!(window.items.len(), 5, "压缩不删除原始历史");

        let err = compress_context(
            &conn,
            &policy,
            &actor,
            &ContextCompression {
                session_id: "S-1",
                window_start_seq: 9,
                window_end_seq: 5,
                total_tokens: 1,
                summary_ref: "s",
            },
        )
        .expect_err("边界非法必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let err = compress_context(
            &conn,
            &policy,
            &actor,
            &ContextCompression {
                session_id: "S-1",
                window_start_seq: 1,
                window_end_seq: 5,
                total_tokens: 1,
                summary_ref: "  ",
            },
        )
        .expect_err("空 summary_ref 必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            load_summary_pointer(&conn, "S-none")
                .expect("无指针")
                .is_none()
        );
    }
}
