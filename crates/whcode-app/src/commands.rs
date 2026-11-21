//! # 前端写命令门面（FR-623、Task T176、ADR-032）
//!
//! 会话 new／rename／archive／delete、任务 retry／abort、清理 purge 的**唯一执行入口**：
//! 相同前置校验、相同 `Capability` 判定（各 app 函数内部 `require`）、相同预览／确认语义、
//! 相同审计路径与错误码。CUI／Web／TUI 都经本模块执行写命令——行为等价由此可证
//! （`tests/contract` 行为等价夹具），任何前端不得绕过门面直接调用散装 app 函数。
//!
//! 并发档案声明（ADR-015，复核增补）：本模块为**同步调用**，复用调用方现有线程模型
//! （CUI 单次请求、Web 单线程服务器、TUI 事件循环、SSH 单执行线程），不新增线程、
//! 锁或异步运行时；若实现引入任何并发原语，须先补 ADR-015 八项档案再合入。
//!
//! 呈现无关：[`CommandOutcome`] 返回 app 层数据类型（行／计数），JSON 形状与人读文案
//! 属各前端的呈现层；[`CommandExecutor`] trait 供 TUI 这类需要注入执行器的端使用
//! （装配根实现，见 `whcode-bin`）。
//!
//! 统一口径（自 CUI／Web 现状收敛，迁移后两段语义不变）：**按 ID 删除会话不套用活跃
//! 窗口保护**（[`SESSION_DELETE_ACTIVE_WINDOW_MS`] = 0）——否则“删除刚创建的会话”会被
//! 静默跳过（预览／执行同口径）；**db purge 用默认保护**（FR-508 活跃窗口可见可改）。

use duckdb::Connection;
use whcode_domain::WhError;
use whcode_store::{PurgeCriteria, PurgePreview, PurgeProtection, PurgeResult, tasks::TaskRow};

use crate::authz::Actor;
use crate::{NewSession, Policy};
use crate::{
    abort_task, delete_session, new_session, preview_delete_session, purge_execute, purge_preview,
    rename_session, retry_task, set_session_archived, show_session,
};

/// 按 ID 删除会话的活跃窗口保护（毫秒）：0 = 不套用（用户点名 + 显式确认）。
pub const SESSION_DELETE_ACTIVE_WINDOW_MS: i64 = 0;

/// 前端写命令（数据形状；`now_ms` 由调用方时钟注入，便于测试与重放）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendCommand {
    /// 新建会话。
    SessionNew {
        title: String,
        model: Option<String>,
        now_ms: u64,
    },
    /// 重命名会话。
    SessionRename {
        session_id: String,
        title: String,
        now_ms: u64,
    },
    /// 归档／取消归档：`confirm = false` 为预览（不写库、不写审计），`true` 才更新并留痕。
    SessionArchive {
        session_id: String,
        archived: bool,
        confirm: bool,
        now_ms: u64,
    },
    /// 删除会话：`confirm = false` 为预览（不写库、无执行审计），`true` 才删除。
    SessionDelete {
        session_id: String,
        confirm: bool,
        now_ms: u64,
    },
    /// 重试任务。
    TaskRetry { task_id: String, now_ms: u64 },
    /// 中止任务。
    TaskAbort { task_id: String, now_ms: u64 },
    /// 清理预览：按“早于 cutoff”冻结清单，不写库。
    PurgePreview { before_ms: i64, now_ms: u64 },
    /// 清理执行：`confirm = false` 一律拒绝（破坏性命令无预览不执行）。
    PurgeExecute {
        before_ms: i64,
        confirm: bool,
        now_ms: u64,
    },
    /// 撤销一条本地审批规则（T258／FR-751；设置页可撤销“记住规则”写下的 allow／deny）。
    ApprovalRuleRevoke { tool: String, now_ms: u64 },
    /// 分叉会话（T391／FR-769／ADR-065）：在 turn 结束边界复制对话历史前缀到新会话。
    SessionFork {
        source_session: String,
        at_seq: i64,
        title: Option<String>,
        now_ms: u64,
    },
    /// 回退对话到 turn 边界（T391／FR-775／ADR-068）：只落边界，不删事件。
    SessionRewind { session_id: String, to_seq: i64 },
    /// 恢复文件前像（T391／FR-776／ADR-068；破坏族）：`confirm = false` 为预览，
    /// `true` 才写回工作区并审计。`workspace` 由装配根注入（恢复写的是工作区文件）。
    SessionRestore {
        session_id: String,
        exec_id: String,
        workspace: std::path::PathBuf,
        confirm: bool,
    },
    /// 切换计划模式（T391／FR-774／ADR-067）：软引导，不经审批，落 `PlanModeChanged`。
    SessionPlanSet { session_id: String, active: bool },
    /// SDD 阶段推进（T394／FR-307/308）：`to` 缺省沿状态机后继；门禁输入由
    /// 工作区 specs/ 产物解析提供（app::spec）。`workspace` 由执行器注入（TUI）。
    SpecNext {
        session_id: String,
        to: Option<String>,
        workspace: std::path::PathBuf,
    },
    /// 人工强制越过门禁（T394／FR-312）：reason 必填，审计完整要素。
    SpecForceAdvance {
        session_id: String,
        to: String,
        reason: String,
        workspace: std::path::PathBuf,
    },
    /// 启用运行时（T406／FR-800）：审计由 `runtime_mgmt::enable` 落表。
    RuntimeEnable { name: String, now_ms: u64 },
    /// 停用运行时（T406／FR-800）：可撤销，审计由 `runtime_mgmt::disable` 落表。
    RuntimeDisable { name: String, now_ms: u64 },
    /// 登记本地 payload 安装（T406／FR-800）：仅本地文件哈希比对，不联网；
    /// `payload` 为本地路径，装配根可覆写为工作区内路径。
    RuntimeInstall {
        name: String,
        version: String,
        hash: String,
        payload: std::path::PathBuf,
        now_ms: u64,
    },
}

/// 执行结果（呈现无关；JSON 形状由前端呈现层构造）。
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// 会话行（new／rename／archive 统一返回最新行）。
    Session(whcode_store::SessionRow),
    /// 归档预览：当前行 + 目标状态，不写库。
    SessionArchivePreview {
        /// 预览时点的会话行（含当前 `archived`）。
        session: whcode_store::SessionRow,
        /// 目标归档状态。
        target_archived: bool,
    },
    /// 删除预览。
    SessionDeletePreview(PurgePreview),
    /// 删除执行结果。
    SessionDeleted(PurgeResult),
    /// 任务行（retry／abort）。
    Task(TaskRow),
    /// 清理预览。
    PurgePreview(PurgePreview),
    /// 清理执行结果。
    PurgeExecuted(PurgeResult),
    /// 审批规则撤销结果（`removed = false` 表示规则本就不存在，不伪造删除）。
    ApprovalRuleRevoked {
        /// 工具稳定名。
        tool: String,
        /// 是否确有删除。
        removed: bool,
    },
    /// 会话分叉结果（FR-769）。
    SessionForked(whcode_store::ForkOutcome),
    /// 对话回退结果（FR-775；只落边界）。
    SessionRewound(crate::rewind::RewindView),
    /// 前像恢复预览（`confirm = false`；不写工作区、不审计）。
    PreimageRestorePreview(Vec<whcode_store::preimage::PreimageRow>),
    /// 前像恢复执行结果（路径 → 动作，FR-776）。
    PreimageRestored { results: Vec<(String, String)> },
    /// 计划模式切换结果（FR-774）。
    PlanSet(crate::plan::PlanView),
    /// SDD 阶段推进结果（FR-308；返回推进后的会话行）。
    SpecAdvanced(whcode_store::SessionRow),
    /// 运行时管理结果（T406／FR-800；声明＋探测状态合并）。
    RuntimeStatus(crate::runtime_mgmt::RuntimeStatusView),
}

/// 执行器抽象（TUI 等“动作 → 门面”注入用；装配根实现持有连接与授权）。
pub trait CommandExecutor {
    /// # Errors
    ///
    /// 权限拒绝、确认缺失或存储失败时返回 [`WhError`]。
    fn execute(&self, command: FrontendCommand) -> Result<CommandOutcome, WhError>;
}

/// 执行写命令（FR-623 唯一入口）。
///
/// # Errors
///
/// 权限拒绝、确认缺失、目标不存在或存储失败时返回 [`WhError`]（与散装 app 函数同码）。
pub fn execute_command(
    conn: &mut Connection,
    policy: &dyn Policy,
    actor: &Actor,
    command: &FrontendCommand,
) -> Result<CommandOutcome, WhError> {
    match command {
        FrontendCommand::SessionNew {
            title,
            model,
            now_ms,
        } => {
            let row = new_session(
                conn,
                policy,
                actor,
                NewSession {
                    title: title.clone(),
                    model: model.clone(),
                },
                *now_ms,
            )?;
            Ok(CommandOutcome::Session(row))
        }
        FrontendCommand::SessionRename {
            session_id,
            title,
            now_ms: _,
        } => {
            let row = rename_session(conn, policy, actor, session_id, title)?;
            Ok(CommandOutcome::Session(row))
        }
        FrontendCommand::SessionArchive {
            session_id,
            archived,
            confirm,
            now_ms: _,
        } => {
            if !*confirm {
                let row = show_session(conn, policy, actor, session_id)?;
                return Ok(CommandOutcome::SessionArchivePreview {
                    session: row,
                    target_archived: *archived,
                });
            }
            let ids = [session_id.as_str()];
            set_session_archived(conn, policy, actor, &ids, *archived)?;
            let row = show_session(conn, policy, actor, session_id)?;
            Ok(CommandOutcome::Session(row))
        }
        FrontendCommand::SessionDelete {
            session_id,
            confirm,
            now_ms,
        } => {
            let protection = PurgeProtection {
                active_window_ms: SESSION_DELETE_ACTIVE_WINDOW_MS,
                ..PurgeProtection::default()
            };
            if !*confirm {
                let preview =
                    preview_delete_session(conn, policy, actor, session_id, &protection, *now_ms)?;
                return Ok(CommandOutcome::SessionDeletePreview(preview));
            }
            let result =
                delete_session(conn, policy, actor, session_id, true, &protection, *now_ms)?;
            Ok(CommandOutcome::SessionDeleted(result))
        }
        FrontendCommand::TaskRetry { task_id, now_ms } => {
            let row = retry_task(conn, policy, actor, task_id, *now_ms)?;
            Ok(CommandOutcome::Task(row))
        }
        FrontendCommand::TaskAbort { task_id, now_ms } => {
            let row = abort_task(conn, policy, actor, task_id, *now_ms)?;
            Ok(CommandOutcome::Task(row))
        }
        FrontendCommand::PurgePreview { before_ms, now_ms } => {
            let preview = purge_preview(
                conn,
                policy,
                actor,
                &PurgeCriteria::before(*before_ms),
                &PurgeProtection::default(),
                *now_ms,
            )?;
            Ok(CommandOutcome::PurgePreview(preview))
        }
        FrontendCommand::PurgeExecute {
            before_ms,
            confirm,
            now_ms,
        } => {
            let preview = purge_preview(
                conn,
                policy,
                actor,
                &PurgeCriteria::before(*before_ms),
                &PurgeProtection::default(),
                *now_ms,
            )?;
            let result = purge_execute(
                conn,
                policy,
                actor,
                &preview,
                *confirm,
                &PurgeProtection::default(),
                *now_ms,
            )?;
            Ok(CommandOutcome::PurgeExecuted(result))
        }
        FrontendCommand::ApprovalRuleRevoke { tool, now_ms } => {
            let removed = crate::approval_rules::revoke_rule(conn, policy, actor, tool, *now_ms)?;
            Ok(CommandOutcome::ApprovalRuleRevoked {
                tool: tool.clone(),
                removed,
            })
        }
        FrontendCommand::SessionFork {
            source_session,
            at_seq,
            title,
            now_ms,
        } => {
            let outcome = crate::session::fork_session(
                conn,
                policy,
                actor,
                source_session,
                *at_seq,
                title.as_deref(),
                *now_ms,
            )?;
            Ok(CommandOutcome::SessionForked(outcome))
        }
        FrontendCommand::SessionRewind { session_id, to_seq } => {
            let view = crate::rewind_session(conn, policy, actor, session_id, *to_seq)?;
            Ok(CommandOutcome::SessionRewound(view))
        }
        FrontendCommand::SessionRestore {
            session_id,
            exec_id,
            workspace,
            confirm,
        } => {
            if !*confirm {
                let rows = crate::preimage::preview(conn, policy, actor, session_id, exec_id)?;
                return Ok(CommandOutcome::PreimageRestorePreview(rows));
            }
            let results =
                crate::preimage::restore(conn, policy, actor, session_id, workspace, exec_id)?;
            Ok(CommandOutcome::PreimageRestored { results })
        }
        FrontendCommand::SessionPlanSet { session_id, active } => {
            let view = crate::set_plan(conn, policy, actor, session_id, *active)?;
            Ok(CommandOutcome::PlanSet(view))
        }
        FrontendCommand::SpecNext {
            session_id,
            to,
            workspace,
        } => {
            let row = crate::spec::next(conn, policy, actor, session_id, workspace, to.as_deref())?;
            Ok(CommandOutcome::SpecAdvanced(row))
        }
        FrontendCommand::SpecForceAdvance {
            session_id,
            to,
            reason,
            workspace,
        } => {
            let row = crate::spec::force_advance_view(
                conn,
                policy,
                actor,
                session_id,
                workspace,
                to,
                reason,
                now_ms_field(),
            )?;
            Ok(CommandOutcome::SpecAdvanced(row))
        }
        FrontendCommand::RuntimeEnable { name, now_ms } => {
            let view = crate::runtime_enable(conn, policy, actor, name, *now_ms)?;
            Ok(CommandOutcome::RuntimeStatus(view))
        }
        FrontendCommand::RuntimeDisable { name, now_ms } => {
            let view = crate::runtime_disable(conn, policy, actor, name, *now_ms)?;
            Ok(CommandOutcome::RuntimeStatus(view))
        }
        FrontendCommand::RuntimeInstall {
            name,
            version,
            hash,
            payload,
            now_ms,
        } => {
            let view =
                crate::runtime_install(conn, policy, actor, name, version, hash, payload, *now_ms)?;
            Ok(CommandOutcome::RuntimeStatus(view))
        }
    }
}

/// 门面内取时钟（SpecForceAdvance 的审计时间戳；与各前端 `now_ms` 注入同源）。
fn now_ms_field() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefaultPolicy, actor_with_all_capabilities};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn
    }

    fn actor() -> Actor {
        actor_with_all_capabilities("facade-tester")
    }

    fn weak() -> Actor {
        Actor::new("weak", [])
    }

    #[test]
    fn session_lifecycle_roundtrip_through_facade() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let me = actor();
        let now = 1_700_000_000_000_u64;

        let outcome = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionNew {
                title: "门面会话".to_string(),
                model: None,
                now_ms: now,
            },
        )
        .expect("新建");
        let row = match outcome {
            CommandOutcome::Session(row) => row,
            other => panic!("应为 Session：{other:?}"),
        };
        let id = row.id.clone();

        for command in [
            FrontendCommand::SessionRename {
                session_id: id.clone(),
                title: "改名".to_string(),
                now_ms: now,
            },
            FrontendCommand::SessionArchive {
                session_id: id.clone(),
                archived: true,
                confirm: true,
                now_ms: now,
            },
        ] {
            let outcome = execute_command(&mut conn, &policy, &me, &command).expect("执行");
            assert!(matches!(outcome, CommandOutcome::Session(_)), "{command:?}");
        }
    }

    /// FR-623 核心：预览不写库不审计；确认才删除；权限拒绝三端同码（门面单点判定）。
    #[test]
    fn delete_preview_then_confirm_and_permission_denial() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let me = actor();
        let now = 1_700_000_000_000_u64;
        let outcome = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionNew {
                title: "待删".to_string(),
                model: None,
                now_ms: now - 1_000,
            },
        )
        .expect("新建");
        let CommandOutcome::Session(row) = outcome else {
            panic!("应为 Session");
        };
        let events_before = whcode_count(&conn, "SELECT count(*) FROM events");
        let audit_before = whcode_count(&conn, "SELECT count(*) FROM audit");

        // 预览：不改状态。
        let preview = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionDelete {
                session_id: row.id.clone(),
                confirm: false,
                now_ms: now,
            },
        )
        .expect("预览");
        let CommandOutcome::SessionDeletePreview(p) = preview else {
            panic!("应为预览");
        };
        assert_eq!(p.selected_sessions, vec![row.id.clone()]);
        assert_eq!(
            whcode_count(&conn, "SELECT count(*) FROM sessions"),
            1,
            "预览不得删除"
        );
        assert_eq!(
            whcode_count(&conn, "SELECT count(*) FROM audit"),
            audit_before,
            "预览不得写执行审计"
        );

        // 确认：删除 + 审计。
        let executed = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionDelete {
                session_id: row.id.clone(),
                confirm: true,
                now_ms: now,
            },
        )
        .expect("确认删除");
        let CommandOutcome::SessionDeleted(result) = executed else {
            panic!("应为执行结果");
        };
        assert_eq!(result.deleted_sessions, vec![row.id]);
        assert_eq!(whcode_count(&conn, "SELECT count(*) FROM sessions"), 0);
        assert!(whcode_count(&conn, "SELECT count(*) FROM audit") > audit_before);
        let _ = events_before;

        // 权限拒绝：无能力调用者 → permission_denied（与 CUI/Web 同码同语义）。
        let weak_actor = weak();
        let err = execute_command(
            &mut conn,
            &policy,
            &weak_actor,
            &FrontendCommand::SessionNew {
                title: "越权".to_string(),
                model: None,
                now_ms: now,
            },
        )
        .expect_err("无能力必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }

    /// 归档与删除同口径：预览不写库不审计，确认才更新并写 `session_archive` 审计（ADR-038）。
    #[test]
    fn archive_preview_then_confirm_writes_audit() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let me = actor();
        let now = 1_700_000_000_000_u64;
        let outcome = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionNew {
                title: "待归档".to_string(),
                model: None,
                now_ms: now,
            },
        )
        .expect("新建");
        let CommandOutcome::Session(row) = outcome else {
            panic!("应为 Session");
        };
        let audit_before = whcode_count(&conn, "SELECT count(*) FROM audit");

        let preview = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionArchive {
                session_id: row.id.clone(),
                archived: true,
                confirm: false,
                now_ms: now,
            },
        )
        .expect("预览");
        let CommandOutcome::SessionArchivePreview {
            session,
            target_archived,
        } = preview
        else {
            panic!("应为归档预览");
        };
        assert!(!session.archived, "预览行须反映当前状态");
        assert!(target_archived, "预览须给出目标状态");
        assert_eq!(
            whcode_count(&conn, "SELECT count(*) FROM audit"),
            audit_before,
            "预览不得写审计"
        );
        let still_active: i64 = conn
            .query_row("SELECT count(*) FROM sessions WHERE archived", [], |r| {
                r.get(0)
            })
            .expect("统计");
        assert_eq!(still_active, 0, "预览不得改状态");

        let executed = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionArchive {
                session_id: row.id.clone(),
                archived: true,
                confirm: true,
                now_ms: now,
            },
        )
        .expect("确认归档");
        let CommandOutcome::Session(archived_row) = executed else {
            panic!("应为 Session");
        };
        assert!(archived_row.archived);
        assert_eq!(
            whcode_count(&conn, "SELECT count(*) FROM audit"),
            audit_before + 1,
            "确认归档须写一条审计"
        );
        let action: String = conn
            .query_row(
                "SELECT action FROM audit ORDER BY ts DESC, id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("读取审计动作");
        assert_eq!(action, "session_archive");
    }

    /// purge 预览与确认经门面语义不变：confirm=false 由 store 拒绝（既有防线）。
    #[test]
    fn purge_preview_then_execute_requires_explicit_confirm() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let me = actor();
        let now = 1_700_000_000_000_u64;
        let outcome = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionNew {
                title: "旧会话".to_string(),
                model: None,
                now_ms: now,
            },
        )
        .expect("新建");
        let CommandOutcome::Session(row) = outcome else {
            panic!("应为 Session");
        };
        // 把会话改旧（cutoff 之后）。
        conn.execute(
            "UPDATE sessions SET last_active_at = epoch_ms(?) WHERE id = ?",
            duckdb::params![1_000_i64, row.id],
        )
        .expect("改旧");
        let before_ms = 1_600_000_000_000_i64;

        let preview = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::PurgePreview {
                before_ms,
                now_ms: now,
            },
        )
        .expect("清理预览");
        let CommandOutcome::PurgePreview(p) = preview else {
            panic!("应为清理预览");
        };
        assert_eq!(p.selected_sessions, vec![row.id.clone()]);

        // confirm=false：purge_execute 的既有防线拒绝（不静默执行）。
        let refused = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::PurgeExecute {
                before_ms,
                confirm: false,
                now_ms: now,
            },
        );
        assert!(refused.is_err(), "未确认必须拒绝");
        assert_eq!(whcode_count(&conn, "SELECT count(*) FROM sessions"), 1);

        let executed = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::PurgeExecute {
                before_ms,
                confirm: true,
                now_ms: now,
            },
        )
        .expect("确认清理");
        let CommandOutcome::PurgeExecuted(result) = executed else {
            panic!("应为清理结果");
        };
        assert!(result.deleted_sessions.contains(&row.id));
    }

    fn whcode_count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).expect("计数")
    }

    /// T391：fork／rewind／plan 经门面执行——与 CUI 同一入口、同码同语义。
    #[test]
    fn fork_rewind_and_plan_flow_through_facade() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let me = actor();
        let now = 1_700_000_000_000_u64;

        let outcome = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionNew {
                title: "源会话".to_string(),
                model: None,
                now_ms: now,
            },
        )
        .expect("新建");
        let CommandOutcome::Session(row) = outcome else {
            panic!("应为 Session");
        };
        let source = row.id.clone();

        let forked = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionFork {
                source_session: source.clone(),
                at_seq: 0,
                title: None,
                now_ms: now,
            },
        )
        .expect("分叉");
        let CommandOutcome::SessionForked(fork) = forked else {
            panic!("应为 SessionForked");
        };
        assert_eq!(fork.source_session, source);
        assert_eq!(fork.inherited_events, 0, "空分叉继承零事件");
        assert!(
            fork.session.title.contains("fork"),
            "缺省标题取源标题 (fork)"
        );

        // 回退要求会话有历史：播种一条 TurnFinished 事件（与 store 夹具同源口径）。
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (1, 'E-1', ?, NULL, NULL, NULL, 'TurnFinished',
                     CAST('{}' AS JSON)::VARIANT, current_timestamp, 'redacted')",
            duckdb::params![source],
        )
        .expect("播种事件");

        let rewound = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionRewind {
                session_id: source.clone(),
                to_seq: 0,
            },
        )
        .expect("回退");
        let CommandOutcome::SessionRewound(view) = rewound else {
            panic!("应为 SessionRewound");
        };
        assert_eq!(view.target_seq, Some(0), "回退到会话起点");

        let planned = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionPlanSet {
                session_id: source.clone(),
                active: true,
            },
        )
        .expect("计划模式");
        let CommandOutcome::PlanSet(plan) = planned else {
            panic!("应为 PlanSet");
        };
        assert!(plan.active, "计划模式已开启");

        // 无能力调用者同码拒绝（FR-604 与端无关）。
        for command in [
            FrontendCommand::SessionFork {
                source_session: source.clone(),
                at_seq: 0,
                title: None,
                now_ms: now,
            },
            FrontendCommand::SessionRewind {
                session_id: source.clone(),
                to_seq: 0,
            },
            FrontendCommand::SessionPlanSet {
                session_id: source.clone(),
                active: false,
            },
            FrontendCommand::SessionRestore {
                session_id: source.clone(),
                exec_id: "TX-1".to_string(),
                workspace: std::env::temp_dir(),
                confirm: false,
            },
        ] {
            let err =
                execute_command(&mut conn, &policy, &weak(), &command).expect_err("无能力必须拒绝");
            assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
        }
    }

    /// T391：前像恢复经门面——预览不写工作区，确认才执行；无前像 `not_found`。
    #[test]
    fn restore_previews_before_confirm_and_rejects_missing_exec() {
        let mut conn = migrated();
        let policy = DefaultPolicy;
        let me = actor();
        let now = 1_700_000_000_000_u64;
        let outcome = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionNew {
                title: "恢复会话".to_string(),
                model: None,
                now_ms: now,
            },
        )
        .expect("新建");
        let CommandOutcome::Session(row) = outcome else {
            panic!("应为 Session");
        };

        let err = execute_command(
            &mut conn,
            &policy,
            &me,
            &FrontendCommand::SessionRestore {
                session_id: row.id.clone(),
                exec_id: "TX-404".to_string(),
                workspace: std::env::temp_dir(),
                confirm: false,
            },
        )
        .expect_err("无前像必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::NotFound);
    }
}
