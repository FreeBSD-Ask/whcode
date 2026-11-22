//! 会话内容关键词检索（P3.3／ADR-076 第一步）。
//!
//! 语义：**大小写不敏感的子串匹配**，范围＝`messages.content_ref`（当前唯一持久化的自由
//! 文本；事件正文按追踪级别可能被脱敏，不作为检索面）。不做分词、不做正则、不引入外部
//! 扩展——DuckDB 的 `fts` 扩展不在预编译 DLL 内（需联网安装），与“本地优先/离线可用”
//! 冲突，故 FTS 显式延后（ADR-076 §6 的失效条件）。
//!
//! 有界性（NFR-215／§7.1）：
//! - 搜索词去空白后 `1..=MAX_SEARCH_NEEDLE_CHARS` 字符；
//! - 返回条数 `1..=MAX_SEARCH_LIMIT`，按消息时间倒序（最新命中优先）；
//! - 每条命中只返回**定长片段**（[`SNIPPET_CHARS`] 字符，含省略号标记），不把整段正文
//!   带进内存与界面。
//!
//! 边界：只读 `messages`／`sessions`；权限与展示归一归 `whcode-app`。

use duckdb::Connection;
use whcode_domain::{ErrorCode, WhError};

/// 搜索词最大字符数（去空白后）。
pub const MAX_SEARCH_NEEDLE_CHARS: usize = 128;

/// 单次检索返回的最大命中条数。
pub const MAX_SEARCH_LIMIT: usize = 100;

/// 命中片段的字符预算（不含省略号）。
pub const SNIPPET_CHARS: usize = 80;

/// 一条内容命中（会话 + 消息 + 定长片段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentHit {
    /// 命中所属会话。
    pub session_id: String,
    /// 会话标题（结果列表直接展示，避免二次查询）。
    pub session_title: String,
    /// 消息 ID。
    pub message_id: String,
    /// 消息角色（`user`／`assistant` 等）。
    pub role: String,
    /// 命中片段（脱去换行与控制字符，定长）。
    pub snippet: String,
}

fn search_err(err: duckdb::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("会话内容检索失败：{err}"))
}

/// 逐字符小写（只取每个字符小写映射的首字符）。
///
/// 不用 `str::to_lowercase()` 直接做字节定位：某些字符小写后**字节长度会变**（如 `İ`），
/// 字节下标无法安全映射回原串；按字符对齐后，片段窗口直接落在原串字符上。
fn lowered_chars(text: &str) -> Vec<char> {
    text.chars()
        .map(|ch| ch.to_lowercase().next().unwrap_or(ch))
        .collect()
}

/// 在 `haystack` 中查找 `needle`（两者都已按 [`lowered_chars`] 归一）的首个字符位置。
fn find_chars(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// 生成命中片段：以命中位置为中心取 [`SNIPPET_CHARS`] 字符窗口，两端截断处加省略号；
/// 换行与其他控制字符折叠成空格（片段要能单行展示）。
#[must_use]
pub fn snippet_around(content: &str, needle: &str) -> String {
    let original: Vec<char> = content.chars().collect();
    let position = find_chars(&lowered_chars(content), &lowered_chars(needle));
    let (start, end) = match position {
        Some(index) => {
            let half = SNIPPET_CHARS / 2;
            let start = index.saturating_sub(half);
            let end = (start + SNIPPET_CHARS).min(original.len());
            (start, end)
        }
        None => (0, SNIPPET_CHARS.min(original.len())),
    };
    let mut text: String = original[start..end]
        .iter()
        .map(|ch| if ch.is_control() { ' ' } else { *ch })
        .collect();
    if start > 0 {
        text.insert(0, '…');
    }
    if end < original.len() {
        text.push('…');
    }
    text
}

/// 按关键词检索会话内容（大小写不敏感子串），最新命中优先。
///
/// # Errors
///
/// 搜索词为空/超长、`limit` 为 0 或数据库查询失败时返回 [`WhError`]。
/// fts 扩展候选路径（ADR-099）：`vendor/duckdb/<ver>/<target>/fts.duckdb_extension`。
///
/// 一次构建只对应一个平台，故取首个存在者；`/vendor` 不入库，由 `fetch-duckdb.py` 获取。
fn fts_extension_candidates() -> Vec<std::path::PathBuf> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let vendor = root.join("vendor/duckdb");
    let mut out = Vec::new();
    let Ok(versions) = std::fs::read_dir(&vendor) else {
        return out;
    };
    for version in versions.flatten() {
        let Ok(targets) = std::fs::read_dir(version.path()) else {
            continue;
        };
        for target in targets.flatten() {
            let candidate = target.path().join("fts.duckdb_extension");
            if candidate.is_file() {
                out.push(candidate);
            }
        }
    }
    out
}

/// fts 扩展是否可用（ADR-099）：按 vendor 路径 → 已安装的 `fts` 顺序尝试 `LOAD`。
///
/// 失败不报错——调用方据此回退到 [`search_messages`] 的 `contains` 路径。
#[must_use]
pub fn fts_available(conn: &Connection) -> bool {
    for path in fts_extension_candidates() {
        let escaped = path.display().to_string().replace('\'', "''");
        if conn.execute_batch(&format!("LOAD '{escaped}'")).is_ok() {
            return true;
        }
    }
    conn.execute_batch("LOAD fts").is_ok()
}

/// 索引时效：`(max(ts) 微秒, 行数)`。
fn messages_stamp(conn: &Connection) -> Result<(i64, i64), WhError> {
    conn.query_row(
        "SELECT COALESCE(epoch_us(max(ts)), 0), count(*) FROM messages",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(search_err)
}

/// 确保 fts 索引覆盖当前消息（ADR-099）：无新消息时零成本跳过，否则表级重建。
///
/// `create_fts_index` 是表级操作（DuckDB 无会话级增量接口），重建成本随消息量线性。
fn ensure_fts_index(conn: &Connection) -> Result<(), WhError> {
    let (max_ts_us, rows) = messages_stamp(conn)?;
    let fresh: Option<(i64, i64)> = conn
        .query_row(
            "SELECT indexed_max_ts_us, indexed_rows FROM fts_index_meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();
    if fresh == Some((max_ts_us, rows)) {
        return Ok(());
    }
    conn.execute_batch("PRAGMA create_fts_index('messages', 'id', 'content_ref', overwrite=1)")
        .map_err(search_err)?;
    conn.execute("DELETE FROM fts_index_meta WHERE id = 1", [])
        .map_err(search_err)?;
    conn.execute(
        "INSERT INTO fts_index_meta (id, indexed_max_ts_us, indexed_rows, updated_at)
         VALUES (1, ?, ?, current_timestamp)",
        duckdb::params![max_ts_us, rows],
    )
    .map_err(search_err)?;
    Ok(())
}

/// 相关度排序检索（ADR-099）：fts 可用且索引就绪时按 `match_bm25` 排序，
/// 否则回退 [`search_messages`]。返回 `(命中, 是否按相关度排序)`——
/// `false` 表示走了子串回退路径，调用方须如实展示，不得冒充排序结果。
///
/// # Errors
///
/// 搜索词为空/超长、limit 为 0 或存储失败时返回 [`WhError`]。
pub fn search_messages_ranked(
    conn: &Connection,
    needle: &str,
    limit: usize,
) -> Result<(Vec<ContentHit>, bool), WhError> {
    let needle_trimmed = needle.trim();
    if needle_trimmed.is_empty() || needle_trimmed.chars().count() > MAX_SEARCH_NEEDLE_CHARS {
        // 复用 search_messages 的校验与错误文案（单一出处）。
        let _ = search_messages(conn, needle, limit)?;
    }
    if limit == 0 {
        let _ = search_messages(conn, needle, limit)?;
    }
    if !fts_available(conn) || ensure_fts_index(conn).is_err() {
        return Ok((search_messages(conn, needle, limit)?, false));
    }
    let limit = limit.min(MAX_SEARCH_LIMIT);
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, title, role, content FROM (
                 SELECT m.id AS id, m.session_id AS session_id, s.title AS title,
                        m.role AS role, m.content_ref AS content,
                        fts_main_messages.match_bm25(m.id, ?) AS score
                 FROM messages m JOIN sessions s ON s.id = m.session_id
             ) WHERE score IS NOT NULL
             ORDER BY score DESC, id DESC
             LIMIT ?",
        )
        .map_err(search_err)?;
    let mut rows = stmt
        .query([&needle_trimmed as &dyn duckdb::ToSql, &(limit as i64)])
        .map_err(search_err)?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next().map_err(search_err)? {
        let content: String = row.get(4).map_err(search_err)?;
        hits.push(ContentHit {
            message_id: row.get(0).map_err(search_err)?,
            session_id: row.get(1).map_err(search_err)?,
            session_title: row.get(2).map_err(search_err)?,
            role: row.get(3).map_err(search_err)?,
            snippet: snippet_around(&content, needle_trimmed),
        });
    }
    Ok((hits, true))
}

pub fn search_messages(
    conn: &Connection,
    needle: &str,
    limit: usize,
) -> Result<Vec<ContentHit>, WhError> {
    let needle = needle.trim();
    let needle_chars = needle.chars().count();
    if needle_chars == 0 {
        return Err(WhError::invalid_state("搜索词不能为空"));
    }
    if needle_chars > MAX_SEARCH_NEEDLE_CHARS {
        return Err(WhError::invalid_state(format!(
            "搜索词超过 {MAX_SEARCH_NEEDLE_CHARS} 字符"
        )));
    }
    if limit == 0 {
        return Err(WhError::invalid_state("检索 limit 必须大于 0"));
    }
    let limit = limit.min(MAX_SEARCH_LIMIT);

    // `contains` 是字面量子串匹配：搜索词里的 `%`／`_` 不会被当作通配符（LIKE 会）。
    let mut stmt = conn
        .prepare(
            "SELECT m.id, m.session_id, s.title, m.role, m.content_ref
             FROM messages m JOIN sessions s ON s.id = m.session_id
             WHERE contains(lower(m.content_ref), lower(?))
             ORDER BY m.ts DESC, m.id DESC
             LIMIT ?",
        )
        .map_err(search_err)?;
    let mut rows = stmt
        .query([&needle as &dyn duckdb::ToSql, &(limit as i64)])
        .map_err(search_err)?;
    let mut hits = Vec::new();
    while let Some(row) = rows.next().map_err(search_err)? {
        let content: String = row.get(4).map_err(search_err)?;
        hits.push(ContentHit {
            message_id: row.get(0).map_err(search_err)?,
            session_id: row.get(1).map_err(search_err)?,
            session_title: row.get(2).map_err(search_err)?,
            role: row.get(3).map_err(search_err)?,
            snippet: snippet_around(&content, needle),
        });
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::create_session;
    use crate::schema::apply_migrations;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_message(
        conn: &Connection,
        id: &str,
        session_id: &str,
        role: &str,
        content: &str,
        ts: u64,
    ) {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, ?, ?, NULL, to_timestamp(?), NULL)",
            duckdb::params![id, session_id, role, content, ts as f64 / 1000.0],
        )
        .expect("插入消息");
    }

    #[test]
    fn search_is_case_insensitive_and_reports_session_context() {
        let conn = fixture();
        let session = create_session(&conn, "DuckDB 调优", None, "active", 1_000).expect("建会话");
        insert_message(
            &conn,
            "M-1",
            &session.id,
            "user",
            "请检查 DuckDB 的 WAL 行为",
            1_001,
        );
        insert_message(
            &conn,
            "M-2",
            &session.id,
            "assistant",
            "wal 需要显式 checkpoint",
            1_002,
        );

        let hits = search_messages(&conn, "duckdb", 10).expect("检索");
        assert_eq!(hits.len(), 1, "只应命中消息正文一次：{hits:?}");
        assert_eq!(hits[0].session_id, session.id);
        assert_eq!(hits[0].session_title, "DuckDB 调优");
        assert_eq!(hits[0].role, "user");

        let wal = search_messages(&conn, "WAL", 10).expect("检索");
        assert_eq!(wal.len(), 2, "大小写不敏感应命中两条：{wal:?}");
    }

    #[test]
    fn snippet_is_bounded_and_single_line() {
        let long = format!("前缀\n{}命中词{}", "甲".repeat(200), "乙".repeat(200));
        let snippet = snippet_around(&long, "命中词");
        assert!(!snippet.contains('\n'), "片段必须单行：{snippet:?}");
        assert!(
            snippet.chars().count() <= SNIPPET_CHARS + 2,
            "片段长度必须有界：{}",
            snippet.chars().count()
        );
        assert!(
            snippet.contains("命中词"),
            "片段必须包含命中词：{snippet:?}"
        );
        assert!(
            snippet.starts_with('…') && snippet.ends_with('…'),
            "两端截断须显式：{snippet:?}"
        );
    }

    #[test]
    fn search_respects_limit_and_orders_newest_first() {
        let conn = fixture();
        let session = create_session(&conn, "排序", None, "active", 1_000).expect("建会话");
        for index in 0..5 {
            insert_message(
                &conn,
                &format!("M-{index}"),
                &session.id,
                "user",
                "同一关键词",
                1_100 + index,
            );
        }
        let hits = search_messages(&conn, "同一关键词", 3).expect("检索");
        assert_eq!(hits.len(), 3, "limit 必须生效");
        assert_eq!(hits[0].message_id, "M-4", "最新命中优先：{hits:?}");
        assert_eq!(hits[2].message_id, "M-2");
    }

    #[test]
    fn percent_and_underscore_are_literal() {
        let conn = fixture();
        let session = create_session(&conn, "字面量", None, "active", 1_000).expect("建会话");
        insert_message(&conn, "M-1", &session.id, "user", "覆盖率 90% 以上", 1_001);
        insert_message(&conn, "M-2", &session.id, "user", "普通文本 90 条", 1_002);

        let percent = search_messages(&conn, "90%", 10).expect("检索");
        assert_eq!(percent.len(), 1, "% 应按字面量匹配：{percent:?}");
        let underscore = search_messages(&conn, "9_0", 10).expect("检索");
        assert!(underscore.is_empty(), "_ 不得当通配符：{underscore:?}");
    }

    #[test]
    fn validation_rejects_empty_and_overlong_and_zero_limit() {
        let conn = fixture();
        assert!(
            search_messages(&conn, "  ", 10).is_err(),
            "空搜索词必须拒绝"
        );
        let long = "x".repeat(MAX_SEARCH_NEEDLE_CHARS + 1);
        assert!(
            search_messages(&conn, &long, 10).is_err(),
            "超长搜索词必须拒绝"
        );
        assert!(search_messages(&conn, "x", 0).is_err(), "limit=0 必须拒绝");
    }

    #[test]
    fn no_match_returns_empty_without_error() {
        let conn = fixture();
        let session = create_session(&conn, "空结果", None, "active", 1_000).expect("建会话");
        insert_message(&conn, "M-1", &session.id, "user", "无关内容", 1_001);
        assert!(
            search_messages(&conn, "不存在的词", 10)
                .expect("检索")
                .is_empty()
        );
    }
    /// ADR-099：fts 可用时按相关度排序；不可用时回退子串路径且 ranked=false。
    #[test]
    fn ranked_search_prefers_relevance_and_falls_back() {
        let conn = fixture();
        let session = create_session(&conn, "检索", None, "active", 1_000).expect("会话");
        insert_message(&conn, "M-1", &session.id, "user", "alpha beta gamma", 1_001);
        insert_message(&conn, "M-2", &session.id, "user", "alpha", 1_002);
        insert_message(&conn, "M-3", &session.id, "user", "无关内容", 1_003);

        let (hits, ranked) = search_messages_ranked(&conn, "alpha", 10).expect("检索");
        assert!(!hits.is_empty(), "应命中含 alpha 的消息");
        assert!(
            hits.iter().all(|h| h.message_id != "M-3"),
            "无关消息不得命中：{hits:?}"
        );
        // 本机 vendor 存在扩展时 ranked 为 true；缺失时为 false（两条路径都必须正确）。
        if ranked {
            // 相关度：M-2（仅 alpha，短文档）通常高于 M-1（含更多词）。
            assert_eq!(hits[0].message_id, "M-2", "相关度排序应把短文档排前");
        } else {
            // 回退路径按时间倒序：M-2 比 M-1 新。
            assert_eq!(hits[0].message_id, "M-2");
        }
    }

    /// ADR-099：无扩展时 `fts_available` 为 false 且回退不报错（本机若无 vendor 扩展则走此分支）。
    #[test]
    fn fts_fallback_is_not_an_error() {
        let conn = fixture();
        let session = create_session(&conn, "回退", None, "active", 1_000).expect("会话");
        insert_message(&conn, "M-9", &session.id, "user", "fallback needle", 1_001);
        let (hits, _ranked) = search_messages_ranked(&conn, "needle", 5).expect("不得报错");
        assert_eq!(hits.len(), 1);
    }
    /// ADR-099：确认 vendor 扩展被加载（本机存在时应为 true），并验证索引可创建。
    #[test]
    fn fts_extension_is_loaded_from_vendor() {
        let conn = fixture();
        let available = fts_available(&conn);
        println!("fts_available = {available}");
        if available {
            ensure_fts_index(&conn).expect("索引应可创建");
            let (max_ts, rows) = messages_stamp(&conn).expect("戳记");
            let meta: (i64, i64) = conn
                .query_row(
                    "SELECT indexed_max_ts_us, indexed_rows FROM fts_index_meta WHERE id = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("meta");
            assert_eq!(meta, (max_ts, rows), "索引戳记须与当前消息一致");
        }
    }
}
