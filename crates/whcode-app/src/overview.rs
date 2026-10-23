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
//!
//! 本模块还提供 [`converge_differences`]（Check 179、FR-310/311）：按任务状态把
//! 会话任务映射为验收判定，经 `whcode_speckit::converge` 归纳出 diff 视图的差异行。
//! 与 [`task_overview`] 同属**强一致**的任务读命令，权限口径相同（`TaskRead`）。

use std::collections::BTreeMap;

use duckdb::Connection;
use whcode_domain::{TaskStatus, WhError};
use whcode_speckit::{AcceptanceResult, AcceptanceStatus, converge};
use whcode_store::tasks::{self, TaskRow};

use crate::authz::{Actor, Capability, Policy, require};
use crate::view_source::DiffLine;

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

/// converge 差异视图的行数上限（有界；超出截断并置 `truncated`，不静默丢弃）。
pub const MAX_CONVERGE_DIFFERENCES: usize = 200;

/// converge 差异视图（diff 视图的数据源；Check 179、FR-310/311）。
///
/// `differences` 直接采用 [`DiffLine`] 形状：`Fail` 行 marker 为 `-`，
/// `NotTested` 行 marker 为空格；text 为 `<task_id> <criterion> — <gap>`。
/// 行数有界 [`MAX_CONVERGE_DIFFERENCES`]；截断**只裁剪行清单**——pass/fail/
/// not_tested 计数始终按全量统计，`truncated` 显式标记清单不完整。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvergeDiffView {
    /// 会话 ID。
    pub session_id: String,
    /// 参与核对的任务数（≤ [`whcode_store::tasks::MAX_SESSION_TASKS`]）。
    pub task_count: usize,
    /// 判定 `pass` 的验收条件数。
    pub pass_count: usize,
    /// 判定 `fail` 的验收条件数（含 JSON 损坏、无验收条件等整任务无法核对的差异）。
    pub fail_count: usize,
    /// 判定 `not_tested` 的验收条件数。
    pub not_tested_count: usize,
    /// 差异行是否超出上限被截断（显式标记，不静默丢弃）。
    pub truncated: bool,
    /// 差异行（有界 [`MAX_CONVERGE_DIFFERENCES`]）。
    pub differences: Vec<DiffLine>,
}

/// 汇总会话任务的 converge 差异（Check 179、FR-310/311；diff 视图数据源）。
///
/// **状态映射（诚实口径）**：任务状态是库内记录的事实，证据引用只指向任务行本身，
/// 不虚构验收核对记录——
///
/// - `completed` → 每条验收条件 `Pass`，evidence_ref = `task:<id>:completed`；
/// - `failed` / `aborted` → 每条 `Fail`，evidence_ref = `task:<id>:<status>`；
/// - 其余（pending / ready / running / blocked / interrupted / unknown）→ `NotTested`。
///
/// 失效条件：若未来要求“completed 必须附验收核对记录”（FR-311 收紧），completed 的
/// 映射不得沿用——须改为读取核对记录，缺失即记差异。
///
/// 验收条件来自 `acceptance_json`（JSON 字符串数组）。解析失败，或任务无验收条件
/// （converge 拒绝空条件，FR-304）时，该任务整体记一条 `Fail` 差异（marker `-`，
/// 计入 `fail_count`），gap 写明原因——不静默，也不中断其他任务的核对。
///
/// # Errors
///
/// 权限拒绝或存储失败时返回 [`WhError`]。
pub fn converge_differences(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<ConvergeDiffView, WhError> {
    require(policy, actor, Capability::TaskRead)?;
    let rows = tasks::tasks_of_session(conn, session_id, tasks::MAX_SESSION_TASKS)?;
    let mut view = ConvergeDiffView {
        session_id: session_id.to_string(),
        task_count: rows.len(),
        pass_count: 0,
        fail_count: 0,
        not_tested_count: 0,
        truncated: false,
        differences: Vec::new(),
    };
    for row in rows {
        let criteria = match serde_json::from_str::<Vec<String>>(&row.acceptance_json) {
            Ok(criteria) => criteria,
            Err(err) => {
                view.fail_count += 1;
                push_difference(
                    &mut view,
                    &row.id,
                    '-',
                    &format!("acceptance_json — 验收条件 JSON 损坏：{err}"),
                );
                continue;
            }
        };
        let status = match row.status {
            TaskStatus::Completed => AcceptanceStatus::Pass,
            TaskStatus::Failed | TaskStatus::Aborted => AcceptanceStatus::Fail,
            TaskStatus::Pending
            | TaskStatus::Ready
            | TaskStatus::Running
            | TaskStatus::Blocked
            | TaskStatus::Interrupted
            | TaskStatus::Unknown => AcceptanceStatus::NotTested,
        };
        // 证据引用只指向任务行本身（不虚构核对记录）：completed 的证据是“任务行
        // 标记为 completed”这一事实，failed/aborted 同理。
        let evidence_ref = match status {
            AcceptanceStatus::Pass => Some(format!("task:{}:completed", row.id)),
            AcceptanceStatus::Fail => Some(format!("task:{}:{}", row.id, row.status.as_str())),
            AcceptanceStatus::NotTested | AcceptanceStatus::NotApplicable => None,
        };
        let results: Vec<AcceptanceResult> = criteria
            .iter()
            .map(|criterion| AcceptanceResult {
                criterion: criterion.clone(),
                status,
                evidence_ref: evidence_ref.clone(),
            })
            .collect();
        let report = match converge(&row.id, &criteria, &results) {
            Ok(report) => report,
            // 空验收条件被 converge 拒绝（FR-304）：整任务一条差异，不静默。
            Err(err) => {
                view.fail_count += 1;
                push_difference(&mut view, &row.id, '-', &format!("acceptance_json — {err}"));
                continue;
            }
        };
        for result in &report.results {
            match result.status {
                AcceptanceStatus::Pass => view.pass_count += 1,
                AcceptanceStatus::Fail => view.fail_count += 1,
                // 本映射不产生 not_applicable；若出现按未测计（不静默为通过）。
                AcceptanceStatus::NotTested | AcceptanceStatus::NotApplicable => {
                    view.not_tested_count += 1;
                }
            }
        }
        for difference in report.differences {
            let marker = match difference.status {
                AcceptanceStatus::Fail => '-',
                _ => ' ',
            };
            push_difference(
                &mut view,
                &difference.task_id,
                marker,
                &format!("{} — {}", difference.criterion, difference.gap),
            );
        }
    }
    Ok(view)
}

/// 追加一条差异行（text = `<task_id> <criterion> — <gap>`）；行数达到
/// [`MAX_CONVERGE_DIFFERENCES`] 后置 `truncated` 并丢弃后续行。
fn push_difference(view: &mut ConvergeDiffView, task_id: &str, marker: char, body: &str) {
    if view.differences.len() >= MAX_CONVERGE_DIFFERENCES {
        view.truncated = true;
        return;
    }
    view.differences.push(DiffLine {
        marker,
        text: format!("{task_id} {body}"),
    });
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

    /// converge 夹具：completed（2 条件）/ failed（1 条件）/ running（2 条件）
    /// / pending + 损坏 JSON 各一任务；updated_at 互不相同保证顺序确定。
    fn seed_converge(conn: &Connection) {
        for (id, stage, acceptance, ts) in [
            ("T-C", "plan", "[\"条件甲\",\"条件乙\"]", 1_000_u64),
            ("T-F", "implement", "[\"条件丙\"]", 2_000),
            ("T-R", "implement", "[\"条件丁\",\"条件戊\"]", 3_000),
            ("T-X", "verify", "{oops", 4_000),
        ] {
            insert_task(
                conn,
                &NewTask {
                    id,
                    session_id: "S-D",
                    title: "差异核对",
                    stage,
                    parent_id: None,
                    acceptance_json: acceptance,
                },
                ts,
            )
            .expect("插入任务");
        }
        update_task_status(conn, "T-C", TaskStatus::Completed, 0, 5_100).expect("完成");
        update_task_status(conn, "T-F", TaskStatus::Failed, 0, 5_200).expect("失败");
        update_task_status(conn, "T-R", TaskStatus::Running, 0, 5_300).expect("执行中");
    }

    #[test]
    fn converge_diff_maps_task_states_honestly() {
        let conn = migrated();
        seed_converge(&conn);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        let view = converge_differences(&conn, &policy, &actor, "S-D").expect("差异视图");
        assert_eq!(view.session_id, "S-D");
        assert_eq!(view.task_count, 4);
        assert_eq!(view.pass_count, 2, "completed 的每条条件 pass");
        assert_eq!(view.fail_count, 2, "failed 的条件 + 损坏 JSON 的整任务");
        assert_eq!(view.not_tested_count, 2, "running 的条件不虚构通过");
        assert!(!view.truncated);
        // 差异行按 updated_at 倒序聚合：T-R（2 未测）→ T-F（1 失败）→ T-X（损坏）。
        assert_eq!(view.differences.len(), 4, "pass 不产生差异行");
        assert_eq!(view.differences[0].marker, ' ');
        assert_eq!(
            view.differences[0].text,
            "T-R 条件丁 — 未测试：无运行结果引用"
        );
        assert_eq!(
            view.differences[1].text,
            "T-R 条件戊 — 未测试：无运行结果引用"
        );
        assert_eq!(view.differences[2].marker, '-');
        assert_eq!(
            view.differences[2].text, "T-F 条件丙 — 验收失败：task:T-F:failed",
            "证据引用只指向任务行本身，不虚构核对记录"
        );
        assert_eq!(view.differences[3].marker, '-');
        assert!(
            view.differences[3]
                .text
                .starts_with("T-X acceptance_json — 验收条件 JSON 损坏："),
            "损坏 JSON 必须显式成行：{}",
            view.differences[3].text
        );
    }

    #[test]
    fn converge_diff_empty_session_and_permission_denied() {
        let conn = migrated();
        seed_converge(&conn);
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        // 无任务会话：正确的空（全零 + 空差异），不是错误。
        let empty = converge_differences(&conn, &policy, &actor, "S-none").expect("空会话");
        assert_eq!(empty.task_count, 0);
        assert_eq!(empty.pass_count, 0);
        assert_eq!(empty.fail_count, 0);
        assert_eq!(empty.not_tested_count, 0);
        assert!(empty.differences.is_empty());
        assert!(!empty.truncated);

        let err = converge_differences(
            &conn,
            &policy,
            &crate::authz::Actor::new("无能力", []),
            "S-D",
        )
        .expect_err("无 TaskRead 必须拒绝");
        assert_eq!(err.code, whcode_domain::ErrorCode::PermissionDenied);
        assert!(!err.retryable);
    }

    #[test]
    fn converge_diff_differences_are_bounded_and_mark_truncated() {
        let conn = migrated();
        // 2 个 running 任务 × 150 条件 = 300 条未测差异 > 上限 200。
        let mut criteria = String::from("[");
        for index in 0..150 {
            if index > 0 {
                criteria.push(',');
            }
            criteria.push_str(&format!("\"条件{index}\""));
        }
        criteria.push(']');
        for id in ["T-B1", "T-B2"] {
            insert_task(
                &conn,
                &NewTask {
                    id,
                    session_id: "S-Big",
                    title: "超限",
                    stage: "implement",
                    parent_id: None,
                    acceptance_json: &criteria,
                },
                1_000,
            )
            .expect("插入任务");
            update_task_status(&conn, id, TaskStatus::Running, 0, 2_000).expect("执行中");
        }
        let view = converge_differences(
            &conn,
            &DefaultPolicy,
            &actor_with_all_capabilities("t"),
            "S-Big",
        )
        .expect("差异视图");
        assert_eq!(view.differences.len(), MAX_CONVERGE_DIFFERENCES, "行数有界");
        assert!(view.truncated, "超限必须显式标记，不静默丢弃");
        // 计数按全量统计：截断只裁剪行清单。
        assert_eq!(view.not_tested_count, 300);
        assert_eq!(view.pass_count, 0);
        assert_eq!(view.fail_count, 0);
    }
}
