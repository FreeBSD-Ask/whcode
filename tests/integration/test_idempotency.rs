//! 幂等键集成测试（T044、V-106）：重复幂等键返回既有结果，不重复创建。
//!
//! 在文件库上跨写者重启验证：
//! 1. 首次提交确认 `seq`；
//! 2. 重启写者后同键重放返回既有 `seq`（`Deduplicated`），库内无重复行；
//! 3. 同键不同载荷同样返回既有结果——冲突语义为"返回既有"（writer 的去重契约），
//!    而不是部分接受；
//! 4. 新键在重启后继续分配单调 `seq`；
//! 5. 关键事件逐条确认与非关键批次跨 flush 的去重等价。

use std::path::PathBuf;

use duckdb::Connection;
use whcode_domain::{EventKind, EventRecord, PersistenceLevel, SCHEMA_VERSION};
use whcode_store::{EventWriter, WriteOutcome, WriterConfig, WriterService, apply_migrations};

fn temp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("whcode-idempotency-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir.join(format!("{tag}.duckdb"))
}

fn event_record(seq: u64, payload: &str) -> EventRecord {
    EventRecord {
        schema_version: SCHEMA_VERSION,
        seq,
        id: format!("E-{seq}"),
        kind: EventKind::ToolCallStarted,
        session_id: Some("S-1".to_string()),
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: serde_json::json!({ "v": payload }),
        ts_ms: 1_700_000_000_000 + seq,
        persistence_level: PersistenceLevel::Redacted,
    }
}

fn count_events(db: &PathBuf) -> i64 {
    let conn = Connection::open(db).expect("打开库");
    conn.query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
        .expect("统计事件")
}

#[test]
fn duplicate_key_across_restart_returns_existing_seq_without_recreation() {
    let db = temp_db("restart");
    {
        let conn = Connection::open(&db).expect("打开库");
        apply_migrations(&conn).expect("迁移");
        let service = WriterService::start(conn, WriterConfig::default()).expect("启动写者");
        let outcome = service
            .submit(event_record(1, "first"), "idem-1")
            .expect("首次提交");
        assert_eq!(outcome, WriteOutcome::Committed { seq: 1 });
        service.shutdown().expect("干净关闭");
    }
    assert_eq!(count_events(&db), 1);

    // 重启后同键重放：返回既有 seq，不重复创建。
    {
        let conn = Connection::open(&db).expect("重开库");
        let service = WriterService::start(conn, WriterConfig::default()).expect("重启写者");
        let outcome = service
            .submit(event_record(2, "replay"), "idem-1")
            .expect("重放提交");
        assert_eq!(
            outcome,
            WriteOutcome::Deduplicated { seq: 1 },
            "同键重放必须返回既有结果"
        );
        // 新键继续单调分配。
        let outcome = service
            .submit(event_record(2, "new-key"), "idem-2")
            .expect("新键提交");
        assert_eq!(outcome, WriteOutcome::Committed { seq: 2 });
        service.shutdown().expect("关闭");
    }
    assert_eq!(count_events(&db), 2, "重复键不得产生第二行");
}

#[test]
fn same_key_with_different_payload_is_deterministically_deduplicated() {
    let db = temp_db("conflict");
    let conn = Connection::open(&db).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    let service = WriterService::start(conn, WriterConfig::default()).expect("启动写者");

    let first = service
        .submit(event_record(1, "payload-a"), "idem-same")
        .expect("首次提交");
    assert_eq!(first, WriteOutcome::Committed { seq: 1 });

    for attempt in 0..5 {
        let replay = service
            .submit(event_record(2, &format!("payload-{attempt}")), "idem-same")
            .expect("重放提交");
        assert_eq!(
            replay,
            WriteOutcome::Deduplicated { seq: 1 },
            "同键不同载荷的第 {attempt} 次重放必须确定性地返回既有结果"
        );
    }
    service.shutdown().expect("关闭");

    let conn = Connection::open(&db).expect("重开库");
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM m0_events WHERE idempotency_key = 'idem-same'",
            [],
            |row| row.get(0),
        )
        .expect("按键统计");
    assert_eq!(count, 1, "键在库内至多一行");
    let stored: String = conn
        .query_row(
            "SELECT payload_json FROM m0_events WHERE idempotency_key = 'idem-same'",
            [],
            |row| row.get(0),
        )
        .expect("读取载荷");
    assert!(
        stored.contains("payload-a"),
        "首次载荷必须原样保留，不得被重放覆写：{stored}"
    );
}

#[test]
fn dedup_holds_across_batch_flush_boundaries_in_one_writer() {
    let db = temp_db("flush");
    let conn = Connection::open(&db).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    let mut writer = EventWriter::new(
        conn,
        WriterConfig {
            batch_rows: 3,
            batch_window_ms: 3_600_000, // 不靠时间窗口，只按行数阈值触发 flush。
            ..WriterConfig::default()
        },
    )
    .expect("建立写者");

    // 非关键事件：未达阈值先入缓冲（确认边界外的语义，FR-502）。
    let record = |seq: u64, v: &str| EventRecord {
        kind: EventKind::StreamingChunk,
        ..event_record(seq, v)
    };
    let outcome = writer
        .submit_at(record(1, "a"), "flush-1", 1_000)
        .expect("第 1 条");
    assert_eq!(outcome, WriteOutcome::Buffered, "未达阈值先入缓冲");
    // 同键已在待写批次中：不得重复入队，返回仍是 Buffered（无新增行）。
    let outcome = writer
        .submit_at(record(2, "a"), "flush-1", 1_001)
        .expect("缓冲中重放");
    assert_eq!(outcome, WriteOutcome::Buffered, "缓冲中的同键不得重复入队");
    writer.flush().expect("手动 flush");
    // flush 之后同键重放：返回既有 seq（跨 flush 边界去重）。
    let outcome = writer
        .submit_at(record(3, "a"), "flush-1", 1_002)
        .expect("flush 后重放");
    assert_eq!(
        outcome,
        WriteOutcome::Deduplicated { seq: 1 },
        "跨 flush 边界的同键重放必须返回既有结果"
    );
    let outcome = writer
        .submit_at(record(4, "b"), "flush-2", 1_003)
        .expect("新键");
    assert_eq!(outcome, WriteOutcome::Buffered);
    writer.flush().expect("收尾 flush");
    drop(writer); // 写者持有文件锁，统计前必须释放。
    assert_eq!(count_events(&db), 2);
}
