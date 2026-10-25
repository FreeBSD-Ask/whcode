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
//! 保留命令（`run`、`events show`、`spec next/validate/analyze/converge/force-advance`、
//! `runtime status/enable/disable/install`）：统一返回 `not_supported`（退出码 55，
//! FR-620、ADR-032），details 由登记表（`whcode_app::surface`）驱动——`surface_id`、
//! 本端状态与里程碑/理由码；不静默成功、不伪装成 404 语义；`serve`/`doctor` 由主入口处理。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use serde_json::{Value, json};
use whcode_app::surface::{FrontendId, Support};
use whcode_app::{Actor, DefaultPolicy, actor_with_all_capabilities};
use whcode_cui::cli::{
    Command, ContextCommand, DbCommand, EventsCommand, FontCommand, ParseOutcome, RuntimeCommand,
    SessionCommand, SpecCommand, TaskCommand, parse_outcome,
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

/// `whcode <command>` 入口：解析 → 执行 → 输出（返回进程退出码）。
pub fn dispatch(args: &[String]) -> ExitCode {
    exit_code(dispatch_code(args))
}

/// 调度实现（返回 i32 退出码，供测试直接断言稳定码表）。
fn dispatch_code(args: &[String]) -> i32 {
    let argv = std::iter::once("whcode".to_string()).chain(args.iter().cloned());
    let cli = match parse_outcome(argv) {
        ParseOutcome::Command(cli) => cli,
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
    // 文本层（FR-615）：环境检测（LANG/LC_ALL）→ 默认 zh-CN；PO 覆盖内建；数据字段不翻译。
    let (locale, locale_source) = detect_locale(None);
    let texts = crate::load_texts(&locale, locale_source);
    // 出站信任库（ADR-040 决策 2、T191）：与 TUI／GUI 同一装配口径——失败只告警、不阻断
    // 命令执行；真正的失败会在 LLM 调用时以“无信任根”报出，不在装配阶段伪装成功。
    crate::ensure_outbound_trust(&workspace, "CUI");
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
    let config = config_for(workspace);
    let actor = local_actor();
    let policy = policy();
    let now = now_ms()?;
    match command {
        Command::Session { command } => {
            session_command(command, workspace, &config, &policy, &actor, now, texts)
        }
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
        Command::Events { command } => events_command(command, workspace, &config, &policy, &actor),
        Command::Db { command } => {
            db_command(command, workspace, &config, &policy, &actor, now, texts)
        }
        // 出站证书库初始化（ADR-040／T191）随 LLM 真正接线一并加入：当前 CUI 没有任何
        // 发起出站请求的路径，故不在只读命令里创建证书库文件（避免只读命令产生副作用）。
        Command::Run { .. } => Err(not_supported(
            "llm.run",
            "run 未接线：LLM 流式交互需 Provider 与会话流装配（M3 桩已交付，CLI 执行属后续）",
        )),
        Command::Font { command } => font_command(command, texts),
        Command::Runtime { command } => runtime_command(command),
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
    let styler = Styler::detect();
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
fn runtime_command(command: &RuntimeCommand) -> Result<(Value, String), WhError> {
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
        RuntimeCommand::Status { name } => Err(not_supported(
            "runtime.status",
            &format!("runtime status {name} 属运行时装配开关（M8），当前不伪造启用状态"),
        )),
        RuntimeCommand::Enable { name } => Err(not_supported(
            "runtime.enable",
            &format!("runtime enable {name} 属运行时装配开关（M8），当前不伪造启用状态"),
        )),
        RuntimeCommand::Disable { name } => Err(not_supported(
            "runtime.disable",
            &format!("runtime disable {name} 属运行时装配开关（M8），当前不伪造启用状态"),
        )),
        RuntimeCommand::Install { name } => Err(not_supported(
            "runtime.install",
            &format!("runtime install {name} 属运行时装配开关（M8），当前不伪造启用状态"),
        )),
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
        SessionCommand::Show { session } => {
            let conn = open_db(workspace, config)?;
            let row = whcode_app::show_session(&conn, policy, actor, session)?;
            Ok((session_json(&row), session_record(&row, texts)))
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
            let styler = Styler::detect();
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
        // SDD 门禁命令（FR-307—312）：应用层已交付但前端接线未排期（登记表预留），
        // 统一 not_supported（退出码 55），不伪装成 404 或静默成功。
        SpecCommand::Next { .. } => Err(not_supported(
            "spec.next",
            "spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——接线未排期",
        )),
        SpecCommand::Validate { .. } => Err(not_supported(
            "spec.validate",
            "spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——接线未排期",
        )),
        SpecCommand::Analyze { .. } => Err(not_supported(
            "spec.analyze",
            "spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——接线未排期",
        )),
        SpecCommand::Converge { .. } => Err(not_supported(
            "spec.converge",
            "spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——接线未排期",
        )),
        SpecCommand::ForceAdvance { .. } => Err(not_supported(
            "spec.force-advance",
            "spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——接线未排期",
        )),
    }
}

fn events_command(
    command: &EventsCommand,
    workspace: &Path,
    config: &Config,
    policy: &DefaultPolicy,
    actor: &Actor,
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
                let human = value["events"]
                    .as_array()
                    .map(|events| {
                        events
                            .iter()
                            .map(|event| {
                                format!(
                                    "{} {}",
                                    event["seq"].as_i64().unwrap_or_default(),
                                    event["kind"].as_str().unwrap_or_default()
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                Ok((value, human))
            }
        },
        EventsCommand::Show { event } => Err(not_supported(
            "events.show",
            &format!("events show {event} 未接线：单事件按 ID 查询的应用服务接口未提供"),
        )),
    }
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
fn sessions_table(rows: &[whcode_store::SessionRow], texts: &I18n) -> String {
    if rows.is_empty() {
        return texts.resolve("cui.empty").to_string();
    }
    let styler = Styler::detect();
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
    let styler = Styler::detect();
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
    let styler = Styler::detect();
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
    use super::{CONTEXT_DISPLAY_BUDGET, dispatch_code, execute, local_actor, not_supported};
    use std::path::PathBuf;
    use whcode_app::surface::{self, FrontendId, Support};
    use whcode_cui::cli::{
        Command, ContextCommand, EventsCommand, FontCommand, RuntimeCommand, SessionCommand,
        SpecCommand, TaskCommand,
    };
    use whcode_cui::output::{error_envelope, error_exit_code};
    use whcode_domain::ErrorCode;

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
        assert!(human.starts_with("Session "), "英文摘要：{human}");
        let zh = whcode_domain::i18n::I18n::default();
        let (_, human) = execute(
            &Command::Task {
                command: TaskCommand::List { session: id },
            },
            &workspace,
            &zh,
        )
        .expect("任务列表");
        assert!(human.starts_with("会话 "), "中文摘要：{human}");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// 保留命令显式失败（不静默成功）：not_supported 语义（FR-620，退出码 55）。
    #[test]
    fn unwired_commands_fail_explicitly() {
        let workspace = temp_workspace("unwired");
        for command in [
            Command::Run {
                session: "S-1".into(),
                model: "stub".into(),
                trace_level: "redacted".into(),
            },
            Command::Runtime {
                command: RuntimeCommand::Status { name: "js".into() },
            },
            Command::Spec {
                command: SpecCommand::Next {
                    session: "S-1".into(),
                },
            },
        ] {
            let err = execute(&command, &workspace, &whcode_domain::i18n::I18n::default())
                .expect_err("必须显式失败");
            assert!(
                err.message.contains("未实现"),
                "错误必须标注未实现：{}",
                err.message
            );
            assert_eq!(err.code, ErrorCode::NotSupported, "保留命令统一 55 语义");
            assert_eq!(error_exit_code(&err), 55);
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// 全部保留命令的 details 由登记表驱动（FR-620）：surface_id、CUI 端状态、
    /// 预留里程碑/排除理由码，与 `whcode_app::surface` 一致——不硬编码第二份清单。
    #[test]
    fn unavailable_commands_carry_registry_details() {
        let workspace = temp_workspace("registry-details");
        let cases = [
            (
                Command::Run {
                    session: "S-1".into(),
                    model: "stub".into(),
                    trace_level: "redacted".into(),
                },
                "llm.run",
            ),
            (
                Command::Events {
                    command: EventsCommand::Show {
                        event: "E-1".into(),
                    },
                },
                "events.show",
            ),
            (
                Command::Spec {
                    command: SpecCommand::Next {
                        session: "S-1".into(),
                    },
                },
                "spec.next",
            ),
            (
                Command::Spec {
                    command: SpecCommand::Validate {
                        session: "S-1".into(),
                    },
                },
                "spec.validate",
            ),
            (
                Command::Spec {
                    command: SpecCommand::Analyze {
                        session: "S-1".into(),
                    },
                },
                "spec.analyze",
            ),
            (
                Command::Spec {
                    command: SpecCommand::Converge {
                        session: "S-1".into(),
                    },
                },
                "spec.converge",
            ),
            (
                Command::Spec {
                    command: SpecCommand::ForceAdvance {
                        session: "S-1".into(),
                        to: "plan".into(),
                        reason: "r".into(),
                    },
                },
                "spec.force-advance",
            ),
            (
                Command::Runtime {
                    command: RuntimeCommand::Status { name: "js".into() },
                },
                "runtime.status",
            ),
            (
                Command::Runtime {
                    command: RuntimeCommand::Enable { name: "js".into() },
                },
                "runtime.enable",
            ),
            (
                Command::Runtime {
                    command: RuntimeCommand::Disable { name: "js".into() },
                },
                "runtime.disable",
            ),
            (
                Command::Runtime {
                    command: RuntimeCommand::Install { name: "js".into() },
                },
                "runtime.install",
            ),
        ];
        for (command, expected_id) in cases {
            let err = execute(&command, &workspace, &whcode_domain::i18n::I18n::default())
                .expect_err("保留命令必须显式失败");
            assert_eq!(error_exit_code(&err), 55, "{expected_id} 退出码 55");
            let envelope = error_envelope(&err);
            assert_eq!(envelope.code, "not_supported", "{expected_id}");
            assert_eq!(
                envelope.schema_version,
                whcode_domain::SCHEMA_VERSION,
                "错误信封必须带 schema_version"
            );
            assert!(!envelope.retryable, "{expected_id} 不可重试");
            assert_eq!(
                envelope.details["surface_id"], expected_id,
                "details.surface_id 必须来自命令映射的登记面"
            );
            // 状态与里程碑/理由码以登记表为准（CUI 端）。
            let entry = surface::find(expected_id)
                .unwrap_or_else(|| panic!("{expected_id} 未登记（not_supported 只认登记面）"));
            let support = entry.support(FrontendId::Cui);
            assert_eq!(
                envelope.details["state"],
                support.state_code(),
                "{expected_id} 的 state 必须取登记表状态码"
            );
            match support {
                Support::Reserved { milestone } => {
                    assert_eq!(envelope.details["milestone"], *milestone);
                }
                Support::Excluded { reason } => {
                    assert_eq!(envelope.details["reason_code"], reason.as_str());
                }
                other => panic!("{expected_id} 的 CUI 状态不应为 {other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// `--json` 错误路径：保留命令经完整调度输出 not_supported 信封并返回 55。
    #[test]
    fn unavailable_commands_exit_55_via_dispatch() {
        let workspace = temp_workspace("exit-55");
        let workspace_arg = workspace.display().to_string();
        let base = ["--workspace", workspace_arg.as_str()];
        for argv in [
            vec!["runtime", "status", "--name", "js"],
            vec!["spec", "next", "--session", "S-1"],
            vec!["events", "show", "--event", "E-1"],
            vec!["run", "--session", "S-1"],
        ] {
            let mut json_args = vec![base[0], base[1], "--json"];
            json_args.extend_from_slice(&argv);
            assert_eq!(
                dispatch_code(&args(&json_args)),
                55,
                "--json 路径退出码 55：{argv:?}"
            );
            // 文本路径同样显式给工作区：装配根会在工作区建出站证书库（T191），
            // 不给 --workspace 时会落到进程当前目录并污染源码树。
            let mut text_args = vec![base[0], base[1]];
            text_args.extend_from_slice(&argv);
            assert_eq!(
                dispatch_code(&args(&text_args)),
                55,
                "文本路径退出码 55：{argv:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&workspace);
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
                vec!["available", "capability", "name", "unsupported"],
                "条目字段与 Web 注入的 inventory 同形"
            );
            assert!(entry["available"].is_null(), "界面路径不探测子进程");
            assert!(
                ["full", "restricted", "shell_only"]
                    .contains(&entry["capability"].as_str().expect("能力名")),
                "能力等级名不翻译：{}",
                entry["capability"]
            );
        }
        assert!(human.contains("js="), "人类可读摘要：{human}");

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
            "人类可读摘要（记录式）：{human}"
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

    /// 同一 SurfaceId 的不可用语义在 CUI 与 Web 同源（FR-620）：Web 守卫返回
    /// 501 not_supported 信封，CUI 返回 55 not_supported 信封——details 的
    /// surface_id/state 一致（同一登记表条目驱动，当前守卫面双端均预留）。
    #[test]
    fn cui_and_web_unavailable_details_share_registry() {
        let mut conn = duckdb::Connection::open_in_memory().expect("内存库");
        for (method, path, id) in [
            ("GET", "/api/events/show", "events.show"),
            ("POST", "/api/spec/next", "spec.next"),
            ("POST", "/api/spec/force-advance", "spec.force-advance"),
            ("GET", "/api/runtimes/status", "runtime.status"),
            ("POST", "/api/llm/run", "llm.run"),
        ] {
            // Web 侧：守卫 → 501 + not_supported 信封。
            let request = whcode_web::api::HttpRequest {
                method: if method == "GET" {
                    whcode_web::api::Method::Get
                } else {
                    whcode_web::api::Method::Post
                },
                path: path.to_string(),
                headers: vec![("origin".to_string(), "http://127.0.0.1".to_string())],
                body: Vec::new(),
            };
            let response = whcode_web::api::handle_request(
                &mut conn,
                false,
                None,
                &whcode_web::security::ALLOWED_ORIGINS,
                &request,
                &local_actor(),
            );
            assert_eq!(response.status, 501, "{path} 守卫必须 501");
            let web: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON");
            assert_eq!(web["code"], "not_supported", "{path}");
            assert_eq!(web["retryable"], false, "{path}");
            assert_eq!(web["schema_version"], whcode_domain::SCHEMA_VERSION);
            assert_eq!(web["details"]["surface_id"], id, "{path}");

            // CUI 侧：同一 surface_id 的 not_supported 错误信封。
            let err = not_supported(id, "对等测试");
            let envelope = error_envelope(&err);
            assert_eq!(envelope.code, "not_supported", "{id}");
            assert_eq!(envelope.details["surface_id"], id, "{id}");
            assert_eq!(
                envelope.details["state"], web["details"]["state"],
                "{id} 的 details.state 在 CUI 与 Web 必须一致（同一登记表面）"
            );
            assert_eq!(
                envelope.details["state"], "reserved",
                "{id} 当前在双端均登记为预留"
            );
        }
    }

    /// not_supported 的登记面必须已登记且 CUI 端为预留/排除（防装配漂移的负向 tripwire）。
    #[test]
    fn registry_details_reflect_registry_state() {
        let details = super::registry_details("runtime.status");
        assert_eq!(details["surface_id"], "runtime.status");
        assert_eq!(details["state"], "reserved");
        assert_eq!(details["milestone"], "M8");

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
        assert!(human.contains("内置字体状态"), "中文摘要：{human}");
        let en = whcode_domain::i18n::I18n::load("en", whcode_domain::i18n::LocaleSource::Explicit);
        let (_, human) = execute(
            &Command::Font {
                command: FontCommand::Status,
            },
            &workspace,
            &en,
        )
        .expect("status 可执行");
        assert!(human.contains("Built-in fonts status"), "英文摘要：{human}");
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
        assert!(human.contains("[dry-run"), "摘要：{human}");
        assert!(human.contains("Sarasa Mono SC"), "含终端配置片段：{human}");
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
        assert!(
            !show.contains('\u{1b}'),
            "非 TTY 输出不得含 ANSI 转义：{show:?}"
        );
        assert!(
            show.starts_with(&format!("会话 {id}")),
            "记录必须有标题行：{show:?}"
        );
        for label in ["标题", "阶段", "状态", "归档"] {
            assert!(show.contains(label), "记录缺少标签 {label}：{show:?}");
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
            "列表必须为带表头的对齐表格：{list:?}"
        );
        assert!(
            list.lines().count() >= 2,
            "列表必须包含至少一条数据行：{list:?}"
        );

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
}
