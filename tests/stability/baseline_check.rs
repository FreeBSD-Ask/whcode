//! 基线基准夹具正确性检查（tasks.md T027；V-006、R-12）。
//!
//! 这里验证**正确性**而不是性能达标：固定数据规模下检查事件条数、幂等键去重、
//! 分位数单调性与样本数、运行期引擎版本可见性。性能数字只作记录，不作通过判据；
//! 报告必须带完整测量条件，单机单次样本不构成预算冻结。
//!
//! 夹具实现与 `crates/whcode-bin/src/bench.rs` 是**同一份源码**（`#[path]` 引入），
//! 避免测试另写一套逻辑而与真实报告漂移。

#[path = "../../crates/whcode-bin/src/bench.rs"]
mod bench;

use std::path::PathBuf;

use duckdb::Connection;
use serde_json::json;
use whcode_domain::{EventKind, EventRecord, PersistenceLevel, SCHEMA_VERSION};
use whcode_store::{EventWriter, WriteOutcome, WriterConfig};

/// 固定数据规模：每次重复写入的吞吐事件数（远小于默认 20000，保证测试快速且确定性）。
const EVENTS: usize = 500;
/// 固定重复次数。
const REPEAT: usize = 3;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "whcode-baseline-check-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn assert_quantiles(quantiles: &bench::Quantiles, context: &str) {
    assert!(
        quantiles.p50 <= quantiles.p95,
        "{context}：p50 不得大于 p95（nearest-rank 分位数）"
    );
    assert!(
        quantiles.p95 <= quantiles.p99,
        "{context}：p95 不得大于 p99（nearest-rank 分位数）"
    );
    assert!(quantiles.p50 > 0.0, "{context}：实测延迟必须为正");
    assert_eq!(
        quantiles.unit, "microseconds",
        "{context}：延迟单位必须注明"
    );
}

#[test]
fn baseline_fixture_reports_consistent_counts_and_quantiles() {
    let dir = temp_dir("fixture");
    let out = dir.join("baseline.json");
    let options = bench::BaselineOptions::validated(EVENTS, REPEAT).expect("固定参数必须合法");

    let report = bench::measure(&options).expect("固定数据集上的基准夹具必须成功");

    assert!(
        !report.duckdb_engine_version.is_empty(),
        "引擎版本必须来自运行期实测（V-006）"
    );
    assert_eq!(report.runs.len(), REPEAT, "重复次数必须与 --repeat 一致");
    assert_eq!(
        report.duckdb_engine_version, report.measurement_discipline.duckdb_engine_version,
        "顶层与测量纪律块必须报告同一运行期引擎版本"
    );

    let expected_rows = EVENTS + bench::LATENCY_SAMPLES_PER_RUN;
    for run in &report.runs {
        assert_eq!(run.events_written, EVENTS, "吞吐阶段事件数必须正确");
        assert_eq!(
            run.latency_samples,
            bench::LATENCY_SAMPLES_PER_RUN,
            "每次重复的延迟样本数必须固定"
        );
        assert_eq!(
            run.event_rows, expected_rows as i64,
            "写入后数据库事件行数应为吞吐样本与延迟样本之和"
        );
        assert!(run.db_bytes > 0, "CHECKPOINT 后数据库文件应有大小");
        assert_quantiles(&run.latency_us, &format!("run {}", run.run));
        assert_eq!(
            run.latency_us.sample_count,
            bench::LATENCY_SAMPLES_PER_RUN,
            "分位数样本数必须与实测样本数一致"
        );
    }

    assert_quantiles(&report.aggregate.latency_us, "aggregate");
    assert_eq!(
        report.aggregate.latency_us.sample_count,
        REPEAT * bench::LATENCY_SAMPLES_PER_RUN,
        "汇总分位数的样本数必须等于各次重复样本之和"
    );
    assert_eq!(report.aggregate.runs, REPEAT);
    assert_eq!(
        report.aggregate.total_events_written,
        REPEAT * EVENTS,
        "汇总事件总数必须与重复次数一致"
    );

    // 落盘内容必须携带测量纪律字段块（V-006）。
    bench::write_json(&report, &out).expect("基准报告必须可落盘");
    let raw = std::fs::read_to_string(&out).expect("读取已落盘的基准报告");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("报告必须是合法 JSON");
    assert_eq!(value["schema_version"], json!(SCHEMA_VERSION));
    assert_eq!(value["command"], json!("bench baseline"));
    let discipline = &value["measurement_discipline"];
    for key in [
        "os_name",
        "os_version",
        "cpu",
        "memory",
        "build_profile",
        "rust_toolchain",
        "duckdb_engine_version",
        "database_parameters",
        "data_scale",
        "sampling",
        "script_path",
        "generated_at_utc",
    ] {
        assert!(
            discipline.get(key).is_some(),
            "测量纪律字段缺失：{key}（V-006）"
        );
    }
    assert!(
        discipline["data_scale"]["events_per_run"] == json!(EVENTS),
        "报告中的数据规模必须与固定夹具一致"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn duplicate_idempotency_key_does_not_add_rows() {
    let dir = temp_dir("dedup");
    let db = dir.join("dedup.duckdb");
    let conn = Connection::open(&db).expect("打开测试库");
    let mut writer = EventWriter::new(conn, WriterConfig::default()).expect("在测试库上建立写者");

    let record = || EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: 0,
        id: "E-DUP".to_string(),
        kind: EventKind::ToolCallStarted,
        session_id: Some("S-BASELINE".to_string()),
        task_id: None,
        request_id: None,
        tool_call_id: None,
        payload: json!({ "text": "x" }),
        ts_ms: 1_700_000_000_000,
        persistence_level: PersistenceLevel::Redacted,
    };

    let first = writer
        .submit(record(), "baseline-dup")
        .expect("首次提交应成功");
    assert!(
        matches!(first, WriteOutcome::Committed { .. }),
        "首次提交必须返回持久化确认"
    );
    let second = writer
        .submit(record(), "baseline-dup")
        .expect("重复提交应成功返回既有 seq");
    assert!(
        matches!(second, WriteOutcome::Deduplicated { .. }),
        "同一幂等键必须命中既有记录（V-106）"
    );
    writer.flush().expect("刷新待写缓冲");
    drop(writer);

    let conn = Connection::open(&db).expect("重开数据库读数");
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
        .expect("统计事件行数");
    assert_eq!(rows, 1, "同一幂等键重复提交不得增加行数（V-106）");
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
}
