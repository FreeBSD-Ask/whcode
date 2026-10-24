//! 极限规模集成测试（2026-10-22 修订：1M 事件单会话，资源预算铁律 X1—X5）。
//!
//! **现实工作负载**：会话库可达 30—60GB（≈4 亿事件 @130B/事件）。本测试以 1M
//! 事件夹具（≈150MB 库文件，访问模式与 4 亿事件同构——B+ 树深度仅差 1—2 层）
//! 验证**操作内存与库大小无关（O(1)）**的不变量；测试进程峰值 RSS 由外部测量
//! （`script/measure_resources.py`，X6 标准 ≤ 512 MB）。
//!
//! 覆盖（全在 1M 行夹具上）：
//! - **X2 全序翻页 O(1) 内存**：1000 页 × 1000 行遍历**不物化全表**——仅累计
//!   计数/求和/相邻 `seq` 严格递增比较（进程内存恒定）；
//! - **X3 大表聚合**：`count(*)`/`MAX(seq)` 走索引/元数据，秒级完成（时序断言）；
//! - **X4 极限清理**：purge 预览计数 + 执行删除在 1M 行上完成；
//! - **X5 极限切换**：会话切换返回恒定 50 条尾部 + 游标（不随库大小增长）。

use std::time::{Duration, Instant};

use duckdb::Connection;
use whcode_app::{
    DefaultPolicy, REASON_BUDGET_EXHAUSTED, SWITCH_TAIL_LIMIT, actor_with_all_capabilities,
    load_context, switch_session,
};
use whcode_store::{
    MAX_PAGE_SIZE, apply_migrations, create_session_with_id, events_page, latest_seq,
};

use whcode_store::Direction;

/// 极限夹具行数：1M 事件（访问模式与 60GB 会话库同构，见模块文档）。
const EXTREME_EVENTS: usize = 1_000_000;

/// 单批插入行数（500 行 × 4 绑定 = 2000 参数，DuckDB 参数上限内）。
const BATCH: usize = 500;

fn temp_db(tag: &str) -> Connection {
    let dir = std::env::temp_dir().join(format!("whcode-extreme-{}-{tag}", std::process::id()));
    // 上次运行被中断（进程被杀）时目录会遗留；Windows 会复用 PID，先清再建（夹具幂等）。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let conn = Connection::open(dir.join("extreme.duckdb")).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    conn
}

/// 批量插入 1M 事件（seq 连续；payload 64 字节填充模拟真实载荷）。
fn seed_extreme(conn: &Connection, session_id: &str, events: usize) -> Duration {
    let started = Instant::now();
    // events.seq 是工作区级主键：从当前最大值续起，同库多次生成不冲突。
    let seq_base: usize = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
            row.get(0)
        })
        .map(|max: i64| max as usize)
        .expect("读取事件游标");
    let mut inserted = 0usize;
    while inserted < events {
        let count = BATCH.min(events - inserted);
        let mut values = Vec::with_capacity(count);
        let mut binds: Vec<duckdb::types::Value> = Vec::with_capacity(count * 4);
        for seq in seq_base + inserted + 1..=seq_base + inserted + count {
            values.push(
                "(?, ?, ?, NULL, NULL, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT, current_timestamp, 'redacted')"
                    .to_string(),
            );
            binds.push(duckdb::types::Value::BigInt(seq as i64));
            binds.push(duckdb::types::Value::Text(format!("E-{seq}")));
            binds.push(duckdb::types::Value::Text(session_id.to_string()));
            binds.push(duckdb::types::Value::Text(format!(
                "{{\"pad\":\"{}\",\"seq\":{seq}}}",
                "x".repeat(64)
            )));
        }
        let sql = format!(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES {}",
            values.join(", ")
        );
        conn.execute(sql.as_str(), duckdb::params_from_iter(binds.iter()))
            .expect("批量插入事件");
        inserted += count;
    }
    started.elapsed()
}

#[test]
fn extreme_scale_pagination_aggregation_cleanup_and_switch() {
    let conn = temp_db("x");
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("tester");

    // ---- X1 极限夹具：会话行 + 1M 事件（时序记录）。
    create_session_with_id(&conn, "S-X", "极限会话", "active", 1_000).expect("创建夹具会话");
    let seed_elapsed = seed_extreme(&conn, "S-X", EXTREME_EVENTS);
    eprintln!("X1 夹具：{EXTREME_EVENTS} 事件写入耗时 {seed_elapsed:?}");

    // ---- X3 大表聚合：count/MAX 走索引/元数据，秒级完成。
    let started = Instant::now();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .expect("聚合");
    let cursor = latest_seq(&conn).expect("游标");
    let aggregate_elapsed = started.elapsed();
    assert_eq!(count, EXTREME_EVENTS as i64);
    assert_eq!(cursor, EXTREME_EVENTS as i64);
    assert!(
        aggregate_elapsed < Duration::from_secs(5),
        "X3：大表聚合 {aggregate_elapsed:?} 超过 5s，未走索引/元数据"
    );
    eprintln!("X3 聚合：count/MAX 耗时 {aggregate_elapsed:?}");

    // ---- X2 全序翻页 O(1) 内存：1000 页遍历，仅累计计数/求和/相邻递增比较
    //      ——**不物化全表**（无 1M 元素集合），进程内存恒定。
    let started = Instant::now();
    let mut seen = 0i64;
    let mut sum = 0i64;
    let mut last_seq = 0i64;
    let mut cursor = None;
    let mut pages = 0usize;
    loop {
        let page =
            events_page(&conn, "S-X", Direction::Forward, cursor, MAX_PAGE_SIZE).expect("翻页");
        pages += 1;
        for event in &page.items {
            assert!(
                event.seq > last_seq,
                "seq 严格递增（无重复/回退）：{last_seq} → {}",
                event.seq
            );
            seen += 1;
            sum += event.seq;
            last_seq = event.seq;
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let walk_elapsed = started.elapsed();
    assert_eq!(seen, EXTREME_EVENTS as i64, "X2：翻页必须覆盖全部事件");
    assert_eq!(
        sum,
        (EXTREME_EVENTS as i64) * ((EXTREME_EVENTS as i64) + 1) / 2,
        "X2：seq 连续无遗漏"
    );
    assert_eq!(pages, EXTREME_EVENTS / MAX_PAGE_SIZE, "页数恰为 1000");
    eprintln!("X2 翻页：{pages} 页耗时 {walk_elapsed:?}（O(1) 内存——无全表物化）");

    // ---- X5 极限切换：恒定 50 条尾部 + 游标，不随库大小增长。
    let started = Instant::now();
    let view = switch_session(&conn, &policy, &actor, "S-X").expect("切换");
    let switch_elapsed = started.elapsed();
    assert_eq!(view.tail.len(), SWITCH_TAIL_LIMIT, "X5：尾部恒定 50 条");
    assert_eq!(view.event_cursor, EXTREME_EVENTS as i64);
    assert!(
        switch_elapsed < Duration::from_secs(2),
        "X5：切换 {switch_elapsed:?} 超过 2s（NFR-207 候选 200ms 的极限上限）"
    );
    eprintln!(
        "X5 切换：{switch_elapsed:?}（尾部恒定 {} 条）",
        view.tail.len()
    );

    // ---- 上下文有界：预算截断原因可查（1M 行库上仍 O(1)）。
    for index in 0..10 {
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES (?, ?, 'user', ?, 100, current_timestamp, NULL)",
            duckdb::params![format!("M-{index}"), "S-X", format!("ref-{index}")],
        )
        .expect("插入消息");
    }
    let window = load_context(&conn, &policy, &actor, "S-X", 950).expect("上下文");
    assert_eq!(window.items.len(), 9, "预算 950 只装 9 条（900 token）");
    assert_eq!(window.truncated_reason, Some(REASON_BUDGET_EXHAUSTED));

    // ---- X4 极限清理：purge 预览计数 + 执行删除在 1M 行上完成。
    // 先建第二个会话并复制同规模数据（删除目标），原会话保留。
    let mut conn = conn;
    create_session_with_id(&conn, "S-DEL", "清理目标", "active", 1_000).expect("创建");
    let seed2 = seed_extreme(&conn, "S-DEL", 1_000_000);
    eprintln!("X4 清理目标：1M 事件写入耗时 {seed2:?}");

    // now_ms 取远期值：会话 last_active=1_000ms，须在 24h 活跃窗口之外才入选
    // （与 T039 测试同口径的交互语义）。
    let purge_now = 10_000_000_000_000u64;
    let preview = whcode_store::preview_purge(
        &conn,
        &whcode_store::PurgeCriteria::sessions(["S-DEL"]),
        &whcode_store::PurgeProtection::default(),
        purge_now,
    )
    .expect("清理预览");
    assert_eq!(
        preview.selected_sessions,
        vec!["S-DEL".to_string()],
        "X4：清理集恰为 S-DEL"
    );
    assert!(
        preview.counts.events == 1_000_000,
        "X4：预览计数覆盖 1M 行，实际 {}",
        preview.counts.events
    );

    let started = Instant::now();
    let result = whcode_store::execute_purge(
        &mut conn,
        &preview,
        true,
        "tester",
        &whcode_store::PurgeProtection::default(),
        purge_now,
    )
    .expect("执行清理");
    let purge_elapsed = started.elapsed();
    eprintln!(
        "X4 探针：execute_purge 总耗时 {purge_elapsed:?}（9 张表 DELETE + 审计 + COMMIT；events 表 1M 行 MVCC 删除为成本主体）"
    );
    assert_eq!(result.deleted.events, 1_000_000, "X4：删除 1M 行");
    assert!(
        purge_elapsed < Duration::from_secs(300),
        "X4：清理 {purge_elapsed:?} 超过实测标准 300s（125s @1M 行，DuckDB MVCC 删除成本）"
    );
    // 清理后原会话数据完好。
    let remaining: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .expect("统计");
    assert_eq!(remaining, EXTREME_EVENTS as i64, "S-X 数据完好");
    eprintln!("X4 清理：删除 1M 行耗时 {purge_elapsed:?}");

    // ---- 收尾。
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("whcode-extreme-{}-x", std::process::id())),
    )
    .ok();
}
