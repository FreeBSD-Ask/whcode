//! # whcode 主程序（骨架阶段 + M0 最小 TUI 闭环）
//!
//! 主程序、配置加载、日志初始化与 feature 装配（任务书 §6.2 `whcode-bin`）。
//!
//! **当前状态：骨架。** 可用入口：`--version`、`doctor [--config PATH] [--json]`、
//! `bench baseline --out PATH [--events N] [--repeat R]`（tasks.md T027、V-006）、
//! `serve --web [--bind 127.0.0.1:PORT] [--workspace DIR]`（M6 装配：静态前端 +
//! `/api/*`，默认仅回环）；无子命令时按平台默认启动界面——**Windows 默认 GUI**
//! （`whcode gui`，FR-711／ADR-027），其他平台为 TUI；`--tui`／`--cli`／`--gui`
//! 可显式指定入口（2026-10-25 用户指令）。完整命令面自 M2 起（tasks.md T046+）迁移至
//! `whcode-cui` 并由本程序装配。
//!
//! 未实现事项明确标记（宪法原则 VII / §15）：DuckDB 存取 M1/T032+；命令面 M2/T046+；
//! SDD 门禁 M4/T068+；Web/SSH M6—M7；运行时 M8/T100+。

mod bench;
mod cert_probe;
mod config;
mod cui_dispatch;
mod llm_provider;
mod logging;
mod runtimes;
mod serve;
mod store_source;
mod tls;
mod workspace;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use whcode_domain::{ErrorCode, SCHEMA_VERSION, WhError};

/// 二进制版本戳（精确到分钟；由 `build.rs` 注入 `WHCODE_BUILD_STAMP`）。
///
/// 版本号定义为**构建时间**（用户要求，2026-10-23）：本地构建为墙钟（本地时区偏移，
/// 探测不可用时 UTC `Z`）；发布构建由 CI 从 git 提交时间注入 UTC `Z` 形式，使同一
/// 提交的全新构建字节一致（ADR-047）。未注入时显式 `unknown`（不伪装）。
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

/// 入口选择器（2026-10-25 用户指令）：Windows 下无子命令时默认 GUI，
/// `--tui`／`--cli` 显式切到终端界面或命令行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntrySelector {
    Gui,
    Cli,
    Tui,
}

impl EntrySelector {
    fn from_flag(flag: &str) -> Option<Self> {
        match flag {
            "--gui" => Some(Self::Gui),
            "--cli" => Some(Self::Cli),
            "--tui" => Some(Self::Tui),
            _ => None,
        }
    }

    const fn flag(self) -> &'static str {
        match self {
            Self::Gui => "--gui",
            Self::Cli => "--cli",
            Self::Tui => "--tui",
        }
    }
}

/// 取出入口选择器，返回其余参数。
///
/// 选择器是全局开关，可以出现在任意位置，不要求写在子命令前。`--workspace`／
/// `--config` 的取值按值处理：目录恰好叫 `--tui` 时不会被误吃。
/// 同时给出两个不同的选择器返回用法错误——不猜优先级。
fn take_entry_selector(args: &[String]) -> Result<(Option<EntrySelector>, Vec<String>), String> {
    let mut selector: Option<EntrySelector> = None;
    let mut rest = Vec::with_capacity(args.len());
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "--workspace" | "--config") {
            rest.push(arg.clone());
            if let Some(value) = iter.next() {
                rest.push(value.clone());
            }
            continue;
        }
        match EntrySelector::from_flag(arg) {
            Some(found) => match selector {
                Some(previous) if previous != found => {
                    return Err(format!(
                        "入口选择器冲突：{} 与 {} 只能保留一个",
                        previous.flag(),
                        found.flag()
                    ));
                }
                _ => selector = Some(found),
            },
            None => rest.push(arg.clone()),
        }
    }
    Ok((selector, rest))
}

/// 未显式选择入口时的默认值：Windows 用 GUI，其他平台维持 TUI。
///
/// 平台分支只经 `whcode_platform::is_windows()` 取得（NFR-101 分层纪律）。
const fn default_entry(is_windows: bool) -> EntrySelector {
    if is_windows {
        EntrySelector::Gui
    } else {
        EntrySelector::Tui
    }
}

/// 无交互控制台时的提示：不启动界面，退出码 0。
///
/// CI 与管道调用依赖这条语义（`tui-smoke.py` 第 3 项：stdin 关闭时不得挂起）；
/// 默认 GUI 在这类环境里打开窗口会失败或挂住，因此先判定再决定是否建窗口。
fn print_cli_hint() -> ExitCode {
    println!("whcode {BUILD_STAMP} —— 当前没有可用的交互控制台，未启动界面");
    println!("可用：--tui（终端界面）、--cli <子命令>、--help");
    ExitCode::SUCCESS
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
    // 入口选择器先于子命令解析：`--cli doctor` 与 `--tui --workspace DIR` 都成立。
    let (selector, args) = match take_entry_selector(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
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
        // - 仅含旗标（如 `--workspace DIR`）或无参数 → 按入口选择器/平台默认启动界面；
        // - `--help`/`-h` → CUI 帮助（成功退出）。
        _ => {
            let wants_help = args.iter().any(|arg| arg == "--help" || arg == "-h");
            if wants_help || first_positional(&args).is_some() {
                cui_dispatch::dispatch(&args)
            } else {
                launch_selected_entry(selector, &args)
            }
        }
    }
}

/// 无子命令时的入口：显式选择器优先，否则按平台默认（Windows＝GUI，其他＝TUI）。
///
/// 默认 GUI 只在有交互控制台时启动：CI 与管道调用没有桌面会话，建窗口会失败或挂住，
/// 此时打印提示并成功退出。显式 `--gui` 不套这层保护——用户明确要求窗口，失败要如实报错。
fn launch_selected_entry(selector: Option<EntrySelector>, args: &[String]) -> ExitCode {
    match selector {
        Some(EntrySelector::Cli) => cui_dispatch::dispatch(args),
        Some(EntrySelector::Tui) => launch_interactive(args),
        Some(EntrySelector::Gui) => gui_command(args),
        None => match default_entry(whcode_platform::is_windows()) {
            EntrySelector::Gui => {
                if whcode_platform::term::terminal_size().is_some()
                    && whcode_platform::term::stdin_is_interactive()
                {
                    gui_command(args)
                } else {
                    print_cli_hint()
                }
            }
            _ => launch_interactive(args),
        },
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
        return print_cli_hint();
    }

    let FrontendParts {
        theme,
        texts,
        source,
        executor,
        llm_executor,
    } = match prepare_frontend(args) {
        Ok(prepared) => prepared,
        Err(code) => return code,
    };
    let probe: fn() -> Option<bool> = whcode_platform::term::is_window_minimized;
    let executor_ref: Option<&dyn whcode_app::commands::CommandExecutor> = executor
        .as_ref()
        .map(|executor| executor as &dyn whcode_app::commands::CommandExecutor);
    let llm_ref: Option<&dyn whcode_tui::LlmExecutor> = llm_executor
        .as_ref()
        .map(|executor| executor as &dyn whcode_tui::LlmExecutor);
    let options = whcode_tui::RunOptions {
        theme,
        texts,
        minimize_probe: Some(&probe),
        command_executor: executor_ref,
        llm_executor: llm_ref,
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
    llm_executor: Option<crate::llm_provider::TuiLlmExecutor>,
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
    // 出站信任库（ADR-040 决策 2、T191）：只建出站文件，不生成入站证书；失败不阻断界面。
    ensure_outbound_trust(&workspace, "TUI／GUI");
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
    let executor = shared.clone().map(store_source::TuiCommandExecutor::new);
    // LLM 执行器（T167）：与写执行器共享同一条连接；装配失败显式告警但不阻断界面
    // （LLM 缺失只影响 llm 动作，界面以 not_supported 提示降级）。
    let llm_executor = match (&shared, workspace::resolve_workspace(args, &config)) {
        (Some(conn), Ok(ws)) => {
            match crate::llm_provider::TuiLlmExecutor::from_env(&ws, std::rc::Rc::clone(conn)) {
                Ok(executor) => Some(executor),
                Err(err) => {
                    eprintln!("LLM 装配不可用（llm 动作以 not_supported 提示）：{err}");
                    None
                }
            }
        }
        _ => None,
    };
    Ok(FrontendParts {
        theme,
        texts,
        source,
        executor,
        llm_executor,
    })
}

/// 出站信任库装配（TUI／GUI／CUI 共用，ADR-040 决策 2、T191）。
///
/// 失败**不阻断**界面或命令：出站信任缺失只影响 LLM 调用，不应让只读界面不可用；
/// 但必须显式告警（含证书库路径与修法），不得静默。成功时只输出证书库自身的提示
/// （如“内置副本已修改且内置版本较新”），正常路径不打印噪声。
pub(crate) fn ensure_outbound_trust(workspace: &Path, frontend: &str) {
    let paths = whcode_app::cert::CertPaths::for_workspace(workspace);
    match whcode_app::cert::ensure_outbound_initialized(&paths, now_unix_ms()) {
        Ok(report) => {
            for note in &report.notes {
                eprintln!("证书库：{note}");
            }
        }
        Err(err) => eprintln!(
            "{frontend}：出站证书库初始化失败（本次继续运行；LLM 调用将因无信任根失败）：{err}；\
             证书库目录 {}（修正权限或删除该目录可在下次启动重建，ADR-040 决策 1）",
            paths.dir.display()
        ),
    }
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
    // 证书库状态（T191／ADR-040）：**只读**——不创建文件、不发网络请求；系统库份数读取
    // 失败或为 0 都在文本与 JSON 中显式标注，不以空对象冒充成功。
    let tls = tls_report(&config.workspace_path, now_unix_ms());

    if args.iter().any(|a| a == "--json") {
        let (engine_version, engine_error) = match &engine {
            Ok(version) => (Some(version.as_str()), None),
            Err(err) => (None, Some(err.to_string())),
        };
        let payload = doctor_json_payload(&config, engine_version, engine_error, tls);
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
        for line in &tls.lines {
            println!("{line}");
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

/// 当前 Unix 毫秒（证书库初始化时间戳与证书有效期判定）。
fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// `doctor --json` 信封装配（与文本模式同源）：独立成函数，使 `tls` 键的存在与结构
/// 可被单测断言，而不必启动子进程。
fn doctor_json_payload(
    config: &config::Config,
    engine_version: Option<&str>,
    engine_error: Option<String>,
    tls: TlsDoctorReport,
) -> serde_json::Value {
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        // 版本标记：与 `--version` 同源（构建时间戳，精确到分钟），供脚本与 CI 校验。
        "whcode_version": BUILD_STAMP,
        "duckdb_engine_version": engine_version,
        "duckdb_error": engine_error,
        // 三档能力等级在门禁输出中可见（FR-702、Check 101）；探针仅在显式 doctor 执行。
        "runtimes": runtimes::inventory(true),
        // 证书库（T191／ADR-040）：`tls` 为 `whcode_domain::tls::TlsStatus` 原样序列化；
        // 不可用时为 `null` 且 `tls_error` 给出可操作原因（不返回空对象或伪造的 0 份）。
        "tls": tls.value,
        "tls_error": tls.error,
        "tls_cert_dir": tls.cert_dir,
        "tls_initialized": tls.initialized,
        "doctor": config,
    })
}

/// `doctor` 的证书库报告块：文本行与 JSON 值**同源**（同一份 `TlsStatus`，不两处各自表述）。
///
/// 只读（ADR-040 决策 1）：不创建目录或文件、不发起网络请求。未初始化工作区如实标
/// “未初始化”；证书库不可用时文本打印 `tls: 不可用（原因）`、JSON 的 `tls` 为 `null`
/// 且 `tls_error` 给出原因——不伪造 0 份。
struct TlsDoctorReport {
    /// 文本模式逐行输出。
    lines: Vec<String>,
    /// `TlsStatus` 原样序列化（不可用时为 `Value::Null`）。
    value: serde_json::Value,
    /// 不可用的可操作原因（可用时为 `None`）。
    error: Option<String>,
    /// 证书库目录（供 JSON 侧定位）。
    cert_dir: String,
    /// 出站四文件齐备。
    initialized: bool,
}

/// 构造 `doctor` 的证书库报告块（见 [`TlsDoctorReport`]）。
fn tls_report(workspace: &Path, now_ms: u64) -> TlsDoctorReport {
    let paths = whcode_app::cert::CertPaths::for_workspace(workspace);
    // “已初始化”按**出站四文件齐备**判定（设置、内置副本、追加文件、种子清单）——这正是
    // `ensure_outbound_initialized` 的产物集合；只存在其中一个文件不算初始化完成。
    let initialized = paths.settings().is_file()
        && paths.builtin_ca().is_file()
        && paths.extra_ca().is_file()
        && paths.seed_manifest().is_file();
    let cert_dir = paths.dir.display().to_string();
    let mut lines = vec![
        format!("tls_cert_dir: {cert_dir}"),
        format!(
            "tls_initialized: {}",
            if initialized {
                "是".to_string()
            } else {
                "否（证书库未初始化：仅报告缺失，doctor 不创建文件）".to_string()
            }
        ),
    ];
    // 可用性前置判定：设置文件损坏、schema 不受支持或路径被同名的非目录占用时，出站信任
    // 不可用。此时只打印 `tls: 不可用（原因）`，不输出伪造的 0 份（ADR-040 决策 2）。
    if let Some(reason) = cert_store_unavailable(&paths) {
        lines.push(format!("tls: 不可用（{reason}）"));
        return TlsDoctorReport {
            lines,
            value: serde_json::Value::Null,
            error: Some(reason),
            cert_dir,
            initialized,
        };
    }
    match whcode_app::cert::status(&paths, now_ms) {
        Ok(status) => {
            lines.extend(tls_status_lines(&status));
            match serde_json::to_value(&status) {
                Ok(value) => TlsDoctorReport {
                    lines,
                    value,
                    error: None,
                    cert_dir,
                    initialized,
                },
                Err(err) => {
                    let reason = format!("证书库状态序列化失败：{err}");
                    lines.push(format!("tls: 不可用（{reason}）"));
                    TlsDoctorReport {
                        lines,
                        value: serde_json::Value::Null,
                        error: Some(reason),
                        cert_dir,
                        initialized,
                    }
                }
            }
        }
        Err(err) => {
            let reason = err.message;
            lines.push(format!("tls: 不可用（{reason}）"));
            TlsDoctorReport {
                lines,
                value: serde_json::Value::Null,
                error: Some(reason),
                cert_dir,
                initialized,
            }
        }
    }
}

/// 证书库可用性前置判定（**只读**，不创建、不修复文件）。
///
/// 只判定“确定不可用”：路径被同名文件占用，或设置文件存在但不可读／不可解析（含超限与
/// 不支持的 schema）。权限不足同样经同一路径报出（读取失败即不可用）；doctor 不尝试修复。
fn cert_store_unavailable(paths: &whcode_app::cert::CertPaths) -> Option<String> {
    if paths.dir.exists() && !paths.dir.is_dir() {
        return Some(format!(
            "证书库路径存在但不是目录：{}（删除该文件后可在下次启动重建，ADR-040 决策 1）",
            paths.dir.display()
        ));
    }
    whcode_app::cert::load_settings(paths).err().map(|err| {
        format!(
            "证书库设置不可用：{}；文件 {}（修正或删除该文件可恢复内置默认，ADR-040 决策 1）",
            err.message,
            paths.settings().display()
        )
    })
}

/// 证书库状态 → `doctor` 文本行（出站档位与份数、入站档位与证书元数据）。
fn tls_status_lines(status: &whcode_domain::TlsStatus) -> Vec<String> {
    let outbound = &status.outbound;
    let builtin = match &outbound.builtin {
        Some(item) if item.cert_count > 0 => format!(
            "{} 份（已修改={}，内置副本版本={}，内置有更新={}）",
            item.cert_count,
            yes_no(item.modified),
            item.bundled_version.as_deref().unwrap_or("未知"),
            yes_no(item.newer_bundled_available)
        ),
        Some(item) => format!(
            "0 份（{} 不可用或为空，sha256={}；执行 reset 可重建内置副本）",
            item.path,
            if item.sha256.is_empty() {
                "未取得"
            } else {
                item.sha256.as_str()
            }
        ),
        None => "不适用（system 档位不读内置副本）".to_string(),
    };
    let extra = match &outbound.extra {
        Some(item) => format!("{} 份（{}）", item.cert_count, item.path),
        None => "0 份（ca-extra.pem 不存在）".to_string(),
    };
    let effective = outbound
        .effective_root_count
        .map_or_else(|| "未取得".to_string(), |count| count.to_string());
    let mut lines = vec![
        format!("tls_outbound: {}", outbound_summary(outbound)),
        format!("tls_outbound_builtin: {builtin}"),
        format!("tls_outbound_extra: {extra}"),
        format!(
            "tls_outbound_system_roots: {}",
            system_roots_text(outbound.mode, outbound.system_root_count)
        ),
        format!("tls_outbound_effective_roots: {effective}"),
        format!("tls_inbound: {}", inbound_summary(&status.inbound)),
        format!("tls_inbound_mode: {}", status.inbound.mode.as_str()),
        format!("tls_inbound_cert: {}", inbound_cert_text(status)),
        format!(
            "tls_inbound_key_present: {}",
            yes_no(status.inbound.key_present)
        ),
        "tls_restart_required: 不适用（doctor 为只读进程，不改变监听）".to_string(),
    ];
    for note in &status.notes {
        lines.push(format!("tls_note: {note}"));
    }
    lines
}

/// 出站速览行：档位、生效根证书数、内置副本修改标记与追加份数。
///
/// 生效份数未取得时写 `unknown`（`system` 档位读库失败），**不写成 0**；内置副本在
/// `system` 档位不参与，写 `n/a` 而不是 `false`（ADR-040 决策 2）。
fn outbound_summary(outbound: &whcode_domain::OutboundStatus) -> String {
    let roots = outbound
        .effective_root_count
        .map_or_else(|| "unknown".to_string(), |count| count.to_string());
    let modified = outbound
        .builtin
        .as_ref()
        .map_or_else(|| "n/a".to_string(), |item| item.modified.to_string());
    let extra = outbound.extra.as_ref().map_or(0, |item| item.cert_count);
    format!(
        "{} roots={roots} builtin_modified={modified} extra={extra}",
        outbound.mode.as_str()
    )
}

/// 入站速览行：档位、证书指纹、到期时间与剩余天数。
///
/// 关闭或证书缺失时各项写 `n/a`——缺失不是“0 天”，不做数值化伪装。
fn inbound_summary(inbound: &whcode_domain::ServerCertStatus) -> String {
    let sha = inbound.sha256.as_deref().unwrap_or("n/a");
    let not_after = inbound.not_after.as_deref().unwrap_or("n/a");
    let days = inbound
        .days_remaining
        .map_or_else(|| "n/a".to_string(), |days| days.to_string());
    format!(
        "{} sha256={sha} not_after={not_after} days={days}",
        inbound.mode.as_str()
    )
}

/// 入站证书摘要（主题／SAN／有效期／剩余天数／指纹）；缺失时区分档位说明补救方向。
fn inbound_cert_text(status: &whcode_domain::TlsStatus) -> String {
    let inbound = &status.inbound;
    if inbound.mode == whcode_domain::InboundMode::Off {
        return "不适用（入站 TLS 已关闭，serve --web 以明文 HTTP 运行）".to_string();
    }
    if !inbound.cert_present {
        return "缺失或不可用（builtin 档位在 serve --web 启动时生成；external 档位需先上传证书链与私钥）"
            .to_string();
    }
    format!(
        "主题={}；SAN={}；生效={}；到期={}；剩余={} 天；指纹={}",
        inbound.subject.as_deref().unwrap_or("未知"),
        if inbound.sans.is_empty() {
            "未知".to_string()
        } else {
            inbound.sans.join("、")
        },
        inbound.not_before.as_deref().unwrap_or("未知"),
        inbound.not_after.as_deref().unwrap_or("未知"),
        inbound
            .days_remaining
            .map_or_else(|| "未知".to_string(), |days| days.to_string()),
        inbound.sha256.as_deref().unwrap_or("未取得"),
    )
}

/// 系统证书库份数文本：`system` 档位 0 份与读取失败都必须显式区分，不得互相冒充
/// （ADR-040 决策 2 的“不静默回落”）。
fn system_roots_text(mode: whcode_domain::TrustMode, count: Option<usize>) -> String {
    match (mode, count) {
        (whcode_domain::TrustMode::Builtin, _) => "不适用（builtin 档位不读系统库）".to_string(),
        (whcode_domain::TrustMode::System, Some(0)) => {
            "0 份（读取成功但为空：出站连接将失败，不回落内置库，ADR-040 决策 2）".to_string()
        }
        (whcode_domain::TrustMode::System, Some(count)) => format!("{count} 份（操作系统证书库）"),
        (whcode_domain::TrustMode::System, None) => {
            "未取得（读取失败，原因见 tls_note；不伪装成 0）".to_string()
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "是" } else { "否" }
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
    use super::{
        BUILD_STAMP, EntrySelector, default_entry, doctor_json_payload, system_roots_text,
        take_entry_selector, tls_report,
    };

    /// 临时工作区（Drop 清理；不写入仓库）。
    struct TempWorkspace {
        path: std::path::PathBuf,
    }

    impl TempWorkspace {
        fn new(tag: &str) -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "whcode-doctor-tls-{}-{tag}-{stamp}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("创建测试工作区");
            Self { path }
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// doctor 只读：未初始化工作区如实报“未初始化”与 0 份，且不创建证书库目录。
    #[test]
    fn doctor_tls_report_is_readonly_and_flags_uninitialized_workspace() {
        let workspace = TempWorkspace::new("uninit");
        let report = tls_report(&workspace.path, 1_793_000_000_000);

        assert!(report.error.is_none(), "读取成功时不得报不可用");
        assert!(!report.initialized, "未初始化工作区不得报已初始化");
        assert_eq!(
            report.cert_dir,
            workspace
                .path
                .join(".whcode")
                .join("certs")
                .display()
                .to_string()
        );
        assert_eq!(report.value["schema_version"], serde_json::json!(1));
        assert_eq!(
            report.value["outbound"]["builtin"]["cert_count"],
            serde_json::json!(0),
            "未初始化工作区的份数必须如实为 0"
        );
        assert_eq!(
            report.value["outbound"]["mode"],
            serde_json::json!("builtin")
        );
        assert!(report.value["inbound"].is_object(), "入站状态必须可解析");
        assert_eq!(
            report.value["restart_required"],
            serde_json::json!(false),
            "doctor 只读：状态投影的 restart_required 恒为 false"
        );
        assert!(report.value["notes"].is_array(), "notes 必须为数组");
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("tls_initialized: 否") && line.contains("未初始化")),
            "文本必须显式标注未初始化：{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line == "tls_outbound: builtin roots=0 builtin_modified=false extra=0"),
            "出站速览行必须给出档位、份数与修改标记：{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line == "tls_inbound: builtin sha256=n/a not_after=n/a days=n/a"),
            "入站速览行不得把缺失证书伪装成数值：{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.starts_with("tls_note:")),
            "缺失原因必须经 tls_note 说明：{:?}",
            report.lines
        );
        assert!(
            !report
                .lines
                .iter()
                .any(|line| line.starts_with("tls: 不可用")),
            "可用时不得报不可用：{:?}",
            report.lines
        );
        assert!(
            !workspace.path.join(".whcode").exists(),
            "doctor 不得创建证书库目录（只读）"
        );

        // 只有设置文件不算初始化完成（出站四文件齐备才算），且合法设置不算不可用。
        let certs = workspace.path.join(".whcode").join("certs");
        std::fs::create_dir_all(&certs).expect("创建证书库目录");
        std::fs::write(
            certs.join("settings.json"),
            br#"{"schema_version":1,"outbound":"builtin","inbound":"builtin"}"#,
        )
        .expect("写设置");
        let partial = tls_report(&workspace.path, 1_793_000_000_000);
        assert!(!partial.initialized, "只有 settings.json 不算初始化完成");
        assert!(partial.error.is_none(), "合法设置不得报不可用");
    }

    /// doctor 文本与 JSON 同源：出站份数、入站缺失与“不含私钥”同时成立；
    /// `--json` 信封含可解析的 `tls` 键。
    #[test]
    fn doctor_tls_report_reflects_initialized_store_without_private_key() {
        let workspace = TempWorkspace::new("init");
        let paths = whcode_app::cert::CertPaths::for_workspace(&workspace.path);
        whcode_app::cert::ensure_outbound_initialized(&paths, 1_793_000_000_000)
            .expect("仅出站初始化");
        let report = tls_report(&workspace.path, 1_793_000_000_000);

        assert!(report.error.is_none(), "初始化成功后不得报不可用");
        assert!(report.initialized, "出站四文件齐备即已初始化");
        assert_eq!(
            report.value["outbound"]["builtin"]["cert_count"],
            serde_json::json!(118),
            "内置副本份数必须与资产一致"
        );
        assert_eq!(
            report.value["outbound"]["effective_root_count"],
            serde_json::json!(118)
        );
        assert_eq!(
            report.value["inbound"]["cert_present"],
            serde_json::json!(false),
            "仅出站初始化不生成入站证书"
        );
        assert!(
            report.lines.iter().any(
                |line| line == "tls_outbound: builtin roots=118 builtin_modified=false extra=0"
            ),
            "速览行必须给出实际份数：{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("tls_outbound_builtin: 118 份")),
            "文本必须给出份数：{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.starts_with("tls_inbound: builtin sha256=n/a")),
            "入站速览行必须存在且不伪造指纹：{:?}",
            report.lines
        );
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.contains("tls_inbound_cert: 缺失")),
            "入站缺失必须显式：{:?}",
            report.lines
        );
        let text = serde_json::to_string(&report.value).expect("序列化");
        assert!(
            !text.contains("PRIVATE KEY"),
            "doctor JSON 不得含私钥：{text}"
        );

        // `--json` 信封：`tls` 键存在且结构可解析（与文本同一份状态）。
        let config = crate::config::Config::for_workspace(workspace.path.clone());
        let payload = doctor_json_payload(&config, Some("v1.2.0"), None, report);
        assert!(payload["tls"].is_object(), "doctor JSON 必须含 tls 对象");
        assert_eq!(
            payload["tls"]["outbound"]["builtin"]["cert_count"],
            serde_json::json!(118)
        );
        assert!(
            payload["tls_error"].is_null(),
            "可用时 tls_error 必须为 null"
        );
        assert_eq!(payload["tls_initialized"], serde_json::json!(true));
        assert!(payload["tls_cert_dir"].is_string(), "必须给出证书库路径");
        assert_eq!(
            payload["duckdb_engine_version"],
            serde_json::json!("v1.2.0")
        );
    }

    /// 证书库不可用（设置损坏或路径被同名文件占用）：`tls` 为 null、`tls_error` 给原因，
    /// 文本打印 `tls: 不可用（…）`，不伪造出站 0 份（ADR-040 决策 1／2）。
    #[test]
    fn doctor_tls_report_reports_unavailable_store_instead_of_fake_zero() {
        let workspace = TempWorkspace::new("broken");
        let certs = workspace.path.join(".whcode").join("certs");
        std::fs::create_dir_all(&certs).expect("创建证书库目录");
        std::fs::write(certs.join("settings.json"), b"{ this is not json").expect("写损坏设置");

        let report = tls_report(&workspace.path, 1_793_000_000_000);
        assert!(
            report.value.is_null(),
            "不可用时 tls 必须为 null：{}",
            report.value
        );
        let error = report.error.as_ref().expect("必须给出不可用原因");
        assert!(error.contains("证书库设置不可用"), "{error}");
        assert!(
            report
                .lines
                .iter()
                .any(|line| line.starts_with("tls: 不可用（")),
            "文本必须打印 tls: 不可用（原因）：{:?}",
            report.lines
        );
        assert!(
            !report
                .lines
                .iter()
                .any(|line| line.starts_with("tls_outbound:") || line.starts_with("tls_inbound:")),
            "不可用时不得伪造出站/入站 0 份：{:?}",
            report.lines
        );
        let config = crate::config::Config::for_workspace(workspace.path.clone());
        let payload = doctor_json_payload(&config, None, None, report);
        assert!(payload["tls"].is_null(), "JSON 的 tls 必须为 null");
        assert!(
            payload["tls_error"].is_string(),
            "JSON 必须给出 tls_error 原因"
        );

        // 路径被同名文件占用：同样判不可用，而不是把 0 份当成功。
        let blocked = TempWorkspace::new("blocked");
        std::fs::create_dir_all(blocked.path.join(".whcode")).expect("创建 .whcode");
        std::fs::write(blocked.path.join(".whcode").join("certs"), b"not a dir")
            .expect("写同名文件");
        let blocked_report = tls_report(&blocked.path, 1_793_000_000_000);
        assert!(blocked_report.value.is_null());
        let blocked_error = blocked_report.error.expect("必须给出不可用原因");
        assert!(blocked_error.contains("不是目录"), "{blocked_error}");
    }

    /// 速览行不把“未知”写成 0：`system` 档位读库失败写 `unknown`，内置副本不参与写 `n/a`。
    #[test]
    fn summaries_do_not_fabricate_zero() {
        use super::{inbound_summary, outbound_summary};
        use whcode_domain::{InboundMode, OutboundStatus, ServerCertStatus, TrustMode};

        let outbound = OutboundStatus {
            mode: TrustMode::System,
            source: "system".to_string(),
            builtin: None,
            extra: None,
            system_root_count: None,
            effective_root_count: None,
        };
        assert_eq!(
            outbound_summary(&outbound),
            "system roots=unknown builtin_modified=n/a extra=0"
        );

        let inbound = ServerCertStatus {
            mode: InboundMode::Off,
            cert_present: false,
            key_present: false,
            subject: None,
            sans: Vec::new(),
            not_before: None,
            not_after: None,
            sha256: None,
            days_remaining: None,
        };
        assert_eq!(
            inbound_summary(&inbound),
            "off sha256=n/a not_after=n/a days=n/a"
        );
    }

    /// 系统档位：0 份、读取失败与 builtin 不适用三种口径互不冒充（ADR-040 决策 2）。
    #[test]
    fn system_roots_text_distinguishes_zero_unknown_and_not_applicable() {
        use whcode_domain::TrustMode;
        let zero = system_roots_text(TrustMode::System, Some(0));
        assert!(zero.starts_with("0 份"), "{zero}");
        assert!(zero.contains("不回落内置库"), "{zero}");
        let count = system_roots_text(TrustMode::System, Some(141));
        assert!(count.contains("141 份"), "{count}");
        let unknown = system_roots_text(TrustMode::System, None);
        assert!(unknown.contains("未取得"), "{unknown}");
        assert!(
            !unknown.contains("0 份"),
            "读取失败不得伪装成 0 份：{unknown}"
        );
        let not_applicable = system_roots_text(TrustMode::Builtin, None);
        assert!(not_applicable.contains("不适用"), "{not_applicable}");
    }

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
    /// 选择器可出现在任意位置，并被移出子命令参数。
    #[test]
    fn entry_selector_is_extracted_from_any_position() {
        let args = vec![
            "--tui".to_string(),
            "--workspace".to_string(),
            "ws".to_string(),
        ];
        let (selector, rest) = take_entry_selector(&args).expect("选择器解析");
        assert_eq!(selector, Some(EntrySelector::Tui));
        assert_eq!(rest, vec!["--workspace".to_string(), "ws".to_string()]);

        let args = vec![
            "doctor".to_string(),
            "--cli".to_string(),
            "--json".to_string(),
        ];
        let (selector, rest) = take_entry_selector(&args).expect("选择器解析");
        assert_eq!(selector, Some(EntrySelector::Cli));
        assert_eq!(rest, vec!["doctor".to_string(), "--json".to_string()]);
    }

    /// 同一选择器重复出现按一个处理（幂等），不报错。
    #[test]
    fn repeated_entry_selector_is_idempotent() {
        let args = vec!["--gui".to_string(), "--gui".to_string()];
        let (selector, rest) = take_entry_selector(&args).expect("选择器解析");
        assert_eq!(selector, Some(EntrySelector::Gui));
        assert!(rest.is_empty());
    }

    /// 两个不同选择器同时出现是用法错误：不猜优先级。
    #[test]
    fn conflicting_entry_selectors_are_usage_error() {
        let args = vec!["--cli".to_string(), "--tui".to_string()];
        let err = take_entry_selector(&args).expect_err("必须报冲突");
        assert!(err.contains("--cli") && err.contains("--tui"), "{err}");
    }

    /// `--workspace`／`--config` 的取值按值处理：目录名写成旗标样式也不会被吃掉。
    #[test]
    fn entry_selector_does_not_eat_flag_values() {
        let args = vec!["--workspace".to_string(), "--tui".to_string()];
        let (selector, rest) = take_entry_selector(&args).expect("选择器解析");
        assert_eq!(selector, None);
        assert_eq!(rest, vec!["--workspace".to_string(), "--tui".to_string()]);
    }

    /// 平台默认入口：Windows 为 GUI，其他平台维持 TUI。
    #[test]
    fn default_entry_follows_platform() {
        assert_eq!(default_entry(true), EntrySelector::Gui);
        assert_eq!(default_entry(false), EntrySelector::Tui);
    }
}
