//! CUI 命令调度（批次 2）：`whcode <command>` → 应用服务/存储 → 稳定输出与退出码。
//!
//! 分层与职责：
//! - 解析：`whcode_cui::cli`（clap 树，T084/ADR-019）；
//! - 执行：权限判定经 `whcode-app`（FR-604），维护命令经 `whcode-store`（SQL 只在 store，
//!   ADR-018）；本模块**只做装配**，不写 SQL；
//! - 输出：`--json` 统一信封（`schema_version` + `data`；contracts/cui-cli.md §3）；
//!   错误对象四字段写 stderr；退出码用 `whcode_cui::output` 的稳定映射（0/2/10…99）。
//!
//! 会话定位：契约 §1 要求显式 `--session`/`--task`——**不设隐式"当前会话"**，
//! 避免脚本路径依赖隐藏状态。
//!
//! 已接线命令随批次翻转（`events show`＝T390、spec 门禁命令＝T394、runtime 四命令＝T395、
//! `session switch/share/fork/restore/rewind/plan` 与 `db checkpoint/restore` 随各自批次）。
//! **当前无「已声明未接线」命令**；`not_supported`（退出码 55，FR-620、ADR-032）机制保留为
//! tripwire——命令映射面若在本端为预留或不适用，details 由登记表（`whcode_app::surface`）
//! 驱动：`surface_id`、本端状态与里程碑/理由码；不静默成功、不伪装成 404 语义；
//! `serve`/`doctor` 由主入口处理。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use serde_json::{Value, json};
use whcode_app::surface::{FrontendId, Support};
use whcode_app::tool_call::{TOOL_STARTED, describe};
use whcode_app::{Actor, DefaultPolicy, actor_with_all_capabilities};
use whcode_cui::cli::{
    Command, ContextCommand, DbCommand, EventsCommand, FontCommand, LlmCommand, ParseOutcome,
    RuntimeCommand, SessionCommand, SpecCommand, TaskCommand, WebhookCommand, parse_outcome,
};
use whcode_cui::format::Styler;
use whcode_cui::output::{EXIT_USAGE, error_envelope, error_exit_code, success_envelope};
use whcode_domain::i18n::{I18n, detect_locale};
use whcode_domain::{ErrorCode, TraceLevel, WhError};
use whcode_store::{PurgeCriteria, PurgeProtection};

use crate::config::Config;
use crate::workspace;

/// 单次 CUI 命令的最大事件条数（与 store 分页上限一致）。
const EVENTS_PAGE_LIMIT: usize = 1000;

/// 上下文展示预算（token 数；与 TUI 上下文视图 `store_source::CONTEXT_DISPLAY_BUDGET`
/// 同口径 4096——`context budget` 缺省 `--budget` 时使用）。
const CONTEXT_DISPLAY_BUDGET: usize = 4096;

/// 人类可读输出的排版风格：强调按终端检测，方向随会话语言（FR-766／T279）。
///
/// 阿拉伯语／乌尔都语等 RTL 语言的记录与表格按镜像排版；`--json` 信封路径不经过
/// 本函数，不受影响。
fn human_styler(texts: &I18n) -> Styler {
    Styler::detect().with_locale(&texts.locale)
}

/// `whcode <command>` 入口：解析 → 执行 → 输出（返回进程退出码）。
pub fn dispatch(args: &[String]) -> ExitCode {
    exit_code(dispatch_code(args))
}

/// `whcode sdk`（T294／FR-778／ADR-071）：装配网关并进入 stdio 服务循环。
fn sdk_exit(workspace: &Path) -> i32 {
    use std::sync::{Arc, Mutex};
    use whcode_cui::output::{EXIT_SUCCESS, error_exit_code};
    let config = config_for(workspace);
    let conn = match open_db(workspace, &config) {
        Ok(conn) => Arc::new(Mutex::new(conn)),
        Err(err) => {
            eprintln!("SDK 装配失败（工作区连接）：{err}");
            return error_exit_code(&err);
        }
    };
    let llm = match crate::llm_provider::ServeLlm::from_env(workspace) {
        Ok(llm) => Arc::new(llm),
        Err(err) => {
            eprintln!("SDK 装配失败（LLM 剖面）：{err}");
            return error_exit_code(&err);
        }
    };
    let gateway = crate::sdk::AppSdkGateway {
        conn,
        llm,
        policy: Arc::new(policy()),
        actor: local_actor(),
        workspace: workspace.to_path_buf(),
    };
    let mut source = crate::sdk::StdioSource::default();
    let sink = Box::new(crate::sdk::StdioSink);
    match crate::sdk::run_sdk_server(
        Arc::new(gateway),
        crate::sdk::OUT_CHANNEL_CAPACITY,
        &mut source,
        sink,
    ) {
        Ok(()) => {
            eprintln!("SDK 会话结束（stdin EOF）");
            EXIT_SUCCESS
        }
        Err(err) => {
            eprintln!("SDK 服务异常退出：{err}");
            error_exit_code(&err)
        }
    }
}

/// ACP automation-only 服务端装配（ADR-081）：与 `sdk_exit` 同构——工作区连接、
/// LLM 剖面、顶层配置（审批规则表口径）注入网关，stdio 承载协议。
fn acp_exit(workspace: &Path, provider: Option<&str>) -> i32 {
    use std::sync::{Arc, Mutex};
    let config = config_for(workspace);
    let conn = match open_db(workspace, &config) {
        Ok(conn) => Arc::new(Mutex::new(conn)),
        Err(err) => {
            eprintln!("ACP 装配失败（工作区连接）：{err}");
            return error_exit_code(&err);
        }
    };
    let llm = match crate::llm_provider::ServeLlm::from_env(workspace) {
        Ok(llm) => Arc::new(llm),
        Err(err) => {
            eprintln!("ACP 装配失败（LLM 剖面）：{err}");
            return error_exit_code(&err);
        }
    };
    let gateway = crate::acp::AppAcpGateway {
        conn,
        llm,
        config: config.clone(),
        workspace: workspace.to_path_buf(),
    };
    let mut source = crate::sdk::StdioSource::default();
    let sink = Box::new(crate::sdk::StdioSink);
    match crate::acp::run_acp_server(
        Arc::new(gateway),
        provider.map(str::to_string),
        &mut source,
        sink,
    ) {
        Ok(()) => {
            eprintln!("ACP 会话结束（stdin EOF）");
            whcode_cui::output::EXIT_SUCCESS
        }
        Err(err) => {
            eprintln!("ACP 服务异常退出：{err}");
            error_exit_code(&err)
        }
    }
}

/// 调度实现（返回 i32 退出码，供测试直接断言稳定码表）。
fn dispatch_code(args: &[String]) -> i32 {
    // 显式语言必须先于解析确定：`--help` 的渲染发生在 clap 内部（FR-615）。
    let explicit_lang = match whcode_cui::cli::prescan_lang(args) {
        Ok(lang) => lang,
        Err(message) => {
            eprintln!("{message}");
            return EXIT_USAGE;
        }
    };
    // 文本层（FR-615）：显式 --lang → 环境检测（LANG/LC_ALL）→ 默认 zh-CN；
    // PO 覆盖内建；数据字段不翻译。
    let (locale, locale_source) = detect_locale(explicit_lang.as_deref());
    let texts = crate::load_texts(&locale, locale_source);
    let argv = std::iter::once("whcode".to_string()).chain(args.iter().cloned());
    let cli = match parse_outcome(argv, Some(&texts)) {
        ParseOutcome::Command(cli) => *cli,
        ParseOutcome::Display(text) => {
            print!("{text}");
            return 0;
        }
        ParseOutcome::Usage(text) => {
            eprint!("{text}");
            return EXIT_USAGE;
        }
    };
    let json_mode = cli.json;
    let workspace = cli
        .workspace
        .as_deref()
        .map_or_else(default_workspace, PathBuf::from);
    // 出站信任库（ADR-040 决策 2、T191）：与 TUI／GUI 同一装配口径——失败只告警、不阻断
    // 命令执行；真正的失败会在 LLM 调用时以“无信任根”报出，不在装配阶段伪装成功。
    crate::ensure_outbound_trust(&workspace, "CUI");
    // SDK（T294／FR-778／ADR-071）：长驻 stdio 服务，不走 (Value, human) 形状——
    // 帧由服务端直写 stdout，人读摘要走 stderr；退出码 0＝优雅关闭。
    if let Command::Sdk = cli.command {
        return sdk_exit(&workspace);
    }
    if let Command::Acp { provider } = &cli.command {
        return acp_exit(&workspace, provider.as_deref());
    }
    match execute(&cli.command, &workspace, &texts) {
        Ok((data, human)) => {
            if json_mode {
                match serde_json::to_string_pretty(&success_envelope(data)) {
                    Ok(text) => println!("{text}"),
                    Err(err) => {
                        eprintln!("输出序列化失败：{err}");
                        return 99;
                    }
                }
            } else {
                println!("{human}");
            }
            0
        }
        Err(err) => {
            if json_mode {
                match serde_json::to_string_pretty(&error_envelope(&err)) {
                    Ok(text) => eprintln!("{text}"),
                    Err(_) => eprintln!("错误 [{}] {}", err.code.as_str(), err.message),
                }
            } else {
                eprintln!("错误 [{}] {}", err.code.as_str(), err.message);
            }
            error_exit_code(&err)
        }
    }
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// 默认工作区：cwd（与 `Config::for_workspace` 的约定一致）。
fn default_workspace() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn config_for(workspace: &Path) -> Config {
    Config::for_workspace(workspace)
}

fn open_db(workspace: &Path, config: &Config) -> Result<Connection, WhError> {
    workspace::open_workspace_db(workspace, config)
}

fn now_ms() -> Result<u64, WhError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| WhError::internal(format!("系统时钟异常：{err}")))?;
    Ok(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

fn policy() -> DefaultPolicy {
    DefaultPolicy
}

fn local_actor() -> Actor {
    actor_with_all_capabilities("cui-local")
}

/// 破坏性命令的确认语义：默认 dry-run；`--yes` 执行；两者同时给出为用法错误。
fn confirm_flags(dry_run: bool, yes: bool) -> Result<bool, WhError> {
    if dry_run && yes {
        return Err(WhError::invalid_state(
            "--dry-run 与 --yes 互斥：预览或执行，二选一（§8.1）",
        ));
    }
    Ok(!yes)
}

/// 执行一条命令：返回 (JSON 数据, 人类可读文本)。
fn execute(command: &Command, workspace: &Path, texts: &I18n) -> Result<(Value, String), WhError> {
    // FR-619／FR-620：先登记后接线——命令所映射的登记面若在本端为预留或不适用，
    // 统一在此返回 not_supported（退出码 55），不在各命令实现里各写一份拒绝。
    // 本轮预留面已清零；机制保留为 tripwire，新增预留面自动获得同一语义。
    let surface_id = whcode_cui::surface_map::surface_of(command);
    if let Some(entry) = whcode_app::surface::find(surface_id) {
        match entry.support(whcode_app::surface::FrontendId::Cui) {
            whcode_app::surface::Support::Reserved { milestone } => {
                return Err(not_supported(
                    surface_id,
                    &format!("{surface_id} 在 CUI 端预留（里程碑 {milestone}）"),
                ));
            }
            whcode_app::surface::Support::NotApplicable => {
                return Err(not_supported(
                    surface_id,
                    &format!("{surface_id} 在 CUI 端不适用"),
                ));
            }
            _ => {}
        }
    }
    let config = config_for(workspace);
    let actor = local_actor();
    let policy = policy();
    let now = now_ms()?;
    match command {
        Command::Session { command } => {
            session_command(command, workspace, &config, &policy, &actor, now, texts)
        }
        // webhook（P6.4／FR-788／ADR-079）：显式投递，无后台循环。
        Command::Webhook { command } => webhook_command(command, workspace, &policy, &actor, texts),
        // SDK（T294／ADR-071）在 dispatch_code 拦截直跑（长驻 stdio 服务，不走
        // (Value, human) 形状）；本臂仅为穷尽性兜底，正常流程不可达。
        Command::Sdk => Err(WhError::invalid_state(
            "sdk 由调度层拦截直跑（长驻 stdio 服务），不经命令形状".to_string(),
        )),
        Command::Acp { .. } => Err(WhError::invalid_state(
            "acp 由调度层拦截直跑（长驻 stdio 服务），不经命令形状".to_string(),
        )),
        Command::Status { session } => {
            let conn = open_db(workspace, &config)?;
            let snapshot = whcode_app::build_snapshot(&conn, &policy, &actor, session)?;
            let value = json!({
                "schema_version": whcode_domain::SCHEMA_VERSION,
                "session": {
                    "id": snapshot.session.id,
                    "title": snapshot.session.title,
                    "stage": snapshot.session.stage,
                    "status": snapshot.session.status,
                },
                "event_cursor": snapshot.event_cursor,
                "overview": snapshot.overview.map(|overview| json!({
                    "total_tasks": overview.total_tasks,
                    "done": overview.done,
                    "blocked": overview.blocked,
                })),
                "active_task": snapshot.active_task.map(|task| json!({
                    "id": task.id, "status": task.status, "retry_count": task.retry_count,
                })),
            });
            let human = texts.resolve_fmt(
                "cui.status_line",
                &[
                    ("session", &snapshot.session.id),
                    ("title", &snapshot.session.title),
                    (
                        "stage",
                        snapshot.session.stage.as_deref().unwrap_or("[unknown]"),
                    ),
                    ("status", &snapshot.session.status),
                    ("cursor", &snapshot.event_cursor.to_string()),
                ],
            );
            Ok((value, human))
        }
        Command::Task { command } => {
            task_command(command, workspace, &config, &policy, &actor, now, texts)
        }
        Command::Context { command } => {
            context_command(command, workspace, &config, &policy, &actor, texts)
        }
        Command::Spec { command } => {
            spec_command(command, workspace, &config, &policy, &actor, texts)
        }
        Command::Events { command } => {
            events_command(command, workspace, &config, &policy, &actor, texts)
        }
        Command::Db { command } => {
            db_command(command, workspace, &config, &policy, &actor, now, texts)
        }
        // llm run／cancel（T167）：经 `whcode_app::llm` 真实编排（权限、会话、剖面、
        // 凭据与取消），装配根只注入 Provider 工厂与取消目录（ADR-031 决策 1）。
        Command::Llm { command } => llm_command(command, workspace, &config, &policy, &actor),
        Command::Font { command } => font_command(command, texts),
        Command::Runtime { command } => runtime_command(command, workspace, &policy, &actor),
        Command::Serve { .. } => Err(not_wired(
            "serve 由主入口装配：使用 `whcode serve --web [--bind …] [--workspace …]`",
        )),
        Command::Doctor { .. } => Err(not_wired(
            "doctor 由主入口处理：使用 `whcode doctor [--json]`",
        )),
    }
}

/// 走错入口的兜底（`serve`/`doctor` 在登记表 CUI 端为已实现，主入口已处理）：
/// 语义是“入口装配错误”而非“该端不提供”，故保持 `invalid_state`，
/// 不与 `not_supported`（退出码 55）混用。
fn not_wired(message: &str) -> WhError {
    WhError::new(
        ErrorCode::InvalidState,
        format!("未实现：{message}（显式失败，不静默成功）"),
    )
}

/// 统一“该端未提供”错误（FR-620、ADR-032；contracts/cui-cli.md §2 退出码 55）。
///
/// details 由登记表驱动（不在本处硬编码第二份状态清单）：`surface_id` 为登记
/// SurfaceId，`state` 取登记表 CUI 端状态码，预留附 `milestone`、排除附 `reason_code`。
fn not_supported(surface_id: &str, message: &str) -> WhError {
    WhError::not_supported(format!("未实现：{message}（显式失败，不静默成功）"))
        .with_details(registry_details(surface_id))
}

/// 由登记表构造 `not_supported` 详情：surface_id + CUI 端状态 + 里程碑/理由码。
fn registry_details(surface_id: &str) -> Value {
    let Some(entry) = whcode_app::surface::find(surface_id) else {
        // 未登记面：不伪造状态码（缺 state 即装配错误信号，由测试拦截）。
        return json!({ "surface_id": surface_id });
    };
    let support = entry.support(FrontendId::Cui);
    let mut details = json!({
        "surface_id": surface_id,
        "state": support.state_code(),
    });
    match support {
        Support::Reserved { milestone } => {
            details["milestone"] = json!(milestone);
        }
        Support::Excluded { reason } => {
            details["reason_code"] = json!(reason.as_str());
        }
        _ => {}
    }
    details
}

/// `llm run`／`llm cancel`（T167、FR-201—208、ADR-031）。
///
/// 剖面来自 `WHCODE_LLM_PROFILES`（仓库外 JSON），凭据由剖面 `auth` 声明（环境变量或
/// 仓库外文件）；两者都不进入输出、事件与日志。真实 Provider 由装配根工厂注入——
/// 客户端未就绪时返回结构化错误，不伪造成功（T163）。
fn llm_command(
    command: &LlmCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
) -> Result<(Value, String), WhError> {
    let factory = crate::llm_provider::LocalProviderFactory::for_workspace(workspace)?;
    let cancel_dir = whcode_app::llm::cancel_dir_from_env();
    let (registry, registry_error) = match whcode_app::llm::ProviderRegistry::from_env() {
        Ok(registry) => (Some(registry), None),
        Err(err) => (None, Some(err.message)),
    };
    // shell 治理束（T290／ADR-070）：未 opt-in 或 bash 不可用为 None；配置写错显式失败。
    let shell = crate::shell_tool::build_governance(workspace)?;
    // LSP 执行器（P5.3b／ADR-078）：未配置 `.whcode/lsp.json` 为 None；坏配置显式失败。
    let lsp = crate::lsp_tool::build_executor(workspace)?;
    let context = whcode_app::llm::LlmContext {
        registry: registry.as_ref(),
        registry_error: registry_error.as_deref(),
        factory: &factory,
        cancel_dir: &cancel_dir,
        shell,
        lsp,
    };
    match command {
        LlmCommand::Run {
            session,
            provider,
            prompt,
            model,
            max_tokens,
            request_id,
            trace_level,
            approve,
        } => {
            // 追踪级别是显式输入：未知取值直接拒绝，不静默回落到默认档。
            let trace_level = parse_trace_level(trace_level)?;
            let mut conn = open_db(workspace, config)?;
            // ── 逐次显式授权（ADR-090）：`--approve` 授权范围 → 审批闸门 Allow，
            // 未授权写类 Ask→无应答者 fail-closed（默认拒绝不变）；shell 额外受
            // `.whcode/shell.json` 白名单闸门组合约束；闸门审计照常落表。
            let mut granted_write = false;
            let mut granted_process = false;
            for scope in approve {
                // 缺省时 clap 的 default_value 产生空串项——空项＝未授权，跳过。
                let scope = scope.trim();
                if scope.is_empty() {
                    continue;
                }
                match scope {
                    "workspace-write" => granted_write = true,
                    "process" => granted_process = true,
                    other => {
                        return Err(WhError::invalid_state(format!(
                            "未知授权范围：{other}（允许 workspace-write／process）"
                        )));
                    }
                }
            }
            // ── 四端写姿态统一（ADR-090 修订，用户裁定：CUI 默认闸门＋审计）──
            // 闸门恒建（规则默认 Allow——执行行为不变，写调用审计从此落表）；
            // stored rules 照常叠加（用户配置的 ask 规则在 CUI 生效）；`--approve`
            // 为操作者逐次宣告——在 stored 之后应用（操作者即应答者，覆盖 ask）；
            // shell 仍受白名单闸门组合约束。plan 退出与破坏性内置 ask 不变。
            let tool_registry = whcode_tools::ToolRegistry::new();
            let mut approval_policy = whcode_app::ApprovalPolicy::new(
                whcode_domain::approval::ApprovalRule::Allow,
                whcode_domain::approval::ApprovalRule::Allow,
            )
            .with_builtin_destructive_ask()
            .with_stored_rules(
                whcode_store::approval_rules::list_rules(&conn)?
                    .into_iter()
                    .map(|row| (row.tool, row.rule)),
            )?;
            if granted_write {
                for descriptor in tool_registry.descriptors() {
                    if descriptor.side_effect_scope == whcode_tools::SideEffectScope::WorkspaceWrite
                    {
                        approval_policy = approval_policy.with_tool_rule(
                            descriptor.name,
                            whcode_domain::approval::ApprovalRule::Allow,
                        )?;
                    }
                }
            }
            if granted_process {
                approval_policy = approval_policy
                    .with_tool_rule("shell", whcode_domain::approval::ApprovalRule::Allow)?;
            }
            let mut gate = whcode_app::approval::ApprovalGate::for_store(
                "cui-local",
                approval_policy,
                None,
                1_000,
            );
            let request = whcode_app::turn::TurnRequest {
                session_id: session.clone(),
                provider: provider.clone(),
                prompt: prompt.clone(),
                model: model.clone(),
                max_tokens: *max_tokens,
                request_id: request_id.clone(),
                iterations_limit: None,
                budget_tokens: None,
                workspace_root: Some(workspace.to_path_buf()),
                depth: 0,
            };
            let now = now_ms()?;
            // 四端写姿态统一：CUI 恒走带闸门路径（执行不变，审计从此落表）。
            let outcome = whcode_app::turn::run_turn_with_approval(
                &mut conn, policy, actor, &context, &request, None, None, &mut gate, now,
            )?;
            let mut value = serde_json::to_value(&outcome)
                .map_err(|err| WhError::internal(format!("LLM 结果序列化失败：{err}")))?;
            value["trace_level"] = json!(trace_level.as_str());
            // 人读输出：正文 + 稳定键值摘要（数据字段不翻译，FR-615 口径）。
            let human = format!(
                "{}\nprovider={} model={} protocol={} chunks={} completed={} truncated={} \
                 integrity_gaps={} finish_reason={}\nrequest_id={}",
                outcome.text,
                outcome.provider,
                outcome.model,
                outcome.protocol,
                outcome.chunks.len(),
                outcome.completed,
                outcome.truncated,
                outcome.integrity_gaps,
                outcome.finish_reason.as_deref().unwrap_or("none"),
                outcome.request_id,
            );
            Ok((value, human))
        }
        LlmCommand::Cancel { request } => {
            let outcome = whcode_app::llm::cancel(policy, actor, &context, request)?;
            let value = serde_json::to_value(&outcome)
                .map_err(|err| WhError::internal(format!("取消失败结果序列化失败：{err}")))?;
            let human = format!(
                "request={} state={} effective={}",
                outcome.request_id, outcome.state, outcome.effective
            );
            Ok((value, human))
        }
    }
}

/// 追踪级别解析（`off`／`metadata`／`redacted`／`full`；未知值显式失败）。
fn parse_trace_level(value: &str) -> Result<TraceLevel, WhError> {
    match value {
        "off" => Ok(TraceLevel::Off),
        "metadata" => Ok(TraceLevel::Metadata),
        "redacted" => Ok(TraceLevel::Redacted),
        "full" => Ok(TraceLevel::Full),
        other => Err(WhError::parse_error(format!(
            "未知追踪级别“{other}”；合法取值：off/metadata/redacted/full（不得静默回落）"
        ))),
    }
}

/// `context` 命令（FR-105/108；T173）：预算视图与 TUI 上下文视图同源
/// （`whcode_app::load_context`），缺省预算用展示默认口径。
fn context_command(
    command: &ContextCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    let ContextCommand::Budget { session, budget } = command;
    let budget = budget.unwrap_or(CONTEXT_DISPLAY_BUDGET);
    let conn = open_db(workspace, config)?;
    // 缺会话 → NotFound（既有语义，FR-622“正确的空”）：先经应用服务确认会话存在，
    // 不把“会话不存在”伪装成空的预算窗口。
    whcode_app::show_session(&conn, policy, actor, session)?;
    let value = whcode_cui::context_cmd::budget_view(&conn, policy, actor, session, budget)?;
    let styler = human_styler(texts);
    let title = texts.resolve_fmt("cui.record.session", &[("session", session)]);
    let none = texts.resolve("summary.none");
    let budget_tokens = value["budget_tokens"].to_string();
    let consumed = value["consumed_budget"].to_string();
    let items = value["items"].to_string();
    let truncated = value["truncated_reason"].as_str().unwrap_or(none);
    let cursor = value["next_cursor"]["id"].as_str().unwrap_or(none);
    let human = styler.record(
        Some(&title),
        &[
            (texts.resolve("cui.record.budget"), budget_tokens.as_str()),
            (texts.resolve("cui.record.used"), consumed.as_str()),
            (texts.resolve("cui.record.items"), items.as_str()),
            (texts.resolve("cui.record.truncated"), truncated),
            (texts.resolve("cui.record.cursor"), cursor),
        ],
    );
    Ok((value, human))
}

/// `runtime` 命令（FR-701—710；T173）：`list` 与 Web `GET /api/runtimes` 同形
/// （schema_version/wired/runtimes，能力等级名不翻译）；其余子命令属 M8，
/// 统一 `not_supported`（退出码 55，FR-620）。清单来自声明读取，不触库。
fn runtime_command(
    command: &RuntimeCommand,
    workspace: &Path,
    policy: &DefaultPolicy,
    actor: &Actor,
) -> Result<(Value, String), WhError> {
    match command {
        RuntimeCommand::List => {
            // 界面路径不探测子进程（probe=false，Check 101）：只展示声明与不支持项。
            let value = json!({
                "schema_version": whcode_domain::SCHEMA_VERSION,
                "wired": true,
                "runtimes": crate::runtimes::inventory(false),
            });
            Ok((value, crate::runtimes::summary(false)))
        }
        // T395（ADR-089 批 6）：管理命令面翻实现——状态落 runtime_state 表＋审计；
        // js/bsdshell 结构化拒绝（不伪造启用）；status 显式命令路径允许探测。
        RuntimeCommand::Status { name } => {
            let conn = open_db(workspace, &config_for(workspace))?;
            let view = whcode_app::runtime_status(&conn, policy, actor, name, true)?;
            let human = format!(
                "{} [{}] enabled={} version={}{}",
                view.name,
                view.capability,
                view.enabled,
                if view.version.is_empty() {
                    "未安装"
                } else {
                    &view.version
                },
                view.available
                    .map(|available| if available {
                        " · 可用"
                    } else {
                        " · 不可用"
                    })
                    .unwrap_or_default(),
            );
            let value = serde_json::to_value(&view)
                .map_err(|err| WhError::internal(format!("运行时状态序列化失败：{err}")))?;
            Ok((value, human))
        }
        RuntimeCommand::Enable { name } => {
            let conn = open_db(workspace, &config_for(workspace))?;
            let view = whcode_app::runtime_enable(&conn, policy, actor, name, now_ms()?)?;
            Ok((
                serde_json::to_value(&view)
                    .map_err(|err| WhError::internal(format!("运行时状态序列化失败：{err}")))?,
                format!(
                    "{} 已启用（enable 只写状态；可用性取决于本机解释器/payload）",
                    view.name
                ),
            ))
        }
        RuntimeCommand::Disable { name } => {
            let conn = open_db(workspace, &config_for(workspace))?;
            let view = whcode_app::runtime_disable(&conn, policy, actor, name, now_ms()?)?;
            Ok((
                serde_json::to_value(&view)
                    .map_err(|err| WhError::internal(format!("运行时状态序列化失败：{err}")))?,
                format!("{} 已停用", view.name),
            ))
        }
        RuntimeCommand::Install {
            name,
            version,
            hash,
            payload,
        } => {
            let conn = open_db(workspace, &config_for(workspace))?;
            let view = whcode_app::runtime_install(
                &conn,
                policy,
                actor,
                name,
                version,
                hash,
                Path::new(payload),
                now_ms()?,
            )?;
            Ok((
                serde_json::to_value(&view)
                    .map_err(|err| WhError::internal(format!("运行时状态序列化失败：{err}")))?,
                format!(
                    "{} 安装已登记（version={}；哈希比对通过，FR-703）",
                    view.name, view.version
                ),
            ))
        }
    }
}

/// `webhook` 命令（P6.4／FR-788／ADR-079）。
///
/// 配置缺失＝零端点（不是错误）：`status` 明确说“未配置端点”，`deliver` 明确回 0 条。
/// 配置写错是显式错误（`load_config` 报出具体字段与上限），不降级成“未配置”。
/// 投递默认 dry-run；`--yes` 才认领并发送，凭据由装配根的发送器按次求值。
fn webhook_command(
    command: &WebhookCommand,
    workspace: &Path,
    policy: &DefaultPolicy,
    actor: &Actor,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    let config = whcode_app::webhooks::load_config(workspace)?;
    match command {
        WebhookCommand::Status => {
            let conn = open_db(workspace, &config_for(workspace))?;
            let view = whcode_app::webhooks::status(&conn, policy, actor, &config)?;
            let recent: Vec<Value> = view
                .recent
                .iter()
                .map(|item| {
                    json!({
                        "idempotency_key": item.idempotency_key,
                        "status": item.status,
                        "attempts": item.attempts,
                        "last_status_code": item.last_status_code,
                        "last_error": item.last_error,
                    })
                })
                .collect();
            let value = json!({
                "schema_version": whcode_domain::SCHEMA_VERSION,
                "wired": true,
                "configured": !config.is_inert(),
                "endpoints": view.endpoints,
                "enabled_endpoints": view.enabled_endpoints,
                "counts": {
                    "claimed": view.counts[0],
                    "delivered": view.counts[1],
                    "failed": view.counts[2],
                },
                "recent": recent,
            });
            let human = if config.is_inert() {
                texts.resolve("cui.webhook_not_configured").to_string()
            } else {
                texts.resolve_fmt(
                    "cui.webhook_status",
                    &[
                        ("endpoints", &view.endpoints.to_string()),
                        ("enabled", &view.enabled_endpoints.to_string()),
                        ("claimed", &view.counts[0].to_string()),
                        ("delivered", &view.counts[1].to_string()),
                        ("failed", &view.counts[2].to_string()),
                    ],
                )
            };
            Ok((value, human))
        }
        WebhookCommand::Deliver { dry_run, yes } => {
            let is_dry_run = confirm_flags(*dry_run, *yes)?;
            if config.is_inert() {
                let value = json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "wired": true,
                    "configured": false,
                    "planned": 0,
                    "attempted": 0,
                    "delivered": 0,
                    "skipped": 0,
                });
                return Ok((
                    value,
                    texts.resolve("cui.webhook_not_configured").to_string(),
                ));
            }
            let conn = open_db(workspace, &config_for(workspace))?;
            if is_dry_run {
                let plan = whcode_app::webhooks::plan(&conn, policy, actor, &config, None)?;
                let value = json!({
                    "schema_version": whcode_domain::SCHEMA_VERSION,
                    "wired": true,
                    "configured": true,
                    "dry_run": true,
                    "planned": plan.len(),
                });
                let human = texts.resolve_fmt(
                    "cui.webhook_dry_run",
                    &[("planned", &plan.len().to_string())],
                );
                return Ok((value, human));
            }
            let sender = crate::webhook_sender::LlmWebhookSender;
            let summary =
                whcode_app::webhooks::deliver(&conn, policy, actor, &config, &sender, None)?;
            let value = json!({
                "schema_version": whcode_domain::SCHEMA_VERSION,
                "wired": true,
                "configured": true,
                "dry_run": false,
                "attempted": summary.attempted,
                "delivered": summary.delivered,
                "retryable_failures": summary.retryable_failures,
                "permanent_failures": summary.permanent_failures,
                "skipped": summary.skipped,
            });
            let human = texts.resolve_fmt(
                "cui.webhook_delivered",
                &[
                    ("attempted", &summary.attempted.to_string()),
                    ("delivered", &summary.delivered.to_string()),
                    ("retryable", &summary.retryable_failures.to_string()),
                    ("permanent", &summary.permanent_failures.to_string()),
                ],
            );
            Ok((value, human))
        }
    }
}

/// HKCU Fonts 键的 reg.exe 实现（glue：`whcode-platform` 提供材料，此处只接线；
/// 应用层 [`whcode_app::fonts::FontRegistry`] 的真实实现，同 `ViewSource` 注入模式）。
struct PlatformFontRegistry;

impl whcode_app::fonts::FontRegistry for PlatformFontRegistry {
    fn query_value(&self, value_name: &str) -> Result<Option<String>, WhError> {
        whcode_platform::fonts::reg_query_value(value_name)
    }

    fn set_value(&self, value_name: &str, data: &str) -> Result<(), WhError> {
        whcode_platform::fonts::reg_set_value(value_name, data)
    }

    fn delete_value(&self, value_name: &str) -> Result<bool, WhError> {
        whcode_platform::fonts::reg_delete_value(value_name)
    }
}

/// `font` 命令（FR-717）：本机资源操作，不触库、不经会话授权、无工作区要求。
fn font_command(command: &FontCommand, texts: &I18n) -> Result<(Value, String), WhError> {
    let env = whcode_app::fonts::FontEnv {
        registry: &PlatformFontRegistry,
        user_fonts_dir: whcode_platform::fonts::user_fonts_dir(),
        terminal_settings: whcode_platform::fonts::windows_terminal_settings_paths(),
    };
    match command {
        FontCommand::Install { from, dry_run } => {
            let report =
                whcode_app::fonts::install(&env, from.as_deref().map(Path::new), *dry_run)?;
            let dry_prefix = if *dry_run {
                texts.resolve("cui.font.dry_run").to_string()
            } else {
                String::new()
            };
            let mut lines = vec![texts.resolve_fmt(
                "cui.font.install.header",
                &[
                    ("dry_run", &dry_prefix),
                    ("source", &report.source_dir),
                    ("dir", &report.fonts_dir),
                ],
            )];
            for item in &report.installed {
                let reg_state = if item.registered {
                    texts.resolve("cui.font.registered")
                } else {
                    texts.resolve("cui.font.reg_pending")
                };
                lines.push(texts.resolve_fmt(
                    "cui.font.install.line",
                    &[
                        ("family", &item.family),
                        ("source", &item.source),
                        ("target", &item.target),
                        ("reg_state", reg_state),
                    ],
                ));
            }
            lines.extend(report.terminal_hint.iter().cloned());
            Ok((report_value(&report)?, lines.join("\n")))
        }
        FontCommand::Status => {
            let report = whcode_app::fonts::status(&env);
            let mut lines =
                vec![texts.resolve_fmt("cui.font.status.header", &[("dir", &report.fonts_dir)])];
            for font in &report.fonts {
                let file_state = if font.file_installed {
                    texts.resolve("cui.font.installed")
                } else {
                    texts.resolve("cui.font.missing")
                };
                let reg_state = if font.registered {
                    texts.resolve("cui.font.registered")
                } else {
                    texts.resolve("cui.font.unregistered")
                };
                lines.push(texts.resolve_fmt(
                    "cui.font.status.line",
                    &[
                        ("family", &font.family),
                        ("file", &font.file_name),
                        ("file_state", file_state),
                        ("reg_state", reg_state),
                    ],
                ));
            }
            match &report.terminal.settings_file {
                Some(file) => {
                    let faces = report.terminal.declared_faces.join(", ");
                    let hits = report.terminal.builtin_in_use.join(", ");
                    lines.push(texts.resolve_fmt(
                        "cui.font.terminal.found",
                        &[("file", file), ("faces", &faces), ("hits", &hits)],
                    ));
                }
                None => lines.push(texts.resolve("cui.font.terminal.none").to_string()),
            }
            if let Some(note) = &report.terminal.note {
                lines.push(note.clone());
            }
            Ok((report_value(&report)?, lines.join("\n")))
        }
        FontCommand::Uninstall { dry_run } => {
            let report = whcode_app::fonts::uninstall(&env, *dry_run)?;
            let dry_prefix = if *dry_run {
                texts.resolve("cui.font.dry_run").to_string()
            } else {
                String::new()
            };
            let mut lines =
                vec![texts.resolve_fmt("cui.font.uninstall.header", &[("dry_run", &dry_prefix)])];
            for item in &report.removed {
                let unreg_state = if item.unregistered {
                    texts.resolve("cui.font.removed")
                } else {
                    texts.resolve("cui.font.unregistered")
                };
                let file_state = if item.file_deleted {
                    texts.resolve("cui.font.deleted")
                } else {
                    texts.resolve("cui.font.missing")
                };
                lines.push(texts.resolve_fmt(
                    "cui.font.uninstall.line",
                    &[
                        ("family", &item.family),
                        ("unreg_state", unreg_state),
                        ("file_state", file_state),
                    ],
                ));
                if let Some(error) = &item.file_error {
                    lines.push(error.clone());
                }
            }
            Ok((report_value(&report)?, lines.join("\n")))
        }
    }
}

/// 报告结构 → JSON 数据（serde 失败视为内部错误，不静默丢字段）。
fn report_value<T: serde::Serialize>(report: &T) -> Result<Value, WhError> {
    serde_json::to_value(report)
        .map_err(|err| WhError::new(ErrorCode::Internal, format!("报告序列化失败：{err}")))
}

fn session_command(
    command: &SessionCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
    now: u64,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    match command {
        SessionCommand::New { title, model } => {
            let mut conn = open_db(workspace, config)?;
            // 写命令经门面（FR-623）：与 TUI/Web 同一执行入口与保护口径。
            let outcome = whcode_app::execute_command(
                &mut conn,
                policy,
                actor,
                &whcode_app::FrontendCommand::SessionNew {
                    title: title.clone(),
                    model: model.clone(),
                    now_ms: now,
                },
            )?;
            let whcode_app::CommandOutcome::Session(row) = outcome else {
                return Err(whcode_domain::WhError::internal(
                    "门面返回形状不符：应为 Session",
                ));
            };
            Ok((session_json(&row), row_line(&row)))
        }
        SessionCommand::List { all } => {
            let conn = open_db(workspace, config)?;
            let rows = whcode_app::list_sessions(&conn, policy, actor, *all, 200)?;
            let human = sessions_table(&rows, texts);
            Ok((
                json!({ "sessions": rows.iter().map(row_value).collect::<Vec<_>>() }),
                human,
            ))
        }
        SessionCommand::Search { query, limit } => {
            // 内容检索（P3.3／ADR-076）：只读、有界；片段正文不写日志。
            let conn = open_db(workspace, config)?;
            let hits = whcode_app::search_session_content(
                &conn,
                policy,
                actor,
                &whcode_app::ContentSearchQuery {
                    needle: query.clone(),
                    limit: limit.unwrap_or(whcode_app::CONTENT_SEARCH_DEFAULT_LIMIT),
                },
            )?;
            let values: Vec<Value> = hits
                .iter()
                .map(|hit| {
                    json!({
                        "session_id": hit.session_id,
                        "session_title": hit.session_title,
                        "message_id": hit.message_id,
                        "role": hit.role,
                        "snippet": hit.snippet,
                    })
                })
                .collect();
            Ok((
                json!({ "hits": values, "count": hits.len() }),
                search_hits_table(&hits, texts),
            ))
        }
        SessionCommand::Show { session } => {
            let conn = open_db(workspace, config)?;
            let row = whcode_app::show_session(&conn, policy, actor, session)?;
            Ok((session_json(&row), session_record(&row, texts)))
        }
        SessionCommand::Switch { session } => {
            // 会话切换（FR-102／T390）：有界加载——元数据＋概览＋活跃尾部＋游标。
            let conn = open_db(workspace, config)?;
            let view = whcode_app::session::switch_session(&conn, policy, actor, session)?;
            let value = json!({
                "session": row_value(&view.session),
                "overview": view.overview.as_ref().map(|overview| json!({
                    "total": overview.total,
                    "done": overview.done,
                    "blocked": overview.blocked,
                    "refreshed_at_us": overview.refreshed_at_us,
                })),
                "tail_count": view.tail.len(),
                "tail_complete": view.tail_complete,
                "event_cursor": view.event_cursor,
            });
            let styler = human_styler(texts);
            let title = texts.resolve_fmt("cui.record.session", &[("session", session)]);
            let human = styler.record(
                Some(&title),
                &[
                    (texts.resolve("tui.col.stage"), view.session.stage.as_str()),
                    (
                        texts.resolve("tui.col.status"),
                        view.session.status.as_str(),
                    ),
                    ("tail", &view.tail.len().to_string()),
                    ("cursor", &view.event_cursor.to_string()),
                ],
            );
            Ok((value, human))
        }
        SessionCommand::Share { session, out } => {
            let conn = open_db(workspace, config)?;
            let export = whcode_app::share::export_session(
                &conn,
                policy,
                actor,
                session,
                Path::new(out),
                now,
            )?;
            let human = format!(
                "share path={} sha256={} events={} tasks={} truncated={}",
                export.path, export.sha256, export.events, export.tasks, export.truncated
            );
            Ok((report_value(&export)?, human))
        }
        SessionCommand::Fork {
            session,
            at_seq,
            title,
        } => {
            let mut conn = open_db(workspace, config)?;
            let outcome = whcode_app::session::fork_session(
                &mut conn,
                policy,
                actor,
                session,
                *at_seq,
                title.as_deref(),
                now,
            )?;
            let human = format!(
                "fork child={} source={} at_seq={} events={} messages={}",
                outcome.session.id,
                outcome.source_session,
                outcome.forked_at_seq,
                outcome.inherited_events,
                outcome.inherited_messages
            );
            let value = serde_json::to_value(&outcome).map_err(|err| {
                whcode_domain::WhError::parse_error(format!("分叉结果序列化失败：{err}"))
            })?;
            Ok((value, human))
        }
        SessionCommand::Restore {
            session,
            exec_id,
            yes,
        } => {
            // 文件前像恢复（T289／FR-776／ADR-068）：破坏族——默认 dry-run，--yes 执行。
            let conn = open_db(workspace, config)?;
            if !*yes {
                let rows = whcode_app::preview_preimage(&conn, policy, actor, session, exec_id)?;
                let items: Vec<Value> = rows
                    .iter()
                    .map(|row| {
                        json!({
                            "path": row.path,
                            "size_bytes": row.size_bytes,
                            "content": row.content.is_some(),
                            "skipped_reason": row.skipped_reason,
                        })
                    })
                    .collect();
                let line = texts.resolve("cui.restore.preview");
                return Ok((
                    json!({ "session": session, "exec_id": exec_id, "dry_run": true, "items": items }),
                    line.to_string(),
                ));
            }
            let results =
                whcode_app::restore_preimage(&conn, policy, actor, session, workspace, exec_id)?;
            let line = texts.resolve("cui.restore.done");
            Ok((
                json!({
                    "session": session,
                    "exec_id": exec_id,
                    "restored": results.iter().map(|(path, outcome)| json!({
                        "path": path,
                        "outcome": outcome,
                    })).collect::<Vec<_>>(),
                }),
                line.to_string(),
            ))
        }
        SessionCommand::Rewind { session, to_seq } => {
            // 对话剪枝（T288／FR-775／ADR-068）：只落边界，事件与消息全部保留。
            let conn = open_db(workspace, config)?;
            let view = whcode_app::rewind_session(&conn, policy, actor, session, *to_seq)?;
            let line = texts.resolve_fmt("cui.rewind.done", &[("seq", &to_seq.to_string())]);
            Ok((
                json!({
                    "session": session,
                    "target_seq": view.target_seq,
                    "rewind_seq": view.rewind_seq,
                }),
                line.to_string(),
            ))
        }
        SessionCommand::Plan { session, off } => {
            // 计划模式（T287／FR-774／ADR-067）：用户即决策者，不经审批；仍落事件。
            let conn = open_db(workspace, config)?;
            let view = whcode_app::set_plan(&conn, policy, actor, session, !*off)?;
            let line = texts.resolve(if view.active {
                "cui.plan.on"
            } else {
                "cui.plan.off"
            });
            Ok((
                json!({
                    "session": session,
                    "plan_mode": view.active,
                    "seq": view.seq,
                }),
                line.to_string(),
            ))
        }
        SessionCommand::Rename { session, title } => {
            let mut conn = open_db(workspace, config)?;
            let outcome = whcode_app::execute_command(
                &mut conn,
                policy,
                actor,
                &whcode_app::FrontendCommand::SessionRename {
                    session_id: session.clone(),
                    title: title.clone(),
                    now_ms: now,
                },
            )?;
            let whcode_app::CommandOutcome::Session(row) = outcome else {
                return Err(whcode_domain::WhError::internal(
                    "门面返回形状不符：应为 Session",
                ));
            };
            Ok((session_json(&row), row_line(&row)))
        }
        SessionCommand::Archive {
            session,
            unarchive,
            dry_run,
            yes,
        } => {
            let is_dry_run = confirm_flags(*dry_run, *yes)?;
            let mut conn = open_db(workspace, config)?;
            let archived = !*unarchive;
            if is_dry_run {
                let outcome = whcode_app::execute_command(
                    &mut conn,
                    policy,
                    actor,
                    &whcode_app::FrontendCommand::SessionArchive {
                        session_id: session.clone(),
                        archived,
                        confirm: false,
                        now_ms: now,
                    },
                )?;
                let whcode_app::CommandOutcome::SessionArchivePreview {
                    session: row,
                    target_archived,
                } = outcome
                else {
                    return Err(whcode_domain::WhError::internal(
                        "门面返回形状不符：应为归档预览",
                    ));
                };
                let value = json!({
                    "dry_run": true,
                    "session": session,
                    "current_archived": row.archived,
                    "archived": target_archived,
                    "changed": row.archived != target_archived,
                });
                let key = if archived {
                    "cui.archive_dry_run"
                } else {
                    "cui.unarchive_dry_run"
                };
                let human = texts.resolve_fmt(key, &[("session", session)]);
                return Ok((value, human));
            }
            let outcome = whcode_app::execute_command(
                &mut conn,
                policy,
                actor,
                &whcode_app::FrontendCommand::SessionArchive {
                    session_id: session.clone(),
                    archived,
                    confirm: true,
                    now_ms: now,
                },
            )?;
            let whcode_app::CommandOutcome::Session(row) = outcome else {
                return Err(whcode_domain::WhError::internal(
                    "门面返回形状不符：应为 Session",
                ));
            };
            let key = if archived {
                "cui.archived"
            } else {
                "cui.unarchived"
            };
            let human = texts.resolve_fmt(key, &[("session", session)]);
            Ok((session_json(&row), human))
        }
        SessionCommand::Delete {
            session,
            dry_run,
            yes,
        } => {
            let is_dry_run = confirm_flags(*dry_run, *yes)?;
            let mut conn = open_db(workspace, config)?;
            if is_dry_run {
                let outcome = whcode_app::execute_command(
                    &mut conn,
                    policy,
                    actor,
                    &whcode_app::FrontendCommand::SessionDelete {
                        session_id: session.clone(),
                        confirm: false,
                        now_ms: now,
                    },
                )?;
                let whcode_app::CommandOutcome::SessionDeletePreview(preview) = outcome else {
                    return Err(whcode_domain::WhError::internal(
                        "门面返回形状不符：应为删除预览",
                    ));
                };
                let value = json!({
                    "dry_run": true,
                    "session": session,
                    "selected_sessions": preview.selected_sessions,
                    "skipped_protected": preview.skipped_protected,
                    "counts": counts_json(&preview.counts),
                });
                let human = texts.resolve_fmt(
                    "cui.delete_dry_run",
                    &[
                        ("session", session),
                        ("events", &preview.counts.events.to_string()),
                        ("tasks", &preview.counts.tasks.to_string()),
                        ("skipped", &preview.skipped_protected.len().to_string()),
                    ],
                );
                return Ok((value, human));
            }
            let outcome = whcode_app::execute_command(
                &mut conn,
                policy,
                actor,
                &whcode_app::FrontendCommand::SessionDelete {
                    session_id: session.clone(),
                    confirm: true,
                    now_ms: now,
                },
            )?;
            let whcode_app::CommandOutcome::SessionDeleted(result) = outcome else {
                return Err(whcode_domain::WhError::internal(
                    "门面返回形状不符：应为删除结果",
                ));
            };
            let value = json!({
                "dry_run": false,
                "deleted_sessions": result.deleted_sessions,
                "skipped_protected": result.skipped_protected,
                "deleted": counts_json(&result.deleted),
            });
            Ok((
                value,
                texts.resolve_fmt("cui.deleted", &[("session", session)]),
            ))
        }
    }
}

fn task_command(
    command: &TaskCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
    now: u64,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    match command {
        TaskCommand::List { session } => {
            let conn = open_db(workspace, config)?;
            let overview = whcode_app::task_overview(&conn, policy, actor, Some(session))?;
            let active = overview
                .active
                .iter()
                .map(|row| {
                    json!({
                        "id": row.id,
                        "title": row.title,
                        "status": row.status.as_str(),
                        "retry_count": row.retry_count,
                    })
                })
                .collect::<Vec<_>>();
            let value = json!({
                "session_id": overview.session_id,
                "total": overview.total,
                "done": overview.done,
                "blocked": overview.blocked,
                "by_stage": overview.by_stage,
                "active": active,
            });
            let human = texts.resolve_fmt(
                "cui.tasks_summary",
                &[
                    ("session", session),
                    ("total", &overview.total.to_string()),
                    ("done", &overview.done.to_string()),
                    ("blocked", &overview.blocked.to_string()),
                    ("active", &overview.active.len().to_string()),
                ],
            );
            Ok((value, human))
        }
        TaskCommand::Show { task } => {
            let conn = open_db(workspace, config)?;
            let detail = whcode_app::task_detail(&conn, policy, actor, task)?;
            let value = json!({
                "task": {
                    "id": detail.task.id,
                    "title": detail.task.title,
                    "status": detail.task.status.as_str(),
                    "stage": detail.task.stage,
                    "retry_count": detail.task.retry_count,
                    "parent_id": detail.task.parent_id,
                },
                "acceptance": detail.acceptance,
                "checkpoints": detail.checkpoints.len(),
                "artifacts": detail.artifacts.len(),
                "failure_reason": detail.failure_reason,
            });
            let human = task_record(&detail, texts);
            Ok((value, human))
        }
        TaskCommand::Retry { task } => {
            let mut conn = open_db(workspace, config)?;
            let outcome = whcode_app::execute_command(
                &mut conn,
                policy,
                actor,
                &whcode_app::FrontendCommand::TaskRetry {
                    task_id: task.clone(),
                    now_ms: now,
                },
            )?;
            let whcode_app::CommandOutcome::Task(row) = outcome else {
                return Err(whcode_domain::WhError::internal(
                    "门面返回形状不符：应为 Task",
                ));
            };
            Ok((
                task_json(&row),
                format!(
                    "{} 已重试（retry={}）→ {}",
                    row.id,
                    row.retry_count,
                    row.status.as_str()
                ),
            ))
        }
        TaskCommand::Abort { task } => {
            let mut conn = open_db(workspace, config)?;
            let outcome = whcode_app::execute_command(
                &mut conn,
                policy,
                actor,
                &whcode_app::FrontendCommand::TaskAbort {
                    task_id: task.clone(),
                    now_ms: now,
                },
            )?;
            let whcode_app::CommandOutcome::Task(row) = outcome else {
                return Err(whcode_domain::WhError::internal(
                    "门面返回形状不符：应为 Task",
                ));
            };
            Ok((
                task_json(&row),
                format!("{} 已中止 → {}", row.id, row.status.as_str()),
            ))
        }
    }
}

fn spec_command(
    command: &SpecCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    match command {
        SpecCommand::Status { session } => {
            let conn = open_db(workspace, config)?;
            let snapshot = whcode_app::build_snapshot(&conn, policy, actor, session)?;
            let value = json!({
                "session_id": snapshot.session.id,
                "stage": snapshot.session.stage,
                "status": snapshot.session.status,
                "event_cursor": snapshot.event_cursor,
                "overview": snapshot.overview.map(|overview| json!({
                    "total_tasks": overview.total_tasks,
                    "done": overview.done,
                    "blocked": overview.blocked,
                    "refreshed_at_ms": overview.refreshed_at_ms,
                })),
            });
            let styler = human_styler(texts);
            let title = texts.resolve_fmt("cui.record.session", &[("session", session)]);
            let human = styler.record(
                Some(&title),
                &[
                    (
                        texts.resolve("tui.col.stage"),
                        snapshot.session.stage.as_deref().unwrap_or("[unknown]"),
                    ),
                    (
                        texts.resolve("tui.col.status"),
                        snapshot.session.status.as_str(),
                    ),
                    (
                        texts.resolve("cui.record.cursor"),
                        &snapshot.event_cursor.to_string(),
                    ),
                ],
            );
            Ok((value, human))
        }
        // SDD 门禁命令（T394／FR-307—312）：应用层编排已交付，工作区 specs/ 产物
        // 装载 → 解析 → 门禁判定（四端同源，FR-314）。门禁失败保持 gate_failed 语义。
        SpecCommand::Next { session, to } => {
            let conn = open_db(workspace, config)?;
            let row =
                whcode_app::spec_next(&conn, policy, actor, session, workspace, to.as_deref())?;
            Ok((
                json!({
                    "session_id": row.id,
                    "stage": row.stage,
                    "advanced": true,
                }),
                format!("阶段推进 → {}", row.stage),
            ))
        }
        SpecCommand::Validate { session } => {
            let conn = open_db(workspace, config)?;
            let view = whcode_app::spec_validate(&conn, policy, actor, session, workspace)?;
            let human = if view.passed {
                format!("校验通过（阶段 {}）", view.stage)
            } else {
                let missing: Vec<String> = view
                    .artifacts
                    .iter()
                    .filter(|a| !a.present || !a.clean)
                    .map(|a| a.name.to_string())
                    .collect();
                format!("校验未通过（阶段 {}）：{}", view.stage, missing.join(", "))
            };
            let value = serde_json::to_value(&view)
                .map_err(|err| WhError::internal(format!("校验视图序列化失败：{err}")))?;
            Ok((value, human))
        }
        SpecCommand::Analyze { session } => {
            let conn = open_db(workspace, config)?;
            let view = whcode_app::analyze_view(&conn, policy, actor, session, workspace)?;
            let human = if view.passed {
                "Analyze 通过（五类缺陷皆空）".to_string()
            } else {
                format!(
                    "Analyze 未通过：遗漏 {} · 孤儿 {} · 无效引用 {} · 环 {} · 覆盖不足 {}",
                    view.missing_requirements.len(),
                    view.orphan_tasks.len(),
                    view.invalid_refs.len(),
                    view.cycles.len(),
                    view.uncovered_acceptance.len()
                )
            };
            let value = serde_json::to_value(&view)
                .map_err(|err| WhError::internal(format!("分析视图序列化失败：{err}")))?;
            Ok((value, human))
        }
        SpecCommand::Converge { session } => {
            let conn = open_db(workspace, config)?;
            let view = whcode_app::converge_view(&conn, policy, actor, session)?;
            let human = if view.converged {
                format!(
                    "收敛通过（pass {} · fail {} · not_tested {}）",
                    view.passed, view.failed, view.not_tested
                )
            } else {
                format!(
                    "未收敛（pass {} · fail {} · not_tested {}）——差异 {} 项（V-205：不静默完成）",
                    view.passed,
                    view.failed,
                    view.not_tested,
                    view.differences.len()
                )
            };
            let value = serde_json::to_value(&view)
                .map_err(|err| WhError::internal(format!("收敛视图序列化失败：{err}")))?;
            Ok((value, human))
        }
        SpecCommand::ForceAdvance {
            session,
            to,
            reason,
        } => {
            let conn = open_db(workspace, config)?;
            let row = whcode_app::force_advance_view(
                &conn,
                policy,
                actor,
                session,
                workspace,
                to,
                reason,
                now_ms()?,
            )?;
            Ok((
                json!({
                    "session_id": row.id,
                    "stage": row.stage,
                    "forced": true,
                    "reason": reason,
                    "note": "人工越过门禁已留审计（actor/reason/跳过门禁/产物版本/时间戳，FR-312）",
                }),
                format!("强制放行 → {}（已留审计）", row.stage),
            ))
        }
    }
}

fn events_command(
    command: &EventsCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    match command {
        EventsCommand::List { session, after }
        | EventsCommand::Export {
            session,
            after,
            format: _,
        } => match command {
            EventsCommand::Export { format, .. } if format == "jsonl" => {
                let conn = open_db(workspace, config)?;
                let text = whcode_cui::events_cmd::events_jsonl(
                    &conn,
                    policy,
                    actor,
                    session,
                    *after,
                    EVENTS_PAGE_LIMIT,
                    TraceLevel::default(),
                )?;
                println!("{text}");
                Ok((json!({ "format": "jsonl", "lines_printed": true }), text))
            }
            _ => {
                let conn = open_db(workspace, config)?;
                let value = whcode_cui::events_cmd::events_json(
                    &conn,
                    policy,
                    actor,
                    session,
                    *after,
                    EVENTS_PAGE_LIMIT,
                    TraceLevel::default(),
                )?;
                let human = human_event_lines(&value, texts);
                Ok((value, human))
            }
        },
        EventsCommand::Show { event } => {
            // 单事件查看（FR-202／T390）：按事件 ID 定位，脱敏与三态标注同列表口径。
            let conn = open_db(workspace, config)?;
            let detail =
                whcode_app::show_event(&conn, policy, actor, event, TraceLevel::default())?;
            let value = serde_json::to_value(&detail)
                .map_err(|err| WhError::internal(format!("事件详情序列化失败：{err}")))?;
            let payload_brief = {
                let raw = detail.event.payload_json.as_str();
                let brief: String = raw.chars().take(160).collect();
                if raw.chars().count() > 160 {
                    format!("{brief}…（截断；完整载荷见 JSON）")
                } else {
                    brief
                }
            };
            let human = format!(
                "{} #{} [{}] {} {} {}",
                detail.event.id,
                detail.event.seq,
                detail.event.kind,
                detail.state.as_str(),
                detail.trace_level,
                payload_brief
            );
            Ok((value, human))
        }
    }
}

/// 事件列表的人类可读行（Check 348：工具调用给结构化文本）。
///
/// 结束事件通常不带工具名，先用同页开始事件按 `tool_call_id` 配对；未配对时显示 `?`，
/// 不推断。非工具事件保持既有的 `seq kind` 两列，数据字段不随语言变化。
fn human_event_lines(value: &Value, texts: &I18n) -> String {
    let Some(events) = value["events"].as_array() else {
        return String::new();
    };
    // 三态标注（T283／FR-770）：`states` 与 `events` 同序；按 seq 索引以容错。
    let states: HashMap<i64, String> = value["states"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let seq = item["seq"].as_i64()?;
                    Some((seq, item["state"].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    for event in events {
        if event["kind"].as_str() == Some(TOOL_STARTED)
            && let Some(payload) = event["payload_json"].as_str()
            && let Some(parsed) = describe(TOOL_STARTED, payload)
            && let Some(tool) = parsed.tool
        {
            tool_names.insert(parsed.call_id, tool);
        }
    }
    events
        .iter()
        .map(|event| {
            let seq = event["seq"].as_i64().unwrap_or_default();
            let kind = event["kind"].as_str().unwrap_or_default();
            let payload = event["payload_json"].as_str().unwrap_or_default();
            let detail = describe(kind, payload).and_then(|parsed| {
                let hint = parsed
                    .tool
                    .clone()
                    .or_else(|| tool_names.get(&parsed.call_id).cloned());
                crate::tool_call_text::event_summary(texts, kind, payload, hint.as_deref())
            });
            // 无标注（调用方未提供 states）时不加括号：不伪造状态。
            let state = states.get(&seq).map(String::as_str);
            match (state, detail) {
                (Some(state), Some(text)) => format!("{seq} {kind} [{state}] {text}"),
                (Some(state), None) => format!("{seq} {kind} [{state}]"),
                (None, Some(text)) => format!("{seq} {kind} {text}"),
                (None, None) => format!("{seq} {kind}"),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn db_command(
    command: &DbCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
    now: u64,
    texts: &I18n,
) -> Result<(Value, String), WhError> {
    match command {
        DbCommand::Stats => {
            let conn = open_db(workspace, config)?;
            let db_path = workspace::workspace_db_path(workspace);
            let stats = whcode_store::monitor::database_stats(&conn)?;
            let wal = whcode_store::monitor::wal_status(&conn)?;
            let space = whcode_store::capture_space(&conn, &db_path)?;
            let value = json!({
                "engine_version": stats.engine_version,
                "schema_version": stats.schema_version,
                "database_name": stats.database_name,
                "database_size": stats.database_size,
                "file_size_bytes": space.file_size_bytes,
                "wal_size_bytes": space.wal_size_bytes,
                "total_blocks": space.total_blocks,
                "used_blocks": space.used_blocks,
                "free_blocks": space.free_blocks,
                "blocks_consistent": space.blocks_consistent(),
                "wal_status": { "wal_size": wal.wal_size, "wal_autocheckpoint": wal.wal_autocheckpoint },
            });
            let human = format!(
                "引擎 {} · schema {} · 文件 {} B · WAL {} B · 块 {}/{}/{}",
                stats.engine_version,
                stats.schema_version,
                space.file_size_bytes,
                space.wal_size_bytes,
                space.used_blocks,
                space.free_blocks,
                space.total_blocks
            );
            Ok((value, human))
        }
        DbCommand::Check => {
            let conn = open_db(workspace, config)?;
            let missing = whcode_store::indexes::missing_indexes(&conn)?;
            if !missing.is_empty() {
                return Err(WhError::new(
                    ErrorCode::InvalidState,
                    format!("缺少必需索引：{missing:?}"),
                ));
            }
            let value = json!({ "ok": true, "missing_indexes": Vec::<String>::new() });
            Ok((value, texts.resolve("cui.indexes_ok").to_string()))
        }
        DbCommand::Purge {
            before,
            dry_run,
            yes,
        } => {
            let is_dry_run = confirm_flags(*dry_run, *yes)?;
            let mut conn = open_db(workspace, config)?;
            let protection = PurgeProtection::default();
            let criteria = PurgeCriteria::before(*before);
            // 经应用层入口：权限与 Web `/api/purge` 同一判定（FR-604）。
            let preview =
                whcode_app::purge_preview(&conn, policy, actor, &criteria, &protection, now)?;
            if is_dry_run {
                let value = json!({
                    "dry_run": true,
                    "cutoff_ms": preview.cutoff_ms,
                    "selected_sessions": preview.selected_sessions,
                    "skipped_protected": preview.skipped_protected,
                    "counts": counts_json(&preview.counts),
                });
                let human = texts.resolve_fmt(
                    "cui.purge_dry_run",
                    &[
                        ("sessions", &preview.selected_sessions.len().to_string()),
                        ("events", &preview.counts.events.to_string()),
                        ("skipped", &preview.skipped_protected.len().to_string()),
                    ],
                );
                return Ok((value, human));
            }
            let result = whcode_app::purge_execute(
                &mut conn,
                policy,
                actor,
                &preview,
                true,
                &protection,
                now,
            )?;
            let value = json!({
                "dry_run": false,
                "deleted_sessions": result.deleted_sessions,
                "skipped_protected": result.skipped_protected,
                "deleted": counts_json(&result.deleted),
            });
            Ok((
                value,
                texts.resolve_fmt(
                    "cui.purged",
                    &[("events", &result.deleted.events.to_string())],
                ),
            ))
        }
        DbCommand::Compact { dry_run, yes } => {
            let is_dry_run = confirm_flags(*dry_run, *yes)?;
            let db_path = workspace::workspace_db_path(workspace);
            let space = {
                let conn = open_db(workspace, config)?;
                whcode_store::capture_space(&conn, &db_path)?
            };
            if is_dry_run {
                let value = json!({
                    "dry_run": true,
                    "file_size_bytes": space.file_size_bytes,
                    "wal_size_bytes": space.wal_size_bytes,
                    "note": "compact 为复制—校验—切换；不承诺缩小文件（FR-510、R-09）",
                });
                let human = texts.resolve_fmt(
                    "cui.compact_dry_run",
                    &[
                        ("file", &space.file_size_bytes.to_string()),
                        ("wal", &space.wal_size_bytes.to_string()),
                    ],
                );
                return Ok((value, human));
            }
            let report = whcode_store::compact_database(&db_path)?;
            let value = json!({
                "dry_run": false,
                "switched": report.switched,
                "before_file_size_bytes": report.before.file_size_bytes,
                "after_file_size_bytes": report.after.file_size_bytes,
            });
            Ok((
                value,
                texts.resolve_fmt(
                    "cui.compact_done",
                    &[("switched", &report.switched.to_string())],
                ),
            ))
        }
        DbCommand::Archive { export } => {
            let conn = open_db(workspace, config)?;
            let report = whcode_store::export_workspace(&conn, Path::new(export), &actor.id, now)?;
            let value = json!({
                "path": report.path,
                "content_hash": report.content_hash,
                "schema_version": report.manifest.schema_version,
            });
            let human = texts.resolve_fmt(
                "cui.exported",
                &[("path", &report.path), ("hash", &report.content_hash)],
            );
            Ok((value, human))
        }
        DbCommand::Explain { query } => {
            let trimmed = query.trim_start();
            let upper = trimmed
                .chars()
                .take(6)
                .collect::<String>()
                .to_ascii_uppercase();
            if !(upper.starts_with("SELECT") || upper.starts_with("WITH")) {
                return Err(WhError::parse_error(
                    "db explain 只接受只读查询（SELECT/WITH 开头，契约 §2-2）",
                ));
            }
            if query.len() > 4096 {
                return Err(WhError::parse_error("查询超过 4096 字符上限"));
            }
            let conn = open_db(workspace, config)?;
            let plan = whcode_store::monitor::plan_query(&conn, trimmed, false)?;
            let value = json!({
                "sql": plan.sql,
                "analyze": plan.analyze,
                "rows": plan.rows.iter().map(|row| json!({ "key": row.key, "value": row.value })).collect::<Vec<_>>(),
            });
            let human =
                texts.resolve_fmt("cui.plan_rows", &[("rows", &plan.rows.len().to_string())]);
            Ok((value, human))
        }
        DbCommand::Checkpoint {
            threshold,
            autocheckpoint,
        } => {
            let mut conn = open_db(workspace, config)?;
            let wal = whcode_app::maintenance::WalPolicyOverride {
                checkpoint_threshold: threshold.clone(),
                wal_autocheckpoint: autocheckpoint.clone(),
            };
            // 经应用层入口：权限与 purge 同口径（maintenance 模块注释）。
            let outcome =
                whcode_app::maintenance::checkpoint_database(&mut conn, policy, actor, &wal)?;
            let value = json!({
                "checkpoint_threshold": outcome.effective.checkpoint_threshold,
                "wal_autocheckpoint": outcome.effective.wal_autocheckpoint,
                "wal_size_before": outcome.report.wal_size_before,
                "wal_size_after": outcome.report.wal_size_after,
                "reason": outcome.report.reason.as_str(),
            });
            let human = texts.resolve_fmt(
                "cui.checkpoint_done",
                &[
                    ("before", &outcome.report.wal_size_before),
                    ("after", &outcome.report.wal_size_after),
                ],
            );
            Ok((value, human))
        }
        DbCommand::Restore { from, yes } => {
            // 恢复是写操作且无预览：缺 --yes 一律拒绝（maintenance::restore_workspace 同判）。
            if !*yes {
                return Err(WhError::new(
                    ErrorCode::InvalidState,
                    "db restore 是写操作且无预览：确认目标为空工作区后加 --yes 执行",
                ));
            }
            let mut conn = open_db(workspace, config)?;
            let report = whcode_app::maintenance::restore_workspace(
                &mut conn,
                policy,
                actor,
                Path::new(from),
                true,
            )?;
            let value = json!({
                "source": report.source,
                "inserted": report.inserted,
            });
            let human = texts.resolve_fmt("cui.restore_done", &[("source", &report.source)]);
            Ok((value, human))
        }
    }
}

/// 会话列表 → 对齐表格（T196／ADR-041）；空列表输出显式空态，不输出空白。
/// 内容检索命中表（P3.3／ADR-076）：ID／标题／角色／摘要（定长片段）。
fn search_hits_table(hits: &[whcode_store::ContentHit], texts: &I18n) -> String {
    if hits.is_empty() {
        return texts.resolve("cui.empty").to_string();
    }
    let styler = human_styler(texts);
    let headers = [
        texts.resolve("tui.col.id"),
        texts.resolve("tui.col.title"),
        texts.resolve("tui.col.role"),
        texts.resolve("tui.col.summary"),
    ];
    let cells: Vec<Vec<String>> = hits
        .iter()
        .map(|hit| {
            vec![
                hit.session_id.clone(),
                hit.session_title.clone(),
                hit.role.clone(),
                hit.snippet.clone(),
            ]
        })
        .collect();
    styler.table(&headers, &cells)
}

fn sessions_table(rows: &[whcode_store::SessionRow], texts: &I18n) -> String {
    if rows.is_empty() {
        return texts.resolve("cui.empty").to_string();
    }
    let styler = human_styler(texts);
    let headers = [
        texts.resolve("tui.col.id"),
        texts.resolve("tui.col.title"),
        texts.resolve("tui.col.stage"),
        texts.resolve("tui.col.status"),
    ];
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let status = if row.archived {
                format!("{} (archived)", row.status)
            } else {
                row.status.clone()
            };
            vec![row.id.clone(), row.title.clone(), row.stage.clone(), status]
        })
        .collect();
    styler.table(&headers, &cells)
}

/// 单个会话 → 对齐记录（T196／ADR-041）：标题行 + 标签列/值列。
fn session_record(row: &whcode_store::SessionRow, texts: &I18n) -> String {
    let styler = human_styler(texts);
    let title = texts.resolve_fmt("cui.record.session", &[("session", &row.id)]);
    let archived = if row.archived {
        texts.resolve("cui.record.yes")
    } else {
        texts.resolve("cui.record.no")
    };
    let mut fields = vec![
        (texts.resolve("tui.col.title"), row.title.as_str()),
        (texts.resolve("tui.col.stage"), row.stage.as_str()),
        (texts.resolve("tui.col.status"), row.status.as_str()),
        (texts.resolve("cui.record.archived"), archived),
    ];
    if let Some(model) = row.model.as_deref() {
        fields.push((texts.resolve("cui.record.model"), model));
    }
    styler.record(Some(&title), &fields)
}

/// 任务详情 → 对齐记录（T196／ADR-041）。
fn task_record(detail: &whcode_app::TaskDetail, texts: &I18n) -> String {
    let styler = human_styler(texts);
    let title = texts.resolve_fmt("cui.record.task", &[("task", &detail.task.id)]);
    let retry = detail.task.retry_count.to_string();
    let checkpoints = detail.checkpoints.len().to_string();
    let artifacts = detail.artifacts.len().to_string();
    let mut fields = vec![
        (texts.resolve("tui.col.title"), detail.task.title.as_str()),
        (texts.resolve("tui.col.status"), detail.task.status.as_str()),
        (texts.resolve("tui.col.stage"), detail.task.stage.as_str()),
        (texts.resolve("tui.col.retry"), retry.as_str()),
        (
            texts.resolve("cui.record.checkpoints"),
            checkpoints.as_str(),
        ),
        (texts.resolve("cui.record.artifacts"), artifacts.as_str()),
    ];
    if let Some(reason) = detail.failure_reason.as_deref() {
        fields.push((texts.resolve("cui.record.failure"), reason));
    }
    styler.record(Some(&title), &fields)
}

fn counts_json(counts: &whcode_store::PurgeCounts) -> Value {
    // 与 Web `/api/purge` 共用同一形状（FR-604/FR-610）：字段少报即隐瞒将被删除的数据。
    whcode_app::purge_counts_value(counts)
}

fn session_json(row: &whcode_store::SessionRow) -> Value {
    json!({ "session": row_value(row) })
}

fn row_value(row: &whcode_store::SessionRow) -> Value {
    whcode_app::session_value(row)
}

fn row_line(row: &whcode_store::SessionRow) -> String {
    format!(
        "{} {} [{}] {}{}",
        row.id,
        row.title,
        row.stage,
        row.status,
        if row.archived { " (archived)" } else { "" }
    )
}

fn task_json(row: &whcode_store::tasks::TaskRow) -> Value {
    json!({ "task": whcode_app::task_value(row) })
}

#[cfg(test)]
mod tests {
    use super::{
        CONTEXT_DISPLAY_BUDGET, dispatch_code, execute, human_event_lines, local_actor,
        not_supported,
    };
    use std::path::PathBuf;
    use whcode_app::surface::{self, FrontendId};
    use whcode_cui::cli::{
        Command, ContextCommand, EventsCommand, FontCommand, LlmCommand, RuntimeCommand,
        SessionCommand, SpecCommand, TaskCommand,
    };
    use whcode_cui::output::{error_envelope, error_exit_code};
    use whcode_domain::ErrorCode;
    use whcode_domain::i18n::I18n;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("whcode-cui-dispatch-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时工作区");
        dir
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_string()).collect()
    }

    /// 文件前像恢复（T289／FR-776／ADR-068）：默认 dry-run；--yes 恢复并写审计。
    #[test]
    fn session_restore_previews_then_applies_preimage() {
        let workspace = temp_workspace("restore");
        let create = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "恢复会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = create.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        std::fs::write(workspace.join("doc.md"), "改后").expect("当前文件");
        let config = crate::config::Config::for_workspace(&workspace);
        let conn = super::open_db(&workspace, &config).expect("打开库");
        whcode_store::preimage::record(&conn, &id, "TX-demo", "doc.md", 6, Some("改前"), None)
            .expect("登记前像");
        drop(conn);

        let preview = execute(
            &Command::Session {
                command: SessionCommand::Restore {
                    session: id.clone(),
                    exec_id: "TX-demo".into(),
                    yes: false,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("预览");
        assert_eq!(preview.0["dry_run"], true);
        assert_eq!(preview.0["items"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            std::fs::read_to_string(workspace.join("doc.md")).expect("未动"),
            "改后",
            "预览不得写工作区"
        );

        let applied = execute(
            &Command::Session {
                command: SessionCommand::Restore {
                    session: id,
                    exec_id: "TX-demo".into(),
                    yes: true,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("恢复");
        assert_eq!(applied.0["restored"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            std::fs::read_to_string(workspace.join("doc.md")).expect("已恢复"),
            "改前"
        );
    }

    /// 对话剪枝（T288／FR-775／ADR-068）：CUI 回退只落边界；非法目标显式失败。
    #[test]
    fn session_rewind_records_boundary_and_rejects_invalid_targets() {
        let workspace = temp_workspace("rewind");
        let create = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "剪枝会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = create.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        let err = execute(
            &Command::Session {
                command: SessionCommand::Rewind {
                    session: id.clone(),
                    to_seq: 0,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect_err("无事件时无可回退历史");
        assert_eq!(err.code, whcode_domain::ErrorCode::NotFound);

        // 用计划模式开关制造一条事件（seq 1），使 0 成为合法边界。
        execute(
            &Command::Session {
                command: SessionCommand::Plan {
                    session: id.clone(),
                    off: false,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("开启计划模式");
        let out = execute(
            &Command::Session {
                command: SessionCommand::Rewind {
                    session: id.clone(),
                    to_seq: 0,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("回退到起点");
        assert_eq!(out.0["target_seq"], 0);
        assert_eq!(out.0["rewind_seq"], 2);
        assert!(out.1.contains("回退"), "人类行说明边界：{}", out.1);

        let err = execute(
            &Command::Session {
                command: SessionCommand::Rewind {
                    session: id,
                    to_seq: 2,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect_err("回退到末尾是空操作");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    }

    /// 会话内容检索（P3.3／ADR-076）：消息正文关键词、大小写不敏感、片段有界。
    #[test]
    fn session_search_returns_content_hits_with_bounded_snippet() {
        let workspace = temp_workspace("search");
        let create = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "检索会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = create.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        let config = crate::config::Config::for_workspace(&workspace);
        let conn = super::open_db(&workspace, &config).expect("打开库");
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
             VALUES ('M-1', ?, 'user', ?, NULL, current_timestamp, NULL)",
            duckdb::params![&id, "关于 WAL 与 checkpoint 的讨论"],
        )
        .expect("插入消息");
        drop(conn);

        let (value, human) = execute(
            &Command::Session {
                command: SessionCommand::Search {
                    query: "wal".into(),
                    limit: Some(5),
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("检索");
        assert_eq!(value["count"], 1, "{value}");
        assert_eq!(value["hits"][0]["session_id"], id);
        assert!(human.contains("WAL"), "人类输出须含命中片段：{human}");

        let err = execute(
            &Command::Session {
                command: SessionCommand::Search {
                    query: "   ".into(),
                    limit: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect_err("空搜索词必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    }

    /// 计划模式（T287／FR-774／ADR-067）：CUI 直接切换、不受审批；两次切换各自落事件。
    #[test]
    fn session_plan_toggles_state_without_approval() {
        let workspace = temp_workspace("plan");
        let create = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "计划会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = create.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        let on = execute(
            &Command::Session {
                command: SessionCommand::Plan {
                    session: id.clone(),
                    off: false,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("开启计划模式");
        assert_eq!(on.0["plan_mode"], true);
        assert_eq!(on.0["seq"], 1);
        assert!(on.1.contains("计划模式"), "人类行明写状态：{}", on.1);

        let off = execute(
            &Command::Session {
                command: SessionCommand::Plan {
                    session: id.clone(),
                    off: true,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("关闭计划模式");
        assert_eq!(off.0["plan_mode"], false);
        assert_eq!(off.0["seq"], 2, "整值替换：每次切换一条状态事件");
    }

    /// 会话生命周期：new → list → rename → archive → delete（dry-run → yes）。
    #[test]
    fn session_lifecycle_commands_execute() {
        let workspace = temp_workspace("lifecycle");
        let workspace_arg = workspace.display().to_string();
        let base = ["--workspace", workspace_arg.as_str()];

        let create = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "CUI 会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = create.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();
        assert_eq!(create.0["session"]["title"], "CUI 会话");

        let list = execute(
            &Command::Session {
                command: SessionCommand::List { all: false },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("列出会话");
        assert_eq!(list.0["sessions"].as_array().map(Vec::len), Some(1));

        let rename = execute(
            &Command::Session {
                command: SessionCommand::Rename {
                    session: id.clone(),
                    title: "改名".into(),
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("重命名");
        assert_eq!(rename.0["session"]["title"], "改名");

        let archive_preview = execute(
            &Command::Session {
                command: SessionCommand::Archive {
                    session: id.clone(),
                    unarchive: false,
                    dry_run: false,
                    yes: false,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("归档 dry-run");
        assert_eq!(archive_preview.0["dry_run"], true, "默认 dry-run");
        assert_eq!(archive_preview.0["current_archived"], false);
        assert_eq!(archive_preview.0["archived"], true);
        assert_eq!(archive_preview.0["changed"], true);

        let archive = execute(
            &Command::Session {
                command: SessionCommand::Archive {
                    session: id.clone(),
                    unarchive: false,
                    dry_run: false,
                    yes: true,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("归档");
        assert_eq!(archive.0["session"]["archived"], true);

        let preview = execute(
            &Command::Session {
                command: SessionCommand::Delete {
                    session: id.clone(),
                    dry_run: false,
                    yes: false,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("dry-run 删除");
        assert_eq!(preview.0["dry_run"], true, "默认 dry-run");

        let deleted = execute(
            &Command::Session {
                command: SessionCommand::Delete {
                    session: id.clone(),
                    dry_run: false,
                    yes: true,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("确认删除");
        assert_eq!(deleted.0["dry_run"], false);
        assert_eq!(deleted.0["deleted_sessions"][0], id);

        // 退出码与信封：--json 走统一信封；未知命令 → 用法错误 2。
        let code = dispatch_code(&args(&[base[0], base[1], "session", "list", "--json"]));
        assert_eq!(code, 0, "--json 成功退出码 0");
        let usage = dispatch_code(&args(&["--nonsense"]));
        assert_eq!(usage, 2, "未知参数 → 用法错误退出码 2");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// T261／FR-752：`session share` 导出本地脱敏文件并回报 sha256；重复导出拒绝，
    /// 不覆盖既有文件。
    #[test]
    fn session_share_writes_local_bundle_and_refuses_overwrite() {
        let workspace = temp_workspace("share");
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "分享会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = created.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();
        let out = workspace.join("share.json");
        let command = Command::Session {
            command: SessionCommand::Share {
                session: id.clone(),
                out: out.display().to_string(),
            },
        };
        let (value, human) = execute(&command, &workspace, &whcode_domain::i18n::I18n::default())
            .expect("导出分享包");
        assert_eq!(value["redacted"], true);
        assert_eq!(value["trace_level"], "redacted");
        assert_eq!(
            value["sha256"].as_str().map(str::len),
            Some(64),
            "sha256 必须是 64 位十六进制"
        );
        assert!(human.starts_with("share path="), "{human}");
        let text = std::fs::read_to_string(&out).expect("分享文件");
        assert!(text.contains("whcode-session-share"), "{text}");
        assert!(text.contains("分享会话"), "标题保留");

        let err = execute(&command, &workspace, &whcode_domain::i18n::I18n::default())
            .expect_err("目标已存在必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// T282／FR-769：CUI `session fork` 在 turn 结束边界分叉；缺省标题与半截 turn 拒绝。
    #[test]
    fn session_forks_at_turn_boundary_and_refuses_mid_turn() {
        let workspace = temp_workspace("fork");
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "源会话".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = created.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        // 直开库播种一个已闭合 turn（事件 1—2 ＋ 尾随消息 3）；DuckDB 独占库文件，
        // 播种连接必须在 execute 重开之前释放。
        {
            let conn =
                duckdb::Connection::open(workspace.join("whcode.duckdb")).expect("打开工作区库");
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, 'req-1', NULL, ?, CAST(? AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                duckdb::params![1, "EV-1", id, "TurnStarted", "{\"kind\":\"TurnStarted\"}"],
            )
            .expect("播种 TurnStarted");
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, ?, NULL, 'req-1', NULL, ?, CAST(? AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                duckdb::params![2, "EV-2", id, "TurnFinished", "{\"kind\":\"TurnFinished\"}"],
            )
            .expect("播种 TurnFinished");
            conn.execute(
                "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
                 VALUES (?, ?, 'assistant', ?, 6, current_timestamp, NULL)",
                duckdb::params!["MSG-3", id, "边界轮回复"],
            )
            .expect("播种尾随消息");
        }

        let command = Command::Session {
            command: SessionCommand::Fork {
                session: id.clone(),
                at_seq: 2,
                title: None,
            },
        };
        let (value, human) =
            execute(&command, &workspace, &whcode_domain::i18n::I18n::default()).expect("分叉");
        assert_eq!(value["source_session"], id.as_str());
        assert_eq!(value["forked_at_seq"], 2);
        assert_eq!(value["inherited_events"], 2);
        assert_eq!(value["inherited_messages"], 1);
        assert_eq!(value["session"]["title"], "源会话 (fork)");
        assert!(human.starts_with("fork child="), "{human}");

        // 半截 turn 边界拒绝（事件 1 是 TurnStarted）。
        let mid = Command::Session {
            command: SessionCommand::Fork {
                session: id.clone(),
                at_seq: 1,
                title: Some("半截".into()),
            },
        };
        let err = execute(&mid, &workspace, &whcode_domain::i18n::I18n::default())
            .expect_err("turn 内边界必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// FR-615：CUI 人类可读输出随文本层切换；数据字段（ID/状态）不翻译。
    #[test]
    fn human_output_localizes_while_data_fields_stay_stable() {
        let workspace = temp_workspace("i18n");
        let en = whcode_domain::i18n::I18n::load("en", whcode_domain::i18n::LocaleSource::Explicit);
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "I18n".into(),
                    model: None,
                },
            },
            &workspace,
            &en,
        )
        .expect("创建会话");
        let id = created.0["session"]["id"].as_str().expect("ID").to_string();
        let (_, human) = execute(
            &Command::Task {
                command: TaskCommand::List {
                    session: id.clone(),
                },
            },
            &workspace,
            &en,
        )
        .expect("任务列表");
        assert!(human.starts_with("Session "), "英文摘要必须英文起头");
        let zh = whcode_domain::i18n::I18n::default();
        let (_, human) = execute(
            &Command::Task {
                command: TaskCommand::List { session: id },
            },
            &workspace,
            &zh,
        )
        .expect("任务列表");
        assert!(human.starts_with("会话 "), "中文摘要必须中文起头");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// T395／ADR-089 批 6：runtime 管理命令 CUI 正向接线——status 出结构化状态
    /// （js 能力等级可见、默认关闭）；enable js 结构化拒绝（不伪造启用，ADR-024）。
    #[test]
    fn runtime_management_commands_are_wired() {
        let workspace = temp_workspace("runtime-mgmt");
        let texts = whcode_domain::i18n::I18n::default();

        let (value, human) = execute(
            &Command::Runtime {
                command: RuntimeCommand::Status { name: "js".into() },
            },
            &workspace,
            &texts,
        )
        .expect("运行时状态");
        assert_eq!(value["name"], "js");
        assert_eq!(value["capability"], "restricted");
        assert_eq!(value["enabled"], false, "表空＝默认关闭（FR-704）");
        assert!(human.contains("js"), "人类可读行含运行时名：{human}");

        // enable js：结构化拒绝（不是 not_supported 退出码 55，而是 invalid_state）。
        let err = execute(
            &Command::Runtime {
                command: RuntimeCommand::Enable { name: "js".into() },
            },
            &workspace,
            &texts,
        )
        .expect_err("js 启用必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.message.contains("ADR-024"),
            "拒绝必须指向决策依据：{}",
            err.message
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// T390／events.show：CUI 正向接线——按事件 ID 出详情，redacted 级别秘密不出现；
    /// 不存在的事件 `not_found`（退出码 60）。
    #[test]
    fn events_show_is_wired_with_redaction_and_notfound() {
        let workspace = temp_workspace("events-show");
        let texts = whcode_domain::i18n::I18n::default();
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "单事件".into(),
                    model: None,
                },
            },
            &workspace,
            &texts,
        )
        .expect("创建会话");
        let session = created.0["session"]["id"].as_str().expect("会话 ID");

        // 直接经 store 播种一条带秘密的事件（与 events_export 测试同源口径）。
        let db = workspace.join(crate::workspace::WORKSPACE_DB_FILE);
        let conn = duckdb::Connection::open(&db).expect("打开工作区库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (1, 'E-1', ?, NULL, NULL, NULL, 'StreamingChunk',
                     CAST(? AS JSON)::VARIANT, current_timestamp, 'redacted')",
            duckdb::params![session, r#"{"api_key":"sk-secret9","note":"正文"}"#],
        )
        .expect("插入事件");
        drop(conn);

        let (value, human) = execute(
            &Command::Events {
                command: EventsCommand::Show {
                    event: "E-1".into(),
                },
            },
            &workspace,
            &texts,
        )
        .expect("单事件查看");
        assert_eq!(value["event"]["id"], "E-1");
        assert_eq!(value["state"], "log_only", "无孪生→log_only");
        assert_eq!(value["trace_level"], "redacted");
        let serialized = serde_json::to_string(&value).expect("序列化");
        assert!(
            !serialized.contains("sk-secret9"),
            "redacted 级别秘密不得出现：{serialized}"
        );
        assert!(serialized.contains("正文"), "普通内容保留");
        assert!(human.contains("E-1"), "人类可读行含事件 ID：{human}");

        let err = execute(
            &Command::Events {
                command: EventsCommand::Show {
                    event: "E-404".into(),
                },
            },
            &workspace,
            &texts,
        )
        .expect_err("不存在必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(error_exit_code(&err), 60);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// T390／session.switch：CUI 有界加载切换（FR-102）——元数据＋概览＋尾部计数＋
    /// 游标；不存在的会话 `not_found`。
    #[test]
    fn session_switch_is_wired_with_bounded_load() {
        let workspace = temp_workspace("session-switch");
        let texts = whcode_domain::i18n::I18n::default();
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "切换目标".into(),
                    model: None,
                },
            },
            &workspace,
            &texts,
        )
        .expect("创建会话");
        let session = created.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        let (value, human) = execute(
            &Command::Session {
                command: SessionCommand::Switch {
                    session: session.clone(),
                },
            },
            &workspace,
            &texts,
        )
        .expect("切换会话");
        assert_eq!(value["session"]["id"], session.as_str());
        assert!(value["overview"].is_null(), "空会话概览为正确的空");
        assert_eq!(value["tail_count"], 0, "空会话尾部为零");
        assert_eq!(value["tail_complete"], true);
        assert!(human.contains("cursor"), "人类可读行含游标：{human}");

        let err = execute(
            &Command::Session {
                command: SessionCommand::Switch {
                    session: "S-404".into(),
                },
            },
            &workspace,
            &texts,
        )
        .expect_err("不存在必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(error_exit_code(&err), 60);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// `llm run`／`llm cancel`（T167）：CUI 已接线，前置校验与取消语义可观测；
    /// 不依赖 `WHCODE_LLM_PROFILES` 的路径（未知会话／空提示词／未知追踪级别／未知请求）。
    #[test]
    fn llm_commands_are_wired_and_validated_before_transport() {
        let workspace = temp_workspace("llm");
        let texts = whcode_domain::i18n::I18n::default();
        let create = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "LLM 会话".into(),
                    model: None,
                },
            },
            &workspace,
            &texts,
        )
        .expect("创建会话");
        let session = create.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        let run = |session: &str, prompt: &str, trace: &str| {
            execute(
                &Command::Llm {
                    command: LlmCommand::Run {
                        session: session.to_string(),
                        provider: "p".into(),
                        prompt: prompt.to_string(),
                        model: None,
                        max_tokens: None,
                        request_id: None,
                        trace_level: trace.to_string(),
                        approve: Vec::new(),
                    },
                },
                &workspace,
                &texts,
            )
        };

        // 未知会话：not_found（先于剖面与凭据，不伪装成空成功）。
        let err = run("S-不存在", "hi", "redacted").expect_err("未知会话必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);

        // 空提示词：invalid_state（不隐式读取已存内容）。
        let err = run(&session, "   ", "redacted").expect_err("空提示词必须拒绝");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(err.message.contains("提示词"), "{err}");

        // 未知追踪级别：parse_error（不静默回落到 redacted）。
        let err = run(&session, "hi", "loud").expect_err("未知追踪级别必须拒绝");
        assert_eq!(err.code, ErrorCode::ParseError);

        // 取消未知请求：not_found（取消只对同机活跃调用生效，不制造无主标记）。
        let err = execute(
            &Command::Llm {
                command: LlmCommand::Cancel {
                    request: format!("no-such-{}", std::process::id()),
                },
            },
            &workspace,
            &texts,
        )
        .expect_err("未知请求必须拒绝");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(!err.retryable);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// FR-620 退出码语义（机制级）：`not_supported` 错误在 CUI 侧统一映射为 55。
    /// T390—T395 把全部保留命令翻实现后，命令面不再有 not_supported 样本——
    /// 本测试改为直接验证「错误 → 退出码」映射与登记表详情构造，防止语义回归。
    #[test]
    fn not_supported_maps_to_exit_55_with_registry_details() {
        let error = not_supported("spec.next", "机制级样本");
        assert_eq!(error.code, ErrorCode::NotSupported);
        assert!(!error.retryable, "not_supported 不可重试（FR-620）");
        assert_eq!(error_exit_code(&error), 55, "CUI 退出码必须为 55");
        let envelope = error_envelope(&error);
        assert_eq!(envelope.code, "not_supported");
        assert_eq!(envelope.details["surface_id"], "spec.next");
        // 已实现面：details 带 state（不伪造里程碑／理由码）。
        assert_eq!(envelope.details["state"], "implemented");
        // 未登记面：只有 surface_id，不伪造 state（装配错误信号）。
        let unknown = not_supported("no.such.surface", "未登记样本");
        let envelope = error_envelope(&unknown);
        assert_eq!(envelope.details["surface_id"], "no.such.surface");
        assert!(envelope.details.get("state").is_none());
    }

    /// `runtime list`（T173）：与 Web `GET /api/runtimes` 同形——顶层与条目字段集合
    /// 一致，能力等级名不翻译（full/restricted/shell_only），界面路径不探测子进程。
    #[test]
    fn runtime_list_matches_web_runtimes_shape() {
        let workspace = temp_workspace("runtime-list");
        let (value, human) = execute(
            &Command::Runtime {
                command: RuntimeCommand::List,
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("runtime list");
        assert_eq!(value["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(value["wired"], true);
        let entries = value["runtimes"].as_array().expect("runtimes 数组");
        assert_eq!(entries.len(), 4, "四个运行时（js/python/gitbash/bsdshell）");
        for entry in entries {
            let mut keys: Vec<&str> = entry
                .as_object()
                .expect("条目对象")
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                vec![
                    "available",
                    "capability",
                    "default_enabled",
                    "enable_requirement",
                    "name",
                    "unsupported"
                ],
                "条目字段与 Web 注入的 inventory 同形"
            );
            assert!(entry["available"].is_null(), "界面路径不探测子进程");
            assert_eq!(
                entry["default_enabled"],
                serde_json::json!(false),
                "运行时不默认启用（P5.6／ADR-024 修订）"
            );
            assert!(
                entry["enable_requirement"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "默认值必须可解释：{}",
                entry["enable_requirement"]
            );
            assert!(
                ["full", "restricted", "shell_only"]
                    .contains(&entry["capability"].as_str().expect("能力名")),
                "能力等级名不翻译：{}",
                entry["capability"]
            );
        }
        assert!(human.contains("js="), "人类可读摘要必须含运行时行");

        // 同形比对：同一 inventory 注入 Web API，顶层与条目字段集合一致。
        let mut conn = duckdb::Connection::open_in_memory().expect("内存库");
        let inventory = serde_json::to_value(crate::runtimes::inventory(false)).expect("序列化");
        let request = whcode_web::api::HttpRequest {
            method: whcode_web::api::Method::Get,
            path: "/api/runtimes".to_string(),
            headers: vec![("origin".to_string(), "http://127.0.0.1".to_string())],
            body: Vec::new(),
        };
        let response = whcode_web::api::handle_request_with_runtimes(
            &mut conn,
            false,
            None,
            &whcode_web::security::ALLOWED_ORIGINS,
            &request,
            &local_actor(),
            Some(&inventory),
            "zh-CN",
        );
        assert_eq!(response.status, 200);
        let web: serde_json::Value = serde_json::from_slice(&response.body).expect("Web JSON");
        let mut web_keys: Vec<&str> = web
            .as_object()
            .expect("对象")
            .keys()
            .map(String::as_str)
            .collect();
        let mut cui_keys: Vec<&str> = value
            .as_object()
            .expect("对象")
            .keys()
            .map(String::as_str)
            .collect();
        web_keys.sort_unstable();
        cui_keys.sort_unstable();
        assert_eq!(
            cui_keys, web_keys,
            "runtime list 与 Web /api/runtimes 顶层字段必须同形"
        );
        for (cui_entry, web_entry) in entries
            .iter()
            .zip(web["runtimes"].as_array().expect("Web 数组"))
        {
            assert_eq!(cui_entry["name"], web_entry["name"]);
            assert_eq!(cui_entry["capability"], web_entry["capability"]);
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// `context budget`（T173，FR-105/108）：缺省预算用展示默认口径；显式 --budget 生效；
    /// 缺会话 → NotFound（既有语义，不伪装成空窗口）。
    #[test]
    fn context_budget_reports_budget_and_missing_session_is_not_found() {
        let workspace = temp_workspace("context-budget");
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "预算命令".into(),
                    model: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("创建会话");
        let id = created.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        // 缺省 --budget：4096 展示口径；会话存在但无消息 = 正确的空（items 0）。
        let (value, human) = execute(
            &Command::Context {
                command: ContextCommand::Budget {
                    session: id.clone(),
                    budget: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("预算视图");
        assert_eq!(value["schema_version"], whcode_domain::SCHEMA_VERSION);
        assert_eq!(value["budget_tokens"], CONTEXT_DISPLAY_BUDGET);
        assert_eq!(value["items"], 0);
        assert_eq!(value["consumed_budget"], 0);
        assert!(value["sources"].is_array());
        // T196／ADR-041：人类可读输出改为“标签列 + 值列”的记录式，标签按 12 显示列补齐。
        assert!(
            human
                .lines()
                .any(|line| line.starts_with("预算") && line.contains("4096")),
            "人类可读摘要（记录式）必须含预算行"
        );

        // 显式 --budget：透传应用服务（FR-108）。
        let (value, _) = execute(
            &Command::Context {
                command: ContextCommand::Budget {
                    session: id.clone(),
                    budget: Some(2048),
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("显式预算");
        assert_eq!(value["budget_tokens"], 2048);

        // 缺会话：NotFound（退出码 60），不是空的预算窗口。
        let err = execute(
            &Command::Context {
                command: ContextCommand::Budget {
                    session: "S-404".into(),
                    budget: None,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect_err("缺会话必须失败");
        assert_eq!(err.code, ErrorCode::NotFound);
        assert_eq!(error_exit_code(&err), 60);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// FR-620：不可用信封由登记表驱动（同源，不硬编码第二份清单）。
    /// 四态按前端分别登记（`task.show` 在 CUI 已实现、Web 排除属正当差异），
    /// 因此断言的是「details 回读该前端登记值」与「状态码落在四态词表内」，
    /// 而不是两端状态码相同。
    #[test]
    fn cui_and_web_unavailable_details_share_registry() {
        for entry in surface::SURFACES {
            let envelope = error_envelope(&not_supported(entry.id, "对等测试"));
            assert_eq!(envelope.details["surface_id"], entry.id);
            assert_eq!(
                envelope.details["state"],
                entry.support(FrontendId::Cui).state_code(),
                "{} 的 details.state 必须回读 CUI 登记状态",
                entry.id
            );
            for frontend in [FrontendId::Cui, FrontendId::Web] {
                let code = entry.support(frontend).state_code();
                assert!(
                    [
                        "implemented",
                        "partial",
                        "reserved",
                        "excluded",
                        "not_applicable"
                    ]
                    .contains(&code),
                    "{} 的 {frontend:?} 状态码 {code} 不在四态词表内",
                    entry.id
                );
            }
        }
    }

    /// registry_details 与登记表逐面一致（防装配漂移的 tripwire）：
    /// 状态码取 CUI 端登记值；未登记面不伪造状态（缺 state 即装配错误信号）。
    #[test]
    fn registry_details_reflect_registry_state() {
        // runtime.status 已随 T395 翻实现：state 必须是 implemented，且不写里程碑。
        let details = super::registry_details("runtime.status");
        assert_eq!(details["surface_id"], "runtime.status");
        assert_eq!(details["state"], "implemented");
        assert!(details.get("milestone").is_none(), "已实现面不写里程碑");

        // 排除面：state 与理由码同源（examples：cli_local_admin 族）。
        let details = super::registry_details("db.stats");
        let entry = surface::find("db.stats").expect("已登记");
        assert_eq!(
            details["state"],
            entry.support(FrontendId::Cui).state_code()
        );

        // 未登记面：不伪造状态（缺 state 即装配错误信号）。
        let details = super::registry_details("no.such.surface");
        assert_eq!(details["surface_id"], "no.such.surface");
        assert!(details.get("state").is_none());
    }

    /// `font status`（只读）走真实注册表查询：四份清单固定，信封含目录与终端检测。
    #[test]
    fn font_status_reports_four_fonts() {
        let workspace = temp_workspace("font-status");
        let (value, human) = execute(
            &Command::Font {
                command: FontCommand::Status,
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("status 是只读命令");
        assert_eq!(value["fonts"].as_array().expect("fonts 数组").len(), 4);
        assert!(value["fonts_dir"].is_string());
        assert!(value["terminal"].is_object());
        assert!(human.contains("内置字体状态"), "中文摘要必须含字体状态");
        let en = whcode_domain::i18n::I18n::load("en", whcode_domain::i18n::LocaleSource::Explicit);
        let (_, human) = execute(
            &Command::Font {
                command: FontCommand::Status,
            },
            &workspace,
            &en,
        )
        .expect("status 可执行");
        assert!(
            human.contains("Built-in fonts status"),
            "英文摘要必须含字体状态"
        );
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// `font install --dry-run` 无系统副作用，但会解析真实来源（仓库 assets）。
    #[test]
    fn font_install_dry_run_reports_plan() {
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("fonts")
            .join("source");
        let workspace = temp_workspace("font-install");
        let (value, human) = execute(
            &Command::Font {
                command: FontCommand::Install {
                    from: Some(source.display().to_string()),
                    dry_run: true,
                },
            },
            &workspace,
            &whcode_domain::i18n::I18n::default(),
        )
        .expect("dry-run 无副作用");
        assert_eq!(value["dry_run"], serde_json::json!(true));
        assert_eq!(value["installed"].as_array().expect("计划项").len(), 4);
        assert!(human.contains("[dry-run"), "摘要必须标 dry-run");
        assert!(human.contains("Sarasa Mono SC"), "摘要必须含终端配置片段");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// T196：人类可读输出为对齐记录/表格；测试环境（stdout 非 TTY）不得含 ANSI 转义，
    /// 空列表给出显式空态而非空白（ADR-041 决策 3、3 的“正确的空”）。
    #[test]
    fn human_output_is_record_style_and_plain_without_tty() {
        let workspace = temp_workspace("record-style");
        let texts = whcode_domain::i18n::I18n::default();
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "排版会话".into(),
                    model: None,
                },
            },
            &workspace,
            &texts,
        )
        .expect("创建会话");
        let id = created.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        let (_, show) = execute(
            &Command::Session {
                command: SessionCommand::Show {
                    session: id.clone(),
                },
            },
            &workspace,
            &texts,
        )
        .expect("会话详情");
        assert!(!show.contains('\u{1b}'), "非 TTY 输出不得含 ANSI 转义");
        assert!(show.starts_with(&format!("会话 {id}")), "记录必须有标题行");
        for label in ["标题", "阶段", "状态", "归档"] {
            assert!(show.contains(label), "记录缺少标签 {label}");
        }

        let (_, list) = execute(
            &Command::Session {
                command: SessionCommand::List { all: false },
            },
            &workspace,
            &texts,
        )
        .expect("会话列表");
        let header = list.lines().next().unwrap_or_default();
        assert!(
            header.contains("ID") && header.contains("标题"),
            "列表必须为带表头的对齐表格"
        );
        assert!(list.lines().count() >= 2, "列表必须包含至少一条数据行");

        // 空工作区：显式空态，不输出空白。
        let empty_workspace = temp_workspace("record-style-empty");
        let (_, empty) = execute(
            &Command::Session {
                command: SessionCommand::List { all: false },
            },
            &empty_workspace,
            &texts,
        )
        .expect("空列表");
        assert_eq!(empty, texts.resolve("cui.empty"), "空列表必须显式提示");

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&empty_workspace);
    }

    /// Check 292：`task show` 与 `spec status` 为对齐记录、空任务列表显式空态、
    /// 非 TTY 输出不含 ANSI 转义。
    #[test]
    fn task_show_and_spec_status_are_aligned_records_without_ansi() {
        use whcode_store::tasks::{NewTask, insert_task};

        let workspace = temp_workspace("task-spec-records");
        let texts = whcode_domain::i18n::I18n::default();
        let created = execute(
            &Command::Session {
                command: SessionCommand::New {
                    title: "记录排版会话".into(),
                    model: None,
                },
            },
            &workspace,
            &texts,
        )
        .expect("创建会话");
        let id = created.0["session"]["id"]
            .as_str()
            .expect("会话 ID")
            .to_string();

        // 空任务列表：显式空态，不输出空白。
        let (_, empty_tasks) = execute(
            &Command::Task {
                command: TaskCommand::List {
                    session: id.clone(),
                },
            },
            &workspace,
            &texts,
        )
        .expect("空任务列表");
        assert!(
            empty_tasks.contains("总计 0"),
            "空任务列表必须给出显式零计数"
        );
        assert!(!empty_tasks.contains('\u{1b}'), "空任务列表不得含 ANSI");

        // 直接经 store 播种一条任务（CUI 没有 task create 命令）。
        let db = workspace.join(crate::workspace::WORKSPACE_DB_FILE);
        let conn = duckdb::Connection::open(&db).expect("打开工作区库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        insert_task(
            &conn,
            &NewTask {
                id: "T-1",
                session_id: &id,
                title: "补齐记录排版",
                stage: "implement",
                parent_id: None,
                acceptance_json: r#"{"items":[]}"#,
            },
            1,
        )
        .expect("插入任务");
        drop(conn);

        let (value, human) = execute(
            &Command::Task {
                command: TaskCommand::Show { task: "T-1".into() },
            },
            &workspace,
            &texts,
        )
        .expect("任务详情");
        assert_eq!(value["task"]["id"], "T-1");
        assert!(human.contains("补齐记录排版"), "记录必须含任务标题");
        assert!(!human.contains('\u{1b}'), "非 TTY 不得含 ANSI");
        assert!(
            human.lines().any(|line| line.starts_with("状态")),
            "记录必须有状态标签"
        );

        let (value, human) = execute(
            &Command::Spec {
                command: SpecCommand::Status {
                    session: id.clone(),
                },
            },
            &workspace,
            &texts,
        )
        .expect("spec status");
        assert_eq!(value["session_id"], id);
        assert!(human.contains(&id), "记录标题必须含会话");
        assert!(!human.contains('\u{1b}'), "非 TTY 不得含 ANSI");
        assert!(
            human.lines().any(|line| line.starts_with("阶段")),
            "记录必须有阶段标签"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// Check 348：CUI 事件列表对工具调用给结构化文本，并按同页开始事件配对工具名。
    #[test]
    fn events_human_output_structures_tool_calls_and_pairs_names() {
        let value = serde_json::json!({
            "events": [
                {
                    "seq": 1,
                    "kind": "ToolCallStarted",
                    "payload_json": "{\"tool_name\":\"read_file\",\"tool_call_id\":\"call-1\",\"exec_id\":\"exec-9\",\"iteration\":2}"
                },
                {
                    "seq": 2,
                    "kind": "ToolCallFinished",
                    "payload_json": "{\"tool_call_id\":\"call-1\",\"exec_id\":\"exec-9\",\"state\":\"completed\",\"duration_ms\":42,\"output_value\":\"done\"}"
                },
                { "seq": 3, "kind": "StreamingChunk", "payload_json": "{\"text\":\"正文\"}" }
            ]
        });
        let lines = human_event_lines(&value, &I18n::default());
        let rows: Vec<&str> = lines.lines().collect();
        assert_eq!(rows.len(), 3);
        assert!(
            rows[0].contains("read_file") && rows[0].contains("exec-9"),
            "开始行结构化：{}",
            rows[0]
        );
        assert!(
            rows[1].contains("read_file") && rows[1].contains("state=completed"),
            "结束行靠配对补工具名：{}",
            rows[1]
        );
        assert!(rows[1].contains("42") && rows[1].contains("输出=done"));
        assert_eq!(rows[2], "3 StreamingChunk", "非工具事件保持两列");
    }

    /// T283／FR-770：事件行在提供三态标注时以 `[state]` 显示；未提供时不伪造标注。
    #[test]
    fn events_human_output_shows_context_state_only_when_present() {
        let with_states = serde_json::json!({
            "events": [
                { "seq": 1, "kind": "StreamingChunk", "payload_json": "{}" },
                { "seq": 2, "kind": "TurnFinished", "payload_json": "{}" }
            ],
            "states": [
                { "seq": 1, "state": "current" },
                { "seq": 2, "state": "log_only" }
            ]
        });
        let lines = human_event_lines(&with_states, &I18n::default());
        let rows: Vec<&str> = lines.lines().collect();
        assert_eq!(rows[0], "1 StreamingChunk [current]");
        assert_eq!(rows[1], "2 TurnFinished [log_only]");

        let without_states = serde_json::json!({
            "events": [{ "seq": 1, "kind": "StreamingChunk", "payload_json": "{}" }]
        });
        let lines = human_event_lines(&without_states, &I18n::default());
        assert_eq!(lines, "1 StreamingChunk", "无标注时不加括号");
    }
    /// webhook（P6.4／FR-788）：未配置＝零端点显式；dry-run 只计划不认领；坏配置显式报错。
    #[test]
    fn webhook_reports_missing_configuration_without_pretending_success() {
        let workspace = temp_workspace("webhook-none");
        let texts = I18n::default();
        let (value, human) = execute(
            &Command::Webhook {
                command: super::WebhookCommand::Status,
            },
            &workspace,
            &texts,
        )
        .expect("状态");
        assert_eq!(value["configured"], serde_json::json!(false));
        assert_eq!(value["endpoints"], serde_json::json!(0));
        assert!(human.contains("未配置端点"), "{human}");

        let (value, human) = execute(
            &Command::Webhook {
                command: super::WebhookCommand::Deliver {
                    dry_run: true,
                    yes: false,
                },
            },
            &workspace,
            &texts,
        )
        .expect("dry-run");
        assert_eq!(value["planned"], serde_json::json!(0));
        assert!(human.contains("未配置端点"), "{human}");
    }

    /// 配置存在但无事件：configured=true、planned=0（区分“没配置”与“没东西可投”）。
    #[test]
    fn webhook_distinguishes_configured_with_nothing_pending() {
        let workspace = temp_workspace("webhook-empty");
        std::fs::create_dir_all(workspace.join(".whcode")).expect("创建 .whcode");
        std::fs::write(
            workspace.join(".whcode/webhooks.json"),
            r#"{"endpoints":[{"name":"ci","url":"http://127.0.0.1:9/hook","events":["TurnFinished"]}]}"#,
        )
        .expect("写配置");
        let texts = I18n::default();
        let (value, human) = execute(
            &Command::Webhook {
                command: super::WebhookCommand::Status,
            },
            &workspace,
            &texts,
        )
        .expect("状态");
        assert_eq!(value["configured"], serde_json::json!(true));
        assert_eq!(value["enabled_endpoints"], serde_json::json!(1));
        assert!(human.contains("端点 1"), "{human}");

        let (value, _) = execute(
            &Command::Webhook {
                command: super::WebhookCommand::Deliver {
                    dry_run: true,
                    yes: false,
                },
            },
            &workspace,
            &texts,
        )
        .expect("dry-run");
        assert_eq!(value["planned"], serde_json::json!(0), "无事件可投");
    }

    /// 坏配置：显式失败（不降级成“未配置”），--dry-run 与 --yes 互斥。
    #[test]
    fn webhook_bad_config_and_conflicting_flags_are_explicit() {
        let workspace = temp_workspace("webhook-bad");
        std::fs::create_dir_all(workspace.join(".whcode")).expect("创建 .whcode");
        std::fs::write(
            workspace.join(".whcode/webhooks.json"),
            r#"{"endpoints":[{"name":"ci","url":"https://x.invalid/h","events":["NotAEvent"]}]}"#,
        )
        .expect("写配置");
        let texts = I18n::default();
        let err = execute(
            &Command::Webhook {
                command: super::WebhookCommand::Status,
            },
            &workspace,
            &texts,
        )
        .expect_err("未知事件类型必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(
            err.message.contains("webhook 配置"),
            "未知事件类型的错误须指明配置文件"
        );

        let err = execute(
            &Command::Webhook {
                command: super::WebhookCommand::Deliver {
                    dry_run: true,
                    yes: true,
                },
            },
            &workspace,
            &texts,
        )
        .expect_err("互斥旗标必须报错");
        assert_eq!(err.code, ErrorCode::InvalidState);
    }
}
