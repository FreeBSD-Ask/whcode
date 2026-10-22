//! # 任务总览命令（T048、FR-103、ADR-018）
//!
//! [`task_overview`] 一次输出四类字段（Check 48）：阶段分布、完成度、阻塞数、
//! 活跃任务清单。**一致性口径**：本命令直接读 `tasks` 原始行——是**强一致**视图；
//! 界面展示用的最终一致预聚合视图见 [`crate::snapshot`]（FR-506、§7.3-8）。
//!
//! 语义定义：`done` = `status = completed`；`blocked` = `status = blocked`；
//! 活跃任务 = `status = running`（正在执行），最近更新在前、有界。
//!
//! `session_id = None` 表示工作区级总览（FR-103 要求工作区与当前会话两个层级）。

use std::collections::BTreeMap;

use duckdb::Connection;
use whcode_domain::WhError;
use whcode_store::tasks::{self, TaskRow};

use crate::authz::{Actor, Capability, Policy, require};

/// 任务总览（强一致计算视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOverview {
    /// 会话范围；`None` 为工作区级。
    pub session_id: Option<String>,
    /// 阶段分布（按阶段名排序）。
    pub by_stage: BTreeMap<String, i64>,
    /// 任务总数。
    pub total: i64,
    /// `completed` 任务数。
    pub done: i64,
    /// `blocked` 任务数。
    pub blocked: i64,
    /// 活跃任务（`running`，最近更新在前，有界）。
    pub active: Vec<TaskRow>,
}

/// 生成任务总览（FR-103）。
///
/// # Errors
///
/// 权限拒绝或存储失败（含未知任务状态取值）时返回 [`WhError`]。
pub fn task_overview(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: Option<&str>,
) -> Result<TaskOverview, WhError> {
    require(policy, actor, Capability::TaskRead)?;
    let by_stage = tasks::stage_distribution(conn, session_id)?;
    let statuses = tasks::status_distribution(conn, session_id)?;
    let total: i64 = statuses.values().sum();
    let done = statuses.get("completed").copied().unwrap_or(0);
    let blocked = statuses.get("blocked").copied().unwrap_or(0);
    let active = tasks::active_tasks(conn, session_id)?;
    Ok(TaskOverview {
        session_id: session_id.map(str::to_string),
        by_stage,
        total,
        done,
        blocked,
        active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::Connection;
    use whcode_domain::TaskStatus;
    use whcode_store::tasks::NewTask;
    use whcode_store::{apply_migrations, insert_task, update_task_status};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn seed(conn: &Connection) {
        insert_task(
            conn,
            &NewTask {
                id: "T-1",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("T-1");
        insert_task(
            conn,
            &NewTask {
                id: "T-2",
                session_id: "S-1",
                title: "任务",
                stage: "implement",
                parent_id: None,
                acceptance_json: "[]",
            },
            2_000,
        )
        .expect("T-2");
        insert_task(
            conn,
            &NewTask {
                id: "T-3",
                session_id: "S-1",
                title: "任务",
                stage: "implement",
                parent_id: None,
                acceptance_json: "[]",
            },
            3_000,
        )
        .expect("T-3");
        insert_task(
            conn,
            &NewTask {
                id: "T-4",
                session_id: "S-2",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "[]",
            },
            4_000,
        )
        .expect("T-4");
        update_task_status(conn, "T-1", TaskStatus::Completed, 0, 3_100).expect("完成");
        update_task_status(conn, "T-2", TaskStatus::Blocked, 0, 3_200).expect("阻塞");
        update_task_status(conn, "T-3", TaskStatus::Running, 0, 3_300).expect("执行中");
        update_task_status(conn, "T-4", TaskStatus::Running, 0, 3_400).expect("S-2 执行中");
    }

    #[test]
    fn overview_reports_stage_done_blocked_active_for_both_scopes() {
        let conn = migrated();
        seed(&conn);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let session_view = task_overview(&conn, &policy, &actor, Some("S-1")).expect("会话总览");
        assert_eq!(session_view.total, 3);
        assert_eq!(session_view.done, 1);
        assert_eq!(session_view.blocked, 1);
        assert_eq!(session_view.by_stage.get("plan"), Some(&1));
        assert_eq!(session_view.by_stage.get("implement"), Some(&2));
        assert_eq!(session_view.active.len(), 1);
        assert_eq!(session_view.active[0].id, "T-3");

        let workspace_view = task_overview(&conn, &policy, &actor, None).expect("工作区总览");
        assert_eq!(workspace_view.total, 4);
        assert_eq!(workspace_view.done, 1);
        assert_eq!(workspace_view.blocked, 1);
        assert_eq!(workspace_view.by_stage.get("plan"), Some(&2));
        assert_eq!(workspace_view.active.len(), 2, "S-1 与 S-2 的活跃任务都在");
        assert!(workspace_view.session_id.is_none());
    }

    #[test]
    fn overview_is_empty_for_unknown_session_and_denied_without_capability() {
        let conn = migrated();
        seed(&conn);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let empty = task_overview(&conn, &policy, &actor, Some("S-none")).expect("空总览");
        assert_eq!(empty.total, 0);
        assert!(empty.by_stage.is_empty());
        assert!(empty.active.is_empty());

        let err = task_overview(
            &conn,
            &policy,
            &crate::authz::Actor::new("无能力", []),
            Some("S-1"),
        )
        .expect_err("无 TaskRead 必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }
}
