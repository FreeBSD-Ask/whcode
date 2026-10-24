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
    /// 归档／取消归档。
    SessionArchive {
        session_id: String,
        archived: bool,
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
}

/// 执行结果（呈现无关；JSON 形状由前端呈现层构造）。
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// 会话行（new／rename／archive 统一返回最新行）。
    Session(whcode_store::SessionRow),
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
            now_ms: _,
        } => {
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
    }
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
}
