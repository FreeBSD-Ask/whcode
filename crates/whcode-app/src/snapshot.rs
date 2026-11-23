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
//! | `context` | 已实现（ADR-094：历史 token 估算＋预算＋截断原因） | `messages`＋`context.rs` |
//! | `llm` | 已实现（ADR-094：事件派生的请求 ID／状态／耗时） | `events`（request_id／ts_ms） |
//! | `event_cursor` | 已实现（工作区 `events.seq` 最大值） | `events` |
//! | `runtimes` | 已实现（ADR-094：清单×启用状态×能力等级） | `runtime_state`＋`whcode-runtime::inventory` |
//!
//! 边界：只读；快照不产生任何写操作（ADR-003/005：变更必须经应用服务命令）。

use std::collections::BTreeMap;

use duckdb::Connection;
use serde::Serialize;
use serde_json::json;
use whcode_domain::WhError;
use whcode_store::directory;
use whcode_store::overview as overview_store;
use whcode_store::query;
use whcode_store::runtime_state;
use whcode_store::tasks;

use crate::authz::{Actor, Capability, Policy, require};
use crate::context::history_messages;
use crate::turn::DEFAULT_TURN_BUDGET_TOKENS;

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
    /// 上下文预算字段（ADR-094：历史 token 估算、预算与截断原因；四端同源）。
    pub context: Option<serde_json::Value>,
    /// LLM 状态字段（ADR-094：事件派生的请求 ID／状态／耗时；无活动为 `None`）。
    pub llm: Option<serde_json::Value>,
    /// 持久化事件游标（断线补拉基线，FR-202/605）。
    pub event_cursor: i64,
    /// 各运行时启用状态与能力等级（ADR-094：清单×`runtime_state`）。
    pub runtimes: BTreeMap<String, serde_json::Value>,
}

/// 上下文字段（ADR-094）：历史 token 估算、预算与截断原因。
///
/// 复用 `history_messages` 的同一剪枝口径（尾部优先、rewind 边界）；`truncated` 由
/// "实际装入 token < 会话全部消息 token"判定，为真时附可查原因，符合契约 §2。
fn derive_context(
    conn: &Connection,
    policy: &dyn Policy,
    actor: &Actor,
    session_id: &str,
) -> Result<serde_json::Value, WhError> {
    let budget = DEFAULT_TURN_BUDGET_TOKENS;
    let loaded = history_messages(conn, policy, actor, session_id, budget)?;
    // ADR-098：按消息侧类别累加（与 estimated_tokens 同源同函数，保证等式可判定）。
    // 注意：system 消息不落 messages 表（turn.rs:214 过滤），故不设 system 分类——
    // 快照无法分解 system 提示，该缺口在 breakdown.note 显式标注，不伪造估算。
    let mut compact_summary = 0usize;
    let mut conversation = 0usize;
    for message in &loaded {
        let tokens = crate::context::estimate_tokens(&message.content);
        if message.content.starts_with("[上下文摘要") {
            compact_summary += tokens;
        } else {
            conversation += tokens;
        }
    }
    let estimated_tokens = compact_summary + conversation;

    // 工具定义单列：默认九件注册表估算（快照不知道 turn 实际装配，故标注近似）。
    let tool_schemas = whcode_tools::registry_with_optional_tools(None, None, None)
        .ok()
        .and_then(|registry| serde_json::to_string(&registry.provider_manifest()).ok())
        .map(|json| crate::context::estimate_tokens(&json))
        .unwrap_or(0);

    // 全量 token 和（含被 rewind 边界排除的消息之前的完整历史），用于判定是否发生
    // 预算截断；查询有界聚合，不取回正文。
    let total_tokens: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(token_count), 0) FROM messages WHERE session_id = ?",
            [session_id],
            |row| row.get(0),
        )
        .map_err(|err| {
            WhError::new(
                whcode_domain::ErrorCode::IoError,
                format!("读取会话 token 总量失败：{err}"),
            )
        })?;
    let total_tokens = total_tokens.max(0) as usize;
    let truncated = total_tokens > budget;
    let mut value = json!({
        "estimated_tokens": estimated_tokens,
        "budget": budget,
        "truncated": truncated,
        // ADR-098：组成分解。前三类之和严格等于 estimated_tokens；tool_schemas 为
        // 默认九件注册表估算，不参与等式（turn 实际清单可能含 shell/LSP/技能/MCP）。
        "breakdown": {
            "compact_summary": compact_summary,
            "conversation": conversation,
            "tool_schemas": tool_schemas,
            "note": "tool_schemas 为默认九件注册表估算（turn 实际清单可能含 shell/LSP/技能/MCP，未计入）；system 提示在 turn 内组装、不落 messages 表，故不在本分解内",
        },
    });
    if truncated {
        value["truncate_reason"] = json!("budget_exceeded");
    }
    Ok(value)
}

/// LLM 状态字段（ADR-094）：从已落盘事件派生最近的请求 ID／状态／耗时。
///
/// 不读内存态——快照须跨端/跨进程同源（FR-314/V-207）。`TurnStarted` 与
/// `TurnFinished` 按 `request_id` 配对，耗时取两者 `epoch_us(ts)` 差；无配对时按
/// `streaming` 处理（已开始未结束），无事件时返回 `None`（无 LLM 活动）。
fn derive_llm(conn: &Connection, session_id: &str) -> Result<Option<serde_json::Value>, WhError> {
    let started: Option<(String, i64)> = conn
        .query_row(
            "SELECT request_id, epoch_us(ts) / 1000 FROM events
             WHERE session_id = ? AND kind = 'TurnStarted' AND request_id IS NOT NULL
             ORDER BY seq DESC LIMIT 1",
            [session_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .ok();
    let Some((request_id, started_ms)) = started else {
        return Ok(None);
    };
    let finished_ms: Option<i64> = conn
        .query_row(
            "SELECT epoch_us(ts) / 1000 FROM events
             WHERE session_id = ? AND kind = 'TurnFinished' AND request_id = ?
             ORDER BY seq DESC LIMIT 1",
            [session_id, request_id.as_str()],
            |row| row.get(0),
        )
        .ok();
    let (state, elapsed_ms) = match finished_ms {
        Some(end) => ("completed", (end - started_ms).max(0)),
        None => ("streaming", 0),
    };
    Ok(Some(json!({
        "request_id": request_id,
        "state": state,
        "elapsed_ms": elapsed_ms,
    })))
}

/// 运行时字段（ADR-094）：静态清单×持久化启用状态×能力等级（FR-702/704）。
///
/// 用 `inventory(false)` 不触发外部探测（快照只读、无副作用）；快照是状态视图，
/// 未启用者仍列出，执行工具列表的过滤由工具层承担（契约 §2）。
fn derive_runtimes(conn: &Connection) -> Result<BTreeMap<String, serde_json::Value>, WhError> {
    let mut result = BTreeMap::new();
    for entry in crate::runtime_mgmt::inventory(false) {
        let state = runtime_state::get(conn, &entry.name)?;
        result.insert(
            entry.name.clone(),
            json!({
                "enabled": state.as_ref().is_some_and(|row| row.enabled),
                "capability": entry.capability,
                "available": entry.available,
                "default_enabled": entry.default_enabled,
            }),
        );
    }
    Ok(result)
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
        context: Some(derive_context(conn, policy, actor, session_id)?),
        llm: derive_llm(conn, session_id)?,
        event_cursor,
        runtimes: derive_runtimes(conn)?,
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

        // ADR-094：三字段由同源函数派生，不再恒占位（契约 §2）。
        let context = snapshot.context.clone().expect("context 已实现");
        assert_eq!(context["budget"], DEFAULT_TURN_BUDGET_TOKENS);
        assert_eq!(context["estimated_tokens"], 0, "夹具无消息");
        assert_eq!(context["truncated"], false);
        assert!(
            snapshot.llm.is_none(),
            "夹具只有 SessionStarted，无 TurnStarted → 无 LLM 活动"
        );
        assert!(
            snapshot.runtimes.contains_key("js") && snapshot.runtimes.contains_key("python"),
            "runtimes 来自四项静态清单，未启用也列出（快照是状态视图）"
        );
        let js = &snapshot.runtimes["js"];
        assert_eq!(js["enabled"], false, "无 runtime_state 行 → 未启用");
        assert!(
            js.get("capability").is_some() && js.get("available").is_some(),
            "契约 §2：可用性与能力等级均在快照中"
        );

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

    /// ADR-094：llm 字段从已落盘事件派生——streaming（started 无 finished）
    /// 与 completed（配对 finished），耗时取 ts 差；四端同源由 build_snapshot 保证。
    #[test]
    fn snapshot_llm_state_derives_from_events() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-llm', 'LLM', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("会话");

        let insert_event = |seq: i64, id: &str, kind: &str, request_id: Option<&str>| {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, 'S-llm', NULL, ?, NULL, ?, CAST('{}' AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                duckdb::params![seq, id, request_id, kind],
            )
            .expect("事件");
        };
        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");

        // 仅 started → streaming，elapsed 0。
        insert_event(1, "E-1", "TurnStarted", Some("R-1"));
        let snapshot = build_snapshot(&conn, &policy, &actor, "S-llm").expect("快照");
        let llm = snapshot.llm.clone().expect("有 LLM 活动");
        assert_eq!(llm["request_id"], "R-1");
        assert_eq!(llm["state"], "streaming");
        assert_eq!(llm["elapsed_ms"], 0);

        // 配对 finished → completed（当前事件实现下 elapsed 为同秒差 ≥0）。
        insert_event(2, "E-2", "TurnFinished", Some("R-1"));
        let snapshot = build_snapshot(&conn, &policy, &actor, "S-llm").expect("快照");
        let llm = snapshot.llm.clone().expect("有 LLM 活动");
        assert_eq!(llm["state"], "completed");
        assert!(llm["elapsed_ms"].as_i64().is_some_and(|v| v >= 0));
    }
    /// ADR-098：breakdown 前三类之和严格等于 estimated_tokens；tool_schemas 单列且标注近似。
    #[test]
    fn snapshot_context_breakdown_sums_to_total() {
        let conn = migrated();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, status)
             VALUES ('S-bd', '分解', current_timestamp, current_timestamp, 'active')",
            [],
        )
        .expect("会话");
        for (id, role, content, ts) in [
            ("M-1", "assistant", "助手回复内容", 1_000.0),
            ("M-2", "user", "用户问题正文", 1_001.0),
            ("M-3", "user", "[上下文摘要｜模型生成] 摘要正文", 1_002.0),
        ] {
            conn.execute(
                "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts, parent_id)
                 VALUES (?, 'S-bd', ?, ?, NULL, to_timestamp(?), NULL)",
                duckdb::params![id, role, content, ts / 1000.0],
            )
            .expect("消息");
        }

        let policy = DefaultPolicy;
        let actor = actor_with_all_capabilities("tester");
        let snapshot = build_snapshot(&conn, &policy, &actor, "S-bd").expect("快照");
        let ctx = snapshot.context.expect("context");
        let b = &ctx["breakdown"];
        let sum = b["compact_summary"].as_u64().expect("summary");
        let conv = b["conversation"].as_u64().expect("conversation");
        assert!(sum > 0, "摘要应单独归类：{b}");
        assert!(conv > 0, "对话应计入：{b}");
        assert_eq!(
            ctx["estimated_tokens"].as_u64().expect("total"),
            sum + conv,
            "两类之和必须等于 estimated_tokens（ADR-098 等式约束）"
        );
        assert!(
            b.get("system_messages").is_none(),
            "system 不落库，不得伪造该分类"
        );
        assert!(
            b["tool_schemas"].as_u64().expect("tools") > 0,
            "默认九件注册表估算应大于 0"
        );
        assert!(
            b["note"].as_str().expect("note").contains("默认九件"),
            "工具侧必须标注为近似"
        );
    }
}
