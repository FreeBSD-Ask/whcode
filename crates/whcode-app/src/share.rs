//! # 会话分享脱敏导出（T261／FR-752；proposal-v3 P3.6）
//!
//! 把一个会话导出为**本地文件**，供离线分享：会话元数据＋任务清单＋事件流。
//! 纪律：
//!
//! - **只脱敏、不提供 full 选项**：事件载荷一律经 `whcode_llm::trace::sanitize`
//!   的 `redacted` 级处理（字段名与值模式命中即换 `[REDACTED]`），并随包注明
//!   `trace_level = "redacted"` 与 `redacted = true`；
//! - **无上传、无联网**：只读库＋`std::fs` 原子写盘，不调用任何网络栈；
//! - **自校验和**：`sha256` 覆盖 `body` 的规范 JSON（键按字典序、紧凑编码）；
//!   校验时对 `body` 重新规范化后求值，与文件排版无关；
//! - **有界**：事件 ≤ [`MAX_SHARE_EVENTS`]、任务 ≤ [`MAX_SESSION_TASKS`]、
//!   文件 ≤ [`MAX_SHARE_BYTES`]；超限显式 `truncated`（事件）或拒绝写盘（字节）；
//! - **不覆盖**：目标已存在即拒绝；原子写盘沿用证书落盘口径（同目录临时文件＋
//!   `sync_all`＋`rename`）。
//!
//! 已知限制（威胁模型见 ADR-057）：标题、任务标题与验收条件、事件正文属自由文本，
//! 脱敏只识别字段名与已知令牌模式；自由文本中的秘密需分享者自行复核。

use std::path::Path;

use duckdb::Connection;
use serde::Serialize;
use serde_json::{Value, json};
use whcode_domain::{ErrorCode, TraceLevel, WhError};
use whcode_store::query::EventRow;

use crate::authz::{Actor, Capability, Policy, require};
use crate::events::{EXPORT_PAGE_LIMIT, events_export};

/// 分享包 schema 版本（结构破坏性变更时递增）。
pub const SHARE_SCHEMA_VERSION: u32 = 1;
/// 单次分享的事件上限（超出显式 `truncated`，计数不伪造）。
pub const MAX_SHARE_EVENTS: usize = 2_000;
/// 分享包字节上限（规范 JSON 主体；超限拒绝写盘，不静默截断）。
pub const MAX_SHARE_BYTES: usize = 16 * 1024 * 1024;

/// 导出结果（写盘后的可核验摘要）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShareExport {
    /// 落盘路径（原样回显）。
    pub path: String,
    /// 规范 JSON 主体的 SHA-256（小写十六进制）。
    pub sha256: String,
    /// 文件字节数（含外层包装与排版）。
    pub bytes: usize,
    /// 导出事件数（≤ [`MAX_SHARE_EVENTS`]）。
    pub events: usize,
    /// 导出任务数（≤ [`whcode_store::tasks::MAX_SESSION_TASKS`]）。
    pub tasks: usize,
    /// 事件是否因上限被截断（显式，不静默）。
    pub truncated: bool,
    /// 是否已脱敏（固定为真：本入口不提供 full）。
    pub redacted: bool,
    /// 追踪级别（固定 `redacted`）。
    pub trace_level: String,
}

/// 导出会话分享包到本地文件。
///
/// # Errors
///
/// 缺 [`Capability::SessionRead`]、会话不存在、目标已存在/路径非法、超过字节上限
/// 或读写失败时返回 [`WhError`]。
pub fn export_session(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
    out: &Path,
    now_ms: u64,
) -> Result<ShareExport, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    if out.as_os_str().is_empty() {
        return Err(WhError::invalid_state("分享导出必须给出目标文件路径"));
    }
    let session = whcode_store::get_session(conn, session_id)?.ok_or_else(|| {
        WhError::new(
            ErrorCode::NotFound,
            format!("会话不存在：{session_id}；用 session list 查看现存会话，或核对会话 ID 后重试"),
        )
    })?;
    if out.exists() {
        return Err(WhError::invalid_state(format!(
            "目标文件已存在：{}（不覆盖；换一个路径或先移除）",
            out.display()
        )));
    }

    let (events, truncated) = collect_events(conn, policy, actor, session_id)?;
    let tasks = collect_tasks(conn, session_id)?;
    let body = json!({
        "kind": "whcode-session-share",
        "share_schema_version": SHARE_SCHEMA_VERSION,
        "schema_version": whcode_domain::SCHEMA_VERSION,
        "created_at_ms": now_ms,
        "trace_level": TraceLevel::Redacted.as_str(),
        "redacted": true,
        "session": {
            "id": session.id,
            "title": session.title,
            "created_at_ms": session.created_at_ms,
            "last_active_at_ms": session.last_active_at_ms,
            "model": session.model,
            "archived": session.archived,
            "status": session.status,
            "stage": session.stage,
        },
        "tasks": tasks,
        "events": events,
        "counts": {
            "tasks": tasks.len(),
            "events": events.len(),
            "truncated": truncated,
        },
    });
    let canonical = canonicalize(&body);
    let body_bytes = serde_json::to_vec(&canonical)
        .map_err(|err| WhError::new(ErrorCode::Internal, format!("分享包序列化失败：{err}")))?;
    if body_bytes.len() > MAX_SHARE_BYTES {
        return Err(WhError::invalid_state(format!(
            "分享包主体 {} 字节超过上限 {MAX_SHARE_BYTES}：请减少事件或改用其他方式导出",
            body_bytes.len()
        )));
    }
    let digest = crate::cert::sha256_hex(&body_bytes);
    let document = json!({ "sha256": digest, "body": canonical });
    let file_bytes = serde_json::to_vec_pretty(&document)
        .map_err(|err| WhError::new(ErrorCode::Internal, format!("分享包序列化失败：{err}")))?;
    crate::cert::atomic_write(out, &file_bytes)?;
    Ok(ShareExport {
        path: out.display().to_string(),
        sha256: digest,
        bytes: file_bytes.len(),
        events: body["counts"]["events"].as_u64().unwrap_or(0) as usize,
        tasks: body["counts"]["tasks"].as_u64().unwrap_or(0) as usize,
        truncated,
        redacted: true,
        trace_level: TraceLevel::Redacted.as_str().to_string(),
    })
}

/// 分页读取事件并按 `redacted` 级脱敏；返回（事件值，是否截断）。
fn collect_events(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<(Vec<Value>, bool), WhError> {
    let mut after_seq = 0_i64;
    let mut events: Vec<Value> = Vec::with_capacity(MAX_SHARE_EVENTS.min(EXPORT_PAGE_LIMIT));
    loop {
        let remaining = MAX_SHARE_EVENTS.saturating_sub(events.len());
        if remaining == 0 {
            break;
        }
        let want = remaining.min(EXPORT_PAGE_LIMIT);
        let page = events_export(
            conn,
            policy,
            actor,
            session_id,
            after_seq,
            want,
            TraceLevel::Redacted,
        )?;
        let received = page.events.len();
        if received == 0 {
            return Ok((events, false));
        }
        for event in page.events {
            after_seq = after_seq.max(event.seq);
            events.push(event_value(event)?);
        }
        if received < want {
            return Ok((events, false));
        }
    }
    // 达到上限后再探测一条：有后续事件才置 truncated（不把“恰好 2000 条”误报为截断）。
    let probe = events_export(
        conn,
        policy,
        actor,
        session_id,
        after_seq,
        1,
        TraceLevel::Redacted,
    )?;
    Ok((events, !probe.events.is_empty()))
}

fn event_value(event: EventRow) -> Result<Value, WhError> {
    let payload: Value = serde_json::from_str(&event.payload_json)
        .map_err(|err| WhError::parse_error(format!("事件 {} 载荷损坏：{err}", event.seq)))?;
    Ok(json!({
        "seq": event.seq,
        "kind": event.kind,
        "ts_us": event.ts_us,
        "persistence_level": event.persistence_level,
        "payload": payload,
    }))
}

/// 任务清单（元数据＋验收条件；解析失败用 `acceptance_valid = false` 显式区分）。
fn collect_tasks(conn: &Connection, session_id: &str) -> Result<Vec<Value>, WhError> {
    let rows = whcode_store::tasks::tasks_of_session(
        conn,
        session_id,
        whcode_store::tasks::MAX_SESSION_TASKS,
    )?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let parsed = serde_json::from_str::<Value>(&row.acceptance_json);
            let (acceptance, acceptance_valid) = match parsed {
                Ok(value) => (value, true),
                Err(_) => (Value::Null, false),
            };
            json!({
                "id": row.id,
                "title": row.title,
                "stage": row.stage,
                "status": row.status.as_str(),
                "parent_id": row.parent_id,
                "retry_count": row.retry_count,
                "updated_at_ms": row.updated_at_ms,
                "acceptance": acceptance,
                "acceptance_valid": acceptance_valid,
            })
        })
        .collect())
}

/// 规范 JSON：对象键按字典序递归排序（不依赖 `serde_json` 的 `preserve_order` 特性）。
fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = serde_json::Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonicalize(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::params;
    use std::path::PathBuf;
    use whcode_store::{apply_migrations, create_session_with_id};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("whcode-share-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("建临时目录");
        path
    }

    fn insert_event(conn: &Connection, seq: i64, session_id: &str, payload: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}"), session_id, payload],
        )
        .expect("插入事件");
    }

    #[test]
    fn share_bundle_redacts_secrets_and_self_verifies() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "分享会话", "active", 1_000).expect("创建");
        insert_event(
            &conn,
            1,
            "S-1",
            r#"{"api_key":"sk-secret1","prompt":"写一个排序算法"}"#,
        );
        insert_event(
            &conn,
            2,
            "S-1",
            r#"{"note":"Bearer eyJhbGciOi","answer":"用归并排序"}"#,
        );
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let dir = temp_dir("redact");
        let out = dir.join("share.json");

        let result = export_session(&conn, &policy, &actor, "S-1", &out, 2_000).expect("导出");
        assert!(result.redacted);
        assert_eq!(result.trace_level, "redacted");
        assert_eq!(result.events, 2);
        assert_eq!(result.tasks, 0);
        assert!(!result.truncated);

        let text = std::fs::read_to_string(&out).expect("读文件");
        assert!(!text.contains("sk-secret1"), "凭据不得出现在分享包");
        assert!(!text.contains("eyJhbGciOi"), "令牌不得出现在分享包");
        assert!(text.contains("写一个排序算法"), "普通正文保留");
        assert!(text.contains("用归并排序"));

        // 校验和覆盖规范 JSON 主体；与文件排版无关。
        let document: Value = serde_json::from_str(&text).expect("解析");
        let canonical = canonicalize(&document["body"]);
        let bytes = serde_json::to_vec(&canonical).expect("序列化");
        assert_eq!(
            document["sha256"].as_str().expect("sha256"),
            crate::cert::sha256_hex(&bytes)
        );
        assert_eq!(document["body"]["counts"]["truncated"], false);

        // 不覆盖：第二次导出到同一路径必须拒绝。
        let err =
            export_session(&conn, &policy, &actor, "S-1", &out, 3_000).expect_err("已存在必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn share_bundle_distinguishes_missing_session_and_empty_session() {
        let conn = migrated();
        create_session_with_id(&conn, "S-empty", "空会话", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let dir = temp_dir("empty");

        let empty = export_session(
            &conn,
            &policy,
            &actor,
            "S-empty",
            &dir.join("empty.json"),
            2_000,
        )
        .expect("存在但零事件是正确的空");
        assert_eq!(empty.events, 0);
        assert_eq!(empty.tasks, 0);

        let err = export_session(
            &conn,
            &policy,
            &actor,
            "S-missing",
            &dir.join("missing.json"),
            2_000,
        )
        .expect_err("不存在的会话必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(!dir.join("missing.json").exists(), "失败不得留下半成品");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn share_bundle_caps_events_and_marks_truncation() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "大量事件", "active", 1_000).expect("创建");
        for seq in 1..=(MAX_SHARE_EVENTS as i64 + 1) {
            insert_event(&conn, seq, "S-1", r#"{"n":1}"#);
        }
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let dir = temp_dir("cap");
        let result = export_session(&conn, &policy, &actor, "S-1", &dir.join("cap.json"), 2_000)
            .expect("导出");
        assert_eq!(result.events, MAX_SHARE_EVENTS);
        assert!(result.truncated, "超出上限必须显式标记");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn share_requires_session_read_capability() {
        let conn = migrated();
        create_session_with_id(&conn, "S-1", "权限", "active", 1_000).expect("创建");
        let policy = DefaultPolicy;
        let actor = crate::authz::Actor::new("no-read", Vec::new());
        let dir = temp_dir("capability");
        let err = export_session(
            &conn,
            &policy,
            &actor,
            "S-1",
            &dir.join("denied.json"),
            2_000,
        )
        .expect_err("缺能力必须拒绝");
        assert_eq!(err.code, ErrorCode::PermissionDenied);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
