//! TUI 视图数据源（装配根实现）：经 `whcode-app` 只读命令产出 [`ViewSnapshot`]。
//!
//! 渲染纪律（§5.3）：查询发生在**准备层**——视图切换、会话选择（`j`/`k`）与显式刷新
//! （`r`）的边界；帧路径只消费快照。每个视图只取自己需要的数据，且全部有界：
//! 会话 ≤ 200、事件尾部 ≤ 200、活跃任务由 `task_overview` 有界返回。
//!
//! 错误处理：查询失败**不 panic、不伪造**——快照的 `summary` 写明数据源错误（显式失败），
//! 视图其余内容为空。
//!
//! 边界：本类型在装配根（`whcode-bin`）中构造并注入，`whcode-tui` 不接触数据库
//! （C-03）；Diff 视图暂无数据源（converge 差异属后续接线），摘要行显式标注。

use duckdb::Connection;
use whcode_app::{Actor, DefaultPolicy, actor_with_all_capabilities};
use whcode_domain::i18n::I18n;
use whcode_domain::{TraceLevel, WhError};
use whcode_tui::{EventRow, SessionRow, TaskRow, ViewId, ViewSnapshot, ViewSource};

/// 会话清单上限。
const SESSION_LIMIT: usize = 200;
/// 事件尾部条数（V-101“有界尾部”口径）。
const EVENT_TAIL: usize = 200;
/// 上下文展示预算（token 数；仅用于概览行）。
const CONTEXT_DISPLAY_BUDGET: usize = 4096;
/// 事件摘要最大字符数（准备层截断；帧路径不解析 JSON）。
const SUMMARY_CHARS: usize = 96;

/// 工作区只读视图数据源。
pub struct StoreViewSource {
    conn: Connection,
    policy: DefaultPolicy,
    actor: Actor,
    texts: I18n,
}

impl StoreViewSource {
    /// 以已打开的工作区连接构造（连接与文本层由装配根持有）。
    #[must_use]
    pub fn new(conn: Connection, texts: I18n) -> Self {
        Self {
            conn,
            policy: DefaultPolicy,
            actor: actor_with_all_capabilities("tui-local"),
            texts,
        }
    }

    fn build(&self, view: ViewId, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        match view {
            ViewId::Sessions => self.build_sessions(),
            ViewId::Tasks => self.build_tasks(session),
            ViewId::Events => self.build_events(session),
            ViewId::Context => self.build_context(session),
            ViewId::Spec => self.build_spec(session),
            ViewId::Diff => {
                let mut snapshot = ViewSnapshot::empty(view);
                snapshot.summary = self.texts.resolve("tui.diff_unwired").to_string();
                Ok(snapshot)
            }
        }
    }

    fn build_sessions(&self) -> Result<ViewSnapshot, WhError> {
        let rows =
            whcode_app::list_sessions(&self.conn, &self.policy, &self.actor, false, SESSION_LIMIT)?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        snapshot.sessions = rows
            .into_iter()
            .map(|row| SessionRow {
                id: row.id,
                title: row.title,
                stage: row.stage,
                status: row.status,
            })
            .collect();
        Ok(snapshot)
    }

    fn build_tasks(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Tasks, &self.texts));
        };
        let overview =
            whcode_app::task_overview(&self.conn, &self.policy, &self.actor, Some(session))?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Tasks);
        snapshot.summary = self.texts.resolve_fmt(
            "summary.tasks",
            &[
                ("session", session),
                ("total", &overview.total.to_string()),
                ("done", &overview.done.to_string()),
                ("blocked", &overview.blocked.to_string()),
            ],
        );
        snapshot.tasks = overview
            .active
            .into_iter()
            .map(|row| TaskRow {
                id: row.id,
                title: row.title,
                status: row.status.as_str().to_string(),
                retry_count: row.retry_count,
            })
            .collect();
        Ok(snapshot)
    }

    fn build_events(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Events, &self.texts));
        };
        let cursor = whcode_app::build_snapshot(&self.conn, &self.policy, &self.actor, session)?
            .event_cursor;
        let after = cursor
            .saturating_sub(i64::try_from(EVENT_TAIL).unwrap_or(i64::MAX))
            .max(0);
        let export = whcode_app::events_export(
            &self.conn,
            &self.policy,
            &self.actor,
            session,
            after,
            EVENT_TAIL,
            TraceLevel::default(),
        )?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Events);
        snapshot.summary = self.texts.resolve_fmt(
            "summary.events",
            &[
                ("session", session),
                ("from", &after.to_string()),
                ("to", &cursor.to_string()),
                ("limit", &EVENT_TAIL.to_string()),
            ],
        );
        snapshot.events = export
            .events
            .into_iter()
            .map(|row| EventRow {
                seq: row.seq,
                kind: row.kind,
                summary: summarize_payload(&row.payload_json),
            })
            .collect();
        Ok(snapshot)
    }

    fn build_context(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Context, &self.texts));
        };
        let window = whcode_app::load_context(
            &self.conn,
            &self.policy,
            &self.actor,
            session,
            CONTEXT_DISPLAY_BUDGET,
        )?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Context);
        let truncated = window
            .truncated_reason
            .map(|reason| {
                self.texts
                    .resolve_fmt("summary.context_truncated", &[("reason", reason)])
            })
            .unwrap_or_default();
        snapshot.summary = self.texts.resolve_fmt(
            "summary.context",
            &[
                ("session", session),
                ("budget", &CONTEXT_DISPLAY_BUDGET.to_string()),
                ("used", &window.consumed_budget.to_string()),
                ("items", &window.items.len().to_string()),
                ("truncated", &truncated),
            ],
        );
        Ok(snapshot)
    }

    fn build_spec(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Spec, &self.texts));
        };
        let snapshot = whcode_app::build_snapshot(&self.conn, &self.policy, &self.actor, session)?;
        let stage = snapshot.session.stage.as_deref().unwrap_or("[unknown]");
        let active = snapshot
            .active_task
            .as_ref()
            .map(|task| format!("{} [{}]", task.id, task.status))
            .unwrap_or_else(|| self.texts.resolve("summary.none").to_string());
        let mut view = ViewSnapshot::empty(ViewId::Spec);
        view.summary = self.texts.resolve_fmt(
            "summary.spec",
            &[
                ("session", session),
                ("stage", stage),
                ("cursor", &snapshot.event_cursor.to_string()),
                ("task", &active),
            ],
        );
        Ok(view)
    }
}

impl ViewSource for StoreViewSource {
    fn snapshot(&self, view: ViewId, session: Option<&str>) -> ViewSnapshot {
        match self.build(view, session) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                // 显式失败：摘要行写明错误（不伪造数据，不静默为空）。
                let mut snapshot = ViewSnapshot::empty(view);
                snapshot.summary = self
                    .texts
                    .resolve_fmt("error.datasource", &[("error", &err.to_string())]);
                snapshot
            }
        }
    }
}

/// 未选择会话时的显式提示（正确的空：不是“暂无数据”而是“还没有选择”）。
fn no_session(view: ViewId, texts: &I18n) -> ViewSnapshot {
    let mut snapshot = ViewSnapshot::empty(view);
    snapshot.summary = texts.resolve("tui.no_session").to_string();
    snapshot
}

/// 载荷摘要：折叠空白并截断到 [`SUMMARY_CHARS`] 字符（不解析 JSON）。
fn summarize_payload(payload: &str) -> String {
    let collapsed: String = payload
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut summary: String = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() > SUMMARY_CHARS {
        summary = summary.chars().take(SUMMARY_CHARS).collect::<String>();
        summary.push('…');
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::{StoreViewSource, summarize_payload};
    use whcode_domain::i18n::I18n;
    use whcode_tui::{ViewId, ViewSource};

    fn migrated_conn() -> duckdb::Connection {
        let conn = duckdb::Connection::open_in_memory().expect("内存库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn empty_workspace_reports_correct_emptiness() {
        let source = StoreViewSource::new(migrated_conn(), I18n::default());
        let sessions = source.snapshot(ViewId::Sessions, None);
        assert!(sessions.sessions.is_empty(), "空工作区无会话");
        assert!(sessions.summary.is_empty(), "会话视图不带摘要");

        let tasks = source.snapshot(ViewId::Tasks, None);
        assert_eq!(
            tasks.summary, "（未选择会话——按 j/k 选择会话）",
            "未选择会话是显式状态"
        );
        // 英文文本层：同一快照的摘要切换语言（数据字段不翻译）。
        let english = StoreViewSource::new(
            migrated_conn(),
            I18n::load("en", whcode_domain::i18n::LocaleSource::Explicit),
        );
        let tasks = english.snapshot(ViewId::Tasks, None);
        assert_eq!(tasks.summary, "(no session selected — press j/k)");
    }

    #[test]
    fn sessions_and_events_come_from_app_service() {
        let conn = migrated_conn();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-T', '数据源测试', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'plan')",
            [],
        )
        .expect("插入会话");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id, kind, payload_json, ts, persistence_level)
             VALUES (1, 'E-1', 'S-T', NULL, NULL, NULL, 'RequestStarted', '{\"model\":\"demo\"}', current_timestamp, 'redacted')",
            [],
        )
        .expect("插入事件");
        let source = StoreViewSource::new(conn, I18n::default());

        let sessions = source.snapshot(ViewId::Sessions, None);
        assert_eq!(sessions.sessions.len(), 1);
        assert_eq!(sessions.sessions[0].id, "S-T");
        assert_eq!(sessions.sessions[0].stage, "plan");

        let events = source.snapshot(ViewId::Events, Some("S-T"));
        assert_eq!(events.events.len(), 1);
        assert_eq!(events.events[0].seq, 1);
        assert_eq!(events.events[0].kind, "RequestStarted");
        assert!(
            events.summary.starts_with("会话 S-T · 游标"),
            "摘要行写明会话与游标：{}",
            events.summary
        );
    }

    #[test]
    fn payload_summary_is_bounded_and_single_line() {
        let raw = "{\n  \"text\": \"line one\\nline two\"\n}";
        let summary = summarize_payload(raw);
        assert!(!summary.contains('\n'), "摘要必须单行");
        let long = "x".repeat(500);
        let summary = summarize_payload(&long);
        assert_eq!(summary.chars().count(), 97, "96 字符 + 省略号");
        assert!(summary.ends_with('…'));
    }
}
