//! 空间统计集成测试（T045、V-107、FR-510、Check 45/204）。
//!
//! 覆盖三件事：
//! 1. 清理（T039 purge）前后四项统计——数据库文件大小、WAL、已用块、可用块——可采集
//!    且块自洽（`used + free == total`）；
//! 2. compact（T041）前后统计齐备，数据完好；
//! 3. **不承诺 VACUUM 释放空间**（R-09/D-10）：测试**不断言**文件变小或块变少——
//!    已删除行占用的块可能保留在文件中，这正是 FR-510 要求报告并解释而非承诺的
//!    语义；任何对"清理后物理空间未缩小"的承诺都被本测试明确排除。

use std::path::PathBuf;

use duckdb::{Connection, params};
use whcode_store::{
    OnProtected, PurgeCriteria, PurgeProtection, apply_migrations, capture_space, compact_database,
    execute_purge, preview_purge,
};

fn temp_db(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("whcode-space-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir.join(format!("{tag}.duckdb"))
}

fn seed_session(conn: &Connection, session_id: &str, start_seq: i64, event_count: i64) {
    conn.execute(
        "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status)
         VALUES (?, '空间统计', epoch_ms(?), epoch_ms(?), NULL, FALSE, 'idle')",
        params![session_id, 1_000_000i64, 1_000_000i64],
    )
    .expect("插入会话");
    for offset in 0..event_count {
        let seq = start_seq + offset;
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', ?,
                     current_timestamp, 'redacted')",
            params![
                seq,
                format!("E-{seq}"),
                session_id,
                format!("{{\"pad\":\"{}\"}}", "x".repeat(64))
            ],
        )
        .expect("插入事件");
    }
}

fn count_events(conn: &Connection) -> i64 {
    conn.query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .expect("统计事件")
}

/// 截止点：足够晚，使 1970 年代 `last_active` 的夹具会话全部入选且不在活跃窗口内。
const BEFORE_CUTOFF_MS: i64 = 10_000_000_000;

#[test]
fn purge_then_compact_reports_four_statistics_and_keeps_data() {
    let db = temp_db("full");
    let now_ms = 10_000_000_000u64;

    let mut conn = Connection::open(&db).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    seed_session(&conn, "S-keep", 1, 200);
    seed_session(&conn, "S-purge", 1_000, 2_000);

    // ---- 清理前：四项统计齐备且自洽。
    let before_purge = capture_space(&conn, &db).expect("清理前统计");
    assert!(before_purge.file_size_bytes > 0);
    assert!(before_purge.blocks_consistent(), "{before_purge:?}");

    // ---- 清理（FR-509 全链：预览 → 确认 → 审计 → 活跃保护）。
    let protection = PurgeProtection {
        on_protected: OnProtected::Skip,
        ..PurgeProtection::default()
    };
    let preview = preview_purge(
        &conn,
        &PurgeCriteria::before(BEFORE_CUTOFF_MS),
        &protection,
        now_ms,
    )
    .expect("清理预览");
    assert_eq!(preview.selected_sessions.len(), 2);
    let result =
        execute_purge(&mut conn, &preview, true, "tester", &protection, now_ms).expect("执行清理");
    assert_eq!(result.deleted.events, 2_200);
    assert_eq!(count_events(&conn), 0);

    let after_purge = capture_space(&conn, &db).expect("清理后统计");
    assert!(after_purge.blocks_consistent(), "{after_purge:?}");
    // FR-510 纪律：这里只记录并解释，不断言空间缩小或未缩小。

    // ---- compact：复制—校验—切换后数据完好、统计齐备。
    drop(conn);
    let report = compact_database(&db).expect("compact");
    assert!(report.switched);
    assert!(report.before.blocks_consistent());
    assert!(report.after.blocks_consistent());
    assert!(report.after.file_size_bytes > 0);
    // 同样不承诺：report.after.file_size_bytes <= report.before.file_size_bytes 不作断言。

    let conn = Connection::open(&db).expect("重开 compact 后的库");
    assert_eq!(count_events(&conn), 0, "空数据集经 compact 后仍可用");
    assert_eq!(
        whcode_store::schema_version(&conn).expect("版本"),
        whcode_store::STORE_SCHEMA_VERSION
    );

    // ---- 重新写入并再 compact 一次：保留数据必须原样存活。
    seed_session(&conn, "S-after", 5_000, 300);
    drop(conn);
    let report = compact_database(&db).expect("二次 compact");
    assert!(report.switched);
    let conn = Connection::open(&db).expect("重开");
    assert_eq!(count_events(&conn), 300, "compact 后保留数据必须完好");
    let final_space = capture_space(&conn, &db).expect("最终统计");
    assert!(final_space.blocks_consistent());

    let _ = std::fs::remove_dir_all(db.parent().expect("临时目录")).ok();
}
