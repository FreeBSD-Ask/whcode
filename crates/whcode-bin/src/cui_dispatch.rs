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
//! 未接线命令（`run`、`events show`、`spec next/validate/analyze/converge/force-advance`、
//! `runtime *`）：显式返回错误并说明原因（不静默成功）；`serve`/`doctor` 由主入口处理。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::Connection;
use serde_json::{Value, json};
use whcode_app::{Actor, DefaultPolicy, NewSession, actor_with_all_capabilities};
use whcode_cui::cli::{
    Command, DbCommand, EventsCommand, ParseOutcome, SessionCommand, SpecCommand, TaskCommand,
    parse_outcome,
};
use whcode_cui::output::{EXIT_USAGE, error_envelope, error_exit_code, success_envelope};
use whcode_domain::i18n::{I18n, detect_locale};
use whcode_domain::{ErrorCode, TraceLevel, WhError};
use whcode_store::{PurgeCriteria, PurgeProtection};

use crate::config::Config;
use crate::workspace;

/// 单次 CUI 命令的最大事件条数（与 store 分页上限一致）。
const EVENTS_PAGE_LIMIT: usize = 1000;

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
        Command::Spec { command } => {
            spec_command(command, workspace, &config, &policy, &actor, texts)
        }
        Command::Events { command } => events_command(command, workspace, &config, &policy, &actor),
        Command::Db { command } => db_command(command, workspace, &config, &actor, now, texts),
        Command::Run { .. } => Err(not_wired(
            "run 未接线：LLM 流式交互需 Provider 与会话流装配（M3 桩已交付，CLI 执行属后续）",
        )),
        Command::Runtime { .. } => Err(not_wired(
            "runtime 未接线：运行时装配开关（M8）属后续任务，当前不伪造启用状态",
        )),
        Command::Serve { .. } => Err(not_wired(
            "serve 由主入口装配：使用 `whcode serve --web [--bind …] [--workspace …]`",
        )),
        Command::Doctor { .. } => Err(not_wired(
            "doctor 由主入口处理：使用 `whcode doctor [--json]`",
        )),
    }
}

fn not_wired(message: &str) -> WhError {
    WhError::new(
        ErrorCode::InvalidState,
        format!("未实现：{message}（显式失败，不静默成功）"),
    )
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
            let conn = open_db(workspace, config)?;
            let row = whcode_app::new_session(
                &conn,
                policy,
                actor,
                NewSession {
                    title: title.clone(),
                    model: model.clone(),
                },
                now,
            )?;
            Ok((session_json(&row), row_line(&row)))
        }
        SessionCommand::List { all } => {
            let conn = open_db(workspace, config)?;
            let rows = whcode_app::list_sessions(&conn, policy, actor, *all, 200)?;
            let human = rows.iter().map(row_line).collect::<Vec<_>>().join("\n");
            Ok((
                json!({ "sessions": rows.iter().map(row_value).collect::<Vec<_>>() }),
                human,
            ))
        }
        SessionCommand::Show { session } => {
            let conn = open_db(workspace, config)?;
            let row = whcode_app::show_session(&conn, policy, actor, session)?;
            Ok((session_json(&row), row_line(&row)))
        }
        SessionCommand::Rename { session, title } => {
            let conn = open_db(workspace, config)?;
            let row = whcode_app::rename_session(&conn, policy, actor, session, title)?;
            Ok((session_json(&row), row_line(&row)))
        }
        SessionCommand::Archive { session, unarchive } => {
            let mut conn = open_db(workspace, config)?;
            let ids = [session.as_str()];
            whcode_app::set_session_archived(&mut conn, policy, actor, &ids, !unarchive)?;
            let row = whcode_app::show_session(&conn, policy, actor, session)?;
            Ok((session_json(&row), row_line(&row)))
        }
        SessionCommand::Delete {
            session,
            dry_run,
            yes,
        } => {
            let is_dry_run = confirm_flags(*dry_run, *yes)?;
            let mut conn = open_db(workspace, config)?;
            // 显式按 ID 删除（用户点名 + --yes）：不套用 TTL 的活跃窗口保护，
            // 否则“删除刚创建的会话”会被静默跳过（预览/执行同口径）。
            let protection = PurgeProtection {
                active_window_ms: 0,
                ..PurgeProtection::default()
            };
            if is_dry_run {
                let preview = whcode_app::preview_delete_session(
                    &conn,
                    policy,
                    actor,
                    session,
                    &protection,
                    now,
                )?;
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
            let result = whcode_app::delete_session(
                &mut conn,
                policy,
                actor,
                session,
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
            let human = format!(
                "{} {} [{}] retry={} 检查点={} 产物={}",
                detail.task.id,
                detail.task.title,
                detail.task.status.as_str(),
                detail.task.retry_count,
                detail.checkpoints.len(),
                detail.artifacts.len()
            );
            Ok((value, human))
        }
        TaskCommand::Retry { task } => {
            let conn = open_db(workspace, config)?;
            let row = whcode_app::retry_task(&conn, policy, actor, task, now)?;
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
            let conn = open_db(workspace, config)?;
            let row = whcode_app::abort_task(&conn, policy, actor, task, now)?;
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
            let human = texts.resolve_fmt(
                "cui.spec_status",
                &[
                    ("session", session),
                    (
                        "stage",
                        snapshot.session.stage.as_deref().unwrap_or("[unknown]"),
                    ),
                    ("cursor", &snapshot.event_cursor.to_string()),
                ],
            );
            Ok((value, human))
        }
        SpecCommand::Next { .. }
        | SpecCommand::Validate { .. }
        | SpecCommand::Analyze { .. }
        | SpecCommand::Converge { .. }
        | SpecCommand::ForceAdvance { .. } => Err(not_wired(
            "spec 门禁命令需 SDD 产物工作区上下文（specs/ 与 workflow 配置加载）——属后续接线",
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
        EventsCommand::Show { event } => Err(not_wired(&format!(
            "events show {event} 未接线：单事件按 ID 查询的应用服务接口未提供"
        ))),
    }
}

fn db_command(
    command: &DbCommand,
    workspace: &Path,
    config: &Config,
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
            let preview = whcode_store::preview_purge(&conn, &criteria, &protection, now)?;
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
            let result = whcode_store::execute_purge(
                &mut conn,
                &preview,
                true,
                &actor.id,
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
        DbCommand::Checkpoint => Err(not_wired(
            "db checkpoint 未接线：独立 CHECKPOINT 属 compact 内部步骤（M1/T041），不作为公开命令",
        )),
    }
}

fn counts_json(counts: &whcode_store::PurgeCounts) -> Value {
    json!({
        "sessions": counts.sessions,
        "tasks": counts.tasks,
        "checkpoints": counts.checkpoints,
        "runtime_executions": counts.runtime_executions,
        "stage_artifacts": counts.stage_artifacts,
        "events": counts.events,
    })
}

fn session_json(row: &whcode_store::SessionRow) -> Value {
    json!({ "session": row_value(row) })
}

fn row_value(row: &whcode_store::SessionRow) -> Value {
    json!({
        "id": row.id,
        "title": row.title,
        "stage": row.stage,
        "status": row.status,
        "archived": row.archived,
        "created_at_ms": row.created_at_ms,
    })
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
    json!({
        "task": {
            "id": row.id,
            "title": row.title,
            "status": row.status.as_str(),
            "stage": row.stage,
            "retry_count": row.retry_count,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{dispatch_code, execute};
    use std::path::PathBuf;
    use whcode_cui::cli::{Command, SessionCommand, TaskCommand};

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

        let archive = execute(
            &Command::Session {
                command: SessionCommand::Archive {
                    session: id.clone(),
                    unarchive: false,
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

    /// 未接线命令显式失败（不静默成功）。
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
                command: whcode_cui::cli::RuntimeCommand::List,
            },
            Command::Spec {
                command: whcode_cui::cli::SpecCommand::Next {
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
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }
}
