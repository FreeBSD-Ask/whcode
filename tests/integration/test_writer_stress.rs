//! 写者服务并发压力实验（T034、ADR-017 §7.2）。
//!
//! 目标：在 Windows 11 amd64 上以固定夹具记录**线程数、队列容量、延迟分位数、RSS**，
//! 并断言 `seq` 唯一、无丢失、无重复确认。默认 `#[ignore]`，不拖慢常规 CI：
//!
//! ```text
//! cargo test -p whcode-tests --test writer_stress -- --ignored --nocapture
//! ```
//!
//! 结果写入 `artifacts/writer-service-stress.json`（UTF-8 无 BOM、LF）。
//!
//! 纪律：
//! - 这是**实验记录器**，不是性能预算冻结：单机单次样本不构成平台承诺（V-006、§5.2）；
//! - RSS 由 `tasklist` 在实验结束时读取，代表进程工作集，**不等于** DuckDB 缓冲管理器占用；
//! - 进程线程数用 PowerShell 辅助读取；配置并发（生产者 + 1 写线程）另行记录，二者不混用；
//! - 背压重试次数单独统计，不把重试等待混入“无背压延迟”。

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use duckdb::Connection;
use serde::Serialize;
use serde_json::json;
use whcode_domain::{ErrorCode, EventKind, EventRecord, PersistenceLevel, SCHEMA_VERSION};
use whcode_store::{WriteOutcome, WriterConfig, WriterService};

/// 并发生产者数（不含写线程）。
const PRODUCERS: usize = 8;
/// 每个生产者提交的事件数。
const EVENTS_PER_PRODUCER: usize = 500;
/// 批次行数上限：与默认基线一致。
const BATCH_ROWS: usize = 500;
/// 批次时间窗口：与默认基线一致。
const BATCH_WINDOW_MS: u64 = 50;
/// 有界队列容量。
const MAX_PENDING: usize = 1024;

/// 单个生产者的记录。
struct ProducerResult {
    producer: usize,
    latencies_us: Vec<u64>,
    backpressure_retries: usize,
    committed: usize,
    deduplicated: usize,
}

/// 延迟分位数（最近秩法，1 基）。
fn percentile(sorted: &[u64], percent: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((percent as f64 / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
}

/// 进程工作集字节数；不可用时返回 `None`（`rss_note` 已说明口径）。
#[cfg(windows)]
fn process_working_set() -> Option<u64> {
    let pid = std::process::id();
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?;
    // 形如 "test.exe","1234","Console","1","123,456 K"
    let last = line.rsplit("\",\"").next()?.trim().trim_matches('"');
    let kb: u64 = last.strip_suffix(" K")?.replace(',', "").parse().ok()?;
    Some(kb * 1024)
}

#[cfg(not(windows))]
fn process_working_set() -> Option<u64> {
    None
}

/// 进程线程数；不可用时返回 `None`。
#[cfg(windows)]
fn process_threads() -> Option<u32> {
    let pid = std::process::id();
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid}).Threads.Count"),
        ])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(not(windows))]
fn process_threads() -> Option<u32> {
    None
}

/// 实际 rustc 版本（可复现构建依据）。
fn rustc_version() -> Option<String> {
    let output = Command::new("rustc").arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn event(producer: usize, index: usize) -> EventRecord {
    let ordinal = producer * EVENTS_PER_PRODUCER + index;
    EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: ordinal as u64 + 1,
        id: format!("E-{producer}-{index}"),
        kind: EventKind::StreamingChunk,
        session_id: Some("S-stress".to_string()),
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: json!({"producer": producer, "index": index}),
        ts_ms: 1_700_000_000_000 + ordinal as u64,
        persistence_level: PersistenceLevel::Redacted,
    }
}

fn run_producer(service: &WriterService, producer: usize) -> ProducerResult {
    let mut latencies_us = Vec::with_capacity(EVENTS_PER_PRODUCER);
    let mut backpressure_retries = 0;
    let mut committed = 0;
    let mut deduplicated = 0;
    for index in 0..EVENTS_PER_PRODUCER {
        let key = format!("k-{producer}-{index}");
        let started = Instant::now();
        loop {
            match service.submit(event(producer, index), key.clone()) {
                Ok(outcome) => {
                    latencies_us.push(started.elapsed().as_micros() as u64);
                    match outcome {
                        WriteOutcome::Committed { .. } => committed += 1,
                        WriteOutcome::Deduplicated { .. } => deduplicated += 1,
                        WriteOutcome::Buffered => {}
                    }
                    break;
                }
                Err(err) if err.code == ErrorCode::Backpressure => {
                    backpressure_retries += 1;
                    std::thread::yield_now();
                }
                Err(err) => panic!("提交失败（producer={producer}, index={index}）：{err}"),
            }
        }
    }
    ProducerResult {
        producer,
        latencies_us,
        backpressure_retries,
        committed,
        deduplicated,
    }
}

/// 延迟汇总（由 `serde_json` 序列化）。
#[derive(Serialize)]
struct LatencySummary {
    samples: usize,
    min_us: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
}

fn summarize(mut samples: Vec<u64>) -> LatencySummary {
    samples.sort_unstable();
    LatencySummary {
        samples: samples.len(),
        min_us: samples.first().copied().unwrap_or(0),
        p50_us: percentile(&samples, 50),
        p95_us: percentile(&samples, 95),
        p99_us: percentile(&samples, 99),
        max_us: samples.last().copied().unwrap_or(0),
    }
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests 包必须位于 workspace 根下")
}

/// 压力实验：8 生产者 × 500 事件。
#[test]
#[ignore = "实验记录器：需用 --ignored 显式运行，结果写入 artifacts/writer-service-stress.json"]
fn writer_service_stress_experiment() {
    let workspace = workspace_root().join("target").join(format!(
        "writer-stress-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|delta| delta.as_millis())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&workspace).expect("创建临时工作目录");
    let db = workspace.join("stress.duckdb");

    let config = WriterConfig {
        batch_rows: BATCH_ROWS,
        batch_window_ms: BATCH_WINDOW_MS,
        max_pending: MAX_PENDING,
        ..WriterConfig::default()
    };
    let expected = PRODUCERS * EVENTS_PER_PRODUCER;

    let rss_before = process_working_set();
    let duckdb_version =
        whcode_store::engine_version().unwrap_or_else(|err| format!("<不可用：{err}>"));
    let rustc = rustc_version().unwrap_or_else(|| "<不可用>".to_string());

    let conn = Connection::open(&db).expect("打开压力测试库");
    let service = WriterService::start(conn, config).expect("启动写者服务");

    println!(
        "== 写者服务并发压力实验（ADR-017 §7.2）==\n\
         生产者 {PRODUCERS} × {EVENTS_PER_PRODUCER} 条 = {expected} 条；\
         批次 {BATCH_ROWS} 行 / {BATCH_WINDOW_MS} ms；队列容量 {MAX_PENDING}"
    );

    let started = Instant::now();
    let results: Vec<ProducerResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..PRODUCERS)
            .map(|producer| {
                let service = &service;
                scope.spawn(move || run_producer(service, producer))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("生产者线程不得 panic"))
            .collect()
    });
    let elapsed = started.elapsed();

    service.shutdown().expect("关闭写者服务并排空队列");
    let rss_after = process_working_set();

    let conn = Connection::open(&db).expect("重开压力测试库");
    let count: i64 = conn
        .query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
        .expect("统计事件");
    let distinct_seq: i64 = conn
        .query_row("SELECT count(DISTINCT seq) FROM m0_events", [], |row| {
            row.get(0)
        })
        .expect("统计唯一 seq");
    let (min_seq, max_seq): (i64, i64) = conn
        .query_row(
            "SELECT COALESCE(MIN(seq), 0), COALESCE(MAX(seq), 0) FROM m0_events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("统计 seq 边界");
    drop(conn);

    let mut latencies = Vec::with_capacity(expected);
    let mut backpressure_retries = 0;
    let mut committed = 0;
    let mut deduplicated = 0;
    let mut per_producer = Vec::with_capacity(results.len());
    for result in &results {
        latencies.extend_from_slice(&result.latencies_us);
        backpressure_retries += result.backpressure_retries;
        committed += result.committed;
        deduplicated += result.deduplicated;
        per_producer.push(json!({
            "producer": result.producer,
            "samples": result.latencies_us.len(),
            "backpressure_retries": result.backpressure_retries,
            "committed": result.committed,
            "deduplicated": result.deduplicated,
        }));
    }
    let latency = summarize(latencies);

    println!(
        "完成 {expected} 条，用时 {:.3} s；提交确认 committed={committed} deduplicated={deduplicated}；\
         背压重试 {backpressure_retries}；延迟 p50={} µs p95={} µs p99={} µs max={} µs",
        elapsed.as_secs_f64(),
        latency.p50_us,
        latency.p95_us,
        latency.p99_us,
        latency.max_us
    );

    assert_eq!(count as usize, expected, "事件总数必须与提交数一致");
    assert_eq!(distinct_seq as usize, expected, "seq 必须唯一");
    assert_eq!(min_seq, 1, "seq 必须从 1 开始");
    assert_eq!(max_seq as usize, expected, "seq 必须连续到提交总数");
    assert_eq!(deduplicated, 0, "压力夹具使用唯一键，不得发生去重");

    let report = json!({
        "schema_version": 1,
        "experiment": "writer-service-stress",
        "adr": "ADR-017",
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logical_cores": std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            "rustc": rustc,
            "duckdb": duckdb_version,
            "machine_note": "Windows 11 10.0.26300 / i7-1260P / 16.9 GB（本机环境事实）",
            "process_threads": process_threads(),
            "configured_threads": PRODUCERS + 1,
            "rss_before_bytes": rss_before,
            "rss_after_bytes": rss_after,
            "rss_note": "tasklist 读取的进程工作集；不代表 DuckDB memory_limit，也不作为预算冻结",
        },
        "fixture": {
            "producers": PRODUCERS,
            "events_per_producer": EVENTS_PER_PRODUCER,
            "events_total": expected,
            "event_kind": EventKind::StreamingChunk.as_str(),
            "persistence_level": PersistenceLevel::Redacted.as_str(),
            "batch_rows": BATCH_ROWS,
            "batch_window_ms": BATCH_WINDOW_MS,
            "queue_capacity": MAX_PENDING,
        },
        "result": {
            "elapsed_ms": elapsed.as_millis() as u64,
            "events_stored": count,
            "distinct_seq": distinct_seq,
            "min_seq": min_seq,
            "max_seq": max_seq,
            "committed_outcomes": committed,
            "deduplicated_outcomes": deduplicated,
            "backpressure_retries": backpressure_retries,
            "latency_us": latency,
            "per_producer": per_producer,
        },
        "notes": [
            "实验记录器：单机单次样本不构成性能预算冻结（V-006、§5.2）",
            "延迟为 submit 的端到端往返（入队到写线程回应），含等待批次提交的时间",
            "队列深度以容量记录；实际占用未逐样本采样，背压重试次数是溢出证据",
            "RSS 与线程数为进程级观测，且在生产者 join、写者关闭后读取，含测试框架自身线程与内存",
        ],
    });

    let out = workspace_root()
        .join("artifacts")
        .join("writer-service-stress.json");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("创建 artifacts 目录");
    }
    fs::write(
        &out,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("压力报告必须可序列化")
        ),
    )
    .expect("写入压力证据");
    println!("证据已写入 {}", out.display());

    let _ = fs::remove_dir_all(&workspace);
}
