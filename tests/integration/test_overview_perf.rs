//! 概览查询性能集成测试（T055、V-102、SC-002）：1 万任务数据集上报告 p50/p95/p99。
//!
//! 测量纪律（§5.2）：
//! - 固定夹具（T053 生成器，固定阶段/状态分布）、同机同库连续采样；
//! - 预热 5 次、采样 50 次，报告分位数（微秒）并写入
//!   `artifacts/overview-perf-m2.json` 留证；
//! - 对比对象是**已冻结**预算 p95 ≤ 50 ms（ADR-025，2026-10-23；冻结值＝spec 候选值，
//!   范围限 Windows 11 amd64）；断言通过即“达标”，断言失败即预算违约的早期信号，
//!   须回到“测量→定位→设计”流程（NFR-216），不得改预算掩盖；
//! - BSD 无运行证据，不参与该预算（ADR-013／ADR-025）。

#[path = "../fixtures/gen_dataset.rs"]
mod gen_dataset;

use std::time::Instant;

use duckdb::Connection;
use whcode_app::{DefaultPolicy, actor_with_all_capabilities, task_overview};

const WARMUP: usize = 5;
const SAMPLES: usize = 50;
/// 冻结预算（ADR-025）：概览查询 p95 ≤ 50 ms（Windows 11 amd64）。
const FROZEN_P95_BUDGET_US: u128 = 50_000;

fn temp_db(tag: &str) -> Connection {
    let dir =
        std::env::temp_dir().join(format!("whcode-overview-perf-{}-{tag}", std::process::id()));
    // 上次运行被中断（进程被杀）时目录会遗留；Windows 会复用 PID，先清再建（夹具幂等）。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let conn = Connection::open(dir.join("perf.duckdb")).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    conn
}

use whcode_store::apply_migrations;

#[test]
#[ignore = "性能预算夹具：由 perf-budget job 以 --ignored 串行运行；\
            普通并行测试受负载污染（实测 p95 52.9 ms 对 50 ms 预算）"]
fn overview_p95_within_frozen_budget_and_report_is_archived() {
    let conn = temp_db("perf");
    let mut conn = conn;
    let report = gen_dataset::generate(&mut conn, "S-PERF", 1_000, 10_000);
    drop(report);
    let conn = conn;
    let policy = DefaultPolicy;
    let actor = actor_with_all_capabilities("tester");

    for _ in 0..WARMUP {
        task_overview(&conn, &policy, &actor, Some("S-PERF")).expect("预热");
    }
    let mut samples: Vec<u128> = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let overview = task_overview(&conn, &policy, &actor, Some("S-PERF")).expect("采样");
        let elapsed = start.elapsed().as_micros();
        assert_eq!(overview.total, 10_000, "采样必须针对完整数据集");
        samples.push(elapsed);
    }
    samples.sort_unstable();
    let percentile = |p: usize| -> u128 {
        let index = ((SAMPLES - 1) * p) / 100;
        samples[index]
    };
    let p50 = percentile(50);
    let p95 = percentile(95);
    let p99 = percentile(99);
    assert!(
        p50 <= p95 && p95 <= p99,
        "分位数必须单调：{p50} {p95} {p99}"
    );

    eprintln!(
        "概览查询（1 万任务）：p50 {p50} µs / p95 {p95} µs / p99 {p99} µs（冻结预算 p95 ≤ {FROZEN_P95_BUDGET_US} µs，ADR-025；范围 Windows 11 amd64）"
    );
    assert!(
        p95 <= FROZEN_P95_BUDGET_US,
        "概览 p95 {p95} µs 超出冻结预算 {FROZEN_P95_BUDGET_US} µs（ADR-025）；须走性能归因流程"
    );

    // 证据归档（§5.2、G4）：报告入库 artifacts/，含测量纪律字段。
    let payload = serde_json::json!({
        "task": "T055",
        "fixture": { "events": 1_000, "tasks": 10_000, "stage_distribution": "fixed" },
        "measurement": {
            "warmup": WARMUP,
            "samples": SAMPLES,
            "unit": "microseconds",
            "scope": "session",
            "clock": "std::time::Instant"
        },
        "p50_us": p50,
        "p95_us": p95,
        "p99_us": p99,
        "budget": {
            "frozen_p95_us": FROZEN_P95_BUDGET_US,
            "frozen": true,
            "note": "ADR-025（2026-10-23）：Windows 11 amd64 冻结，取值＝spec 候选值；BSD 无证据不参与（ADR-013）"
        },
        "platform": "windows-amd64",
        "date": "2026-10-22"
    });
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../artifacts/overview-perf-m2.json");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&payload).expect("序列化报告"),
    )
    .expect("写入性能报告");
    eprintln!("性能报告已写入 {}", path.display());

    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("whcode-overview-perf-{}-perf", std::process::id())),
    )
    .ok();
}
