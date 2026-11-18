//! TUI 视图数据源（装配根实现）：经 `whcode-app` 只读命令产出 [`ViewSnapshot`]。
//!
//! 渲染纪律（§5.3）：查询发生在**准备层**——视图切换、会话选择（`j`/`k`）与显式刷新
//! （`r`）的边界；帧路径只消费快照。每个视图只取自己需要的数据，且全部有界：
//! 会话 ≤ 200、事件尾部 ≤ 200、活跃任务由 `task_overview` 有界返回、diff 视图的
//! 会话任务 ≤ 200 且差异行 ≤ 200（`converge_differences`）。
//!
//! 错误处理：查询失败**不 panic、不伪造**——快照的 `summary` 写明数据源错误（显式失败），
//! 视图其余内容为空。
//!
//! 边界：本类型在装配根（`whcode-bin`）中构造并注入，`whcode-tui` 不接触数据库
//! （C-03）；Diff 视图经 `whcode_app::overview::converge_differences` 产出 converge
//! 差异（Check 179），TUI 与原生 GUI 共用本 trait 实现，自动同源。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use duckdb::Connection;
use whcode_app::tool_call::{TOOL_STARTED, describe};
use whcode_app::view_source::{
    CheckpointItem, EventRow, LedgerRow, LedgerSection, PreimageItem, SessionRow, TaskRow, ViewId,
    ViewSnapshot, ViewSource,
};
use whcode_app::{Actor, DefaultPolicy, actor_with_all_capabilities};
use whcode_domain::i18n::I18n;
use whcode_domain::{TraceLevel, WhError};
use whcode_store::ContentHit;

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
    conn: Arc<Mutex<Connection>>,
    policy: DefaultPolicy,
    actor: Actor,
    texts: I18n,
    /// 工作区根（技能库与 MCP 配置读取用；`None` = 未注入，面板显式为空）。
    workspace: Option<std::path::PathBuf>,
}

impl StoreViewSource {
    /// 以已打开的工作区连接构造（连接与文本层由装配根持有）。
    #[must_use]
    pub fn new(conn: Arc<Mutex<Connection>>, texts: I18n) -> Self {
        Self {
            conn,
            policy: DefaultPolicy,
            actor: actor_with_all_capabilities("tui-local"),
            texts,
            workspace: None,
        }
    }

    /// 追加工作区根（T258：技能库与 MCP 配置读取）。
    #[must_use]
    pub fn with_workspace(mut self, workspace: impl Into<std::path::PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    fn build(&self, view: ViewId, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        match view {
            ViewId::Sessions => self.build_sessions(),
            ViewId::Tasks => self.build_tasks(session),
            ViewId::Events => self.build_events(session),
            ViewId::Context => self.build_context(session),
            ViewId::Spec => self.build_spec(session),
            ViewId::Diff => self.build_diff(session),
            // 对话视图（ADR-056）：无独立快照——输入与摘要由 GUI/TUI 自持状态，
            // turn 历史经 Events 视图查看。
            ViewId::Chat => Ok(ViewSnapshot::default()),
            // 台账视图（FR-787／FR-794）：待办／交付物／发现／干预四节聚合；无会话时只列跨会话发现。
            ViewId::Ledger => self.build_ledger(session),
            // 前像清单（T392／FR-776）；充分性报告（T392／FR-795）。
            ViewId::Preimages => self.build_preimages(session),
            ViewId::Coherence => self.build_coherence(session),
        }
    }

    /// 前像清单（T392／FR-776）：新→旧、有界；无会话时显式空态。
    fn build_preimages(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Preimages, &self.texts));
        };
        let rows = whcode_app::list_preimages(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            session,
            whcode_app::MAX_PREIMAGE_LIST,
        )?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Preimages);
        snapshot.summary = self.texts.resolve_fmt(
            "summary.preimages",
            &[("session", session), ("count", &rows.len().to_string())],
        );
        snapshot.preimages = rows
            .into_iter()
            .map(|row| PreimageItem {
                id: row.id,
                exec_id: row.exec_id,
                path: row.path,
                size_bytes: row.size_bytes,
                has_content: row.content.is_some(),
                skipped_reason: row.skipped_reason,
            })
            .collect();
        Ok(snapshot)
    }

    /// 充分性报告（T392／FR-795）：最近一次计算的只读文本；无报告为正确的空。
    fn build_coherence(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Coherence, &self.texts));
        };
        let view = whcode_app::latest_coherence(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            session,
        )?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Coherence);
        snapshot.coherence_report = match view {
            Some(view) => {
                // 多行文本（准备层生成）：结论、计数、要求文本与反例明细行。
                let mut lines = vec![
                    format!(
                        "{} · 规则 {} · 反例 {} · 未知 {}",
                        view.verdict.as_str(),
                        view.total,
                        view.counterexamples,
                        view.unknowns
                    ),
                    format!("phase={}", view.phase.as_str()),
                ];
                if let Some(id) = &view.id {
                    lines.push(format!("report={id}"));
                }
                if !view.requirement.is_empty() {
                    lines.push(view.requirement.clone());
                }
                for detail in &view.details {
                    lines.push(format!(
                        "反例 {}: {}",
                        detail
                            .get("rule")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?"),
                        detail
                            .get("detail")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                    ));
                }
                lines.join(
                    "
",
                )
            }
            None => String::new(),
        };
        Ok(snapshot)
    }

    /// 持久化事件游标（FR-624/T178）：O(1) 单值查询，供 TUI 低频在线可见检查。
    fn cursor_of(&self) -> Option<i64> {
        whcode_store::latest_seq(&self.conn.lock().expect("store 连接锁中毒")).ok()
    }

    /// 台账视图（FR-787／FR-794）：待办（最近快照）＋交付物（聚合去重）＋发现（按会话）
    /// ＋干预（按会话）。
    ///
    /// 四节各自有界：待办 ≤50 条（写入侧上限）、交付物 ≤200 条（读取侧上限）、发现 ≤20 条、
    /// 干预 ≤20 条；任一来源失败都写进摘要行（`error.datasource` 之外的节级注记），不伪造空。
    fn build_ledger(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let mut snapshot = ViewSnapshot::empty(ViewId::Ledger);
        let mut notes: Vec<String> = Vec::new();
        let conn = self.conn.lock().expect("store 连接锁中毒");
        // 节名一次解析（本地化归属准备层：帧路径不查目录）。
        let todo_label = self
            .texts
            .resolve(LedgerSection::Todo.label_key())
            .to_string();
        let deliverable_label = self
            .texts
            .resolve(LedgerSection::Deliverable.label_key())
            .to_string();
        let finding_label = self
            .texts
            .resolve(LedgerSection::Finding.label_key())
            .to_string();
        let intervention_label = self
            .texts
            .resolve(LedgerSection::Intervention.label_key())
            .to_string();
        let mut todo_count = 0usize;
        let mut deliverable_count = 0usize;
        let mut finding_count = 0usize;
        let mut intervention_count = 0usize;
        match session {
            Some(session) => {
                match whcode_app::todo::current(&conn, &self.policy, &self.actor, session) {
                    Ok(view) => {
                        todo_count = view.items.len();
                        snapshot
                            .ledger
                            .extend(view.items.into_iter().map(|entry| LedgerRow {
                                section: LedgerSection::Todo,
                                section_label: todo_label.clone(),
                                status: entry.status,
                                text: entry.content,
                            }));
                    }
                    Err(err) => notes.push(format!("待办不可用：{err}")),
                }
                match whcode_app::deliverables::list(&conn, &self.policy, &self.actor, session) {
                    Ok(view) => {
                        deliverable_count = view.items.len();
                        snapshot
                            .ledger
                            .extend(view.items.into_iter().map(|item| LedgerRow {
                                section: LedgerSection::Deliverable,
                                section_label: deliverable_label.clone(),
                                status: format!("seq={}", item.seq),
                                text: item.path,
                            }));
                    }
                    Err(err) => notes.push(format!("交付物不可用：{err}")),
                }
                match whcode_app::findings::list_session_findings(
                    &conn,
                    &self.policy,
                    &self.actor,
                    session,
                    Some(LEDGER_FINDING_LIMIT),
                ) {
                    Ok(rows) => {
                        finding_count = rows.len();
                        snapshot
                            .ledger
                            .extend(rows.into_iter().map(|finding| LedgerRow {
                                section: LedgerSection::Finding,
                                section_label: finding_label.clone(),
                                status: format!("{} v{}", finding.status.as_str(), finding.version),
                                text: format!(
                                    "{} [{}] {}",
                                    finding.id,
                                    finding.severity.as_str(),
                                    finding.title
                                ),
                            }));
                    }
                    Err(err) => notes.push(format!("发现不可用：{err}")),
                }
                match whcode_app::list_session_interventions(
                    &conn,
                    &self.policy,
                    &self.actor,
                    session,
                    Some(LEDGER_INTERVENTION_LIMIT),
                ) {
                    Ok(rows) => {
                        intervention_count = rows.len();
                        snapshot
                            .ledger
                            .extend(rows.into_iter().map(|item| LedgerRow {
                                section: LedgerSection::Intervention,
                                section_label: intervention_label.clone(),
                                status: match (&item.status, &item.outcome) {
                                    (whcode_store::InterventionStatus::Active, _) => {
                                        "active".to_string()
                                    }
                                    (whcode_store::InterventionStatus::Concluded, outcome) => {
                                        format!(
                                            "concluded={}",
                                            outcome
                                                .as_ref()
                                                .map(|verdict| verdict.as_str())
                                                .unwrap_or("?")
                                        )
                                    }
                                },
                                text: format!("{} [{}] {}", item.id, item.pattern, item.action),
                            }));
                    }
                    Err(err) => notes.push(format!("干预不可用：{err}")),
                }
            }
            None => notes.push("未选择会话：待办与交付物需要会话上下文".to_string()),
        }
        snapshot.summary = if notes.is_empty() {
            self.texts.resolve_fmt(
                "tui.ledger.summary",
                &[
                    ("todo", &todo_count.to_string()),
                    ("deliverables", &deliverable_count.to_string()),
                    ("findings", &finding_count.to_string()),
                    ("interventions", &intervention_count.to_string()),
                ],
            )
        } else {
            notes.join("；")
        };
        Ok(snapshot)
    }

    fn build_sessions(&self) -> Result<ViewSnapshot, WhError> {
        let rows = whcode_app::list_sessions(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            true,
            SESSION_LIMIT,
        )?;
        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        snapshot.sessions = rows
            .into_iter()
            .map(|row| SessionRow {
                id: row.id,
                title: row.title,
                stage: row.stage,
                status: row.status,
                archived: row.archived,
                created_at_ms: row.created_at_ms,
                last_active_at_ms: row.last_active_at_ms,
            })
            .collect();
        Ok(snapshot)
    }

    fn build_tasks(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Tasks, &self.texts));
        };
        // 任务面板（FR-747）：全状态卡片＋父链依赖，而非只看 running。
        let panel = whcode_app::task_panel(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            Some(session),
            100,
        )?;
        let overview = &panel.overview;
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
        snapshot.tasks = panel
            .tasks
            .into_iter()
            .map(|card| TaskRow {
                id: card.id,
                title: card.title,
                status: card.status.as_str().to_string(),
                retry_count: card.retry_count,
                parent_id: card.parent_id,
            })
            .collect();
        Ok(snapshot)
    }

    fn build_events(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Events, &self.texts));
        };
        let cursor = whcode_app::build_snapshot(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            session,
        )?
        .event_cursor;
        let after = cursor
            .saturating_sub(i64::try_from(EVENT_TAIL).unwrap_or(i64::MAX))
            .max(0);
        let export = whcode_app::events_export(
            &self.conn.lock().expect("store 连接锁中毒"),
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
        // Check 348：结束事件通常不带工具名，先用同页开始事件按 tool_call_id 配对。
        // 配对表上限由事件尾部（EVENT_TAIL）自然约束；未配对的结束事件显示 `?`，不猜。
        let mut tool_names: HashMap<String, String> = HashMap::new();
        for row in &export.events {
            if row.kind == TOOL_STARTED
                && let Some(event) = describe(&row.kind, &row.payload_json)
                && let Some(tool) = event.tool
            {
                tool_names.insert(event.call_id, tool);
            }
        }
        snapshot.events = export
            .events
            .into_iter()
            .map(|row| {
                let hint = describe(&row.kind, &row.payload_json).and_then(|event| {
                    event
                        .tool
                        .clone()
                        .or_else(|| tool_names.get(&event.call_id).cloned())
                });
                let summary = crate::tool_call_text::event_summary(
                    &self.texts,
                    &row.kind,
                    &row.payload_json,
                    hint.as_deref(),
                )
                .unwrap_or_else(|| summarize_payload(&row.payload_json));
                EventRow {
                    seq: row.seq,
                    kind: row.kind,
                    summary,
                }
            })
            .collect();
        Ok(snapshot)
    }

    fn build_context(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Context, &self.texts));
        };
        let window = whcode_app::load_context(
            &self.conn.lock().expect("store 连接锁中毒"),
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
        // 三档能力等级在 UI 可见（FR-702、Check 101）：界面路径不探测子进程，只展示声明。
        let runtimes = crate::runtimes::summary(false);
        snapshot.summary = format!(
            "{} · {}",
            snapshot.summary,
            self.texts
                .resolve_fmt("tui.runtimes", &[("list", &runtimes)])
        );
        Ok(snapshot)
    }

    fn build_spec(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Spec, &self.texts));
        };
        let snapshot = whcode_app::build_snapshot(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            session,
        )?;
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

    /// diff 视图：会话任务的 converge 差异（Check 179、FR-310/311）。
    ///
    /// 数据由 `whcode_app::overview::converge_differences` 产出（权限、状态映射、
    /// 有界与截断标记都在应用层）；本方法只做 i18n 摘要与行投影。
    fn build_diff(&self, session: Option<&str>) -> Result<ViewSnapshot, WhError> {
        let Some(session) = session else {
            return Ok(no_session(ViewId::Diff, &self.texts));
        };
        let view = whcode_app::overview::converge_differences(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            session,
        )?;
        let truncated = if view.truncated {
            self.texts.resolve("summary.diff.truncated").to_string()
        } else {
            String::new()
        };
        let mut snapshot = ViewSnapshot::empty(ViewId::Diff);
        snapshot.summary = self.texts.resolve_fmt(
            "summary.diff",
            &[
                ("session", session),
                ("total", &view.task_count.to_string()),
                ("pass", &view.pass_count.to_string()),
                ("fail", &view.fail_count.to_string()),
                ("untested", &view.not_tested_count.to_string()),
                ("truncated", &truncated),
            ],
        );
        snapshot.diff = view.differences;
        Ok(snapshot)
    }
}

/// 台账视图的发现条数上限（界面读取面，小于存储上限 200）。
const LEDGER_FINDING_LIMIT: usize = 20;
/// 台账视图干预节的读取上限（与发现节同口径，FR-794）。
const LEDGER_INTERVENTION_LIMIT: usize = 20;

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

    fn cursor(&self, _session: Option<&str>) -> Option<i64> {
        self.cursor_of()
    }

    /// 会话内容检索（T319／FR-780）：委托应用命令（同一权限与片段口径）。
    fn content_search(&self, needle: &str, limit: usize) -> Result<Vec<ContentHit>, WhError> {
        let conn = self.conn.lock().expect("store 连接锁中毒");
        whcode_app::search::search_session_content(
            &conn,
            &self.policy,
            &self.actor,
            &whcode_app::search::ContentSearchQuery {
                needle: needle.to_string(),
                limit,
            },
        )
    }

    /// 单事件详情（T392／events.show）：委托应用命令（同一脱敏与三态口径），
    /// 返回一行摘要文本；错误原样上抛（TUI 状态行显示）。
    fn event_detail(&self, event_id: &str) -> Result<String, WhError> {
        let detail = whcode_app::show_event(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            event_id,
            TraceLevel::default(),
        )?;
        Ok(format!(
            "{} #{} [{}] {} {} {}",
            detail.event.id,
            detail.event.seq,
            detail.event.kind,
            detail.state.as_str(),
            detail.trace_level,
            detail.event.payload_json
        ))
    }

    fn conversation_tail(&self, session_id: &str, max: usize) -> Vec<(String, String)> {
        // 视图数据尽力而为（trait 契约）：读取失败返回空，不阻塞界面；
        // 错误可见性由命令面承担（与 cursor 的 .ok() 口径一致）。
        whcode_app::llm::conversation_tail(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            session_id,
            max,
        )
        .map(|entries| {
            entries
                .into_iter()
                .map(|entry| (entry.role, entry.content))
                .collect()
        })
        .unwrap_or_default()
    }

    fn checkpoints(&self, task: &str) -> Vec<CheckpointItem> {
        // FR-748：检查点浏览。读取失败返回空（与 cursor／conversation_tail 同口径的
        // “尽力而为”契约）；回滚能力不在此接口——后端未实现，由命令面显式降级。
        let view = whcode_app::checkpoint_view(
            &self.conn.lock().expect("store 连接锁中毒"),
            &self.policy,
            &self.actor,
            task,
        );
        match view {
            Ok(view) => view
                .checkpoints
                .into_iter()
                .map(|entry| CheckpointItem {
                    id: entry.id,
                    step: entry.step,
                    summary: entry.delta_summary,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    fn panels(&self, session: Option<&str>) -> whcode_app::view_source::PanelsSnapshot {
        // T258／FR-749／750／751：准备阶段读取；每一项错误各自显式，不伪造空。
        let mut snapshot = whcode_app::view_source::PanelsSnapshot::default();
        let skills = whcode_app::panels::skills_panel(self.workspace.as_deref());
        snapshot.skills = skills.skills;
        snapshot.skill_warnings = skills.warnings;

        let conn = self.conn.lock().expect("store 连接锁中毒");
        match whcode_app::panels::mcp_panel(self.workspace.as_deref(), &conn, session) {
            Ok(panel) => {
                snapshot.mcp_configured = panel.configured;
                snapshot.mcp = panel.servers;
                snapshot.mcp_error = panel.error;
            }
            Err(err) => snapshot.mcp_error = Some(err.to_string()),
        }
        match whcode_app::panels::subagent_runs(&conn, session) {
            Ok(runs) => snapshot.subagents = runs,
            Err(err) => {
                if snapshot.error.is_none() {
                    snapshot.error = Some(err.to_string());
                }
            }
        }
        match whcode_app::approval_rules::list_rules(&conn, &self.policy, &self.actor) {
            Ok(rules) => snapshot.rules = rules,
            Err(err) => {
                if snapshot.error.is_none() {
                    snapshot.error = Some(err.to_string());
                }
            }
        }
        snapshot
    }
}

/// TUI/GUI 写命令执行器（T177/FR-623、ADR-056 Phase 2）：与视图数据源共享同一条
/// 工作区连接（`Arc<Mutex>`——GUI 后台执行 turn 需跨线程，互斥保证单写路径；
/// Check 98 口径——写者服务/单连接为唯一写路径），经 `whcode-app::commands` 门面执行。
pub struct TuiCommandExecutor {
    conn: Arc<Mutex<Connection>>,
    /// 工作区根（T392／FR-776）：前像恢复写回工作区文件——装配细节由执行器注入，
    /// 前端命令不携带路径（`SessionRestore.workspace` 在此统一覆写）。
    workspace: Option<std::path::PathBuf>,
}

impl TuiCommandExecutor {
    #[must_use]
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self {
            conn,
            workspace: None,
        }
    }

    /// 注入工作区根（前像恢复需要；未注入时恢复动作显式失败）。
    #[must_use]
    pub fn with_workspace(mut self, workspace: std::path::PathBuf) -> Self {
        self.workspace = Some(workspace);
        self
    }
}

impl whcode_app::commands::CommandExecutor for TuiCommandExecutor {
    fn execute(
        &self,
        command: whcode_app::FrontendCommand,
    ) -> Result<whcode_app::CommandOutcome, WhError> {
        let command = match command {
            whcode_app::FrontendCommand::SessionRestore {
                session_id,
                exec_id,
                confirm,
                ..
            } => whcode_app::FrontendCommand::SessionRestore {
                session_id,
                exec_id,
                workspace: self.workspace.clone().ok_or_else(|| {
                    WhError::invalid_state("前像恢复需要工作区根（装配根未注入）")
                })?,
                confirm,
            },
            other => other,
        };
        let mut conn = self.conn.lock().expect("store 连接锁中毒");
        whcode_app::execute_command(
            &mut conn,
            &DefaultPolicy,
            &whcode_app::actor_with_all_capabilities("tui-local"),
            &command,
        )
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
    use whcode_app::view_source::{ViewId, ViewSource};
    use whcode_domain::TaskStatus;
    use whcode_domain::i18n::{I18n, LocaleSource};
    use whcode_store::tasks::NewTask;
    use whcode_store::{insert_task, update_task_status};

    fn migrated_conn() -> duckdb::Connection {
        let conn = duckdb::Connection::open_in_memory().expect("内存库");
        whcode_store::apply_migrations(&conn).expect("迁移");
        conn
    }

    #[test]
    fn empty_workspace_reports_correct_emptiness() {
        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(migrated_conn())),
            I18n::default(),
        );
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
            std::sync::Arc::new(std::sync::Mutex::new(migrated_conn())),
            I18n::load("en", whcode_domain::i18n::LocaleSource::Explicit),
        );
        let tasks = english.snapshot(ViewId::Tasks, None);
        assert_eq!(tasks.summary, "(no session selected — press j/k)");
    }

    #[test]
    fn checkpoints_are_reachable_through_the_view_source_trait() {
        // 回归（2026-11-07）：`checkpoints` 曾落在固有 impl 上，GUI 经 `dyn ViewSource`
        // 派发只拿到默认空向量——“能编译”不等于“信号可达”（信号链完备原则）。
        let conn = migrated_conn();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-K', '检查点数据源', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'implement')",
            [],
        )
        .expect("插入会话");
        conn.execute(
            "INSERT INTO tasks (id, session_id, title, stage, status, parent_id, acceptance_json,
                                retry_count, updated_at)
             VALUES ('T-K', 'S-K', '检查点任务', 'implement', 'running', NULL,
                     CAST('[]' AS JSON)::VARIANT, 0, current_timestamp)",
            [],
        )
        .expect("插入任务");
        conn.execute(
            "INSERT INTO checkpoints (id, task_id, step, delta_json, message_cursor,
                                      artifact_refs_json, ts)
             VALUES ('C-K', 'T-K', 1, CAST('{\"from\":\"ready\",\"to\":\"running\"}' AS JSON)::VARIANT,
                     'M-1', CAST('[]' AS JSON)::VARIANT, current_timestamp)",
            [],
        )
        .expect("插入检查点");
        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(conn)),
            I18n::default(),
        );

        // 关键：经 trait 对象调用（GUI 的实际路径），而非具体类型。
        let view: &dyn ViewSource = &source;
        let points = view.checkpoints("T-K");
        assert_eq!(points.len(), 1, "trait 派发必须命中装配根实现");
        assert_eq!(points[0].id, "C-K");
        assert_eq!(points[0].step, 1);
        assert!(
            points[0].summary.contains("to=running"),
            "{}",
            points[0].summary
        );

        // 不存在的任务：trait 契约是空向量（不得 panic，也不伪造条目）。
        assert!(view.checkpoints("T-none").is_empty());
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
             VALUES (1, 'E-1', 'S-T', NULL, NULL, NULL, 'RequestStarted', CAST('{\"model\":\"demo\"}' AS JSON)::VARIANT, current_timestamp, 'redacted')",
            [],
        )
        .expect("插入事件");
        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(conn)),
            I18n::default(),
        );

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

    /// diff 夹具：S-T 三个任务（completed 2 条件 / failed 1 条件 / running 1 条件）。
    fn seed_diff_fixture(conn: &duckdb::Connection) {
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-T', '差异测试', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'implement')",
            [],
        )
        .expect("插入会话");
        for (id, acceptance, ts) in [
            ("T-1", "[\"条件甲\",\"条件乙\"]", 1_000_u64),
            ("T-2", "[\"条件丙\"]", 2_000),
            ("T-3", "[\"条件丁\"]", 3_000),
        ] {
            insert_task(
                conn,
                &NewTask {
                    id,
                    session_id: "S-T",
                    title: "任务",
                    stage: "implement",
                    parent_id: None,
                    acceptance_json: acceptance,
                },
                ts,
            )
            .expect("插入任务");
        }
        update_task_status(conn, "T-1", TaskStatus::Completed, 0, 4_100).expect("完成");
        update_task_status(conn, "T-2", TaskStatus::Failed, 0, 4_200).expect("失败");
        update_task_status(conn, "T-3", TaskStatus::Running, 0, 4_300).expect("执行中");
    }

    #[test]
    fn diff_view_wires_converge_differences() {
        let conn = migrated_conn();
        seed_diff_fixture(&conn);
        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(conn)),
            I18n::default(),
        );

        let diff = source.snapshot(ViewId::Diff, Some("S-T"));
        assert_eq!(
            diff.summary, "会话 S-T · 任务 3 · 通过 2 / 失败 1 / 未测 1",
            "摘要行写明核对结果：{}",
            diff.summary
        );
        // 差异行按 updated_at 倒序：T-3（未测，marker 空格）→ T-2（失败，marker '-'）；
        // T-1 全部通过不产生差异行。
        assert_eq!(diff.diff.len(), 2);
        assert_eq!(diff.diff[0].marker, ' ');
        assert_eq!(diff.diff[0].text, "T-3 条件丁 — 未测试：无运行结果引用");
        assert_eq!(diff.diff[1].marker, '-');
        assert_eq!(diff.diff[1].text, "T-2 条件丙 — 验收失败：task:T-2:failed");

        // 未选择会话是显式状态（正确的空），不是“暂无数据”。
        let none = source.snapshot(ViewId::Diff, None);
        assert_eq!(none.summary, "（未选择会话——按 j/k 选择会话）");
        assert!(none.diff.is_empty());

        // 英文文本层：摘要切换语言，差异行数据字段不翻译。
        let english_conn = migrated_conn();
        seed_diff_fixture(&english_conn);
        let english = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(english_conn)),
            I18n::load("en", LocaleSource::Explicit),
        );
        let diff = english.snapshot(ViewId::Diff, Some("S-T"));
        assert_eq!(
            diff.summary,
            "Session S-T · tasks 3 · pass 2 / fail 1 / untested 1"
        );
        assert_eq!(diff.diff.len(), 2);
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

    /// Check 348：TUI 事件行对工具调用给结构化文本，结束事件按同页开始事件配对工具名。
    #[test]
    fn tool_call_event_rows_are_structured_and_paired() {
        let conn = migrated_conn();
        conn.execute(
            "INSERT INTO sessions (id, title, created_at, last_active_at, model, archived, status, stage)
             VALUES ('S-TC', '工具卡', epoch_ms(1000), epoch_ms(1000), NULL, FALSE, 'active', 'implement')",
            [],
        )
        .expect("插入会话");
        for (seq, id, kind, payload) in [
            (
                1_i64,
                "E-TC-1",
                "ToolCallStarted",
                r#"{"tool_name":"read_file","tool_call_id":"call-1","exec_id":"exec-9","iteration":2}"#,
            ),
            (
                2,
                "E-TC-2",
                "ToolCallFinished",
                r#"{"tool_call_id":"call-1","exec_id":"exec-9","state":"completed","duration_ms":42,"output_value":"done"}"#,
            ),
            (
                3,
                "E-TC-3",
                "ToolCallFinished",
                r#"{"tool_call_id":"call-2","state":"failed","error_code":"permission_denied","duration_ms":7}"#,
            ),
        ] {
            conn.execute(
                "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id,
                                     kind, payload_json, ts, persistence_level)
                 VALUES (?, ?, 'S-TC', NULL, NULL, NULL, ?, CAST(? AS JSON)::VARIANT,
                         current_timestamp, 'redacted')",
                duckdb::params![seq, id, kind, payload],
            )
            .expect("插入事件");
        }
        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(conn)),
            I18n::default(),
        );
        let events = source.snapshot(ViewId::Events, Some("S-TC"));
        assert_eq!(events.events.len(), 3);
        let started = &events.events[0].summary;
        assert!(
            started.contains("read_file")
                && started.contains("exec-9")
                && started.contains("迭代 2"),
            "开始行含工具、执行与轮次：{started}"
        );
        let finished = &events.events[1].summary;
        assert!(
            finished.contains("read_file") && finished.contains("state=completed"),
            "结束行靠配对补工具名：{finished}"
        );
        assert!(finished.contains("42") && finished.contains("输出=done"));
        let unpaired = &events.events[2].summary;
        assert!(
            unpaired.contains('?') && unpaired.contains("permission_denied"),
            "未配对结束事件显式呈现缺失：{unpaired}"
        );
    }

    /// 内容检索（T319／FR-780）：数据源委托应用命令，同一权限与片段口径。
    #[test]
    fn content_search_delegates_to_app_command() {
        let conn = migrated_conn();
        whcode_store::create_session_with_id(&conn, "S-C", "检索会话", "active", 1_000)
            .expect("创建会话");
        conn.execute(
            "INSERT INTO messages (id, session_id, role, content_ref, token_count, ts)
             VALUES ('M-1', 'S-C', 'assistant', '台账视图的检索正文里包含关键词', 8,
                     current_timestamp)",
            [],
        )
        .expect("插入消息");
        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(conn)),
            I18n::default(),
        );
        let hits = source.content_search("检索正文", 10).expect("检索");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "S-C");
        assert_eq!(hits[0].role, "assistant");
        assert!(hits[0].snippet.contains("检索正文"));

        // 空词：应用命令的显式错误穿透数据源。
        let err = source.content_search("   ", 10).expect_err("空词");
        assert_eq!(err.code, whcode_domain::ErrorCode::InvalidState);
    }

    /// 台账视图（FR-787）：三节聚合与节级错误注记；无会话是显式状态，不伪造空。
    #[test]
    fn ledger_view_aggregates_three_sections_and_notes_failures() {
        use whcode_app::findings::{FindingDraft, record_finding};
        use whcode_store::create_session_with_id;

        let conn = migrated_conn();
        create_session_with_id(&conn, "S-L", "台账会话", "active", 1_000).expect("创建会话");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, kind, payload_json, ts, persistence_level)
             VALUES (1, 'EV-L1', 'S-L', 'TodoUpdated',
                     CAST('{\"items\":[{\"content\":\"接线台账\",\"status\":\"in_progress\"}],
                            \"pending\":0,\"in_progress\":1,\"completed\":0}' AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            [],
        )
        .expect("插入待办快照");
        conn.execute(
            "INSERT INTO events (seq, id, session_id, kind, payload_json, ts, persistence_level)
             VALUES (2, 'EV-L2', 'S-L', 'DeliverablePresented',
                     CAST('{\"paths\":[\"docs/adr/ADR-080-findings-ledger.md\"],
                            \"tool_call_id\":\"call-L\"}' AS JSON)::VARIANT,
                     current_timestamp, 'redacted')",
            [],
        )
        .expect("插入交付物登记");
        let actor = whcode_app::actor_with_all_capabilities("tester");
        record_finding(
            &conn,
            &whcode_app::DefaultPolicy,
            &actor,
            &FindingDraft {
                session_id: Some("S-L".to_string()),
                title: "缺失败路径".to_string(),
                gap: "失败路径未覆盖".to_string(),
                impact: "失败被当成成功".to_string(),
                owner_fix: "补断言".to_string(),
                verification: "cargo test 转红再转绿".to_string(),
                severity: "high".to_string(),
            },
            1_500,
        )
        .expect("登记发现");

        let source = StoreViewSource::new(
            std::sync::Arc::new(std::sync::Mutex::new(conn)),
            I18n::default(),
        );
        let snapshot = source.snapshot(ViewId::Ledger, Some("S-L"));
        assert_eq!(
            snapshot.ledger.len(),
            3,
            "三节各一行：{:?}",
            snapshot.ledger
        );
        assert_eq!(snapshot.ledger[0].section.as_str(), "todo");
        assert_eq!(snapshot.ledger[0].section_label, "待办", "节名来自文本层");
        assert_eq!(snapshot.ledger[1].section.as_str(), "deliverable");
        assert_eq!(snapshot.ledger[2].section.as_str(), "finding");
        assert!(
            snapshot.ledger[2].status.contains("open"),
            "发现状态进入状态列：{}",
            snapshot.ledger[2].status
        );
        assert!(
            snapshot.summary.contains("待办 1") && snapshot.summary.contains("交付物 1"),
            "摘要含三节计数：{}",
            snapshot.summary
        );

        // 无会话：显式注记，不伪造空。
        let orphan = source.snapshot(ViewId::Ledger, None);
        assert!(orphan.ledger.is_empty(), "无会话无行");
        assert!(
            orphan.summary.contains("未选择会话"),
            "无会话是显式状态：{}",
            orphan.summary
        );

        // 会话不存在：节级错误进摘要（显式失败），不 panic。
        let missing = source.snapshot(ViewId::Ledger, Some("S-404"));
        assert!(
            missing.summary.contains("待办不可用") && missing.summary.contains("S-404"),
            "错误写进摘要行：{}",
            missing.summary
        );
    }
}
