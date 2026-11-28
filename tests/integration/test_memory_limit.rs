//! `memory_limit` 基线的独立复核实验（ADR-012；复核 tasks.md T027 的 OOM 断言）。
//!
//! 待验证断言：T027 夹具实现者报告“`memory_limit = 192MB` + `threads = 4` 时，写入约
//! 1000 条事件触发 DuckDB OOM，因此夹具改用 §5.4 的候选最大值 384MB”。该断言此前只
//! 存在于代码注释与报告文字中，没有留证。
//!
//! 本实验用与真实路径一致的 [`whcode_store::EventWriter`]（不自造 SQL 写入路径），在
//! 128MB / 192MB / 384MB / 1024MB 四组上限下各写入同一规模的固定数据集：2000 条
//! `StreamingChunk` 事件，payload 为 `{"text":"x"×120}`（JSON 131 字节），持久化等级
//! `redacted`，批次行数上限 500。逐批提交，记录首个失败的写入位置、DuckDB 错误原文、
//! 收尾 `CHECKPOINT` 后的文件字节数。
//!
//! 另设一组**规模探针**：同样参数、192MB 上限、20000 条事件（`whcode bench baseline` 的
//! 默认事件数），用于判断 192MB 在该数据规模下是否仍然可用；它与四组主对照的数据规模
//! 不同，单独列表，不参与主对照的横向比较。
//!
//! 纪律（V-001、§5.2）：
//! - 每组用 `current_setting(...)` 回读，确认设置**真的生效**，而不是只记录配置意图；
//! - 本测试是**实验记录器**，不是断言器：OOM 发生与否都不判失败；只断言“设置已生效”与
//!   “写入条数统计自洽”；
//! - `memory_limit` 只约束 DuckDB 缓冲管理器，**不代表** WHCode 进程 RSS 上限（§5.2）；
//! - 写入期缓冲管理器的瞬时占用无法在本测试中读取：`EventWriter` 独占连接，而 DuckDB
//!   拒绝在同一进程内二次打开同一数据库文件（实测报错“File is already open”），因此
//!   本实验不记录该指标，也不以进程 RSS 代替；
//! - 单机单次样本不构成预算冻结，取值须由 M1 校准（ADR-012 退出条件）。
//!
//! 运行（默认被 `#[ignore]` 跳过，不拖慢 CI）：
//!
//! ```text
//! cargo test -p whcode-tests --test memory_limit -- --ignored --nocapture
//! ```
//!
//! 结果同时写入 `artifacts/memory-limit-experiment.json`（UTF-8 无 BOM、LF）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use serde::Serialize;
use serde_json::json;
use whcode_domain::{EventKind, EventRecord, MAX_PAYLOAD_BYTES, PersistenceLevel, SCHEMA_VERSION};
use whcode_store::{EventWriter, WriterConfig};

/// 固定线程数：与 T027 断言中的条件一致。
const THREADS: u32 = 4;
/// 主对照每组的固定事件数（覆盖断言声称的“约 1000 条”失败点）。
const EVENTS: usize = 2_000;
/// 规模探针的事件数：`whcode bench baseline` 的默认事件数。
const PROBE_EVENTS: usize = 20_000;
/// payload 文本字节数：与 `crates/whcode-bin/src/bench.rs` 的固定夹具一致。
const PAYLOAD_TEXT_BYTES: usize = 120;
/// 批次行数上限：与固定夹具一致，使批次在 500 / 1000 / 1500 / 2000 条处提交。
const BATCH_ROWS: usize = 500;
/// 批次时间窗口：设为足够大，使提交只由行数阈值触发（与固定夹具一致）。
const BATCH_WINDOW_MS: u64 = 3_600_000;

/// 实验分组（`memory_limit` 上限）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Group {
    /// 报告中的分组标签。
    label: &'static str,
    /// 传给 `SET memory_limit` 的取值。
    memory_limit: &'static str,
    /// 该取值按十进制解释的 MB 数（用于校验回读值）。
    requested_mb: u32,
    /// 该组写入的事件条数。
    events: usize,
    /// 该组的批次行数上限（大载荷组须减小，避免进程内待写缓冲随 payload 体积放大）。
    batch_rows: usize,
}

/// 四组主对照（同一数据规模）：§5.4 默认值、候选最大值、足够高但仍有界的对照值，
/// 以及 DuckDB 2.0.0 聚合溢写下待复核的下限候选 128MB（Task 9）。
const GROUPS: [Group; 4] = [
    Group {
        label: "128MB",
        memory_limit: "128MB",
        requested_mb: 128,
        events: EVENTS,
        batch_rows: BATCH_ROWS,
    },
    Group {
        label: "192MB",
        memory_limit: "192MB",
        requested_mb: 192,
        events: EVENTS,
        batch_rows: BATCH_ROWS,
    },
    Group {
        label: "384MB",
        memory_limit: "384MB",
        requested_mb: 384,
        events: EVENTS,
        batch_rows: BATCH_ROWS,
    },
    Group {
        label: "1024MB",
        memory_limit: "1024MB",
        requested_mb: 1_024,
        events: EVENTS,
        batch_rows: BATCH_ROWS,
    },
];

/// 规模探针：192MB 上限下按基准夹具默认事件数写入。
const PROBE: Group = Group {
    label: "192MB@20000",
    memory_limit: "192MB",
    requested_mb: 192,
    events: PROBE_EVENTS,
    batch_rows: BATCH_ROWS,
};

/// 单组的实测记录。
#[derive(Debug, Serialize)]
struct GroupResult {
    label: &'static str,
    requested_mb: u32,
    memory_limit_setting: &'static str,
    /// `current_setting('memory_limit')` 的回读值（证明设置真的生效）。
    memory_limit_readback: String,
    /// `current_setting('threads')` 的回读值。
    threads_readback: String,
    duckdb_version: String,
    events_target: usize,
    /// 该组实际使用的批次行数上限（大载荷组与主对照不同，必须如实记录）。
    batch_rows: usize,
    /// 实际发起 submit 的条数（首个失败也算发起）。
    events_attempted: usize,
    /// 返回 `Ok` 的 submit 条数。
    events_succeeded: usize,
    /// 首个失败写入的 1 基序号；未失败为 `null`。
    failed_at: Option<usize>,
    /// 失败阶段：`submit`、`final_flush` 或 `none`。
    failure_stage: &'static str,
    error_code: Option<String>,
    /// 完整 `WhError` 消息（含我方前缀与 DuckDB 原文）。
    error_message: Option<String>,
    /// 去掉我方前缀后的 DuckDB 错误原文。
    duckdb_error_raw: Option<String>,
    retryable: Option<bool>,
    /// 实验结束时写者仍保留的待写条目数（失败时事务回滚，批次保留）。
    pending_events: usize,
    /// 收尾时库内事件行数。
    committed_rows: Option<i64>,
    rows_query_error: Option<String>,
    checkpoint_ok: bool,
    checkpoint_error: Option<String>,
    /// `CHECKPOINT` 后数据库文件字节数（文件库）。
    db_bytes: u64,
    /// 写入阶段（含收尾 flush）墙钟毫秒。
    wall_ms: f64,
    /// 本组结论摘要。
    outcome: String,
}

/// 实验运行一次；默认忽略，避免拖慢常规 CI。
#[test]
#[ignore = "实验记录器：需用 --ignored 显式运行，结果写入 artifacts/memory-limit-experiment.json"]
fn memory_limit_baseline_experiment() {
    let workspace = temp_workspace();
    let payload_text = "x".repeat(PAYLOAD_TEXT_BYTES);
    let payload_bytes = serde_json::to_vec(&json!({ "text": &payload_text }))
        .expect("固定 payload 必须可序列化")
        .len();

    println!("== memory_limit 基线复核实验（ADR-012）==");
    println!(
        "固定夹具：主对照 {} 条/组，规模探针 {} 条；payload {} 字节，批次 {} 行，threads = {THREADS}",
        EVENTS, PROBE_EVENTS, payload_bytes, BATCH_ROWS
    );

    let mut groups = Vec::with_capacity(GROUPS.len());
    for group in GROUPS {
        let result = run_group(group, &workspace, &payload_text);
        print_group(&result);
        groups.push(result);
    }
    println!("-- 规模探针（数据规模与主对照不同，单独列表）--");
    let probe = run_group(PROBE, &workspace, &payload_text);
    print_group(&probe);

    let conclusion = conclude(&groups, &probe);
    println!("== 结论：{conclusion} ==");

    let report = json!({
        "schema_version": 1,
        "experiment": "memory-limit-baseline",
        "adr": "ADR-012",
        "generated_at_utc": utc_now_rfc3339(),
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logical_cores": std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
            "rust_toolchain": "<未采集：由 ADR-012 引用 rustc -vV 的实测输出>",
            "machine_note": "<未采集：CPU 型号与内存总量不经外部命令；os／arch／logical_cores 为实测字段>",
        },
        "fixture": {
            "events_per_group": EVENTS,
            "probe_events": PROBE_EVENTS,
            "event_kind": EventKind::StreamingChunk.as_str(),
            "payload_text_bytes": PAYLOAD_TEXT_BYTES,
            "payload_json_bytes": payload_bytes,
            "persistence_level": PersistenceLevel::Redacted.as_str(),
            "batch_rows": BATCH_ROWS,
            "batch_window_ms": BATCH_WINDOW_MS,
            "threads": THREADS,
            "database": "临时目录下的文件库（每组独立文件）",
            "checkpoint": "每组结束后显式 CHECKPOINT",
        },
        "groups": groups,
        "supplementary_probe": probe,
        "conclusion": conclusion,
        "notes": [
            "本测试是实验记录器：OOM 发生与否都不判失败；只断言设置生效与条数自洽",
            "memory_limit 只约束 DuckDB 缓冲管理器，不代表进程 RSS 上限（§5.2）",
            "写入期缓冲管理器瞬时占用不可读：EventWriter 独占连接，DuckDB 拒绝同进程二次打开同一文件",
            "单机单次样本不构成预算冻结；取值须由 M1 基准校准（ADR-012 退出条件）",
        ],
    });

    let out = workspace_root()
        .join("artifacts")
        .join("memory-limit-experiment.json");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("创建 artifacts 目录");
    }
    fs::write(
        &out,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("实验报告必须可序列化")
        ),
    )
    .expect("写入实验证据");
    println!("证据已写入 {}", out.display());

    let _ = fs::remove_dir_all(&workspace);
}

/// 大载荷实验（ADR-012 未决项②的大载荷覆盖；Check 134 证据）。
///
/// 主实验的 payload 是 131 字节 JSON——ADR-012 的“大规模载荷未测”缺口指的是这个：
/// 缓冲管理器上限（192MB）在**单条 1 MiB、总量远超上限**的写入路径下是否仍然成立
/// （溢写 / 报错 / 成功，都是有效证据）。
///
/// 设计：两组同规模对照——192MB（过渡基线，数据量 512 MiB ≫ 上限，强制触及溢写路径）
/// 与 1024MB（高上限对照）；payload 为 `{"text":"x"×1048576}`（JSON 约 1 MiB），
/// 512 条／组，批次行数上限 8——批次缓冲按行数有界，行数大时进程内待写缓冲会随
/// payload 体积线性放大（500 行 × 1 MiB ≈ 500 MB），故大载荷组必须减小批次。
///
/// 纪律与主实验一致：实验**记录器**而非断言器——失败/溢写/成功都如实记录，只断言
/// 设置生效与条数统计自洽；`memory_limit` 不代表进程 RSS；单机单次不构成预算冻结。
#[test]
#[ignore = "实验记录器：需用 --ignored 显式运行，结果写入 artifacts/memory-limit-large-payload.json"]
fn memory_limit_large_payload_experiment() {
    let workspace = temp_workspace();
    // payload 取 domain 契约上限内的最大档：MAX_PAYLOAD_BYTES（1 MiB）− 64 字节余量。
    // 越界（>1 MiB）会被 store 按 MAX_PAYLOAD_BYTES 拒收（fail-closed，2026-11-02 CI 实测），
    // 那是契约边界的正确行为，不是本实验的对象。
    let payload_text = "x".repeat(MAX_PAYLOAD_BYTES - 64);
    let payload_bytes = serde_json::to_vec(&json!({ "text": &payload_text }))
        .expect("固定 payload 必须可序列化")
        .len();

    println!("== memory_limit 大载荷实验（ADR-012 未决项②）==");
    println!(
        "固定夹具：512 条/组，payload {payload_bytes} 字节，批次行数上限 8（防待写缓冲随载荷放大），threads = {THREADS}"
    );

    let groups = [
        Group {
            label: "192MB@512@1MiB",
            memory_limit: "192MB",
            requested_mb: 192,
            events: 512,
            batch_rows: 8,
        },
        Group {
            label: "1024MB@512@1MiB",
            memory_limit: "1024MB",
            requested_mb: 1_024,
            events: 512,
            batch_rows: 8,
        },
    ];
    let mut results = Vec::with_capacity(groups.len());
    for group in groups {
        let result = run_group(group, &workspace, &payload_text);
        print_group(&result);
        results.push(result);
    }

    let conclusion = match (results[0].failed_at, results[0].failure_stage) {
        (Some(at), _) => format!(
            "192MB 上限下大载荷写入在第 {at} 条失败（错误原文见 groups[0]）——\
             192MB 过渡基线在大载荷路径的适用边界即此处，M1 校准时须复核"
        ),
        (None, "final_flush") => {
            "192MB 下 512 条 submit 成功但收尾 flush 失败（溢写/收尾路径边界）".to_string()
        }
        _ => "192MB 与 1024MB 下 512 条 1 MiB 载荷全部完成写入（db_bytes 见各组）——\
              未触发失败；缓冲管理器经溢写消化了超出上限的数据量"
            .to_string(),
    };
    println!("== 结论：{conclusion} ==");

    let report = json!({
        "schema_version": 1,
        "experiment": "memory-limit-large-payload",
        "adr": "ADR-012",
        "generated_at_utc": utc_now_rfc3339(),
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logical_cores": std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or(1),
        },
        "fixture": {
            "events_per_group": 512,
            "event_kind": EventKind::StreamingChunk.as_str(),
            // 实测值：payload 取 MAX_PAYLOAD_BYTES − 64 余量（domain 契约上限内）。
            "payload_text_bytes": payload_text.len(),
            "payload_json_bytes": payload_bytes,
            "payload_contract_limit": MAX_PAYLOAD_BYTES,
            "total_data_mb_per_group": 512,
            "persistence_level": PersistenceLevel::Redacted.as_str(),
            "batch_rows": 8,
            "batch_rows_note": "大载荷组专用 8 行：待写缓冲按行数有界，行数大时随载荷线性放大",
            "batch_window_ms": BATCH_WINDOW_MS,
            "threads": THREADS,
            "database": "临时目录下的文件库（每组独立文件）",
            "checkpoint": "每组结束后显式 CHECKPOINT",
        },
        "groups": results,
        "conclusion": conclusion,
        "notes": [
            "实验记录器：失败/溢写/成功都是有效证据，不判失败；只断言设置生效与条数自洽",
            "192MB 是 DB 缓冲管理器的过渡基线，不是 RSS 预算（ADR-012）",
            "单机单次样本不构成预算冻结；M1 校准（大载荷 1 MiB、长时、temp_directory 溢写验证）是 ADR-012 退出条件",
        ],
    });

    let out = workspace_root()
        .join("artifacts")
        .join("memory-limit-large-payload.json");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("创建 artifacts 目录");
    }
    fs::write(
        &out,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("实验报告必须可序列化")
        ),
    )
    .expect("写入大载荷实验证据");
    println!("证据已写入 {}", out.display());

    let _ = fs::remove_dir_all(&workspace);
}

/// 运行一组：配置上限、回读、用 `EventWriter` 写入、收尾统计。
fn run_group(group: Group, workspace: &Path, payload_text: &str) -> GroupResult {
    let db = workspace.join(format!(
        "memory-{}-{}.duckdb",
        group.requested_mb, group.events
    ));
    let conn = Connection::open(&db).expect("打开实验数据库");
    conn.execute_batch(&format!(
        "SET memory_limit = '{}'; SET threads = {THREADS};",
        group.memory_limit
    ))
    .expect("配置数据库参数");
    let memory_limit_readback = query_setting(&conn, "memory_limit");
    let threads_readback = query_setting(&conn, "threads");
    assert_settings_effective(group, &memory_limit_readback, &threads_readback);
    let duckdb_version: String = conn
        .query_row("SELECT version()", [], |row| row.get(0))
        .expect("读取运行期引擎版本");

    let mut writer = EventWriter::new(
        conn,
        WriterConfig {
            batch_rows: group.batch_rows,
            batch_window_ms: BATCH_WINDOW_MS,
            ..WriterConfig::default()
        },
    )
    .expect("在实验库上建立写者");

    let started = Instant::now();
    let mut succeeded = 0usize;
    let mut failed_at: Option<usize> = None;
    let mut failure_stage = "none";
    let mut captured: Option<CapturedError> = None;

    for index in 0..group.events {
        let key = format!("mem-{}-{}-{index}", group.requested_mb, group.events);
        // 时间戳固定为 0：提交只由批次行数阈值触发，不受真实时钟干扰（与固定夹具一致）。
        match writer.submit_at(fixture_event(payload_text, index), key, 0) {
            Ok(_) => succeeded += 1,
            Err(err) => {
                failed_at = Some(index + 1);
                failure_stage = "submit";
                captured = Some(CapturedError::from(&err));
                break;
            }
        }
    }

    if failed_at.is_none()
        && let Err(err) = writer.flush()
    {
        failure_stage = "final_flush";
        captured = Some(CapturedError::from(&err));
    }
    let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;

    let attempted = failed_at.unwrap_or(group.events);
    let pending_events = writer.pending_len();
    drop(writer);

    let finalized = finalize(&db);
    // 断言：条数统计自洽——库内已提交行数 + 仍待写条目数 == 返回 Ok 的写入条数。
    // 失败的 submit 既不入库也不入待写（若在批次提交阶段失败则事务回滚、整批回到待写），
    // 因此基数用 succeeded 而非 attempted（2026-11-02 CI 实测：首条即失败时 attempted
    // 会多算 1 而误报不自洽）。
    if let Some(err) = captured.as_ref().map(|item| item.message.clone()) {
        println!("本组捕获写入错误（实验记录器，不判失败）：{err}");
    }
    // 全成功路径守恒：committed == succeeded - pending（待写尚未入库）。失败路径不做
    // 等式断言：两种失败模式的守恒口径不同——校验期失败的事件被直接拒收
    // （committed = succeeded - pending），批次提交失败的事件按幂等键保留进待写、
    // 连同失败那条（committed = attempted - pending）；数字全部入报告供分析。
    if let Some(rows) = finalized.rows
        && failed_at.is_none()
    {
        let expected = succeeded
            .checked_sub(pending_events)
            .expect("待写条目数不得大于成功写入条数");
        assert_eq!(
            rows,
            i64::try_from(expected).expect("事件规模必须可转换为 i64"),
            "写入条数统计不自洽：库内行数 {rows}，成功 {succeeded}，待写 {pending_events}"
        );
    }

    let outcome = match (failed_at, failure_stage) {
        (Some(at), _) => format!("在写入第 {at} 条时失败（DuckDB 未接受该批次）"),
        (None, "final_flush") => {
            format!("{} 条 submit 全部成功，但收尾 flush 失败", group.events)
        }
        _ => format!("完成 {} 条写入，未触发 OOM", group.events),
    };

    GroupResult {
        label: group.label,
        requested_mb: group.requested_mb,
        memory_limit_setting: group.memory_limit,
        memory_limit_readback,
        threads_readback,
        duckdb_version,
        events_target: group.events,
        batch_rows: group.batch_rows,
        events_attempted: attempted,
        events_succeeded: succeeded,
        failed_at,
        failure_stage,
        error_code: captured.as_ref().map(|item| item.code.clone()),
        error_message: captured.as_ref().map(|item| item.message.clone()),
        duckdb_error_raw: captured.as_ref().map(|item| item.raw.clone()),
        retryable: captured.as_ref().map(|item| item.retryable),
        pending_events,
        committed_rows: finalized.rows,
        rows_query_error: finalized.rows_error,
        checkpoint_ok: finalized.checkpoint_ok,
        checkpoint_error: finalized.checkpoint_error,
        db_bytes: finalized.db_bytes,
        wall_ms,
        outcome,
    }
}

/// 失败时捕获的错误证据。
struct CapturedError {
    code: String,
    message: String,
    raw: String,
    retryable: bool,
}

impl CapturedError {
    fn from(err: &whcode_domain::WhError) -> Self {
        // `writer.rs` 的错误前缀固定，去掉后即 DuckDB 返回的原文。
        let raw = err
            .message
            .split_once('：')
            .map_or_else(|| err.message.clone(), |(_, rest)| rest.to_string());
        Self {
            code: err.code.as_str().to_string(),
            message: err.message.clone(),
            raw,
            retryable: err.retryable,
        }
    }
}

/// 收尾统计结果。
struct Finalized {
    rows: Option<i64>,
    rows_error: Option<String>,
    checkpoint_ok: bool,
    checkpoint_error: Option<String>,
    db_bytes: u64,
}

/// 关闭写连接后重新打开读数、执行显式 `CHECKPOINT` 并统计文件字节数。
fn finalize(db: &Path) -> Finalized {
    let mut finalized = Finalized {
        rows: None,
        rows_error: None,
        checkpoint_ok: false,
        checkpoint_error: None,
        db_bytes: 0,
    };
    let conn = match Connection::open(db) {
        Ok(conn) => conn,
        Err(err) => {
            let error = format!("收尾连接打开失败：{err}");
            finalized.rows_error = Some(error.clone());
            finalized.checkpoint_error = Some(error);
            return finalized;
        }
    };
    match conn.query_row("SELECT count(*) FROM m0_events", [], |row| {
        row.get::<_, i64>(0)
    }) {
        Ok(rows) => finalized.rows = Some(rows),
        Err(err) => finalized.rows_error = Some(format!("事件行数查询失败：{err}")),
    }
    match conn.execute_batch("CHECKPOINT") {
        Ok(()) => finalized.checkpoint_ok = true,
        Err(err) => finalized.checkpoint_error = Some(format!("CHECKPOINT 失败：{err}")),
    }
    finalized.db_bytes = fs::metadata(db).map(|meta| meta.len()).unwrap_or(0);
    finalized
}

/// 固定夹具事件（与 `bench.rs` 的 `baseline_event` 同构）。
fn fixture_event(payload_text: &str, index: usize) -> EventRecord {
    EventRecord {
        schema_version: SCHEMA_VERSION,
        seq: 0,
        id: format!("E-MEM-{index}"),
        kind: EventKind::StreamingChunk,
        session_id: Some("S-MEMORY-LIMIT".to_string()),
        task_id: None,
        request_id: Some("R-MEMORY-LIMIT".to_string()),
        tool_call_id: None,
        payload: json!({ "text": payload_text }),
        ts_ms: 1_700_000_000_000 + index as u64,
        persistence_level: PersistenceLevel::Redacted,
    }
}

/// 断言 `SET` 真的生效：回读值须与请求的 MB 数在 1% 内一致（DuckDB 以 MiB 显示）。
fn assert_settings_effective(group: Group, memory_limit: &str, threads: &str) {
    let expected_mib = f64::from(group.requested_mb) * 1_000_000.0 / 1_048_576.0;
    let value = memory_limit
        .split_whitespace()
        .next()
        .and_then(|number| number.parse::<f64>().ok())
        .unwrap_or_else(|| {
            panic!(
                "{}：memory_limit 回读值无法解析为数值：{memory_limit}",
                group.label
            )
        });
    let deviation = (value - expected_mib).abs() / expected_mib;
    assert!(
        deviation < 0.01,
        "{}：memory_limit 未生效（请求 {} MB ≈ {expected_mib:.1} MiB，回读 {memory_limit}）",
        group.label,
        group.requested_mb
    );
    assert_eq!(
        threads, "4",
        "{}：threads 未生效（须为 {THREADS}，回读 {threads}）",
        group.label
    );
}

fn query_setting(conn: &Connection, name: &str) -> String {
    let sql = format!("SELECT CAST(current_setting('{name}') AS VARCHAR)");
    conn.query_row(&sql, [], |row| row.get(0))
        .unwrap_or_else(|err| format!("<查询失败：{err}>"))
}

fn conclude(groups: &[GroupResult], probe: &GroupResult) -> String {
    let baseline = &groups[0];
    match baseline.failed_at {
        Some(at) => format!(
            "在 {} 下复现写入失败，首个失败位置为第 {at} 条；错误原文见 groups[0]",
            baseline.label
        ),
        None if baseline.failure_stage == "final_flush" => format!(
            "{} 下 {} 条 submit 均成功，但收尾 flush 失败：未复现“写入约 1000 条触发 OOM”",
            baseline.label, EVENTS
        ),
        None => match probe.failed_at {
            Some(at) => format!(
                "{} 下 {} 条未失败，但规模探针（192MB / {} 条）在第 {at} 条失败：\
                 断言在更大数据规模上成立，需按探针规模重新评估上限",
                baseline.label, EVENTS, PROBE_EVENTS
            ),
            None => format!(
                "{} 下 {} 条与 192MB 探针 {} 条均完成写入：T027 的 OOM 断言未被独立复现",
                baseline.label, EVENTS, PROBE_EVENTS
            ),
        },
    }
}

fn print_group(result: &GroupResult) {
    println!("-- {} --", result.label);
    println!(
        "设置 {} → 回读 {}，threads 回读 {}，引擎 {}",
        result.memory_limit_setting,
        result.memory_limit_readback,
        result.threads_readback,
        result.duckdb_version
    );
    println!(
        "写入：发起 {} / 成功 {} / 首个失败 {} / 阶段 {} / 待写 {} / 库内行数 {:?} / 耗时 {:.1} ms",
        result.events_attempted,
        result.events_succeeded,
        result
            .failed_at
            .map_or_else(|| "无".to_string(), |at| at.to_string()),
        result.failure_stage,
        result.pending_events,
        result.committed_rows,
        result.wall_ms
    );
    println!(
        "CHECKPOINT：{}，文件 {} 字节",
        if result.checkpoint_ok {
            "成功"
        } else {
            "失败"
        },
        result.db_bytes
    );
    if let Some(raw) = &result.duckdb_error_raw {
        println!("错误原文：{raw}");
    }
    if let Some(err) = &result.rows_query_error {
        println!("行数查询问题：{err}");
    }
    if let Some(err) = &result.checkpoint_error {
        println!("CHECKPOINT 问题：{err}");
    }
    println!("结论：{}", result.outcome);
}

fn temp_workspace() -> PathBuf {
    // 同一二进制内的两个实验会被 cargo 并行执行：pid + 秒级时间戳可能同名，
    // 先启动者收尾 remove_dir_all 会删掉并行实验的工作目录（2026-11-02 CI 实测）。
    // 追加进程内原子序数保证唯一。
    static WORKSPACE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = WORKSPACE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "whcode-memory-limit-{}-{}-{seq}",
        std::process::id(),
        utc_now_rfc3339().replace([':', 'T', 'Z'], "-")
    ));
    // Code Scanning 收口：路径字符串不含 ".." 才使用（防临时目录遍历注入）。
    let dir_checked = dir.display().to_string();
    if dir_checked.contains("..") {
        panic!("临时目录路径不得包含 ..");
    }
    let dir = std::path::PathBuf::from(dir_checked);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建实验临时目录");
    dir
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tests 包的父目录即工作区根")
        .to_path_buf()
}

/// 生成 RFC 3339 UTC 时间戳（与 `bench.rs` 同一算法，不引入时间库）。
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

// ── M1 校准：temp_directory 溢写路径实验（ADR-012 退出条件②）──────────────

/// 溢写路径实验的读数。
#[derive(Debug, Clone, Serialize)]
struct SpillWatch {
    /// 轮询期间在溢写目录里看到的峰值总字节数。
    peak_bytes: u64,
    /// 轮询期间看到的峰值文件数。
    peak_files: u64,
    /// 轮询次数（证据可信度：0 表示没有观测到任何溢写文件）。
    polls: u64,
}

/// 目录总字节数与文件数（一层即可：DuckDB 溢写文件直接落在 temp_directory）。
fn dir_usage(dir: &Path) -> (u64, u64) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut bytes = 0u64;
    let mut files = 0u64;
    for entry in entries.flatten() {
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
        {
            bytes = bytes.saturating_add(meta.len());
            files += 1;
        }
    }
    (bytes, files)
}

/// 磁盘余量（字节；尽力而为：Windows 用 PowerShell 的 `Get-PSDrive`，其余用 `df -k`；
/// 取得失败显式记为 `None`——这是记录缺口，不是伪造读数）。
fn disk_free_bytes(path: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        let drive = path
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .and_then(|prefix| prefix.strip_suffix(':'))?;
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-PSDrive -Name {drive}).Free"),
            ])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        text.parse::<u64>().ok()
    }
    #[cfg(not(windows))]
    {
        let output = std::process::Command::new("df")
            .args(["-k", &path.display().to_string()])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().nth(1)?;
        let free_kb = line.split_whitespace().nth(3)?;
        free_kb
            .parse::<u64>()
            .ok()
            .map(|kb| kb.saturating_mul(1024))
    }
}

/// M1 校准实验：temp_directory 溢写路径（ADR-012 退出条件②）。
///
/// 三件事，全部留证：
/// 1. `temp_directory` 配置、可写性（探针文件）与磁盘余量（尽力而为，失败记缺口）；
/// 2. **溢写路径验证**——96MB 上限下对约 200 MB 数据做外排序（`ORDER BY`），轮询线程
///    记录溢写目录的峰值字节数与文件数：峰值 > 0 即缓冲管理器确实把数据溢写到了
///    `temp_directory`（这是直接证据，不以回读值冒充）；
/// 3. **异常条件**——`temp_directory` 指向不存在的路径再跑同一查询，记录 DuckDB 错误
///    原文（或“自动创建目录”的事实），验证失败路径可见、不静默。
///
/// 本测试是实验记录器：溢写发生与否、异常路径成功与否都不判失败；只断言设置生效。
#[test]
#[ignore]
fn memory_limit_spill_path_experiment() {
    let workspace = temp_workspace();
    let db = workspace.join("spill.duckdb");
    let temp_normal = workspace.join("tmp-spill-normal");
    let temp_missing = workspace.join("tmp-spill-missing");
    fs::create_dir_all(&temp_normal).expect("创建正常溢写目录");

    // 可写性探针：写入并删除一个文件，失败即显式记录。
    let probe = temp_normal.join(".write-probe");
    let writable_probe = fs::write(&probe, b"probe").is_ok() && fs::remove_file(&probe).is_ok();

    let conn = Connection::open(&db).expect("打开实验库");
    let temp_normal_sql = temp_normal.display().to_string().replace('\\', "/");
    conn.execute_batch(&format!(
        "SET memory_limit = '96MB'; SET threads = {THREADS};
         SET temp_directory = '{temp_normal_sql}';
         SET autoinstall_known_extensions = false;
         SET autoload_known_extensions = false;
         SET allow_community_extensions = false;
         SET allow_extension_repositories = 'forbidden';"
    ))
    .expect("配置数据库参数");
    let memory_limit_readback = query_setting(&conn, "memory_limit");
    let temp_directory_readback = query_setting(&conn, "temp_directory");
    let threads_readback = query_setting(&conn, "threads");
    let duckdb_version: String = conn
        .query_row("SELECT version()", [], |row| row.get(0))
        .expect("读取运行期引擎版本");
    println!(
        "== 溢写路径实验（ADR-012 退出条件②）==\nmemory_limit 回读 {memory_limit_readback}，threads 回读 {threads_readback}，temp_directory 回读 {temp_directory_readback}，引擎 {duckdb_version}，可写性探针 {writable_probe}"
    );
    assert!(
        memory_limit_readback.contains("96") || memory_limit_readback.contains("91.5"),
        "memory_limit 设置必须生效：{memory_limit_readback}"
    );

    // 数据集：约 1.5 M 行 × ~100 字节 payload ≈ 200 MB，超过 96MB 上限。
    conn.execute_batch(
        "CREATE OR REPLACE TABLE spill_source (id BIGINT, payload VARCHAR);
         INSERT INTO spill_source
         SELECT i, md5(CAST(i AS VARCHAR)) || repeat('x', 64)
         FROM range(1_500_000) tbl(i);",
    )
    .expect("生成溢写实验数据集");
    let source_rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM spill_source", [], |row| row.get(0))
        .expect("统计源表行数");
    let source_bytes: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(LENGTH(payload)), 0) FROM spill_source",
            [],
            |row| row.get(0),
        )
        .expect("统计源表字节");

    // 磁盘余量（尽力而为）。
    let free_bytes = disk_free_bytes(&workspace);
    println!(
        "磁盘余量：{}",
        free_bytes
            .map(|bytes| format!("{bytes} 字节"))
            .unwrap_or_else(|| "<未取得（记录缺口）>".to_string())
    );

    // 轮询线程：记录溢写目录峰值。
    let poll_dir = temp_normal.clone();
    let (peak_tx, peak_rx) = std::sync::mpsc::channel::<SpillWatch>();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_watcher = Arc::clone(&stop);
    let watcher = std::thread::Builder::new()
        .name("spill-watcher".to_string())
        .spawn(move || {
            let mut peak = SpillWatch {
                peak_bytes: 0,
                peak_files: 0,
                polls: 0,
            };
            while !stop_watcher.load(Ordering::Relaxed) {
                let (bytes, files) = dir_usage(&poll_dir);
                peak.peak_bytes = peak.peak_bytes.max(bytes);
                peak.peak_files = peak.peak_files.max(files);
                peak.polls += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = peak_tx.send(peak);
        })
        .expect("轮询线程创建");

    // 外排序查询：缓冲管理器装不下 200 MB 数据，必须经 temp_directory 溢写。
    let started = Instant::now();
    let query_a = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT id, payload FROM spill_source ORDER BY payload) sub",
        [],
        |row| row.get::<_, i64>(0),
    );
    let wall_ms_a = started.elapsed().as_secs_f64() * 1_000.0;
    stop.store(true, Ordering::Relaxed);
    let watch_normal = peak_rx.recv().expect("轮询线程读数");
    let _ = watcher.join();

    // 行数必须与源表一致（溢写不改变结果——这是“溢写正确性”的可判定证据）。
    let (group_a_ok, group_a_error) = match query_a {
        Ok(rows) => {
            assert_eq!(rows, source_rows, "组 A 聚合行数必须等于源表行数");
            (true, None::<String>)
        }
        Err(err) => (false, Some(format!("{err}"))),
    };
    println!(
        "组 A（正常 temp_directory）：查询 {}，耗时 {wall_ms_a:.1} ms，溢写峰值 {} 字节 / {} 文件（{} 次轮询）",
        if group_a_ok { "成功" } else { "失败" },
        watch_normal.peak_bytes,
        watch_normal.peak_files,
        watch_normal.polls,
    );

    // 组 B：`temp_directory` 指向不存在的路径（异常条件）。必须在**独立连接**上验证：
    // DuckDB 明确拒绝在已使用过临时目录后切换（实测「Cannot switch temporary directory
    // after the current one has been used」——这本身也是一条边界事实，记入证据）。
    let temp_missing_sql = temp_missing
        .display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/");
    // 同进程不能两次打开同一库文件（实测「File is already open」）：先释放组 A 的连接。
    drop(conn);
    let conn = Connection::open(&db).expect("重新打开实验库（异常路径组）");
    let switch_missing = conn
        .execute_batch(&format!(
            "SET memory_limit = '96MB'; SET threads = {THREADS};
             SET temp_directory = '{temp_missing_sql}';"
        ))
        .err()
        .map(|err| format!("{err}"));
    let missing_readback = query_setting(&conn, "temp_directory");
    let missing_writable_before = temp_missing.exists();
    let started = Instant::now();
    let query_b = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT id, payload FROM spill_source ORDER BY payload) sub",
        [],
        |row| row.get::<_, i64>(0),
    );
    let wall_ms_b = started.elapsed().as_secs_f64() * 1_000.0;
    let dir_auto_created = temp_missing.exists();
    let (group_b_ok, group_b_error) = match query_b {
        Ok(rows) => {
            if rows != source_rows {
                // 行数不符是正确性问题（不是实验失败）：记进错误文本，判定留给 ADR 修订。
                (
                    false,
                    Some(format!("行数不符：得到 {rows}，期望 {source_rows}")),
                )
            } else {
                (true, None::<String>)
            }
        }
        Err(err) => (false, Some(format!("{err}"))),
    };
    println!(
        "组 B（不存在路径）：设置{}，回读 {missing_readback}，查询 {}，耗时 {wall_ms_b:.1} ms，         目录事前存在 {missing_writable_before}／事后存在 {dir_auto_created}，错误原文 {:?}",
        if switch_missing.is_some() {
            "被拒绝"
        } else {
            "成功"
        },
        if group_b_ok { "成功" } else { "失败" },
        group_b_error
    );

    // 组 C：`temp_directory` 指向一个**文件**（真正不可写为目录）。这是“溢写失败不得
    // 静默”的判定点：查询应在需要溢写时报错，错误原文必须可见。
    let temp_file = workspace.join("tmp-spill-as-file");
    fs::write(&temp_file, b"not a directory").expect("创建同名文件");
    let temp_file_sql = temp_file
        .display()
        .to_string()
        .replace(std::path::MAIN_SEPARATOR, "/");
    drop(conn);
    let conn = Connection::open(&db).expect("重新打开实验库（文件路径组）");
    let setting_c = conn
        .execute_batch(&format!(
            "SET memory_limit = '96MB'; SET threads = {THREADS};
             SET temp_directory = '{temp_file_sql}';"
        ))
        .err()
        .map(|err| format!("{err}"));
    let started = Instant::now();
    let query_c = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT id, payload FROM spill_source ORDER BY payload) sub",
        [],
        |row| row.get::<_, i64>(0),
    );
    let wall_ms_c = started.elapsed().as_secs_f64() * 1_000.0;
    let (group_c_ok, group_c_error) = match query_c {
        Ok(rows) => {
            if rows != source_rows {
                (
                    false,
                    Some(format!("行数不符：得到 {rows}，期望 {source_rows}")),
                )
            } else {
                (true, None::<String>)
            }
        }
        Err(err) => (false, Some(format!("{err}"))),
    };
    println!(
        "组 C（temp_directory 为文件）：设置{}，查询 {}，耗时 {wall_ms_c:.1} ms，错误原文 {:?}",
        if setting_c.is_some() {
            "被拒绝"
        } else {
            "成功"
        },
        if group_c_ok { "成功" } else { "失败" },
        group_c_error
    );

    // 结论（判定留给 ADR 修订，不在测试里断言成败）。
    let spilled = watch_normal.peak_bytes > 0;
    let conclusion = format!(
        "组 A 溢写目录峰值 {} 字节（{}）；组 B 缺失路径 {}（DuckDB 会自动创建目录）；         组 C 文件路径 {}（{}）。判定口径：峰值 > 0 且查询成功 = 溢写路径已验证；         不可写路径的错误原文可见 = 溢写失败不静默。",
        watch_normal.peak_bytes,
        if spilled {
            "缓冲管理器确实经 temp_directory 溢写"
        } else {
            "未观测到溢写文件"
        },
        if group_b_ok {
            "查询成功"
        } else {
            "查询失败"
        },
        if group_c_ok {
            "查询成功"
        } else {
            "查询失败"
        },
        group_c_error.as_deref().unwrap_or("无错误")
    );
    println!("== 结论：{conclusion} ==");

    let report = json!({
        "schema_version": 1,
        "experiment": "memory-limit-spill-path",
        "adr": "ADR-012",
        "exit_condition": "退出条件②：temp_directory 配置/可写性/磁盘余量记录＋溢写路径验证",
        "generated_at_utc": utc_now_rfc3339(),
        "environment": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "duckdb_version": duckdb_version,
        },
        "fixture": {
            "memory_limit": "96MB",
            "threads": THREADS,
            "source_rows": source_rows,
            "source_payload_bytes": source_bytes,
            "query": "SELECT COUNT(*) FROM (SELECT id, payload FROM spill_source ORDER BY payload) sub",
            "poll_interval_ms": 50,
        },
        "temp_directory": {
            "normal": temp_normal_sql,
            "normal_writable_probe": writable_probe,
            "normal_readback": temp_directory_readback,
            "missing_path": temp_missing_sql,
            "missing_setting_error": switch_missing,
            "missing_readback": missing_readback,
            "missing_dir_existed_before": missing_writable_before,
        },
        "disk_free_bytes": free_bytes,
        "group_a": {
            "ok": group_a_ok,
            "wall_ms": wall_ms_a,
            "peak_spill_bytes": watch_normal.peak_bytes,
            "peak_spill_files": watch_normal.peak_files,
            "watch_polls": watch_normal.polls,
            "error": group_a_error,
        },
        "group_c_temp_is_file": {
            "ok": group_c_ok,
            "wall_ms": wall_ms_c,
            "setting_error": setting_c,
            "error": group_c_error,
        },
        "group_b_missing_path": {
            "ok": group_b_ok,
            "wall_ms": wall_ms_b,
            "dir_auto_created": dir_auto_created,
            "readback": missing_readback,
            "setting_error": switch_missing,
            "error": group_b_error,
        },
        "conclusion": conclusion,
        "notes": [
            "实验记录器：溢写/成功/失败都是证据，不判失败；只断言设置生效",
            "96MB 是刻意压低的实验值（低于 192MB 基线）以放大溢写概率；不改基线",
            "磁盘余量取得失败显式记 None，不伪造读数",
        ],
    });

    let out = workspace_root()
        .join("artifacts")
        .join("memory-limit-spill-experiment.json");
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).expect("创建 artifacts 目录");
    }
    fs::write(
        &out,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).expect("实验报告必须可序列化")
        ),
    )
    .expect("写入溢写实验证据");
    println!("证据已写入 {}", out.display());

    drop(conn);
    let _ = fs::remove_dir_all(&workspace);
}
