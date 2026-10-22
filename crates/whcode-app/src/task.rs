//! # 任务详情与 retry/abort 命令（T049/T052、FR-104/FR-401/FR-408、ADR-018）
//!
//! [`task_detail`] 一次返回五类信息（FR-104、Check 49）：任务行（含 `parent_id`
//! 层级依赖）、验收条件（解析后的 JSON）、步骤（检查点，有界）、产物**引用**
//! （不含大对象正文，R-05）、最近事件（有界）与失败原因。
//!
//! 失败原因的 M2 口径：任务 `failed` 时取该任务最近一条事件的载荷原文；结构化
//! 错误码归一化属 M3/M4（LLM/门禁链路交付后接入），此处不伪造结构。
//!
//! ## 状态机（M2 口径）
//!
//! - `retry`：仅 `failed` / `aborted` / `interrupted` 可重试，重试把任务置回
//!   `ready` 并递增 `retry_count`（M2 无运行时，执行启动属 M8）；
//! - `abort`：终态（`completed` / `aborted`）与 `unknown` 不可中止——`unknown`
//!   必须先经核对/恢复流程（FR-410）；
//! - 其余非法迁移返回 `invalid_state`（`retryable = false`）向上传播，不静默修正。
//!
//! 重试与中止只改任务行，不触碰会话——可重试失败不丢失会话（FR-401）。

use duckdb::Connection;
use whcode_domain::{ErrorCode, TaskStatus, WhError};
use whcode_store::tasks::{self, TaskRow};

use crate::authz::{Actor, Capability, Policy, require};

/// 任务详情（FR-104 五类信息）。
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDetail {
    /// 任务行（`parent_id` 即层级依赖；完整依赖图属 SDD/M4，FR-406）。
    pub task: TaskRow,
    /// 验收条件（解析后的 JSON；损坏即报错，不静默跳过）。
    pub acceptance: serde_json::Value,
    /// 步骤（检查点，按 `step` 升序，有界）。
    pub checkpoints: Vec<whcode_store::CheckpointRow>,
    /// 产物引用（不含大对象正文，R-05）。
    pub artifacts: Vec<whcode_store::ArtifactRefRow>,
    /// 最近事件（`seq` 降序，有界）。
    pub recent_events: Vec<whcode_store::EventRow>,
    /// 失败原因（仅 `failed` 任务；M2 口径见模块文档）。
    pub failure_reason: Option<String>,
}

fn invalid(message: impl Into<String>) -> WhError {
    WhError::new(ErrorCode::InvalidState, message)
}

fn not_found(id: &str) -> WhError {
    WhError::new(ErrorCode::NotFound, format!("任务不存在：{id}"))
}

fn load_task(conn: &Connection, task_id: &str) -> Result<TaskRow, WhError> {
    tasks::get_task(conn, task_id)?.ok_or_else(|| not_found(task_id))
}

/// 返回任务详情（FR-104）。
///
/// # Errors
///
/// 权限拒绝、任务不存在、验收条件损坏或存储失败时返回 [`WhError`]。
pub fn task_detail(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    task_id: &str,
) -> Result<TaskDetail, WhError> {
    require(policy, actor, Capability::TaskRead)?;
    let task = load_task(conn, task_id)?;
    let acceptance: serde_json::Value = serde_json::from_str(&task.acceptance_json)
        .map_err(|err| WhError::parse_error(format!("任务 {task_id} 的验收条件损坏：{err}")))?;
    let checkpoints = tasks::task_checkpoints(conn, task_id)?;
    let artifacts = tasks::task_artifacts(conn, task_id)?;
    let recent_events = tasks::recent_task_events(conn, task_id)?;
    let failure_reason = if task.status == TaskStatus::Failed {
        recent_events
            .first()
            .map(|event| event.payload_json.clone())
    } else {
        None
    };
    Ok(TaskDetail {
        task,
        acceptance,
        checkpoints,
        artifacts,
        recent_events,
        failure_reason,
    })
}

/// 重试任务（FR-401）：`failed`/`aborted`/`interrupted` → `ready`，重试计数 +1。
///
/// # Errors
///
/// 权限拒绝、任务不存在、状态不可重试或存储失败时返回 [`WhError`]。
pub fn retry_task(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    task_id: &str,
    now_ms: u64,
) -> Result<TaskRow, WhError> {
    require(policy, actor, Capability::TaskRetry)?;
    let task = load_task(conn, task_id)?;
    match task.status {
        TaskStatus::Failed | TaskStatus::Aborted | TaskStatus::Interrupted => {
            tasks::update_task_status(
                conn,
                task_id,
                TaskStatus::Ready,
                task.retry_count + 1,
                now_ms,
            )?
        }
        TaskStatus::Completed => {
            return Err(invalid(
                "已完成的任务不得重试：验收条件未变，重试会破坏完成语义（FR-310/311）",
            ));
        }
        TaskStatus::Unknown => {
            return Err(invalid(
                "任务状态 unknown 须先经核对/恢复流程（FR-410），不得直接重试",
            ));
        }
        other => {
            return Err(invalid(format!(
                "任务当前状态 {other} 不可重试（仅 failed/aborted/interrupted 可重试）"
            )));
        }
    }
    load_task(conn, task_id)
}

/// 中止任务（FR-408）：非终态任务置为 `aborted`。
///
/// # Errors
///
/// 权限拒绝、任务不存在、状态不可中止或存储失败时返回 [`WhError`]。
pub fn abort_task(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    task_id: &str,
    now_ms: u64,
) -> Result<TaskRow, WhError> {
    require(policy, actor, Capability::TaskAbort)?;
    let task = load_task(conn, task_id)?;
    match task.status {
        TaskStatus::Completed => {
            return Err(invalid("已完成的任务不可中止"));
        }
        TaskStatus::Aborted => {
            return Err(invalid("任务已被中止，重复中止无效果"));
        }
        TaskStatus::Unknown => {
            return Err(invalid(
                "任务状态 unknown 须先经核对/恢复流程（FR-410），不得直接中止",
            ));
        }
        _ => {}
    }
    tasks::update_task_status(conn, task_id, TaskStatus::Aborted, task.retry_count, now_ms)?;
    load_task(conn, task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::{DefaultPolicy, actor_with_all_capabilities};
    use duckdb::{Connection, params};
    use whcode_domain::TaskStatus;
    use whcode_store::tasks::NewTask;
    use whcode_store::{apply_migrations, insert_task};

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存数据库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn task_with_status(conn: &Connection, id: &str, status: TaskStatus) {
        insert_task(
            conn,
            &NewTask {
                id,
                session_id: "S-1",
                title: "任务",
                stage: "implement",
                parent_id: Some("T-0"),
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("插入任务");
        tasks::update_task_status(conn, id, status, 0, 1_100).expect("置状态");
    }

    fn insert_event(conn: &Connection, seq: i64, task_id: &str, payload: &str) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                 kind, payload_json, ts, persistence_level)
             VALUES (?, ?, 'S-1', ?, NULL, NULL, 'TaskFailed', ?,
                     current_timestamp, 'redacted')",
            params![seq, format!("E-{seq}"), task_id, payload],
        )
        .expect("插入事件");
    }

    #[test]
    fn detail_returns_five_information_classes() {
        let conn = migrated();
        task_with_status(&conn, "T-1", TaskStatus::Failed);
        insert_event(&conn, 1, "T-1", r#"{"error":"磁盘已满"}"#);
        conn.execute(
            "INSERT INTO checkpoints (id, task_id, step, delta_json, message_cursor,
                                      artifact_refs_json, ts)
             VALUES ('C-1', 'T-1', 1, '{}', NULL, '[]', current_timestamp)",
            [],
        )
        .expect("检查点");
        conn.execute(
            "INSERT INTO stage_artifacts (id, session_id, task_id, stage, artifact_type,
                                          content, refs_json, parser_version, validated,
                                          version, created_at)
             VALUES ('A-1', 'S-1', 'T-1', 'implement', 'diff', '大正文',
                     '[\"FR-101\"]', 'v1', TRUE, 1, current_timestamp)",
            [],
        )
        .expect("产物");

        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let detail = task_detail(&conn, &policy, &actor, "T-1").expect("详情");

        assert_eq!(
            detail.task.parent_id.as_deref(),
            Some("T-0"),
            "依赖（父任务）"
        );
        assert_eq!(detail.acceptance, serde_json::json!([]));
        assert_eq!(detail.checkpoints.len(), 1, "步骤");
        assert_eq!(detail.artifacts.len(), 1, "产物引用");
        assert_eq!(detail.artifacts[0].refs_json, "[\"FR-101\"]");
        assert_eq!(detail.recent_events.len(), 1, "最近事件");
        assert_eq!(
            detail.failure_reason.as_deref(),
            Some(r#"{"error":"磁盘已满"}"#),
            "failed 任务必须有失败原因"
        );
    }

    #[test]
    fn detail_of_healthy_task_has_no_failure_reason_and_rejects_corrupt_acceptance() {
        let conn = migrated();
        insert_task(
            &conn,
            &NewTask {
                id: "T-ok",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("插入");
        insert_task(
            &conn,
            &NewTask {
                id: "T-bad",
                session_id: "S-1",
                title: "任务",
                stage: "plan",
                parent_id: None,
                acceptance_json: "{broken",
            },
            1_000,
        )
        .expect("插入");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let detail = task_detail(&conn, &policy, &actor, "T-ok").expect("健康任务详情");
        assert!(detail.failure_reason.is_none(), "非 failed 任务无失败原因");

        let err = task_detail(&conn, &policy, &actor, "T-bad").expect_err("损坏验收条件");
        assert_eq!(err.code, ErrorCode::ParseError);
        let err = task_detail(&conn, &policy, &actor, "T-none").expect_err("缺失任务");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn retry_moves_failed_to_ready_and_increments_count_without_touching_session() {
        let conn = migrated();
        insert_task(
            &conn,
            &NewTask {
                id: "T-1",
                session_id: "S-1",
                title: "任务",
                stage: "implement",
                parent_id: None,
                acceptance_json: "[]",
            },
            1_000,
        )
        .expect("插入");
        tasks::update_task_status(&conn, "T-1", TaskStatus::Failed, 1, 1_100).expect("失败");
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let retried = retry_task(&conn, &policy, &actor, "T-1", 2_000).expect("重试");
        assert_eq!(retried.status, TaskStatus::Ready);
        assert_eq!(retried.retry_count, 2, "重试计数必须递增（FR-401）");
        assert!(
            whcode_store::get_session(&conn, "S-1").is_ok(),
            "会话不因重试丢失（FR-401）"
        );

        // 重试后的任务再重试：ready 状态不可重试。
        let err = retry_task(&conn, &policy, &actor, "T-1", 2_100).expect_err("ready 不可重试");
        assert_eq!(err.code, ErrorCode::InvalidState);
        assert!(!err.retryable);
    }

    #[test]
    fn retry_rejects_completed_running_pending_and_unknown() {
        let conn = migrated();
        for (id, status) in [
            ("T-c", TaskStatus::Completed),
            ("T-r", TaskStatus::Running),
            ("T-p", TaskStatus::Pending),
            ("T-u", TaskStatus::Unknown),
        ] {
            task_with_status(&conn, id, status);
        }
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        for (id, status) in [
            ("T-c", "completed"),
            ("T-r", "running"),
            ("T-p", "pending"),
            ("T-u", "unknown"),
        ] {
            let err =
                retry_task(&conn, &policy, &actor, id, 2_000).expect_err("不可重试状态必须拒绝");
            assert_eq!(err.code, ErrorCode::InvalidState, "状态 {status}");
        }
        let err = retry_task(&conn, &policy, &actor, "T-none", 2_000).expect_err("缺失任务");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn retry_allows_aborted_and_interrupted() {
        let conn = migrated();
        for (id, status) in [
            ("T-a", TaskStatus::Aborted),
            ("T-i", TaskStatus::Interrupted),
        ] {
            task_with_status(&conn, id, status);
        }
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        for id in ["T-a", "T-i"] {
            let retried = retry_task(&conn, &policy, &actor, id, 2_000).expect("重试");
            assert_eq!(retried.status, TaskStatus::Ready);
        }
    }

    #[test]
    fn abort_moves_active_to_aborted_and_rejects_terminal_and_unknown() {
        let conn = migrated();
        for (id, status) in [
            ("T-run", TaskStatus::Running),
            ("T-ready", TaskStatus::Ready),
            ("T-blocked", TaskStatus::Blocked),
            ("T-interrupted", TaskStatus::Interrupted),
            ("T-done", TaskStatus::Completed),
            ("T-aborted", TaskStatus::Aborted),
            ("T-unknown", TaskStatus::Unknown),
        ] {
            task_with_status(&conn, id, status);
        }
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        for id in ["T-run", "T-ready", "T-blocked", "T-interrupted"] {
            let aborted = abort_task(&conn, &policy, &actor, id, 2_000).expect("中止");
            assert_eq!(aborted.status, TaskStatus::Aborted, "任务 {id}");
        }
        for id in ["T-done", "T-aborted", "T-unknown"] {
            let err =
                abort_task(&conn, &policy, &actor, id, 2_000).expect_err("终态与 unknown 不可中止");
            assert_eq!(err.code, ErrorCode::InvalidState, "任务 {id}");
        }
        let err = abort_task(&conn, &policy, &actor, "T-none", 2_000).expect_err("缺失任务");
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn task_commands_require_capability() {
        let conn = migrated();
        task_with_status(&conn, "T-1", TaskStatus::Failed);
        let policy = DefaultPolicy;
        let actor = crate::authz::Actor::new("无能力", []);
        for (name, result) in [
            (
                "detail",
                task_detail(&conn, &policy, &actor, "T-1").map(|_| ()),
            ),
            (
                "retry",
                retry_task(&conn, &policy, &actor, "T-1", 1).map(|_| ()),
            ),
            (
                "abort",
                abort_task(&conn, &policy, &actor, "T-1", 1).map(|_| ()),
            ),
        ] {
            let err = result.expect_err("无能力必须拒绝");
            assert_eq!(err.code, ErrorCode::PermissionDenied, "命令 {name}");
            assert!(!err.retryable);
        }
        let task = tasks::get_task(&conn, "T-1")
            .expect("库可读")
            .expect("存在");
        assert_eq!(task.status, TaskStatus::Failed, "拒绝路径不得改变状态");
    }
}
