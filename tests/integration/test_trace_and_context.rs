//! 追踪与上下文集成测试（T067、V-103/V-104 关联）：脱敏、链路串联、预算截断与重连补拉。
//!
//! 覆盖四条路径：
//! 1. **脱敏**（FR-203/208、V-110）：`redacted` 级别下疑似秘密字段与秘密值模式不出现在
//!    导出与快照中，且完整性标记可查询；
//! 2. **链路串联**（FR-204、Check 59）：事件按 `request_id/session_id/task_id/
//!    tool_call_id` 可完整串联，断链被检出；
//! 3. **预算截断**（§4.1、FR-105）：上下文加载返回契约四元组，截断原因可查；
//! 4. **重连补拉**（FR-202/605、V-103）：断线前快照的关键事件集合与补拉结果一致。
//!
//! V-104（进程终止重启）由 `durability` 测试覆盖（T043），不在此重复。

use std::collections::BTreeSet;

use duckdb::{Connection, params};
use whcode_app::{
    DefaultPolicy, REPLAY_PAGE_LIMIT, actor_with_all_capabilities, compress_context, events_export,
    load_context, load_summary_pointer, replay_since,
};
use whcode_domain::{EventKind, EventRecord, PersistenceLevel, SCHEMA_VERSION, TraceLevel};
use whcode_llm::{Correlation, chain_breaks, sanitize};
use whcode_store::{apply_migrations, create_session_with_id};

fn migrated() -> Connection {
    let conn = Connection::open_in_memory().expect("内存数据库");
    apply_migrations(&conn).expect("迁移");
    conn
}

fn seed_llm_events(conn: &Connection, session_id: &str) -> Vec<i64> {
    let correlation = Correlation::new("R-889")
        .with_session(session_id)
        .with_task("T-1");
    let mut seqs = Vec::new();
    let mut seq = 0i64;
    let mut push = |conn: &Connection, seq: &mut i64, record: &mut EventRecord| {
        *seq += 1;
        record.seq = *seq as u64;
        record.id = format!("E-{seq}");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, ?, ?, ?, ?, CAST(? AS JSON)::VARIANT, current_timestamp, ?)",
            params![
                record.seq,
                record.id,
                record.session_id,
                record.task_id,
                record.request_id,
                record.tool_call_id,
                record.kind.as_str(),
                serde_json::to_string(&record.payload).expect("载荷"),
                record.persistence_level.as_str(),
            ],
        )
        .expect("插入事件");
        seqs.push(*seq);
    };

    let mut record = EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: 0,
        id: String::new(),
        kind: EventKind::RequestStarted,
        session_id: None,
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: serde_json::json!({ "model": "stub", "api_key": "sk-leaky" }),
        ts_ms: 1_000,
        persistence_level: PersistenceLevel::Redacted,
    };
    correlation.apply(&mut record);
    push(conn, &mut seq, &mut record);

    let mut chunk = EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: 0,
        id: String::new(),
        kind: EventKind::StreamingChunk,
        session_id: None,
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: serde_json::json!({ "text": "增量正文" }),
        ts_ms: 1_100,
        persistence_level: PersistenceLevel::Metadata,
    };
    correlation.apply(&mut chunk);
    push(conn, &mut seq, &mut chunk);

    let mut completed = EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: 0,
        id: String::new(),
        kind: EventKind::RequestCompleted,
        session_id: None,
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: serde_json::json!({ "prompt_tokens": 21, "completion_tokens": 5,
                                    "latency_ms": 120, "cost_estimate": 0.001,
                                    "cost_is_estimate": true }),
        ts_ms: 1_200,
        persistence_level: PersistenceLevel::Redacted,
    };
    correlation.apply(&mut completed);
    push(conn, &mut seq, &mut completed);

    seqs
}

fn critical_kinds() -> BTreeSet<&'static str> {
    BTreeSet::from(["RequestStarted", "RequestCompleted", "RequestFailed"])
}

#[test]
fn trace_and_context_passes_four_paths() {
    let conn = migrated();
    create_session_with_id(&conn, "S-1", "追踪与上下文", "active", 1_000).expect("创建");
    seed_llm_events(&conn, "S-1");
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("tester");

    // ---- 1. 脱敏（V-110）：redacted 导出下秘密不得出现，完整性标记可查询。
    let export =
        events_export(&conn, &policy, &actor, "S-1", 0, 100, TraceLevel::Redacted).expect("导出");
    let serialized = serde_json::to_string(&export).expect("序列化");
    assert!(
        !serialized.contains("sk-leaky"),
        "redacted 下秘密不得出现在导出：{serialized}"
    );
    assert!(export.integrity.redacted, "脱敏标记必须可查询（FR-208）");
    assert!(export.redacted);
    let redacted = sanitize(
        &serde_json::json!({"api_key": "sk-leaky"}),
        TraceLevel::Redacted,
    );
    assert_eq!(
        redacted.payload.as_ref().expect("载荷")["api_key"],
        "[REDACTED]"
    );
    assert!(redacted.integrity.redacted);

    // ---- 2. 链路串联（FR-204）：按 request_id 可完整串联。
    let records: Vec<EventRecord> = export
        .events
        .iter()
        .map(|row| EventRecord {
            schema_version: SCHEMA_VERSION,
            seq: row.seq as u64,
            id: row.id.clone(),
            kind: EventKind::RequestStarted,
            session_id: row.session_id.clone(),
            task_id: row.task_id.clone(),
            request_id: row.request_id.clone(),
            tool_call_id: row.tool_call_id.clone(),
            payload: serde_json::json!({}),
            ts_ms: row.ts_us.div_euclid(1000).max(0) as u64,
            persistence_level: PersistenceLevel::Redacted,
        })
        .collect();
    assert!(
        chain_breaks(&records).is_empty(),
        "库内 LLM 事件必须都带 request_id"
    );
    let linked = whcode_llm::link(&records);
    assert_eq!(linked.len(), 1, "同一 request_id 串联为一条链");
    assert_eq!(linked["R-889"].len(), 3, "三事件完整串联");

    // ---- 3. 预算截断（§4.1）：契约四元组 + 截断原因可查。
    for index in 0..5 {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, 'user', ?, 100, current_timestamp, NULL)",
            params![format!("M-{index}"), "S-1", format!("ref-{index}")],
        )
        .expect("插入消息");
    }
    let window = load_context(&conn, &policy, &actor, "S-1", 350).expect("上下文加载");
    assert_eq!(window.items.len(), 3);
    assert_eq!(window.consumed_budget, 300);
    assert_eq!(
        window.truncated_reason,
        Some(whcode_app::REASON_BUDGET_EXHAUSTED)
    );

    // 压缩指针：记录后原文仍可查（FR-107）。
    compress_context(
        &conn,
        &policy,
        &actor,
        &whcode_app::ContextCompression {
            session_id: "S-1",
            window_start_seq: 1,
            window_end_seq: 3,
            total_tokens: 300,
            summary_ref: "summary:A-1",
        },
    )
    .expect("压缩指针");
    let pointer = load_summary_pointer(&conn, "S-1")
        .expect("指针")
        .expect("存在");
    assert_eq!(pointer.summary_ref, "summary:A-1");
    let full = load_context(&conn, &policy, &actor, "S-1", 1_000_000).expect("原文可查");
    assert_eq!(full.items.len(), 5, "压缩不删除原始历史");

    // ---- 4. 重连补拉（V-103）：断线期间的关键事件在补拉中齐备。
    // 断线点 seq=1：客户端只见过 seq=1（RequestStarted）；断线期间的关键事件
    // （seq > 1 且 critical）= {3: RequestCompleted}，必须在补拉中齐备。
    let missed: BTreeSet<i64> = export
        .events
        .iter()
        .filter(|row| row.seq > 1 && critical_kinds().contains(row.kind.as_str()))
        .map(|row| row.seq)
        .collect();
    assert_eq!(
        missed,
        BTreeSet::from([3]),
        "断线期间缺失的关键事件必须为 RequestCompleted"
    );

    let replay = replay_since(&conn, &policy, &actor, "S-1", 1, REPLAY_PAGE_LIMIT).expect("补拉");
    assert!(replay.complete);
    let replay_critical: BTreeSet<i64> = replay
        .events
        .iter()
        .filter(|row| critical_kinds().contains(row.kind.as_str()))
        .map(|row| row.seq)
        .collect();
    assert!(
        missed.iter().all(|seq| replay_critical.contains(seq)),
        "断线期间的关键事件必须在补拉中齐备（V-103）：补拉 {replay_critical:?}，缺失 {missed:?}"
    );
    assert_eq!(
        replay.events.len(),
        2,
        "补拉恰好覆盖断线期间的事件（不重复）"
    );
    assert!(
        replay.events.iter().all(|row| row.seq > 1),
        "补拉从游标之后开始，不重复返回断线前事件"
    );
}
