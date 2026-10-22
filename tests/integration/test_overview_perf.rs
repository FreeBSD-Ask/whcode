//! 概览查询性能集成测试（T055、V-102、SC-002）：1 万任务数据集上报告 p50/p95/p99。
//!
//! 测量纪律（§5.2）：
//! - 固定夹具（T053 生成器，固定阶段/状态分布）、同机同库连续采样；
//! - 预热 5 次、采样 50 次，报告分位数（微秒）并写入
//!   `artifacts/overview-perf-m2.json` 留证；
//! - 对比对象是 plan.md 的**候选**预算 p95 ≤ 50 ms——性能预算仍未冻结
//!   （D-06：缺跨平台基线与长时数据），断言通过**不等于**预算冻结；
//! - 断言失败即预算违约的早期信号，须回到"测量→定位→设计"流程（NFR-216）。

#[path = "../fixtures/gen_dataset.rs"]
mod gen_dataset;

use std::time::Instant;

use duckdb::Connection;
use whcode_app::{DefaultPolicy, actor_with_all_capabilities, task_overview};

const WARMUP: usize = 5;
const SAMPLES: usize = 50;
/// plan.md 候选预算：概览查询 p95 ≤ 50 ms。
const CANDIDATE_P95_BUDGET_US: u128 = 50_000;

fn temp_db(tag: &str) -> Connection {
    let dir =
        std::env::temp_dir().join(format!("whcode-overview-perf-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let conn = Connection::open(dir.join("perf.duckdb")).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    conn
}

use whcode_store::apply_migrations;

#[test]
fn overview_p95_within_candidate_budget_and_report_is_archived() {
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
        "概览查询（1 万任务）：p50 {p50} µs / p95 {p95} µs / p99 {p99} µs（候选预算 p95 ≤ {CANDIDATE_P95_BUDGET_US} µs；预算未冻结）"
    );
    assert!(
        p95 <= CANDIDATE_P95_BUDGET_US,
        "概览 p95 {p95} µs 超出候选预算 {CANDIDATE_P95_BUDGET_US} µs；须走性能归因流程"
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
            "candidate_p95_us": CANDIDATE_P95_BUDGET_US,
            "frozen": false,
            "note": "D-06：预算未冻结——缺跨平台基线与长时数据；本报告不作验收承诺"
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
