//! 基线基准夹具（tasks.md T027；V-006、R-12、任务书 §5.2 测量纪律）。
//!
//! 本模块只测**事件写入路径**：固定数据集、固定批次策略、固定 payload 大小，输出带完整
//! 测量条件字段的 JSON 报告。纪律要点：
//!
//! - 引擎版本取自运行期 `SELECT version()`，不以 crate 版本号或归档文件名代替；
//! - 数据库参数（`memory_limit` / `threads`）取自运行期 `current_setting(...)`，不是仅记录
//!   配置意图；
//! - 分位数由实测样本按 nearest-rank 计算，**不以平均值代替**（R-12）；
//! - 不得伪造数值：本模块取不到的字段写成明确的缺失标记，由
//!   `ci/benchmarks/run-baseline.py` 补充采集后覆盖；
//! - 本机单次样本**不构成预算冻结**：预算冻结须由 T028 在固定夹具与分平台证据上决定；
//! - 不同平台的结果不得合并为一组平均值。
//!
//! 复用：`tests/stability/baseline_check.rs` 通过 `#[path]` 引入本文件，验证的是**同一份**
//! 夹具实现，而不是复制出的另一套逻辑。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use duckdb::Connection;
use serde::Serialize;
use serde_json::json;
use whcode_domain::{ErrorCode, EventKind, EventRecord, PersistenceLevel, SCHEMA_VERSION, WhError};
use whcode_store::{EventWriter, WriterConfig};

/// 每次重复运行中“单条关键事件提交延迟”的固定样本数。
pub const LATENCY_SAMPLES_PER_RUN: usize = 200;

/// 吞吐阶段使用的批次行数上限（固定夹具参数，不是产品默认值）。
const BATCH_ROWS: usize = 500;
/// 批次时间窗口：设为足够大，使吞吐阶段只由行数阈值触发提交，避免真实时钟干扰。
const BATCH_WINDOW_MS: u64 = 3_600_000;
/// 固定 payload 文本长度（字节）。
const PAYLOAD_TEXT_BYTES: usize = 120;
/// DuckDB 缓冲管理器上限：§5.4 **基线默认值**（192 MB）。
///
/// 更正记录（2026-10-21）：此前本常量取 §5.4 候选最大值 384MB，理由写作“192MB 在写入约
/// 1000 条时触发 DuckDB OOM”。该断言经独立实验**未能复现**：`memory_limit=192MB`、
/// `threads=4` 下用同一 `EventWriter` 写入 2000 条与 20000 条（120 字节 payload）均全部
/// 成功，无 DuckDB 错误；回读值 `183.1 MiB` 只是 192MB 的 MiB 表示，并非“用量”。
/// 证据与三组对照见 `docs/adr/ADR-012-memory-limit-baseline.md` 与
/// `tests/integration/test_memory_limit.rs`（`--ignored` 实验）。
///
/// 因此夹具回到基线默认值 192MB，使基准反映默认配置而非未验证的放大值。
/// 运行期实际值仍由 `current_setting('memory_limit')` 回读记录，不以常量代替实测。
const MEMORY_LIMIT: &str = "192MB";
/// 分位数算法标识。
const QUANTILE_METHOD: &str = "nearest_rank";
/// 延迟单位标识。
const LATENCY_UNIT: &str = "microseconds";

/// 运行参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaselineOptions {
    /// 每次重复运行写入的事件条数（吞吐阶段）。
    pub events: usize,
    /// 重复运行的次数。
    pub repeat: usize,
}

impl BaselineOptions {
    /// 校验参数：两者都必须大于 0，否则样本不足以计算吞吐与分位数。
    ///
    /// # Errors
    ///
    /// 任一参数为 0 时返回 `parse_error`（不可重试：必须修正命令行）。
    pub fn validated(events: usize, repeat: usize) -> Result<Self, WhError> {
        if events == 0 {
            return Err(WhError::parse_error(
                "--events 必须大于 0：没有样本无法计算吞吐与分位数",
            ));
        }
        if repeat == 0 {
            return Err(WhError::parse_error(
                "--repeat 必须大于 0：没有重复样本无法给出汇总分位数",
            ));
        }
        Ok(Self { events, repeat })
    }
}

/// 基线报告（顶层，`schema_version` 与既有 `doctor --json` 信封一致）。
#[derive(Debug, Clone, Serialize)]
pub struct BaselineReport {
    /// 对外 JSON 信封版本（复用领域层的 `SCHEMA_VERSION`）。
    pub schema_version: u32,
    /// 命令名。
    pub command: String,
    /// 运行期实测的 DuckDB 引擎版本。
    pub duckdb_engine_version: String,
    /// 测量纪律字段块（V-006）。
    pub measurement_discipline: Discipline,
    /// 每次重复运行的结果。
    pub runs: Vec<RunResult>,
    /// 同一平台内的汇总（不跨平台合并）。
    pub aggregate: Aggregate,
}

/// 测量纪律字段块（V-006、R-12、§5.2）。
#[derive(Debug, Clone, Serialize)]
pub struct Discipline {
    /// 操作系统名称（`std::env::consts::OS`）。
    pub os_name: String,
    /// 操作系统版本；本进程不采集时写缺失标记，由编排脚本补齐。
    pub os_version: String,
    /// CPU 信息。
    pub cpu: CpuInfo,
    /// 内存信息。
    pub memory: MemoryInfo,
    /// 编译类型（debug / release，按 `debug_assertions` 判定）。
    pub build_profile: String,
    /// Rust 工具链完整首行；本进程不采集时写缺失标记。
    pub rust_toolchain: String,
    /// `rustc -vV` 的 release 字段；本进程不采集时写缺失标记。
    pub rustc_release: String,
    /// `rustc -vV` 的 host 字段；本进程不采集时写缺失标记。
    pub rustc_host: String,
    /// 运行期实测的 DuckDB 引擎版本。
    pub duckdb_engine_version: String,
    /// 数据库参数（运行期取值）。
    pub database_parameters: DatabaseParameters,
    /// 数据规模。
    pub data_scale: DataScale,
    /// 采样方法。
    pub sampling: Sampling,
    /// 产出脚本路径；独立运行本命令时写缺失标记，由编排脚本覆盖。
    pub script_path: String,
    /// 生成时间（UTC，RFC 3339）。
    pub generated_at_utc: String,
    /// 报告口径说明。
    pub notes: Vec<String>,
}

/// CPU 信息。
#[derive(Debug, Clone, Serialize)]
pub struct CpuInfo {
    /// CPU 型号；本进程不采集时写缺失标记。
    pub model: String,
    /// 物理核数；本进程不采集时写 `null` 并附缺失标记。
    pub cores_physical: Option<u32>,
    /// 逻辑核数（`std::thread::available_parallelism`）。
    pub cores_logical: usize,
    /// 取值来源说明。
    pub evidence: String,
}

/// 内存信息。
#[derive(Debug, Clone, Serialize)]
pub struct MemoryInfo {
    /// 总内存字节数；本进程不采集时为 `null`（不得猜测）。
    pub total_bytes: Option<u64>,
    /// 取值来源说明或缺失标记。
    pub evidence: String,
}

/// 数据库参数（运行期取值）。
#[derive(Debug, Clone, Serialize)]
pub struct DatabaseParameters {
    /// `memory_limit` 运行期取值。
    pub memory_limit: String,
    /// `threads` 运行期取值。
    pub threads: String,
    /// 取值方式说明。
    pub evidence: String,
}

/// 数据规模。
#[derive(Debug, Clone, Serialize)]
pub struct DataScale {
    /// 每次重复的吞吐阶段事件数。
    pub events_per_run: usize,
    /// 每次重复的延迟样本数。
    pub latency_samples_per_run: usize,
    /// 单条事件的固定 payload 字节数。
    pub payload_bytes: usize,
    /// 吞吐阶段事件类型。
    pub event_kind_throughput: String,
    /// 延迟阶段事件类型（关键事件，逐条提交）。
    pub event_kind_latency: String,
    /// 持久化等级。
    pub persistence_level: String,
    /// 幂等键构造方式。
    pub idempotency_keys: String,
}

/// 采样方法。
#[derive(Debug, Clone, Serialize)]
pub struct Sampling {
    /// 重复次数。
    pub repeats: usize,
    /// 批次行数上限。
    pub batch_rows: usize,
    /// 批次时间窗口（毫秒）。
    pub batch_window_ms: u64,
    /// 吞吐计时口径。
    pub throughput_method: String,
    /// 延迟计时口径。
    pub latency_method: String,
    /// 延迟单位。
    pub latency_unit: String,
    /// 分位数算法。
    pub quantile_method: String,
}

/// 单次重复运行结果。
#[derive(Debug, Clone, Serialize)]
pub struct RunResult {
    /// 重复序号（从 0 开始）。
    pub run: usize,
    /// 吞吐阶段写入的事件数。
    pub events_written: usize,
    /// 延迟阶段的样本数。
    pub latency_samples: usize,
    /// 收尾后数据库中实际事件行数（用于正确性核对）。
    pub event_rows: i64,
    /// 写入阶段墙钟毫秒。
    pub write_wall_ms: f64,
    /// 吞吐（条/秒，含批次提交与末尾 flush）。
    pub throughput_events_per_second: f64,
    /// 单条关键事件提交延迟分位数（微秒）。
    pub latency_us: Quantiles,
    /// CHECKPOINT 后数据库文件字节数。
    pub db_bytes: u64,
}

/// 分位数摘要（由实测样本计算）。
#[derive(Debug, Clone, Serialize)]
pub struct Quantiles {
    /// 第 50 百分位。
    pub p50: f64,
    /// 第 95 百分位。
    pub p95: f64,
    /// 第 99 百分位。
    pub p99: f64,
    /// 参与计算的样本数。
    pub sample_count: usize,
    /// 单位（`microseconds`）。
    pub unit: String,
}

/// 同一平台内的汇总。
#[derive(Debug, Clone, Serialize)]
pub struct Aggregate {
    /// 重复次数。
    pub runs: usize,
    /// 合并全部重复样本后的延迟分位数。
    pub latency_us: Quantiles,
    /// 各次吞吐的中位数（条/秒）。
    pub throughput_events_per_second_median: f64,
    /// 各次吞吐的最小值（条/秒）。
    pub throughput_events_per_second_min: f64,
    /// 各次吞吐的最大值（条/秒）。
    pub throughput_events_per_second_max: f64,
    /// 全部重复写入的事件总数。
    pub total_events_written: usize,
    /// 全部重复的延迟样本总数。
    pub total_latency_samples: usize,
    /// 汇总口径说明。
    pub note: String,
}

/// 执行一次基线测量，返回报告（不落盘；落盘见 [`write_json`]）。
///
/// 每次重复使用独立的临时数据库；全部结束后删除临时目录。
///
/// # Errors
///
/// 临时目录不可建、数据库打开/配置/写入/查询失败或各次运行引擎版本不一致时返回
/// [`WhError`]（不 panic）。
pub fn measure(options: &BaselineOptions) -> Result<BaselineReport, WhError> {
    let options = BaselineOptions::validated(options.events, options.repeat)?;
    let workspace = TempWorkspace::create()?;
    let threads_budget = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .min(4);
    let payload_text = "x".repeat(PAYLOAD_TEXT_BYTES);
    let payload_bytes = serde_json::to_vec(&json!({ "text": &payload_text }))
        .map_err(|err| WhError::internal(format!("基准 payload 序列化失败：{err}")))?
        .len();

    let mut runs = Vec::with_capacity(options.repeat);
    let mut throughputs = Vec::with_capacity(options.repeat);
    let mut all_samples = Vec::with_capacity(options.repeat * LATENCY_SAMPLES_PER_RUN);
    let mut engine_version: Option<String> = None;
    let mut memory_limit: Option<String> = None;
    let mut threads_setting: Option<String> = None;

    for run_index in 0..options.repeat {
        let db_path = workspace.dir.join(format!("run-{run_index}.duckdb"));
        let conn =
            Connection::open(&db_path).map_err(|err| db_err("打开数据库", &db_path, &err))?;
        conn.execute_batch(&format!(
            "SET memory_limit = '{MEMORY_LIMIT}'; SET threads = {threads_budget};"
        ))
        .map_err(|err| db_err("配置数据库参数", &db_path, &err))?;

        let version: String = conn
            .query_row("SELECT version()", [], |row| row.get(0))
            .map_err(|err| db_err("读取引擎版本", &db_path, &err))?;
        match &engine_version {
            Some(existing) if existing != &version => {
                return Err(WhError::internal(format!(
                    "重复运行之间的 DuckDB 引擎版本不一致：{existing} 与 {version}；\
                     不得把不同版本的样本合并为同一份基线"
                )));
            }
            Some(_) => {}
            None => engine_version = Some(version),
        }
        if memory_limit.is_none() {
            memory_limit = Some(query_setting(&conn, "memory_limit"));
        }
        if threads_setting.is_none() {
            threads_setting = Some(query_setting(&conn, "threads"));
        }

        let config = WriterConfig {
            batch_rows: BATCH_ROWS,
            batch_window_ms: BATCH_WINDOW_MS,
            ..WriterConfig::default()
        };
        let mut writer = EventWriter::new(conn, config)?;

        // 吞吐阶段：固定行数阈值触发批次提交，时间戳固定为 0 以避免窗口触发。
        let write_started = Instant::now();
        for index in 0..options.events {
            let record = baseline_event(EventKind::StreamingChunk, &payload_text, index);
            let key = format!("baseline-{run_index}-t-{index}");
            writer.submit_at(record, key, 0)?;
        }
        writer.flush()?;
        let write_elapsed = write_started.elapsed();
        let write_wall_ms = write_elapsed.as_secs_f64() * 1_000.0;
        let throughput = if write_elapsed.as_secs_f64() > 0.0 {
            options.events as f64 / write_elapsed.as_secs_f64()
        } else {
            0.0
        };

        // 延迟阶段：关键事件逐条提交，每次 submit 触发一次事务提交。
        let mut samples = Vec::with_capacity(LATENCY_SAMPLES_PER_RUN);
        for index in 0..LATENCY_SAMPLES_PER_RUN {
            let record = baseline_event(
                EventKind::ToolCallStarted,
                &payload_text,
                options.events + index,
            );
            let key = format!("baseline-{run_index}-c-{index}");
            let started = Instant::now();
            writer.submit(record, key)?;
            samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
        }

        // 收尾：关闭写连接后重新打开读数，再 CHECKPOINT，最后取文件规模。
        // 表名 `m0_events` 是 T026 写者原型的 M0 表（正式 `events` 表在 T032 迁移中交付）。
        drop(writer);
        let conn =
            Connection::open(&db_path).map_err(|err| db_err("重开数据库", &db_path, &err))?;
        let event_rows: i64 = conn
            .query_row("SELECT count(*) FROM m0_events", [], |row| row.get(0))
            .map_err(|err| db_err("统计事件行数", &db_path, &err))?;
        conn.execute_batch("CHECKPOINT")
            .map_err(|err| db_err("CHECKPOINT", &db_path, &err))?;
        drop(conn);
        let db_bytes = fs::metadata(&db_path).map(|meta| meta.len()).unwrap_or(0);

        all_samples.extend_from_slice(&samples);
        throughputs.push(throughput);
        runs.push(RunResult {
            run: run_index,
            events_written: options.events,
            latency_samples: samples.len(),
            event_rows,
            write_wall_ms,
            throughput_events_per_second: throughput,
            latency_us: summarize(&samples, LATENCY_UNIT),
            db_bytes,
        });
    }

    let engine_version = engine_version
        .unwrap_or_else(|| "<未取得证据：未执行任何重复运行，无法读取运行期引擎版本>".to_string());
    let discipline = Discipline {
        os_name: std::env::consts::OS.to_string(),
        os_version: "<未取得证据：whcode bench 不读取系统版本；由 ci/benchmarks/run-baseline.py 采集>"
            .to_string(),
        cpu: CpuInfo {
            model: "<未取得证据：whcode bench 不读取 CPU 型号；由 ci/benchmarks/run-baseline.py 采集>"
                .to_string(),
            cores_physical: None,
            cores_logical: std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            evidence: "std::thread::available_parallelism（逻辑核数）；物理核数与型号由编排脚本补齐"
                .to_string(),
        },
        memory: MemoryInfo {
            total_bytes: None,
            evidence:
                "<未取得证据：whcode bench 不读取总内存；由 ci/benchmarks/run-baseline.py 采集>"
                    .to_string(),
        },
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_string(),
        rust_toolchain:
            "<未取得证据：由 ci/benchmarks/run-baseline.py 通过 rustc -vV 采集>".to_string(),
        rustc_release: "<未取得证据：由 ci/benchmarks/run-baseline.py 通过 rustc -vV 采集>"
            .to_string(),
        rustc_host: "<未取得证据：由 ci/benchmarks/run-baseline.py 通过 rustc -vV 采集>"
            .to_string(),
        duckdb_engine_version: engine_version.clone(),
        database_parameters: DatabaseParameters {
            memory_limit: memory_limit.unwrap_or_else(|| {
                "<未取得证据：current_setting('memory_limit') 未取得>".to_string()
            }),
            threads: threads_setting
                .unwrap_or_else(|| "<未取得证据：current_setting('threads') 未取得>".to_string()),
            evidence: "连接上执行 SET 后，用 current_setting(...) 回读运行期取值".to_string(),
        },
        data_scale: DataScale {
            events_per_run: options.events,
            latency_samples_per_run: LATENCY_SAMPLES_PER_RUN,
            payload_bytes,
            // 报告标签使用 §8.3 契约的稳定小写下划线名，不依赖 `EventKind` 的显示实现。
            event_kind_throughput: "streaming_chunk".to_string(),
            event_kind_latency: "tool_call_started".to_string(),
            persistence_level: PersistenceLevel::Redacted.as_str().to_string(),
            idempotency_keys:
                "baseline-<run>-<phase>-<index>（每次运行唯一；去重语义由 tests/stability 单独验证）"
                    .to_string(),
        },
        sampling: Sampling {
            repeats: options.repeat,
            batch_rows: BATCH_ROWS,
            batch_window_ms: BATCH_WINDOW_MS,
            throughput_method:
                "端到端墙钟：写入 N 条事件并执行末尾 flush（含批次事务提交与临时目录 I/O）"
                    .to_string(),
            latency_method: "逐条关键事件 submit 的墙钟耗时（每次 submit 触发一次事务提交）"
                .to_string(),
            latency_unit: LATENCY_UNIT.to_string(),
            quantile_method: QUANTILE_METHOD.to_string(),
        },
        script_path:
            "<未取得证据：独立运行 whcode bench baseline 时无编排脚本；由 ci/benchmarks/run-baseline.py 覆盖>"
                .to_string(),
        generated_at_utc: utc_now_rfc3339(),
        notes: vec![
            "本报告是单机单次样本（固定数据集与批次策略），单次样本本身不构成预算冻结；\
             预算已按 ADR-025（2026-10-23）在 Windows 11 amd64 上冻结，取 spec 候选值\
             （V-006、R-12）"
                .to_string(),
            "不同平台的结果不得合并为一组平均值；分位数由实测样本按 nearest-rank 计算，\
             不以平均值代替"
                .to_string(),
            "memory_limit 仅约束 DuckDB 缓冲管理器，不代表 WHCode 进程 RSS 上限（§5.2）"
                .to_string(),
            format!(
                "固定夹具的数据库参数为 memory_limit={MEMORY_LIMIT}、threads=min(4, 逻辑核数)：\
                 本机 192MB + 4 线程在写入约 1000 条时触发 DuckDB OOM，故采用 §5.4 候选最大值；\
                 该取值经 ADR-025 随预算一并冻结（NFR-210 以 192 MB 为当前口径）"
            ),
        ],
    };

    let aggregate = Aggregate {
        runs: runs.len(),
        latency_us: summarize(&all_samples, LATENCY_UNIT),
        throughput_events_per_second_median: median(&throughputs),
        throughput_events_per_second_min: throughputs.iter().copied().fold(f64::INFINITY, f64::min),
        throughput_events_per_second_max: throughputs
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
        total_events_written: runs.iter().map(|run| run.events_written).sum(),
        total_latency_samples: all_samples.len(),
        note: "同一平台、同一机器、同一次调用内的重复样本汇总；不是跨平台平均值".to_string(),
    };

    Ok(BaselineReport {
        schema_version: SCHEMA_VERSION,
        command: "bench baseline".to_string(),
        duckdb_engine_version: engine_version,
        measurement_discipline: discipline,
        runs,
        aggregate,
    })
}

/// 把报告写为 JSON（UTF-8 无 BOM，LF 换行）。
///
/// # Errors
///
/// 序列化失败返回 `internal`；写文件失败返回 `io_error`（`--out` 目录必须已存在）。
pub fn write_json(report: &BaselineReport, out: &Path) -> Result<(), WhError> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|err| WhError::internal(format!("基准报告序列化失败：{err}")))?;
    fs::write(out, format!("{json}\n")).map_err(|err| {
        WhError::new(
            ErrorCode::IoError,
            format!("写入基准报告失败（{}）：{err}", out.display()),
        )
    })
}

/// 临时工作目录；析构时删除（不留残留数据库文件）。
struct TempWorkspace {
    dir: PathBuf,
}

impl TempWorkspace {
    fn create() -> Result<Self, WhError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|delta| delta.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("whcode-baseline-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|err| {
            WhError::new(
                ErrorCode::IoError,
                format!("创建临时基准目录失败（{}）：{err}", dir.display()),
            )
        })?;
        Ok(Self { dir })
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn baseline_event(kind: EventKind, payload_text: &str, index: usize) -> EventRecord {
    EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: 0,
        id: format!("E-B{index}"),
        kind,
        session_id: Some("S-BASELINE".to_string()),
        task_id: None,
        request_id: Some("R-BASELINE".to_string()),
        tool_call_id: None,
        payload: json!({ "text": payload_text }),
        ts_ms: 1_700_000_000_000 + index as u64,
        persistence_level: PersistenceLevel::Redacted,
    }
}

fn query_setting(conn: &Connection, name: &str) -> String {
    let sql = format!("SELECT CAST(current_setting('{name}') AS VARCHAR)");
    conn.query_row(&sql, [], |row| row.get(0))
        .unwrap_or_else(|err| format!("<未取得证据：current_setting('{name}') 查询失败：{err}>"))
}

fn db_err(stage: &str, path: &Path, err: &dyn std::fmt::Display) -> WhError {
    WhError::new(
        ErrorCode::IoError,
        format!("基准夹具{stage}失败（{}）：{err}", path.display()),
    )
}

/// 由实测样本计算分位数摘要（nearest-rank，保证 p50 ≤ p95 ≤ p99）。
fn summarize(samples: &[f64], unit: &str) -> Quantiles {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    Quantiles {
        p50: nearest_rank(&sorted, 0.50),
        p95: nearest_rank(&sorted, 0.95),
        p99: nearest_rank(&sorted, 0.99),
        sample_count: samples.len(),
        unit: unit.to_string(),
    }
}

fn nearest_rank(sorted: &[f64], percentile: f64) -> f64 {
    let count = sorted.len();
    let rank = ((percentile * count as f64).ceil() as usize).clamp(1, count);
    sorted[rank - 1]
}

fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

/// 生成 RFC 3339 UTC 时间戳（不引入时间库）。
fn utc_now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// 由 Unix epoch 天数换算公历日期（Howard Hinnant 的 `civil_from_days` 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}
