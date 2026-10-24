//! T116 长跑稳定性采集器（§11.5、NFR-204）。
//!
//! 与既有 `run_stability.sh` 的“外部按小时采样短命工作负载”不同，本夹具是
//! **单个长驻进程**：自建工作区、持续写入／查询／切换／清理，并按间隔自采样。
//! 这样 RSS 斜率归属于同一个 PID，NFR-204 的 RSS 斜率（5.5 小时，ADR-033）才有可解释对象。
//!
//! 采集字段（§11.5 可取得部分）：RSS、累计 CPU 与单核占比、线程数、句柄数、
//! 数据库与 WAL 大小、事件行数与最大 seq、事件缺失计数、failed 任务数、提交
//! 延迟、查询延迟、会话切换延迟。**写者队列深度与最老队列项年龄标记为 null**：
//! 本夹具直连 `Connection`，不经 `WriterService`（其 `m0_events` 原型表不是正式
//! 事件路径），不得以 0 冒充“队列为空”。该缺口按 T116 记录，待真实事件写者接线。
//!
//! 运行方式（由 `tests/stability/run_stability.sh` 驱动）：
//!
//! ```text
//! cargo test --locked -p whcode-tests --test stability_run -- --ignored --nocapture
//! ```
//!
//! 环境变量：`WHCODE_STABILITY_HOURS`（默认未设；设值按小时换算）、`WHCODE_STABILITY_SECONDS`
//! （默认 19 800 = 5.5 小时，ADR-033；冒烟可覆盖）、`WHCODE_STABILITY_INTERVAL_SECS`
//! （默认 300）、`WHCODE_STABILITY_DIR`（工作区）、`WHCODE_STABILITY_OUT`（JSONL 输出）。

#[path = "../fixtures/gen_dataset.rs"]
mod gen_dataset;

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use duckdb::{Connection, params};
use whcode_app::{DefaultPolicy, actor_with_all_capabilities, switch_session};
use whcode_platform::{current_rss_bytes, process_stats};
use whcode_store::{Direction, MAX_PAGE_SIZE, apply_migrations, events_page};

/// 初始数据集规模（与 NFR 夹具同构，V-101）。
const DATASET_EVENTS: usize = 100_000;
const DATASET_TASKS: usize = 10_000;
/// 每周期写入事件数（有界批）。
const BATCH_EVENTS: i64 = 20;
/// 周期间隔（毫秒）：40 事件/秒，5.5 小时约 79 万事件（ADR-033）。
const CYCLE_PAUSE_MS: u64 = 500;
/// 事件总量上限默认值；超过后删除最旧一批，避免长跑把磁盘写满（X4）。
/// 可用 `WHCODE_STABILITY_PURGE_THRESHOLD` / `WHCODE_STABILITY_PURGE_BATCH` 覆盖，
/// 以便冒烟时用小阈值验证清理路径。
const PURGE_THRESHOLD_DEFAULT: i64 = 400_000;
const PURGE_BATCH_DEFAULT: i64 = 100_000;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name).ok().map(PathBuf::from)
}

fn env_i64(name: &str, default: i64) -> i64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(default)
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    if values.len() == 1 {
        return values[0];
    }
    let rank = (p / 100.0) * (values.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let weight = rank - lower as f64;
    values[lower] + (values[upper] - values[lower]) * weight
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// 长跑采集器状态。
struct StabilityRun {
    conn: Connection,
    db_path: PathBuf,
    out: BufWriter<File>,
    started: Instant,
    sample_index: u64,
    next_sample_at: Duration,
    interval: Duration,
    base_events: i64,
    inserted: i64,
    deleted: i64,
    next_seq: i64,
    commit_ms: Vec<f64>,
    query_ms: Vec<f64>,
    switch_ms: Vec<f64>,
    rss_series: Vec<(f64, f64)>,
    last_cpu_seconds: f64,
    purge_threshold: i64,
    purge_batch: i64,
}

impl StabilityRun {
    fn open(dir: &Path, out_path: &Path) -> Self {
        fs::create_dir_all(dir).expect("创建长跑工作区");
        let db_path = dir.join("stability.duckdb");
        let mut conn = Connection::open(&db_path).expect("打开长跑数据库");
        apply_migrations(&conn).expect("应用迁移");

        let big = gen_dataset::generate(&mut conn, "S-BIG", DATASET_EVENTS, DATASET_TASKS);
        assert_eq!(big.events, DATASET_EVENTS, "初始事件规模必须完整");
        let small = gen_dataset::generate(&mut conn, "S-SMALL", 100, 10);
        assert_eq!(small.events, 100, "小会话初始事件规模必须完整");
        // 播种一个 failed 任务：证明 failed 计数是“有信号”的，而不是恒为零。
        conn.execute(
            "INSERT INTO tasks
             (id, session_id, title, stage, status, parent_id, acceptance_json, retry_count, updated_at)
             VALUES ('T-STABILITY-FAILED', 'S-BIG', '长跑失败任务', 'implement', 'failed',
                     NULL, CAST('{}' AS JSON)::VARIANT, 0, current_timestamp)",
            [],
        )
        .expect("播种 failed 任务");

        let base_events: i64 = conn
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("读取初始事件数");
        let max_seq: i64 = conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
                row.get(0)
            })
            .expect("读取初始游标");
        assert_eq!(
            base_events as usize,
            DATASET_EVENTS + 100,
            "初始事件数必须与两会话之和一致"
        );

        let out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(out_path)
            .expect("打开 JSONL 输出");
        let cpu = process_stats(std::process::id()).expect("初始进程资源");
        Self {
            conn,
            db_path,
            out: BufWriter::new(out),
            started: Instant::now(),
            sample_index: 0,
            next_sample_at: Duration::ZERO,
            interval: Duration::from_secs(env_u64("WHCODE_STABILITY_INTERVAL_SECS", 300).max(1)),
            base_events,
            inserted: 0,
            deleted: 0,
            next_seq: max_seq + 1,
            commit_ms: Vec::new(),
            query_ms: Vec::new(),
            switch_ms: Vec::new(),
            rss_series: Vec::new(),
            last_cpu_seconds: cpu.cpu_seconds,
            purge_threshold: env_i64("WHCODE_STABILITY_PURGE_THRESHOLD", PURGE_THRESHOLD_DEFAULT),
            purge_batch: env_i64("WHCODE_STABILITY_PURGE_BATCH", PURGE_BATCH_DEFAULT),
        }
    }

    fn write_batch(&mut self) -> f64 {
        let started = Instant::now();
        self.conn
            .execute_batch("BEGIN TRANSACTION")
            .expect("开始写入事务");
        for offset in 0..BATCH_EVENTS {
            let seq = self.next_seq + offset;
            self.conn
                .execute(
                    "INSERT INTO events
                     (seq, id, session_id, task_id, request_id, tool_call_id, kind,
                      payload_json, ts, persistence_level)
                     VALUES (?, ?, 'S-BIG', NULL, ?, NULL, 'StreamingChunk', CAST(? AS JSON)::VARIANT,
                             current_timestamp, 'redacted')",
                    params![
                        seq,
                        format!("E-STABILITY-{seq}"),
                        format!("R-STABILITY-{seq}"),
                        format!("{{\"pad\":\"{}\"}}", "x".repeat(32))
                    ],
                )
                .expect("写入事件");
        }
        self.conn.execute_batch("COMMIT").expect("提交事件批次");
        self.next_seq += BATCH_EVENTS;
        self.inserted += BATCH_EVENTS;
        started.elapsed().as_secs_f64() * 1000.0
    }

    fn query_page(&mut self) -> f64 {
        let started = Instant::now();
        let page = events_page(
            &self.conn,
            "S-BIG",
            Direction::Backward,
            None,
            MAX_PAGE_SIZE,
        )
        .expect("查询历史单页");
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        assert!(page.items.len() <= MAX_PAGE_SIZE, "单页必须有界");
        elapsed
    }

    fn switch(&mut self) -> f64 {
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("stability");
        let started = Instant::now();
        switch_session(&self.conn, &policy, &actor, "S-BIG").expect("切换会话");
        started.elapsed().as_secs_f64() * 1000.0
    }

    fn purge_if_needed(&mut self) {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM events WHERE session_id = 'S-BIG'",
                [],
                |row| row.get(0),
            )
            .expect("读取事件总量");
        if count <= self.purge_threshold {
            return;
        }
        let cutoff: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MIN(seq), 0) FROM (
                     SELECT seq FROM events WHERE session_id = 'S-BIG' ORDER BY seq DESC LIMIT ?)",
                params![self.purge_batch],
                |row| row.get(0),
            )
            .expect("计算清理阈值");
        let deleted = self
            .conn
            .execute(
                "DELETE FROM events WHERE session_id = 'S-BIG' AND seq < ?",
                params![cutoff],
            )
            .expect("清理最旧事件");
        self.deleted += i64::try_from(deleted).expect("删除行数可转换");
    }

    fn sample(&mut self) {
        self.sample_index += 1;
        let elapsed = self.started.elapsed();
        let rss = current_rss_bytes().expect("读取 RSS（零样本不充当达标）");
        let stats = process_stats(std::process::id()).expect("读取进程资源");
        let db_bytes = file_len(&self.db_path);
        let wal_bytes = file_len(&PathBuf::from(format!("{}.wal", self.db_path.display())));
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("读取事件行数");
        let max_seq: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |row| {
                row.get(0)
            })
            .expect("读取最大 seq");
        let failed_tasks: i64 = self
            .conn
            .query_row(
                "SELECT count(*) FROM tasks WHERE status = 'failed'",
                [],
                |row| row.get(0),
            )
            .expect("读取 failed 任务数");
        let expected = self.base_events + self.inserted - self.deleted;
        let missing = expected - count;
        let cpu_delta = (stats.cpu_seconds - self.last_cpu_seconds).max(0.0);
        self.last_cpu_seconds = stats.cpu_seconds;
        let cpu_percent = if self.sample_index == 1 {
            f64::NAN
        } else {
            cpu_delta / self.interval.as_secs_f64() * 100.0
        };
        let rss_mb = rss as f64 / (1024.0 * 1024.0);
        let elapsed_hours = elapsed.as_secs_f64() / 3_600.0;
        self.rss_series.push((elapsed_hours, rss_mb));
        let record = serde_json::json!({
            "kind": "sample",
            "sample": self.sample_index,
            "ts_epoch_s": epoch_seconds(),
            "elapsed_hours": (elapsed_hours * 10_000.0).round() / 10_000.0,
            "pid": std::process::id(),
            "rss_mb": (rss_mb * 100.0).round() / 100.0,
            "cpu_seconds_total": stats.cpu_seconds,
            "cpu_percent_one_core": if cpu_percent.is_nan() {
                serde_json::Value::Null
            } else {
                serde_json::json!((cpu_percent * 100.0).round() / 100.0)
            },
            "threads": stats.threads,
            "handles": stats.handles,
            "db_bytes": db_bytes,
            "wal_bytes": wal_bytes,
            "events": count,
            "max_seq": max_seq,
            "inserted_total": self.inserted,
            "deleted_total": self.deleted,
            "event_missing": missing,
            "failed_tasks": failed_tasks,
            "commit_p50_ms": round2(percentile(&mut self.commit_ms, 50.0)),
            "commit_p95_ms": round2(percentile(&mut self.commit_ms, 95.0)),
            "query_p50_ms": round2(percentile(&mut self.query_ms, 50.0)),
            "query_p95_ms": round2(percentile(&mut self.query_ms, 95.0)),
            "switch_p50_ms": round2(percentile(&mut self.switch_ms, 50.0)),
            "writer_queue_depth": serde_json::Value::Null,
            "oldest_queue_item_age_ms": serde_json::Value::Null,
            "unavailable": "writer_queue_metrics：夹具直连 Connection，不经 WriterService；\
                            null 不是 0（T116 已登记缺口）",
        });
        self.commit_ms.clear();
        self.query_ms.clear();
        self.switch_ms.clear();
        writeln!(self.out, "{record}").expect("追加采样行");
        self.out.flush().expect("采样行必须立即落盘");
        let cpu_display = if cpu_percent.is_nan() {
            "null（首样本无区间）".to_string()
        } else {
            format!("{cpu_percent:.2}")
        };
        println!(
            "样本 {}：{:.2} h RSS {:.2} MB CPU {}% 线程 {} 句柄 {} 事件 {} 缺失 {}",
            self.sample_index,
            elapsed_hours,
            rss_mb,
            cpu_display,
            stats.threads,
            stats.handles,
            count,
            missing
        );
    }

    fn write_summary(&mut self) {
        let elapsed = self.started.elapsed();
        let duration_hours = elapsed.as_secs_f64() / 3_600.0;
        let first = self.rss_series.first().copied();
        let last = self.rss_series.last().copied();
        let max_rss = self
            .rss_series
            .iter()
            .map(|(_, rss)| *rss)
            .fold(0.0_f64, f64::max);
        let slope = match (first, last) {
            (Some((first_h, first_mb)), Some((last_h, last_mb))) if last_h > first_h => {
                (last_mb - first_mb) / (last_h - first_h)
            }
            _ => f64::NAN,
        };
        let record = serde_json::json!({
            "kind": "summary",
            "samples": self.sample_index,
            "duration_hours": (duration_hours * 10_000.0).round() / 10_000.0,
            "rss_first_mb": first.map(|(_, rss)| round2(rss)),
            "rss_last_mb": last.map(|(_, rss)| round2(rss)),
            "rss_max_mb": round2(max_rss),
            "rss_slope_mb_per_h": if slope.is_nan() {
                serde_json::Value::Null
            } else {
                serde_json::json!((slope * 10_000.0).round() / 10_000.0)
            },
            "event_missing_last": self.base_events + self.inserted - self.deleted
                - self.current_event_count(),
            "failed_tasks": self.current_failed_tasks(),
            "note": "NFR-204 候选斜率；写者队列深度/最老队列项年龄未取得（见 sample.unavailable）",
        });
        writeln!(self.out, "{record}").expect("追加汇总行");
        self.out.flush().expect("汇总行必须立即落盘");
        println!("汇总：{record}");
    }

    fn current_event_count(&self) -> i64 {
        self.conn
            .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
            .expect("读取事件行数")
    }

    fn current_failed_tasks(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT count(*) FROM tasks WHERE status = 'failed'",
                [],
                |row| row.get(0),
            )
            .expect("读取 failed 任务数")
    }
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// T116 长跑入口：单进程持续负载 + 按间隔自采样，逐行 JSONL 落盘。
#[test]
#[ignore = "长跑夹具：由 run_stability.sh 或 stability-long.yml 驱动（默认 5.5 小时，ADR-033）"]
fn stability_run_collects_section_11_5_series() {
    let dir = env_path("WHCODE_STABILITY_DIR").unwrap_or_else(|| {
        std::env::temp_dir().join(format!("whcode-stability-{}", std::process::id()))
    });
    let out_path = env_path("WHCODE_STABILITY_OUT").unwrap_or_else(|| dir.join("timeseries.jsonl"));
    // 默认窗口 5.5 小时（19 800 s，ADR-033）；显式给小时数时仍按小时换算。
    let hours = env_u64("WHCODE_STABILITY_HOURS", 0);
    let default_seconds = if hours > 0 { hours * 3_600 } else { 19_800 };
    let total_seconds = env_u64("WHCODE_STABILITY_SECONDS", default_seconds);
    assert!(total_seconds > 0, "长跑时长必须大于 0");

    let mut run = StabilityRun::open(&dir, &out_path);
    println!(
        "长跑启动：{} 秒，间隔 {} 秒，输出 {}",
        total_seconds,
        run.interval.as_secs(),
        out_path.display()
    );
    let deadline = Instant::now() + Duration::from_secs(total_seconds);
    run.next_sample_at = run.interval;
    let mut cycles: u64 = 0;
    while Instant::now() < deadline {
        let commit = run.write_batch();
        run.commit_ms.push(commit);
        let query = run.query_page();
        run.query_ms.push(query);
        cycles += 1;
        if cycles.is_multiple_of(10) {
            let switch = run.switch();
            run.switch_ms.push(switch);
        }
        run.purge_if_needed();
        if run.started.elapsed() >= run.next_sample_at {
            run.sample();
            run.next_sample_at += run.interval;
        }
        std::thread::sleep(Duration::from_millis(CYCLE_PAUSE_MS));
    }
    // 时长不足一个采样间隔时也至少留一条样本，避免“零样本冒充完成”。
    if run.sample_index == 0 {
        run.sample();
    }
    run.write_summary();
    let missing = run.base_events + run.inserted - run.deleted - run.current_event_count();
    assert_eq!(missing, 0, "长跑结束时事件不得缺失（信号链完备）");
    assert!(
        run.sample_index >= 1,
        "至少一个可归因样本；零样本不得当作稳定"
    );
    assert_eq!(
        run.current_failed_tasks(),
        1,
        "播种的 failed 任务必须可计数（零值不得冒充无失败）"
    );
    assert!(out_path.exists(), "JSONL 必须落盘：{}", out_path.display());
}
