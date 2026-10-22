//! # 游标分页（T035、FR-504、§7.2）
//!
//! 历史查询使用**稳定全序 + 游标**，不使用 `OFFSET`：
//!
//! - `events`：全序键 `seq`（工作区内唯一、单调递增）；
//! - `messages`：全序键 `(ts, id)`——时间戳可重复，必须用 `id` 破平局。
//!
//! 方向语义是显式的：
//! - [`Direction::Forward`]：按全序键**升序**返回；游标是**排他下界**（只取 `> cursor`）；
//! - [`Direction::Backward`]：按全序键**降序**返回；游标是**排他上界**（只取 `< cursor`）。
//!
//! `items` 始终按查询方向排列（Backward 为“最新在前”）；`next` 为继续同方向翻页的游标，
//! `None` 表示已到该方向尽头。实现内部取 `limit + 1` 行判断是否还有下一页，因此单页内存
//! 有界（`1..=MAX_PAGE_SIZE`），且不会出现“有下一页却返回 None”的歧义。
//!
//! 边界：本模块只读，不写库；SQL 中的可变部分只有方向和游标条件，值全部参数化。

use duckdb::types::Value;
use duckdb::{Connection, params_from_iter};
use whcode_domain::{ErrorCode, WhError};

/// 单页最大条数；超过即拒绝，避免调用方用超大 `limit` 绕过有界批次（§7.1）。
pub const MAX_PAGE_SIZE: usize = 1000;

/// 翻页方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// 沿全序键升序（较旧 → 较新）。
    Forward,
    /// 沿全序键降序（较新 → 较旧）。
    Backward,
}

/// 事件游标：排他 `seq`。`None` 表示从该方向的端点开始。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventCursor {
    /// 边界事件的 `seq`；翻页时不重复返回该条。
    pub seq: i64,
}

/// 消息游标：排他 `(ts_us, id)`，与全序键 `(ts, id)` 等价。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageCursor {
    /// 边界消息的 Unix 微秒时间戳。
    pub ts_us: i64,
    /// 边界消息的 `id`（时间戳相同时的破平局键）。
    pub id: String,
}

/// 事件行（只读投影；载荷保持 JSON 原文，避免在存储层做二次解析）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EventRow {
    /// 工作区内唯一序号。
    pub seq: i64,
    /// 事件 ID。
    pub id: String,
    /// 所属会话。
    pub session_id: Option<String>,
    /// 所属任务。
    pub task_id: Option<String>,
    /// 所属模型请求。
    pub request_id: Option<String>,
    /// 工具调用 ID。
    pub tool_call_id: Option<String>,
    /// 事件类型。
    pub kind: String,
    /// 载荷 JSON 原文。
    pub payload_json: String,
    /// Unix 微秒时间戳。
    pub ts_us: i64,
    /// 持久化等级。
    pub persistence_level: String,
}

/// 事件页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPage {
    /// 本页事件，按查询方向排列。
    pub items: Vec<EventRow>,
    /// 继续同方向翻页的游标；`None` 表示到头。
    pub next: Option<EventCursor>,
}

/// 消息行（只读投影）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    /// 消息 ID。
    pub id: String,
    /// 所属会话。
    pub session_id: String,
    /// 角色。
    pub role: String,
    /// 内容引用（大正文不入本表）。
    pub content_ref: String,
    /// token 计数（未知为 `None`）。
    pub token_count: Option<i64>,
    /// Unix 微秒时间戳。
    pub ts_us: i64,
    /// 父消息 ID。
    pub parent_id: Option<String>,
}

/// 消息页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    /// 本页消息，按查询方向排列。
    pub items: Vec<MessageRow>,
    /// 继续同方向翻页的游标；`None` 表示到头。
    pub next: Option<MessageCursor>,
}

fn query_err(err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("游标分页查询失败：{err}"))
}

/// 校验单页条数，并返回内部取数上限（多取一行用于判断是否还有下一页）。
fn fetch_limit(limit: usize) -> Result<i64, WhError> {
    if limit == 0 {
        return Err(WhError::invalid_state("分页 limit 必须大于 0"));
    }
    if limit > MAX_PAGE_SIZE {
        return Err(WhError::invalid_state(format!(
            "分页 limit 不得超过 {MAX_PAGE_SIZE}：历史查询必须有界（§7.1）"
        )));
    }
    i64::try_from(limit + 1).map_err(|_| WhError::invalid_state("分页 limit 超出 i64 范围"))
}

const EVENT_COLUMNS: &str = "seq, id, session_id, task_id, request_id, tool_call_id, kind, \
     payload_json, epoch_us(ts), persistence_level";

const MESSAGE_COLUMNS: &str =
    "id, session_id, role, content_ref, token_count, epoch_us(ts), parent_id";

/// 查询某会话的事件页。
///
/// 排序键是工作区内唯一的 `seq`，因此跨页不会重复或遗漏；游标排他。
///
/// # Errors
///
/// `limit` 为 0、超过 [`MAX_PAGE_SIZE`]，或数据库查询失败时返回 [`WhError`]。
pub fn events_page(
    conn: &Connection,
    session_id: &str,
    direction: Direction,
    cursor: Option<EventCursor>,
    limit: usize,
) -> Result<EventPage, WhError> {
    let fetch = fetch_limit(limit)?;
    let (order, comparison) = match direction {
        Direction::Forward => ("ASC", ">"),
        Direction::Backward => ("DESC", "<"),
    };

    let mut values = vec![Value::Text(session_id.to_string())];
    let cursor_clause = match cursor {
        Some(cursor) => {
            values.push(Value::BigInt(cursor.seq));
            format!(" AND seq {comparison} ?")
        }
        None => String::new(),
    };
    values.push(Value::BigInt(fetch));

    let sql = format!(
        "SELECT {EVENT_COLUMNS} FROM events WHERE session_id = ?{cursor_clause} \
         ORDER BY seq {order} LIMIT ?"
    );
    let mut stmt = conn.prepare(&sql).map_err(query_err)?;
    let mapped = stmt
        .query_map(params_from_iter(values), |row| {
            Ok(EventRow {
                seq: row.get(0)?,
                id: row.get(1)?,
                session_id: row.get(2)?,
                task_id: row.get(3)?,
                request_id: row.get(4)?,
                tool_call_id: row.get(5)?,
                kind: row.get(6)?,
                payload_json: row.get(7)?,
                ts_us: row.get(8)?,
                persistence_level: row.get(9)?,
            })
        })
        .map_err(query_err)?;

    let mut items = Vec::with_capacity(limit);
    for row in mapped {
        items.push(row.map_err(query_err)?);
    }

    let has_more = items.len() > limit;
    if has_more {
        items.truncate(limit);
    }
    let next = has_more
        .then(|| items.last().map(|row| EventCursor { seq: row.seq }))
        .flatten();

    Ok(EventPage { items, next })
}

/// 工作区事件游标：`events.seq` 的当前最大值；空表为 0。
///
/// 快照 `event_cursor`（FR-314）与断线补拉（FR-605）的基线；主键索引取最大键。
///
/// # Errors
///
/// 数据库查询失败时返回 [`WhError`]。
pub fn latest_seq(conn: &Connection) -> Result<i64, WhError> {
    conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
        row.get(0)
    })
    .map_err(query_err)
}

/// 查询某会话的消息页。
///
/// 全序键为 `(ts, id)`：时间戳可重复，`id` 保证稳定破平局（§7.2 禁止仅以时间戳分页）。
///
/// # Errors
///
/// `limit` 为 0、超过 [`MAX_PAGE_SIZE`]，或数据库查询失败时返回 [`WhError`]。
pub fn messages_page(
    conn: &Connection,
    session_id: &str,
    direction: Direction,
    cursor: Option<MessageCursor>,
    limit: usize,
) -> Result<MessagePage, WhError> {
    let fetch = fetch_limit(limit)?;
    let (order, comparison) = match direction {
        Direction::Forward => ("ASC", ">"),
        Direction::Backward => ("DESC", "<"),
    };

    let mut values = vec![Value::Text(session_id.to_string())];
    let cursor_clause = match cursor {
        Some(cursor) => {
            values.push(Value::BigInt(cursor.ts_us));
            values.push(Value::BigInt(cursor.ts_us));
            values.push(Value::Text(cursor.id));
            format!(
                " AND (ts {comparison} make_timestamp(?) \
                   OR (ts = make_timestamp(?) AND id {comparison} ?))"
            )
        }
        None => String::new(),
    };
    values.push(Value::BigInt(fetch));

    let sql = format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages WHERE session_id = ?{cursor_clause} \
         ORDER BY ts {order}, id {order} LIMIT ?"
    );
    let mut stmt = conn.prepare(&sql).map_err(query_err)?;
    let mapped = stmt
        .query_map(params_from_iter(values), |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                content_ref: row.get(3)?,
                token_count: row.get(4)?,
                ts_us: row.get(5)?,
                parent_id: row.get(6)?,
            })
        })
        .map_err(query_err)?;

    let mut items = Vec::with_capacity(limit);
    for row in mapped {
        items.push(row.map_err(query_err)?);
    }

    let has_more = items.len() > limit;
    if has_more {
        items.truncate(limit);
    }
    let next = has_more
        .then(|| {
            items.last().map(|row| MessageCursor {
                ts_us: row.ts_us,
                id: row.id.clone(),
            })
        })
        .flatten();

    Ok(MessagePage { items, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::apply_migrations;
    use duckdb::params;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_event(conn: &Connection, seq: i64, session: &str) {
        conn.execute(
            "INSERT INTO events
             (seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts,
              persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', '{}',
                     CAST('2026-01-01 00:00:00' AS TIMESTAMP), 'redacted')",
            params![seq, format!("E-{seq}"), session],
        )
        .expect("插入事件");
    }

    fn insert_message(conn: &Connection, id: &str, session: &str, ts: &str) {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, 'user', ?, NULL, CAST(? AS TIMESTAMP), NULL)",
            params![id, session, format!("ref-{id}"), ts],
        )
        .expect("插入消息");
    }

    #[test]
    fn forward_pages_cover_events_without_gaps_or_duplicates() {
        let conn = migrated();
        for seq in 1..=25 {
            insert_event(&conn, seq, "S-1");
        }
        for seq in 26..=30 {
            insert_event(&conn, seq, "S-2");
        }

        let mut collected = Vec::new();
        let mut cursor = None;
        let mut pages = 0;
        loop {
            let page = events_page(&conn, "S-1", Direction::Forward, cursor, 10).expect("翻页");
            assert!(page.items.len() <= 10, "单页不得超过 limit");
            collected.extend(page.items.iter().map(|row| row.seq));
            pages += 1;
            if let Some(next) = page.next {
                cursor = Some(next);
            } else {
                break;
            }
        }
        assert_eq!(pages, 3, "25 条按 10 条分页应为 3 页");
        assert_eq!(collected, (1..=25).collect::<Vec<_>>(), "不得重复或遗漏");

        // 同一游标重复请求必须返回完全相同的页（无 OFFSET 漂移）。
        let first = events_page(&conn, "S-1", Direction::Forward, None, 10).expect("首页");
        let again = events_page(&conn, "S-1", Direction::Forward, None, 10).expect("重复首页");
        assert_eq!(first, again);

        let empty = events_page(&conn, "S-unknown", Direction::Forward, None, 10).expect("空会话");
        assert!(empty.items.is_empty());
        assert_eq!(empty.next, None);
    }

    #[test]
    fn backward_pages_cover_events_without_gaps_or_duplicates() {
        let conn = migrated();
        for seq in 1..=25 {
            insert_event(&conn, seq, "S-1");
        }

        let mut collected = Vec::new();
        let mut cursor = None;
        loop {
            let page =
                events_page(&conn, "S-1", Direction::Backward, cursor, 10).expect("反向翻页");
            collected.extend(page.items.iter().map(|row| row.seq));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected,
            (1..=25).rev().collect::<Vec<_>>(),
            "Backward 必须最新在前且无重复无遗漏"
        );

        let first = events_page(&conn, "S-1", Direction::Backward, None, 10).expect("最新页");
        assert_eq!(first.items.first().map(|row| row.seq), Some(25));
        assert_eq!(first.next, Some(EventCursor { seq: 16 }));
    }

    #[test]
    fn cursors_are_exclusive_in_both_directions() {
        let conn = migrated();
        for seq in 1..=20 {
            insert_event(&conn, seq, "S-1");
        }

        let forward = events_page(
            &conn,
            "S-1",
            Direction::Forward,
            Some(EventCursor { seq: 10 }),
            5,
        )
        .expect("正向游标");
        assert_eq!(
            forward.items.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![11, 12, 13, 14, 15],
            "正向游标为排他下界"
        );

        let backward = events_page(
            &conn,
            "S-1",
            Direction::Backward,
            Some(EventCursor { seq: 11 }),
            5,
        )
        .expect("反向游标");
        assert_eq!(
            backward.items.iter().map(|row| row.seq).collect::<Vec<_>>(),
            vec![10, 9, 8, 7, 6],
            "反向游标为排他上界"
        );
    }

    #[test]
    fn message_pages_use_timestamp_and_id_tiebreak() {
        let conn = migrated();
        // 同一毫秒内的四条消息：仅靠时间戳无法形成稳定全序。
        for id in ["m0", "m1", "m2", "m3"] {
            insert_message(&conn, id, "S-1", "2026-01-01 00:00:00.000001");
        }
        for id in ["n0", "n1", "n2"] {
            insert_message(&conn, id, "S-1", "2026-01-01 00:00:01.000000");
        }

        let mut forward = Vec::new();
        let mut cursor = None;
        loop {
            let page = messages_page(&conn, "S-1", Direction::Forward, cursor, 3).expect("正向");
            forward.extend(page.items.iter().map(|row| row.id.clone()));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(
            forward,
            vec!["m0", "m1", "m2", "m3", "n0", "n1", "n2"],
            "同一时间戳必须按 id 破平局，跨页不得重复或遗漏"
        );

        let mut backward = Vec::new();
        let mut cursor = None;
        loop {
            let page = messages_page(&conn, "S-1", Direction::Backward, cursor, 3).expect("反向");
            backward.extend(page.items.iter().map(|row| row.id.clone()));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(
            backward,
            vec!["n2", "n1", "n0", "m3", "m2", "m1", "m0"],
            "反向顺序必须与正向严格互为逆序"
        );

        let first = messages_page(&conn, "S-1", Direction::Forward, None, 3).expect("首页");
        let again = messages_page(&conn, "S-1", Direction::Forward, None, 3).expect("重复首页");
        assert_eq!(first, again, "同页请求两次结果必须一致");
    }

    #[test]
    fn page_size_is_bounded_and_validated() {
        let conn = migrated();
        insert_event(&conn, 1, "S-1");

        let err = events_page(&conn, "S-1", Direction::Forward, None, 0).expect_err("limit 0");
        assert_eq!(err.code, ErrorCode::InvalidState);

        let err = events_page(&conn, "S-1", Direction::Forward, None, MAX_PAGE_SIZE + 1)
            .expect_err("超上限");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("不得超过"), "错误须说明上限");

        let page = events_page(&conn, "S-1", Direction::Forward, None, MAX_PAGE_SIZE)
            .expect("上限本身可用");
        assert_eq!(page.items.len(), 1);

        let err = messages_page(&conn, "S-1", Direction::Forward, None, 0).expect_err("limit 0");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
