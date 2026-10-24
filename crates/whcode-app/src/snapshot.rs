//! # 状态快照（T051、FR-314、contracts/state-snapshot.md）
//!
//! 快照是**只读投影**，四端（TUI/CUI/Web/SSH）同源（FR-314、V-207）；前端不得
//! 自行从历史切片填充字段。字段契约见 `specs/001-whcode-harness/contracts/state-snapshot.md`。
//!
//! ## M2 覆盖范围（未实现部分显式标注，不做占位伪造）
//!
//! | 字段 | M2 状态 | 来源 |
//! |---|---|---|
//! | `schema_version` | 已实现（快照 schema v1） | 常量 |
//! | `session`（id/title/stage/status） | 已实现 | `sessions` 表 |
//! | `session.stage` | 已实现（M4/T080：数据库真实值，四端同源） | `sessions.stage`（v5） |
//! | `overview` | 已实现，**最终一致**（预聚合表，`refreshed_at` 指示滞后） | `session_overview` |
//! | `active_task` | 已实现（最近更新的 `running` 任务） | `tasks` |
//! | `context` | `None`——上下文预算属 M3（T060+） | — |
//! | `llm` | `None`——LLM 状态属 M3（T056+） | — |
//! | `event_cursor` | 已实现（工作区 `events.seq` 最大值） | `events` |
//! | `runtimes` | 空 map——运行时属 M8（FR-704） | — |
//!
//! 边界：只读；快照不产生任何写操作（ADR-003/005：变更必须经应用服务命令）。

use std::collections::BTreeMap;

use duckdb::Connection;
use serde::Serialize;
use whcode_domain::WhError;
use whcode_store::directory;
use whcode_store::overview as overview_store;
use whcode_store::query;
use whcode_store::tasks;

use crate::authz::{Actor, Capability, Policy, require};

/// 快照 schema 版本；字段契约变更须经兼容审查（contracts/state-snapshot.md §2）。
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// 会话快照字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionSnapshot {
    /// 会话 ID。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 当前 SDD 阶段（M4/T080：数据库真实值，稳定名；None 表示历史数据无阶段列）。
    pub stage: Option<String>,
    /// 会话状态（与会话表一致）。
    pub status: String,
}

/// 概览快照字段（**最终一致**：来自预聚合表，可由 `tasks` 重建，FR-506）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OverviewSnapshot {
    /// 任务总数（最终一致）。
    pub total_tasks: i64,
    /// `completed` 任务数（最终一致）。
    pub done: i64,
    /// `blocked` 任务数（最终一致）。
    pub blocked: i64,
    /// 预聚合行的刷新时间（Unix 毫秒；指示滞后程度）。
    pub refreshed_at_ms: i64,
}

/// 活跃任务快照字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActiveTaskSnapshot {
    /// 任务 ID。
    pub id: String,
    /// 状态（domain `TaskStatus` 稳定名）。
    pub status: String,
    /// 重试计数。
    pub retry_count: i64,
}

/// 状态快照（四端同源，FR-314）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StateSnapshot {
    /// 快照 schema 版本。
    pub schema_version: u32,
    /// 会话字段；会话不存在时返回错误而非空快照（显式失败，不静默）。
    pub session: SessionSnapshot,
    /// 任务概览（最终一致）；无预聚合行时为 `None`。
    pub overview: Option<OverviewSnapshot>,
    /// 当前活跃任务；无 `running` 任务时为 `None`。
    pub active_task: Option<ActiveTaskSnapshot>,
    /// 上下文预算字段；M3 未实现，恒为 `None`（契约占位）。
    pub context: Option<serde_json::Value>,
    /// LLM 状态字段；M3 未实现，恒为 `None`（契约占位）。
    pub llm: Option<serde_json::Value>,
    /// 持久化事件游标（断线补拉基线，FR-202/605）。
    pub event_cursor: i64,
    /// 各运行时启用状态；M8 未实现，恒为空 map（FR-704）。
    pub runtimes: BTreeMap<String, serde_json::Value>,
}

/// 生成指定会话的状态快照。
///
/// # Errors
///
/// 权限拒绝、会话不存在或存储失败时返回 [`WhError`]。
pub fn build_snapshot(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<StateSnapshot, WhError> {
    require(policy, actor, Capability::SessionRead)?;
    let session = directory::get_session(conn, session_id)?.ok_or_else(|| {
        WhError::new(
            whcode_domain::ErrorCode::NotFound,
            format!("会话不存在：{session_id}"),
        )
    })?;
    let overview = overview_store::load_overview(conn, session_id)?.map(|row| OverviewSnapshot {
        total_tasks: row.total,
        done: row.done,
        blocked: row.blocked,
        refreshed_at_ms: row.refreshed_at_us.div_euclid(1000),
    });
    let active_task = tasks::active_tasks(conn, Some(session_id))?
        .into_iter()
        .next()
        .map(|task| ActiveTaskSnapshot {
            id: task.id,
            status: task.status.as_str().to_string(),
            retry_count: task.retry_count,
        });
    let event_cursor = query::latest_seq(conn)?;
    Ok(StateSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        session: SessionSnapshot {
            id: session.id,
            title: session.title,
            // T080：阶段状态对四端同源（FR-314）；M4 起为数据库真实值。
            stage: Some(session.stage),
            status: session.status,
        },
        overview,
        active_task,
        context: None,
        llm: None,
        event_cursor,
        runtimes: BTreeMap::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::Connection;
    use whcode_domain::TaskStatus;
    use whcode_store::tasks::NewTask;
    use whcode_store::{apply_migrations, insert_task, rebuild_overview, update_task_status};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn snapshot_fields_match_database_state() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-1', '快照会话', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("插入会话");
        insert_task(
            &conn,
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
            &conn,
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
        update_task_status(&conn, "T-1", TaskStatus::Completed, 0, 1_100).expect("完成");
        update_task_status(&conn, "T-2", TaskStatus::Running, 1, 1_200).expect("执行中");
        let mut conn = conn;
        rebuild_overview(&mut conn, "S-1").expect("重建概览");

        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (42, 'E-42', 'S-1', NULL, NULL, NULL, 'SessionStarted', CAST('{}' AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            [],
        )
        .expect("插入事件");

        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let snapshot = build_snapshot(&conn, &policy, &actor, "S-1").expect("快照");

        assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(snapshot.session.id, "S-1");
        assert_eq!(snapshot.session.title, "快照会话");
        assert_eq!(snapshot.session.status, "active");
        assert!(
            snapshot.session.stage.is_some(),
            "M4 起阶段为数据库真实值（T080 四端同源）"
        );

        let overview = snapshot.overview.clone().expect("概览");
        assert_eq!(overview.total_tasks, 2, "与库内状态一致（Check 51）");
        assert_eq!(overview.done, 1);
        assert_eq!(overview.blocked, 0);

        let active = snapshot.active_task.clone().expect("活跃任务");
        assert_eq!(active.id, "T-2");
        assert_eq!(active.status, "running");
        assert_eq!(active.retry_count, 1);

        assert_eq!(snapshot.event_cursor, 42, "游标须与 events 表一致");
        assert!(snapshot.context.is_none(), "上下文属 M3，显式 None");
        assert!(snapshot.llm.is_none(), "LLM 属 M3，显式 None");
        assert!(snapshot.runtimes.is_empty(), "运行时属 M8，显式空");

        // JSON 序列化字段名与契约一致（snake_case：total_tasks / event_cursor）。
        let json = serde_json::to_value(&snapshot).expect("序列化");
        assert!(json.get("schema_version").is_some());
        assert!(json.get("event_cursor").is_some());
        assert_eq!(json["overview"]["total_tasks"], 2);
    }

    #[test]
    fn snapshot_of_empty_session_has_no_overview_or_active_task() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-empty', '空会话', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("插入会话");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let snapshot = build_snapshot(&conn, &policy, &actor, "S-empty").expect("快照");
        assert!(snapshot.overview.is_none());
        assert!(snapshot.active_task.is_none());
        assert_eq!(snapshot.event_cursor, 0, "空工作区游标为 0");
    }

    #[test]
    fn snapshot_rejects_missing_session_and_denied_callers() {
        let conn = migrated();
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let err = build_snapshot(&conn, &policy, &actor, "S-none").expect_err("缺失会话");
        assert_eq!(err.code, whcode_domain::ErrorCode::NotFound);

        let err = build_snapshot(
            &conn,
            &policy,
            &crate::authz::Actor::new("无能力", []),
            "S-1",
        )
        .expect_err("无能力必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }
}
