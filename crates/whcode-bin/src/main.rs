//! # whcode 主程序（骨架阶段 + M0 最小 TUI 闭环）
//!
//! 主程序、配置加载、日志初始化与 feature 装配（任务书 §6.2 `whcode-bin`）。
//!
//! **当前状态：骨架。** 可用入口：`--version`、`doctor [--config PATH] [--json]`、
//! `bench baseline --out PATH [--events N] [--repeat R]`（tasks.md T027、V-006）、
//! `serve --web [--bind 127.0.0.1:PORT] [--workspace DIR]`（M6 装配：静态前端 +
//! `/api/*`，默认仅回环）；无参数启动且存在可用控制台时进入 `whcode-tui` 的六视图
//! 闭环（tasks.md T024/T082 / FR-601）。完整命令面自 M2 起（tasks.md T046+）迁移至
//! `whcode-cui` 并由本程序装配。
//!
//! 未实现事项明确标记（宪法原则 VII / §15）：DuckDB 存取 M1/T032+；命令面 M2/T046+；
//! SDD 门禁 M4/T068+；Web/SSH M6—M7；运行时 M8/T100+。

mod bench;
mod config;
mod cui_dispatch;
mod logging;
mod runtimes;
mod serve;
mod store_source;
mod workspace;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use whcode_domain::{ErrorCode, SCHEMA_VERSION, WhError};

/// 二进制构建时间戳（UTC，精确到分钟；由 `build.rs` 注入 `WHCODE_BUILD_STAMP`）。
///
/// 版本号定义为**构建时间**（用户要求，2026-10-23）；未注入时显式 `unknown`（不伪装）。
const BUILD_STAMP: &str = match option_env!("WHCODE_BUILD_STAMP") {
    Some(stamp) => stamp,
    None => "unknown",
};

/// 基线默认事件数（每次重复）。
const DEFAULT_BENCH_EVENTS: usize = 20_000;
/// 基线默认重复次数。
const DEFAULT_BENCH_REPEAT: usize = 3;

/// 第一个非旗标参数（用于区分 CUI 子命令与纯旗标启动）。
///
/// 跳过带值旗标的值（`--workspace DIR`/`--config PATH`）：否则路径会被误认成子命令。
fn first_positional(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--workspace" | "--config" => {
                let _ = iter.next();
            }
            other if other.starts_with('-') => {}
            other => return Some(other),
        }
    }
    None
}

/// 文本层装配（FR-615）：PO 资源目录覆盖内建资源，加载失败时告警并回退内建。
///
/// 资源根依次取 `WHCODE_I18N_DIR`（显式、可撤销）与可执行文件所在目录；
/// PO 文件不存在属正常形态（未安装翻译），静默使用内建资源；只有存在但不可读或
/// 解析失败才告警——不把“未安装翻译”误报为错误，也不静默吞掉真实的加载失败。
pub(crate) fn load_texts(
    locale: &str,
    source: whcode_domain::i18n::LocaleSource,
) -> whcode_domain::i18n::I18n {
    use whcode_domain::i18n::I18n;
    let root = std::env::var_os("WHCODE_I18N_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
        });
    let Some(root) = root else {
        return I18n::load(locale, source);
    };
    match I18n::load_with_po(&root, locale, source) {
        Ok(texts) => texts,
        Err(err) => {
            eprintln!("PO 文本层加载失败，回退内建资源（{locale}）：{err}");
            I18n::load(locale, source)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("whcode {BUILD_STAMP}");
            ExitCode::SUCCESS
        }
        Some("doctor") => {
            doctor(&args[1..]);
            ExitCode::SUCCESS
        }
        Some("bench") => bench_command(&args[1..]),
        Some("serve") => serve::serve_command(&args[1..]),
        // 原生桌面 GUI（FR-711／ADR-027）：只读六视图，与 TUI 共用同一只读快照。
        Some("gui") => gui_command(&args[1..]),
        // 批次 2/3：区分“CUI 命令”与“交互入口”——
        // - 含位置参数（子命令或拼写错误）→ CUI 调度（未知子命令由 clap 报用法错误，退出码 2）；
        // - 仅含旗标（如 `--workspace DIR`）或无参数 → 交互入口（TUI/骨架提示）；
        // - `--help`/`-h` → CUI 帮助（成功退出）。
        _ => {
            let wants_help = args.iter().any(|arg| arg == "--help" || arg == "-h");
            if wants_help || first_positional(&args).is_some() {
                cui_dispatch::dispatch(&args)
            } else {
                launch_interactive(&args)
            }
        }
    }
}

/// `bench baseline`：固定数据集与采样方法的写入基线测量（tasks.md T027、V-006）。
///
/// 失败时向 stderr 输出 `WhError` 信封（`schema_version` + 四字段）并返回非零退出码，
/// 不 panic。
fn bench_command(args: &[String]) -> ExitCode {
    let (out, options) = match parse_bench_args(args) {
        Ok(parsed) => parsed,
        Err(err) => return report_bench_error(err),
    };
    let report = match bench::measure(&options) {
        Ok(report) => report,
        Err(err) => return report_bench_error(err),
    };
    if let Err(err) = bench::write_json(&report, &out) {
        return report_bench_error(err);
    }
    print_bench_summary(&report, &out);
    ExitCode::SUCCESS
}

fn report_bench_error(err: WhError) -> ExitCode {
    match serde_json::to_string(&whcode_domain::ErrEnvelope::new(err)) {
        Ok(json) => eprintln!("{json}"),
        Err(_) => eprintln!("bench baseline 失败：错误对象序列化失败"),
    }
    ExitCode::FAILURE
}

fn parse_bench_args(args: &[String]) -> Result<(PathBuf, bench::BaselineOptions), WhError> {
    if args.first().map(String::as_str) != Some("baseline") {
        return Err(WhError::parse_error(
            "未知 bench 子命令：仅支持 baseline（用法：whcode bench baseline --out <路径> \
             [--events N] [--repeat R]）",
        ));
    }

    let mut out: Option<PathBuf> = None;
    let mut events = DEFAULT_BENCH_EVENTS;
    let mut repeat = DEFAULT_BENCH_REPEAT;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| WhError::parse_error(format!("{flag} 缺少取值")))?;
        match flag {
            "--out" => out = Some(PathBuf::from(value)),
            "--events" => events = parse_positive_usize(value, "--events")?,
            "--repeat" => repeat = parse_positive_usize(value, "--repeat")?,
            other => {
                return Err(WhError::parse_error(format!(
                    "未知参数：{other}（支持 --out / --events / --repeat）"
                )));
            }
        }
        index += 2;
    }

    let out = out.ok_or_else(|| {
        WhError::parse_error("缺少 --out <路径>：基准报告必须落盘，不停留在终端输出")
    })?;
    let parent = out
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(WhError::new(
            ErrorCode::IoError,
            format!(
                "输出目录不存在或不可用：{}（基准报告不自动创建目录）",
                parent.display()
            ),
        ));
    }
    if out.is_dir() {
        return Err(WhError::parse_error(format!(
            "--out 指向目录：{}（需要文件路径）",
            out.display()
        )));
    }

    let options = bench::BaselineOptions::validated(events, repeat)?;
    Ok((out, options))
}

fn parse_positive_usize(value: &str, flag: &str) -> Result<usize, WhError> {
    let parsed = value.parse::<usize>().map_err(|err| {
        WhError::parse_error(format!("{flag} 取值不是非负整数：{value}（{err}）"))
    })?;
    if parsed == 0 {
        return Err(WhError::parse_error(format!("{flag} 必须大于 0")));
    }
    Ok(parsed)
}

fn print_bench_summary(report: &bench::BaselineReport, out: &Path) {
    println!("whcode bench baseline（schema v{SCHEMA_VERSION}）");
    println!("duckdb_engine_version: {}", report.duckdb_engine_version);
    for run in &report.runs {
        println!(
            "run {}: {:.1} 条/秒；提交延迟 p50 {:.1} / p95 {:.1} / p99 {:.1} µs；\
             事件行 {}；数据库 {} 字节",
            run.run,
            run.throughput_events_per_second,
            run.latency_us.p50,
            run.latency_us.p95,
            run.latency_us.p99,
            run.event_rows,
            run.db_bytes
        );
    }
    let aggregate = &report.aggregate;
    println!(
        "汇总（{} 次重复）：p50 {:.1} / p95 {:.1} / p99 {:.1} µs（样本 {}）；吞吐中位数 {:.1} 条/秒",
        aggregate.runs,
        aggregate.latency_us.p50,
        aggregate.latency_us.p95,
        aggregate.latency_us.p99,
        aggregate.latency_us.sample_count,
        aggregate.throughput_events_per_second_median
    );
    println!("报告：{}", out.display());
    println!("测量条件见 measurement_discipline；单机单次样本，不构成预算冻结（R-12）");
}

/// 无参数启动：有可用控制台**且 stdin 交互**时进入最小 TUI 闭环，否则打印骨架提示并
/// 成功退出。
///
/// 双重判定（B-05 实测教训）：仅 `terminal_size()` 会把“管道 stdin + 控制台 stdout”
/// 误判为可用 TUI（输出侧有控制台），导致挂起；`stdin_is_interactive()` 补上输入侧检查。
/// 交互入口：有交互控制台时进入 TUI；否则打印骨架提示。
///
/// 装配根接线（T082/T083/T129/T135 + 批次 1 数据接入）：
/// - 主题（FR-612）经配置 `ui.theme` 选择，可被 `WHCODE_UI_THEME` 覆盖；
/// - 最小化探测（T135/ADR-021）注入 `whcode-platform` 的 `is_window_minimized`
///   ——事件循环只在“有待绘制帧且输入静默”时低频调用（≥5s 间隔，非帧路径）；
/// - 视图数据源：打开工作区库（`--workspace DIR` 或配置工作区）并经 `whcode-app`
///   只读命令生成快照；打开失败（含跨进程锁）时**显式警告并回退空数据源**——
///   TUI 仍可用，但不伪造数据。
fn launch_interactive(args: &[String]) -> ExitCode {
    if !(whcode_platform::term::terminal_size().is_some()
        && whcode_platform::term::stdin_is_interactive())
    {
        println!("whcode {BUILD_STAMP} —— 骨架阶段");
        println!("可用：--version、doctor [--config PATH] [--json]");
        println!("其余命令自 M2 起交付（specs/001-whcode-harness/tasks.md）");
        return ExitCode::SUCCESS;
    }

    let FrontendParts {
        theme,
        texts,
        source,
        executor,
    } = match prepare_frontend(args) {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };
    let probe: fn() -> Option<bool> = whcode_platform::term::is_window_minimized;
    let executor_ref: Option<&dyn whcode_app::commands::CommandExecutor> = executor
        .as_ref()
        .map(|executor| executor as &dyn whcode_app::commands::CommandExecutor);
    let options = whcode_tui::RunOptions {
        theme,
        texts,
        minimize_probe: Some(&probe),
        command_executor: executor_ref,
    };
    match whcode_tui::run_with(source.as_ref(), options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("TUI 运行失败：{err}");
            ExitCode::FAILURE
        }
    }
}

/// `whcode gui`：原生桌面 GUI（只读 v1，FR-711／ADR-027）。
///
/// 与 TUI 共用同一只读快照（`whcode_app::view_source`）：GUI 自身不触库，主题沿用配置。
fn gui_command(args: &[String]) -> ExitCode {
    let FrontendParts {
        theme,
        texts,
        source,
        ..
    } = match prepare_frontend(args) {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };
    let gui_theme = whcode_gui::GuiTheme::parse(theme.as_str()).unwrap_or_default();
    let options = whcode_gui::RunOptions {
        theme: gui_theme,
        texts,
    };
    match whcode_gui::run(source, options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("GUI 运行失败：{err}");
            ExitCode::FAILURE
        }
    }
}

/// 前端共用装配：主题、文本层、工作区与只读快照数据源（TUI 与 GUI 同一来源）。
/// 前端装配产物（TUI／GUI 共用）：主题、文本层、只读数据源与 TUI 写执行器。
struct FrontendParts {
    theme: whcode_tui::Theme,
    texts: whcode_domain::i18n::I18n,
    source: Box<dyn whcode_app::view_source::ViewSource>,
    executor: Option<store_source::TuiCommandExecutor>,
}

fn prepare_frontend(args: &[String]) -> Result<FrontendParts, ExitCode> {
    let config = match load_config(args) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("配置加载失败（使用默认主题与当前目录工作区）：{err}");
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            config::Config::for_workspace(cwd)
        }
    };
    let theme = config.effective_theme();
    // 文本层（FR-615）：显式配置 → LANG/LC_ALL → 默认 zh-CN；PO 覆盖内建，en 回退由 I18n 负责。
    let (locale, locale_source) = whcode_domain::i18n::detect_locale(None);
    let texts = load_texts(&locale, locale_source);
    let workspace = match workspace::resolve_workspace(args, &config) {
        Ok(workspace) => workspace,
        Err(err) => {
            eprintln!("工作区解析失败：{err}");
            return Err(ExitCode::FAILURE);
        }
    };
    // 连接经 Rc<RefCell> 共享：视图数据源（读）与 TUI 写命令执行器（FR-623）用同一条
    // 工作区连接（Check 98 口径：单连接为唯一写路径）；库不可用时两者皆缺（只读空源）。
    let shared = match workspace::open_workspace_db(&workspace, &config) {
        Ok(conn) => Some(std::rc::Rc::new(std::cell::RefCell::new(conn))),
        Err(err) => {
            eprintln!("数据源不可用（界面以空数据源运行，视图显示“暂无数据”）：{err}");
            None
        }
    };
    let source: Box<dyn whcode_app::view_source::ViewSource> = match &shared {
        Some(conn) => Box::new(store_source::StoreViewSource::new(
            std::rc::Rc::clone(conn),
            texts.clone(),
        )),
        None => Box::new(whcode_app::view_source::EmptyViewSource),
    };
    let executor = shared.map(store_source::TuiCommandExecutor::new);
    Ok(FrontendParts {
        theme,
        texts,
        source,
        executor,
    })
}

fn doctor(args: &[String]) {
    let logger = logging::Logger::new(logging::Level::Info);
    let config = match load_config(args) {
        Ok(config) => config,
        Err(err) => {
            logger.log(logging::Level::Error, &format!("配置加载失败：{err}"));
            return;
        }
    };

    // 依赖记录（V-001）：DuckDB 引擎版本必须来自运行期实测，缺证据时显式报不可用。
    let engine = whcode_store::engine_version();

    if args.iter().any(|a| a == "--json") {
        let (engine_version, engine_error) = match &engine {
            Ok(version) => (Some(version.as_str()), None),
            Err(err) => (None, Some(err.to_string())),
        };
        let payload = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            // 版本标记：与 `--version` 同源（构建时间戳，精确到分钟），供脚本与 CI 校验。
            "whcode_version": BUILD_STAMP,
            "duckdb_engine_version": engine_version,
            "duckdb_error": engine_error,
            // 三档能力等级在门禁输出中可见（FR-702、Check 101）；探针仅在显式 doctor 执行。
            "runtimes": runtimes::inventory(true),
            "doctor": &config,
        });
        match serde_json::to_string(&payload) {
            Ok(json) => println!("{json}"),
            Err(err) => eprintln!("doctor 输出序列化失败：{err}"),
        }
    } else {
        println!("whcode doctor（schema v{SCHEMA_VERSION}）");
        println!("whcode_version: {BUILD_STAMP}");
        println!("workspace: {}", config.workspace_path.display());
        println!("platform: {}", whcode_platform::termination_policy_for());
        println!("threads_baseline: {}", config.db.threads);
        println!(
            "memory_limit_mb: {}（仅约束 DuckDB 缓冲管理器）",
            config.db.memory_limit_mb
        );
        println!("trace_default: {}", config.trace.default_level);
        // 三档能力等级（FR-702）：受限运行时列明不支持项，不表述为完整生态（D-08）。
        println!("runtimes: {}", runtimes::summary(true));
        for entry in runtimes::inventory(true) {
            if !entry.unsupported.is_empty() {
                println!("  {} 不支持：{}", entry.name, entry.unsupported.join("、"));
            }
        }
        match &engine {
            Ok(version) => println!("duckdb_engine_version: {version}"),
            Err(err) => println!("duckdb_engine_version: 不可用（{err}）"),
        }
        match config.check_temp_directory() {
            Ok(()) => println!("temp_directory: 可写"),
            Err(err) => {
                println!("temp_directory: 不可用（{err}）");
                logger.log(logging::Level::Warn, &format!("临时目录不可用：{err}"));
            }
        }
        println!(
            "未实现（明确标记）：命令面 M2/T046+；DuckDB 存取 M1/T032+；SDD 门禁 M4/T068+；Web/SSH M6—M7；运行时 M8/T100+"
        );
    }

    logger.log(
        logging::Level::Info,
        "doctor 完成（api_key=sk-do-not-leak-123 已在日志路径脱敏）",
    );
    // 调试级诊断（仅 `--verbose` 时可见）——同时是 `Debug` 门限的实际使用路径。
    if args.iter().any(|a| a == "--verbose") {
        logger.log(
            logging::Level::Debug,
            "doctor 调试细节：默认 doctor 以当前目录为工作区根，配置文件优先",
        );
    }
    // 告警路径：未显式设置临时目录空间上限时给出建议（§5.4）。
    if config.db.max_temp_directory_size_mb.is_none() {
        logger.log(
            logging::Level::Warn,
            "max_temp_directory_size 未设置：临时目录空间无显式上限（§5.4 建议显式配置）",
        );
    }
}

fn load_config(args: &[String]) -> Result<config::Config, whcode_domain::WhError> {
    match args.iter().position(|a| a == "--config") {
        Some(index) => {
            let path = args
                .get(index + 1)
                .ok_or_else(|| whcode_domain::WhError::parse_error("--config 缺少路径参数"))?;
            let raw = std::fs::read_to_string(path).map_err(|err| {
                whcode_domain::WhError::new(
                    whcode_domain::ErrorCode::IoError,
                    format!("读取配置失败：{err}"),
                )
            })?;
            config::Config::from_json(&raw)
        }
        None => {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            Ok(config::Config::for_workspace(cwd))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BUILD_STAMP;

    /// 版本号 = 构建时间：本地时间 `YYYY-MM-DDTHH:MM±HH:MM`（精确到分钟）；
    /// 外部时间命令均不可用时回退 UTC `YYYY-MM-DDTHH:MMZ`，并由构建脚本输出
    /// `cargo:warning`（不静默降级）。
    ///
    /// 本用例只校验**格式与年份合理性**，不含平台分支：`whcode-bin` 不是平台层，
    /// 条件编译必须集中在 `whcode-platform`（NFR-101，契约测试强制）；回退情形
    /// 由构建告警人工复核，不以“Windows 必为本地偏移”的恒真断言冒充验证。
    #[test]
    fn build_stamp_is_minute_precision_build_time() {
        assert_ne!(
            BUILD_STAMP, "unknown",
            "build.rs 必须注入 WHCODE_BUILD_STAMP"
        );
        assert!(
            matches!(BUILD_STAMP.len(), 17 | 22),
            "长度须为 22（本地 ±HH:MM）或 17（回退 UTC `Z`）：{BUILD_STAMP}"
        );
        let (head, tail) = BUILD_STAMP.split_at(16);
        let bytes = head.as_bytes();
        assert_eq!(bytes[4], b'-', "日期分隔符：{BUILD_STAMP}");
        assert_eq!(bytes[7], b'-', "日期分隔符：{BUILD_STAMP}");
        assert_eq!(bytes[10], b'T', "日期时间分隔符：{BUILD_STAMP}");
        assert_eq!(bytes[13], b':', "时间分隔符：{BUILD_STAMP}");
        for (index, byte) in bytes.iter().enumerate() {
            if matches!(index, 4 | 7 | 10 | 13) {
                continue;
            }
            assert!(byte.is_ascii_digit(), "必须是数字：{BUILD_STAMP}");
        }
        if tail != "Z" {
            let tail_bytes = tail.as_bytes();
            assert!(
                (tail_bytes[0] == b'+' || tail_bytes[0] == b'-') && tail_bytes[3] == b':',
                "时区偏移须为 ±HH:MM：{BUILD_STAMP}"
            );
            assert!(
                tail_bytes[1..3].iter().all(u8::is_ascii_digit)
                    && tail_bytes[4..6].iter().all(u8::is_ascii_digit),
                "偏移须为数字：{BUILD_STAMP}"
            );
        }
        // 年份 plausibility（防止把其它字符串误当时间戳）。
        let year: u32 = head[0..4].parse().expect("年份数字");
        assert!((2024..2100).contains(&year), "年份不合理：{BUILD_STAMP}");
    }
}
