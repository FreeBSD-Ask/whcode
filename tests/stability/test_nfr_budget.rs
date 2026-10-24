//! NFR 冻结预算测量夹具（T115、ADR-025、V-006 测量纪律）。
//!
//! 承载可进程内测量的四项预算（其余见模块尾注）：
//! - **NFR-207** 会话切换响应（10 万事件）p50/p95/p99 ≤ 200 ms——常规回归门禁；
//! - **NFR-209** 历史单页查询 p95 ≤ 30 ms——常规回归门禁；
//! - **NFR-203** 切换 20 次 RSS 增量 ≤ 10 MB——`#[ignore]`（20 次 tasklist 采样
//!   约 +3 s），由 `script/measure_nfr_budgets.py` 以 `--ignored` 驱动；
//! - **NFR-206** LLM 流式期间 CPU ≤ 8% 单核——`#[ignore]`（CPU% 需外部
//!   `GetProcessTimes` 采样，进程内无合法采样点：workspace 禁 unsafe，P/Invoke
//!   只能走子进程，污染被测区间）；本用例只负责把流式路径跑满 ≥ 3 s 供外部采样。
//!
//! NFR-201/202/205 需要真实产品进程（`whcode serve`／真实控制台 TUI），
//! 由 `script/measure_nfr_budgets.py` 采样承载；NFR-204（72 h 斜率）属 T116 长跑，
//! 保持候选。测量纪律：同夹具（10 万事件 + 1 万任务，与 `test_session_switch` 一致）、
//! 采样方法写明、分位数输出、零样本不充当达标。

#[path = "../fixtures/gen_dataset.rs"]
mod gen_dataset;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use duckdb::Connection;
use whcode_app::switch_session;
use whcode_llm::provider::{CancelToken, LlmRequest, Provider, StubProvider};
use whcode_platform::current_rss_bytes;
use whcode_store::{Direction, MAX_PAGE_SIZE, apply_migrations, events_page};

/// 单会话数据集规模（与 `test_session_switch` 同夹具，V-006）。
const DATASET_EVENTS: usize = 100_000;
const DATASET_TASKS: usize = 10_000;

/// 性能测量串行锁：NFR-207 与 NFR-209 同属一个测试二进制，`cargo test` 默认并行
/// 执行线程；两个夹具同时生成数据集与查询会互相抢占 CPU/磁盘，使 p95 失去可归因性
/// （2026-10-24 全量测试实测：并行时 NFR-209 p95 43 ms 超预算，独占时 21.8 ms）。
/// 测量纪律要求一次只跑一个性能夹具，故在此串行化，不改变判据。
static PERF_LOCK: Mutex<()> = Mutex::new(());

fn temp_db(tag: &str) -> Connection {
    let dir = std::env::temp_dir().join(format!("whcode-nfr-{}-{tag}", std::process::id()));
    // 上次运行被中断时目录会遗留；Windows 会复用 PID，先清再建（夹具幂等）。
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let conn = Connection::open(dir.join("nfr.duckdb")).expect("打开库");
    apply_migrations(&conn).expect("迁移");
    conn
}

/// 分位数（线性插值；输入须已排序；空输入返回 0——调用方保证非空）。
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let weight = rank - lower as f64;
    sorted[lower] + (sorted[upper] - sorted[lower]) * weight
}

fn print_percentiles(label: &str, samples_ms: &mut [f64]) {
    samples_ms.sort_by(|a, b| a.total_cmp(b));
    let p50 = percentile(samples_ms, 50.0);
    let p95 = percentile(samples_ms, 95.0);
    let p99 = percentile(samples_ms, 99.0);
    println!(
        "{label}: n={} p50={p50:.1}ms p95={p95:.1}ms p99={p99:.1}ms min={:.1}ms max={:.1}ms",
        samples_ms.len(),
        samples_ms[0],
        samples_ms[samples_ms.len() - 1]
    );
}

/// NFR-207：会话切换响应（10 万事件）p50/p95/p99 ≤ 200 ms（ADR-025 冻结值）。
///
/// 夹具：S-BIG（10 万事件 + 1 万任务）与 S-SMALL（100 事件）交替切换 20 次，
/// 计时只含 `switch_session` 本身（有界加载路径，FR-102）。
#[test]
fn nfr207_switch_latency_within_frozen_budget() {
    let _guard = PERF_LOCK.lock().expect("性能测量串行锁中毒");
    let mut conn = temp_db("nfr207");
    let big = gen_dataset::generate(&mut conn, "S-BIG", DATASET_EVENTS, DATASET_TASKS);
    assert_eq!(big.events, DATASET_EVENTS);
    let small = gen_dataset::generate(&mut conn, "S-SMALL", 100, 10);
    assert_eq!(small.events, 100);

    let policy = whcode_app::DefaultPolicy;
    let actor = whcode_app::actor_with_all_capabilities("nfr207");

    // 预热两次（首次含 DuckDB 页缓存与分配器增长，不计入样本）。
    switch_session(&conn, &policy, &actor, "S-BIG").expect("预热切换");
    switch_session(&conn, &policy, &actor, "S-SMALL").expect("预热切换");

    let mut samples: Vec<f64> = Vec::with_capacity(20);
    for index in 0..20 {
        let target = if index % 2 == 0 { "S-SMALL" } else { "S-BIG" };
        let started = Instant::now();
        let view = switch_session(&conn, &policy, &actor, target).expect("切换");
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(view.session.id, target);
    }
    print_percentiles("NFR-207 会话切换", &mut samples);
    let p95 = percentile(&samples, 95.0);
    assert!(
        p95 <= 200.0,
        "NFR-207 冻结预算 p95 ≤ 200 ms，实测 p95 {p95:.1} ms（全样本见上方输出）"
    );
}

/// NFR-209：历史单页查询 p95 ≤ 30 ms（ADR-025 冻结值）。
///
/// 夹具：10 万事件会话；样本为“最新一页”（`Backward` + `MAX_PAGE_SIZE`，即 UI
/// 尾页的真实形状），50 次采样。
#[test]
fn nfr209_history_single_page_query_p95_within_frozen_budget() {
    let _guard = PERF_LOCK.lock().expect("性能测量串行锁中毒");
    let mut conn = temp_db("nfr209");
    let report = gen_dataset::generate(&mut conn, "S-BIG", DATASET_EVENTS, DATASET_TASKS);
    assert_eq!(report.events, DATASET_EVENTS);

    let mut samples: Vec<f64> = Vec::with_capacity(50);
    // 预热一次（页缓存）。
    events_page(&conn, "S-BIG", Direction::Backward, None, MAX_PAGE_SIZE).expect("预热查询");
    for _ in 0..50 {
        let started = Instant::now();
        let page =
            events_page(&conn, "S-BIG", Direction::Backward, None, MAX_PAGE_SIZE).expect("单页");
        samples.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(page.items.len(), MAX_PAGE_SIZE, "单页大小应有界且填满");
    }
    print_percentiles("NFR-209 历史单页查询", &mut samples);
    let p95 = percentile(&samples, 95.0);
    assert!(
        p95 <= 30.0,
        "NFR-209 冻结预算 p95 ≤ 30 ms，实测 p95 {p95:.1} ms（全样本见上方输出）"
    );
}

/// NFR-203：切换 20 次后 RSS 增量 ≤ 10 MB（ADR-025 冻结值；`#[ignore]`，
/// 运行方式：`cargo test -p whcode-tests --test nfr_budget
/// nfr203 -- --ignored --nocapture`）。
///
/// 方法：预热 4 次切换后取基线 RSS（`whcode_platform::current_rss_bytes`，
/// tasklist 工作集），随后 20 次交替切换、每次切换完成后采样一次；
/// 判据为“相对基线的最大增量”（切换后的稳态值，不追瞬时峰值——峰值口径
/// 归 NFR-201/202 的进程级采样）。
#[test]
#[ignore = "测量夹具：由 script/measure_nfr_budgets.py 以 --ignored 驱动并留档"]
fn nfr203_switch_rss_increment_bounded() {
    let mut conn = temp_db("nfr203");
    let big = gen_dataset::generate(&mut conn, "S-BIG", DATASET_EVENTS, DATASET_TASKS);
    assert_eq!(big.events, DATASET_EVENTS);
    let small = gen_dataset::generate(&mut conn, "S-SMALL", DATASET_EVENTS, DATASET_TASKS);
    assert_eq!(small.events, DATASET_EVENTS);

    let policy = whcode_app::DefaultPolicy;
    let actor = whcode_app::actor_with_all_capabilities("nfr203");

    // 预热 4 次（页缓存与分配器进入稳态），随后取基线。
    for target in ["S-BIG", "S-SMALL", "S-BIG", "S-SMALL"] {
        switch_session(&conn, &policy, &actor, target).expect("预热切换");
    }
    let baseline = current_rss_bytes().expect("基线 RSS（零样本不充当达标）");

    let mut increments_mb: Vec<f64> = Vec::with_capacity(20);
    for index in 0..20 {
        let target = if index % 2 == 0 { "S-SMALL" } else { "S-BIG" };
        let view = switch_session(&conn, &policy, &actor, target).expect("切换");
        // Check 47：10 万事件会话只加载元数据 + 概览 + 有界尾部（不加载完整历史）。
        assert_eq!(
            view.tail.len(),
            whcode_app::session::SWITCH_TAIL_LIMIT,
            "10 万事件会话的活跃尾部必须恒定有界"
        );
        assert!(!view.tail_complete, "10 万事件必须标注尾部截断");
        assert!(
            view.event_cursor >= i64::try_from(DATASET_EVENTS).expect("数据集规模可转换"),
            "工作区游标必须覆盖最新事件（游标是元数据，不要求加载全文）"
        );
        assert!(
            view.tail
                .iter()
                .all(|event| event.session_id.as_deref() == Some(target)),
            "尾部只能包含目标会话的事件"
        );
        assert!(view.overview.is_some(), "1 万任务会话必须带概览行");
        let rss = current_rss_bytes().expect("切换后 RSS");
        increments_mb.push((rss as f64 - baseline as f64) / (1024.0 * 1024.0));
        println!(
            "NFR-203 切换 {index:02} → {target}: 增量 {:.2} MB",
            increments_mb[index]
        );
    }
    let max_increment = increments_mb.iter().cloned().fold(0.0, f64::max);
    println!("NFR-203 切换 20 次最大 RSS 增量：{max_increment:.2} MB（预算 ≤ 10 MB）");
    assert!(
        max_increment <= 10.0,
        "NFR-203 冻结预算：切换 20 次 RSS 增量 ≤ 10 MB，实测最大 {max_increment:.2} MB"
    );
}

/// NFR-206 判据负载：**代表性速率**（25 chunks/s）的流式，供外部 CPU 采样
/// （`#[ignore]`，运行方式：`cargo test -p whcode-tests --test nfr_budget
/// nfr206_llm_stream_paced -- --ignored --nocapture`；
/// `script/measure_nfr_budgets.py` 采样测试可执行文件的 `GetProcessTimes`）。
///
/// 速率取值依据：真实 LLM 流式输出为 I/O 限速，典型 token 速率 20—100 chunks/s，
/// 取下限附近 25 chunks/s；桩 Provider 本身不限速（全速喂入属“满喂容量”场景，
/// 见 [`nfr206_llm_stream_full_feed_capacity`]）。失效条件：接入真实 Provider
/// （M3 装配）后须以真实流复测。
#[test]
#[ignore = "测量夹具：由 script/measure_nfr_budgets.py 以 --ignored 驱动并采样 CPU"]
fn nfr206_llm_stream_paced_realistic_rate() {
    let provider = StubProvider::new();
    assert!(provider.is_available() && provider.capabilities().streaming);
    let request = LlmRequest {
        request_id: "R-NFR206".to_string(),
        session_id: "S-NFR206".to_string(),
        task_id: None,
        model: "stub".to_string(),
        prompt: "流式预算负载。".repeat(512), // 约 3.6 KiB → 115 chunk/次
        max_tokens: 0,
        timeout: None,
    };
    const RATE_CHUNKS_PER_SEC: u64 = 25;
    let started = Instant::now();
    let mut total_chunks = 0u64;
    while started.elapsed() < Duration::from_secs(8) {
        let cancel = CancelToken::default();
        let stream = provider.execute(&request, &cancel).expect("发起流式");
        for (position, chunk) in stream.enumerate() {
            let chunk = chunk.expect("chunk 不应失败");
            assert_eq!(chunk.index, position as u64, "chunk 序号连续");
            total_chunks += 1;
            // 以代表性速率限速（流式期间的真实形状：等网络，不烧 CPU）。
            std::thread::sleep(Duration::from_millis(1000 / RATE_CHUNKS_PER_SEC));
        }
    }
    let elapsed = started.elapsed();
    println!(
        "NFR-206 限速流式：{total_chunks} chunks / {elapsed:.2?}（速率 25 chunks/s，供外部 CPU 采样）"
    );
    assert!(total_chunks > 0, "流式负载必须实际产出 chunk");
    assert!(elapsed >= Duration::from_secs(8), "负载须持续 ≥ 8 s 供采样");
}

/// NFR-206 余量佐证：**满喂**（不限速）流式的单核容量（`#[ignore]`，同上由
/// campaign 驱动）。不做预算判定——全速喂入不是真实流式形状；本用例输出
/// “chunks/秒/核”容量，证明在代表性速率下的 CPU 占用有数量级余量。
#[test]
#[ignore = "测量夹具：由 script/measure_nfr_budgets.py 以 --ignored 驱动"]
fn nfr206_llm_stream_full_feed_capacity() {
    let provider = StubProvider::new();
    let request = LlmRequest {
        request_id: "R-NFR206F".to_string(),
        session_id: "S-NFR206F".to_string(),
        task_id: None,
        model: "stub".to_string(),
        prompt: "流式容量负载。".repeat(8_192), // 约 57 KiB → 约 1 800 chunk/次
        max_tokens: 0,
        timeout: None,
    };
    let started = Instant::now();
    let mut total_chunks = 0u64;
    while started.elapsed() < Duration::from_secs(3) {
        let cancel = CancelToken::default();
        let stream = provider.execute(&request, &cancel).expect("发起流式");
        for chunk in stream {
            let _chunk = chunk.expect("chunk 不应失败");
            total_chunks += 1;
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "NFR-206 满喂容量：{total_chunks} chunks / {elapsed:.2}s = {:.0} chunks/s/核（余量佐证，不作判据）",
        total_chunks as f64 / elapsed
    );
    assert!(total_chunks > 0, "流式负载必须实际产出 chunk");
}
