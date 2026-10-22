//! 会话切换集成测试（T054、V-101、SC-001）：10 万事件会话的有界加载与游标完整性。
//!
//! 以 T053 夹具生成 10 万事件 + 1 万任务的数据集，验证三件事：
//! 1. **有界加载**（FR-102）：切换只返回元数据 + 概览 + 最新 [`SWITCH_TAIL_LIMIT`]
//!    条尾部 + 游标——尾部长度不随数据集规模变化（Check 47 的结构性断言；
//!    内存增量预算未冻结，见 module-design 与 CHANGELOG 说明）；
//! 2. **游标完整性**（SC-001）：正向与反向翻页各覆盖全部 10 万事件，无重复、
//!    无遗漏（集合相等 + 计数相等 + `seq` 连续）；
//! 3. 概览与库内一致（预聚合行在夹具生成时重建）。

#[path = "../fixtures/gen_dataset.rs"]
mod gen_dataset;

use std::collections::HashSet;

use duckdb::Connection;
use whcode_app::{SWITCH_TAIL_LIMIT, switch_session};
use whcode_domain::TaskStatus;
use whcode_store::{Direction, EventCursor, MAX_PAGE_SIZE, apply_migrations, events_page};

fn temp_db(tag: &str) -> Connection {
    let dir = std::env::temp_dir().join(format!("whcode-switch-{}-{tag}", std::process::id()));
    // 上次运行被中断（进程被杀）时目录会遗留；Windows 会复用 PID，先清再建（夹具幂等）。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let conn = Connection::open(dir.join("switch.duckdb")).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    conn
}

#[test]
fn session_switch_is_bounded_and_cursor_pages_have_no_duplicates_or_gaps() {
    let mut conn = temp_db("big");
    let report = gen_dataset::generate(&mut conn, "S-BIG", 100_000, 10_000);
    assert_eq!(report.events, 100_000);
    assert_eq!(report.tasks, 10_000);

    let policy = whcode_app::DefaultPolicy;
    let actor = whcode_app::actor_with_all_capabilities("tester");

    // ---- 有界加载：元数据 + 概览 + 固定长度尾部 + 游标。
    let view = switch_session(&conn, &policy, &actor, "S-BIG").expect("切换");
    assert_eq!(view.session.id, "S-BIG");
    assert_eq!(view.tail.len(), SWITCH_TAIL_LIMIT, "尾部长度必须有界");
    assert!(!view.tail_complete, "10 万事件必须标记尾部截断");
    assert_eq!(view.tail[0].seq, 100_000, "尾部最新在前");
    assert_eq!(view.event_cursor, 100_000);
    let overview = view.overview.expect("概览");
    assert_eq!(overview.total, 10_000, "概览与库内一致");

    // ---- 正向翻页：游标无重复无遗漏（SC-001）。
    let mut seen: HashSet<i64> = HashSet::with_capacity(100_000);
    let mut cursor = None;
    let mut pages = 0usize;
    loop {
        let page =
            events_page(&conn, "S-BIG", Direction::Forward, cursor, MAX_PAGE_SIZE).expect("翻页");
        pages += 1;
        for event in &page.items {
            assert!(seen.insert(event.seq), "seq {} 重复出现", event.seq);
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen.len(), 100_000, "正向翻页必须覆盖全部事件");
    assert_eq!(
        seen.iter().copied().sum::<i64>(),
        100_000 * 100_001 / 2,
        "seq 必须连续（1..=100000 无遗漏）"
    );
    assert!(
        pages >= 100,
        "10 万事件 / 每页 1000 至少 100 页，实际 {pages}"
    );

    // ---- 反向翻页：从尾部游标向起点回放，同样无重复无遗漏。
    let mut seen_backward: HashSet<i64> = HashSet::with_capacity(100_000);
    let mut cursor = Some(EventCursor { seq: 100_001 });
    loop {
        let page = events_page(&conn, "S-BIG", Direction::Backward, cursor, MAX_PAGE_SIZE)
            .expect("反向翻页");
        for event in &page.items {
            assert!(
                seen_backward.insert(event.seq),
                "seq {} 重复出现",
                event.seq
            );
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen_backward, seen, "双向翻页必须覆盖同一事件集合");

    // ---- 夹具状态分布可复核（V-102 前提）。
    for status in [
        TaskStatus::Running,
        TaskStatus::Completed,
        TaskStatus::Blocked,
    ] {
        assert!(
            report.by_status.get(status.as_str()).copied().unwrap_or(0) > 0,
            "状态 {} 必须非零（V-102 前提）",
            status.as_str()
        );
    }

    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("whcode-switch-{}-big", std::process::id())),
    )
    .ok();
}

#[test]
fn dataset_generator_is_deterministic_across_runs() {
    let mut conn = temp_db("determinism");
    let first = gen_dataset::generate(&mut conn, "S-DET", 450, 200);
    let second = gen_dataset::generate(&mut conn, "S-DET2", 450, 200);
    assert_eq!(first.by_stage, second.by_stage, "固定阶段分布必须可复现");
    assert_eq!(first.by_status, second.by_status, "固定状态分布必须可复现");
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE session_id = 'S-DET'",
            [],
            |r| r.get(0),
        )
        .expect("统计");
    assert_eq!(count, 450);
}
