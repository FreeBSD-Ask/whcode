//! # B4 高级面板数据门面（T258／T259、FR-749／750／751）
//!
//! 设置页与 GUI 高级面板共用的只读数据：
//!
//! - **技能库**（FR-750）：扫描 `.zcode/skills/`，附可达性检查结果；
//! - **MCP 服务器**（FR-750）：配置态（传输、超时、输出上限）＋最近事件窗口里
//!   观测到的工具与调用终态；
//! - **子代理运行**（FR-749）：`spawn_subagent` 的开始／结束事件配对，给出
//!   运行中／完成／失败与产出摘要；
//! - **审批规则**（FR-751）：直接复用 [`crate::approval_rules`] 的门面。
//!
//! 纪律：
//! - 全部有界（技能行、观测工具、子代理行、事件窗口各有上限常量）；
//! - 不 spawn 进程、不发起网络请求：MCP 的“连接状态”来自配置与已落表事件，
//!   真实的 per-turn 连接仍由 `whcode-app::turn` 在 turn 内建立（ADR-054）；
//! - 只读；SQL 仍只在 `whcode-store`（ADR-018），本模块只做投影与解析。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use duckdb::Connection;
use serde_json::Value;

use whcode_domain::WhError;
use whcode_store::query::{Direction, EventRow, events_page};

/// 技能行上限（超出截断并给 warning）。
pub const MAX_SKILL_ROWS: usize = 64;
/// 技能扫描 warning 上限。
pub const MAX_SKILL_WARNINGS: usize = 32;
/// 单技能可达性问题上限。
pub const MAX_SKILL_ISSUES: usize = 16;
/// 子代理运行行上限（最新在前）。
pub const MAX_SUBAGENT_ROWS: usize = 32;
/// 单服务器观测工具上限。
pub const MAX_OBSERVED_TOOLS: usize = 32;
/// 面板读取的事件窗口（尾部条数）。
pub const MAX_PANEL_EVENTS: usize = 200;
/// 产出摘要字符上限（准备层截断；帧路径不再处理长文本）。
pub const MAX_SUMMARY_CHARS: usize = 200;

/// 技能库一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRow {
    /// 技能名（frontmatter `name`）。
    pub name: String,
    /// 描述（frontmatter `description`，已由扫描器截断）。
    pub description: String,
    /// `SKILL.md` 绝对路径。
    pub path: String,
    /// 可达性问题（引用缺失等；空 = 通过）。
    pub issues: Vec<String>,
}

/// 技能库面板。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillsPanel {
    /// 技能行（有界）。
    pub skills: Vec<SkillRow>,
    /// 扫描 warning（坏 frontmatter、缺 SKILL.md、截断提示）。
    pub warnings: Vec<String>,
}

/// 扫描工作区技能库并做可达性检查。
///
/// 传 `None`（数据源未持有工作区）时返回空面板——这是“无法扫描”的空，
/// 与“没有技能”的空由调用方用 `workspace_known` 区分。
#[must_use]
pub fn skills_panel(workspace: Option<&Path>) -> SkillsPanel {
    let Some(workspace) = workspace else {
        return SkillsPanel::default();
    };
    let (entries, mut warnings) = match crate::skills::scan_skills(workspace) {
        Ok(pair) => pair,
        Err(err) => {
            return SkillsPanel {
                skills: Vec::new(),
                warnings: vec![err.to_string()],
            };
        }
    };
    let mut skills = Vec::with_capacity(entries.len().min(MAX_SKILL_ROWS));
    for entry in entries.into_iter().take(MAX_SKILL_ROWS) {
        let dir = entry.path.parent().unwrap_or(workspace);
        let issues = std::fs::read_to_string(&entry.path)
            .map(|content| {
                crate::skills::validate_references(dir, &content)
                    .into_iter()
                    .take(MAX_SKILL_ISSUES)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|err| vec![format!("读取 SKILL.md 失败：{err}")]);
        skills.push(SkillRow {
            name: entry.name,
            description: entry.description,
            path: entry.path.display().to_string(),
            issues,
        });
    }
    if skills.len() == MAX_SKILL_ROWS {
        warnings.push(format!("技能行超过 {MAX_SKILL_ROWS} 条，超出部分未展示"));
    }
    warnings.truncate(MAX_SKILL_WARNINGS);
    SkillsPanel { skills, warnings }
}

/// MCP 服务器状态（稳定码；呈现层负责本地化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStatus {
    /// 配置已启用，事件窗口内无调用记录。
    Configured,
    /// 配置显式关闭（`enabled = false`）。
    Disabled,
    /// 最近窗口内有开始事件、无结束事件。
    Running,
    /// 最近一次调用成功。
    Ok,
    /// 最近一次调用失败。
    Failed,
}

impl McpStatus {
    /// 稳定码（JSON 与 GUI 词条键共用）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Disabled => "disabled",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// MCP 服务器一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRow {
    /// 服务器名。
    pub name: String,
    /// 传输（`stdio`／`http`）。
    pub transport: String,
    /// 是否启用。
    pub enabled: bool,
    /// 请求超时（毫秒）。
    pub timeout_ms: u64,
    /// 单次输出上限（字节）。
    pub max_output_bytes: usize,
    /// 状态（配置 + 最近事件窗口）。
    pub status: McpStatus,
    /// 最近窗口内观测到的工具名（`mcp__<server>__<tool>` 的 `<tool>` 段）。
    pub tools: Vec<String>,
}

/// MCP 状态面板。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpPanel {
    /// 配置文件是否存在且成功解析。
    pub configured: bool,
    /// 服务器行（配置顺序）。
    pub servers: Vec<McpServerRow>,
    /// 解析或事件读取失败时的显式说明（不伪造空）。
    pub error: Option<String>,
}

/// 读取 MCP 配置并与最近事件窗口里的调用记录合并。
///
/// # Errors
///
/// 配置文件解析失败或事件查询失败时返回 [`WhError`]（显式失败，不伪装成空）。
pub fn mcp_panel(
    workspace: Option<&Path>,
    conn: &Connection,
    session: Option<&str>,
) -> Result<McpPanel, WhError> {
    let Some(workspace) = workspace else {
        return Ok(McpPanel::default());
    };
    let config = whcode_mcp::config::McpConfig::load(workspace)?;
    let Some(config) = config else {
        return Ok(McpPanel::default());
    };
    let observed = observed_calls(conn, session)?;
    let mut servers = Vec::with_capacity(config.servers.len());
    for server in &config.servers {
        let calls = observed.get(&server.name);
        servers.push(McpServerRow {
            name: server.name.clone(),
            transport: if server.url.is_some() {
                "http".to_string()
            } else {
                "stdio".to_string()
            },
            enabled: server.enabled,
            timeout_ms: server.timeout_ms,
            max_output_bytes: server.max_output_bytes,
            status: if !server.enabled {
                McpStatus::Disabled
            } else {
                calls.map_or(McpStatus::Configured, |calls| calls.status)
            },
            tools: calls.map(|calls| calls.tools.clone()).unwrap_or_default(),
        });
    }
    Ok(McpPanel {
        configured: true,
        servers,
        error: None,
    })
}

/// 子代理运行状态（稳定码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentState {
    /// 有开始事件、窗口内无结束事件。
    Running,
    /// 结束事件 `completed`。
    Ok,
    /// 结束事件 `failed`（含委派前置失败）。
    Failed,
}

impl SubagentState {
    /// 稳定码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
        }
    }
}

/// 子代理运行一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentRunRow {
    /// 执行 ID（无执行 ID 时用 tool call ID）。
    pub exec_id: String,
    /// 状态。
    pub state: SubagentState,
    /// 产出摘要（事件 `output_summary`；无则空）。
    pub output_summary: Option<String>,
    /// 子 turn 终止原因（`model_finished` 等）。
    pub termination: Option<String>,
    /// 子 turn 迭代轮数。
    pub iterations: Option<u32>,
    /// 实测时长（毫秒）。
    pub duration_ms: Option<u64>,
    /// 结构化错误码（失败时）。
    pub error_code: Option<String>,
}

/// 读取某会话最近窗口里的子代理运行记录（最新在前，有界）。
///
/// # Errors
///
/// 事件查询失败时返回 [`WhError`]。
pub fn subagent_runs(
    conn: &Connection,
    session: Option<&str>,
) -> Result<Vec<SubagentRunRow>, WhError> {
    let Some(session) = session else {
        return Ok(Vec::new());
    };
    let events = tail_events(conn, session)?;
    let mut runs: Vec<SubagentRunRow> = Vec::new();
    for row in &events {
        let Some(payload) = parse_payload(row) else {
            continue;
        };
        if row.kind != "ToolCallStarted" {
            continue;
        }
        let tool = payload
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if tool != crate::turn::SPAWN_SUBAGENT_TOOL {
            continue;
        }
        let call_id = payload
            .get("tool_call_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let finished = events.iter().find(|candidate| {
            candidate.kind == "ToolCallFinished"
                && parse_payload(candidate)
                    .and_then(|value| {
                        value
                            .get("tool_call_id")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .as_deref()
                    == Some(call_id.as_str())
        });
        let mut run = SubagentRunRow {
            exec_id: payload
                .get("exec_id")
                .and_then(Value::as_str)
                .map_or_else(|| call_id.clone(), str::to_string),
            state: SubagentState::Running,
            output_summary: None,
            termination: None,
            iterations: None,
            duration_ms: None,
            error_code: None,
        };
        if let Some(finished) = finished
            && let Some(finished) = parse_payload(finished)
        {
            let state = finished.get("state").and_then(Value::as_str).unwrap_or("");
            run.state = if state == "completed" || state == "ok" {
                SubagentState::Ok
            } else {
                SubagentState::Failed
            };
            run.output_summary = finished
                .get("output_summary")
                .and_then(Value::as_str)
                .map(str::to_string);
            run.termination = finished
                .get("sub_termination_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            run.iterations = finished
                .get("sub_iterations")
                .and_then(Value::as_u64)
                .map(|value| u32::try_from(value).unwrap_or(u32::MAX));
            run.duration_ms = finished.get("duration_ms").and_then(Value::as_u64);
            run.error_code = finished
                .get("error_code")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        runs.push(run);
        if runs.len() >= MAX_SUBAGENT_ROWS {
            break;
        }
    }
    Ok(runs)
}

/// 最近窗口内某服务器的观测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedCalls {
    status: McpStatus,
    tools: Vec<String>,
}

/// 从事件窗口提取 MCP 调用记录（服务器 → 工具集合 + 最近终态）。
fn observed_calls(
    conn: &Connection,
    session: Option<&str>,
) -> Result<BTreeMap<String, ObservedCalls>, WhError> {
    let Some(session) = session else {
        return Ok(BTreeMap::new());
    };
    let events = tail_events(conn, session)?;
    let mut finished: BTreeMap<String, String> = BTreeMap::new();
    for row in &events {
        if row.kind != "ToolCallFinished" {
            continue;
        }
        if let Some(payload) = parse_payload(row)
            && let (Some(call), Some(state)) = (
                payload.get("tool_call_id").and_then(Value::as_str),
                payload.get("state").and_then(Value::as_str),
            )
        {
            finished.insert(call.to_string(), state.to_string());
        }
    }
    let mut observed: BTreeMap<String, ObservedCalls> = BTreeMap::new();
    for row in &events {
        if row.kind != "ToolCallStarted" {
            continue;
        }
        let Some(payload) = parse_payload(row) else {
            continue;
        };
        let Some(tool_name) = payload.get("tool_name").and_then(Value::as_str) else {
            continue;
        };
        let Some((server, tool)) = whcode_mcp::manager::parse_tool_name(tool_name) else {
            continue;
        };
        let entry = observed.entry(server).or_insert_with(|| ObservedCalls {
            status: McpStatus::Configured,
            tools: Vec::new(),
        });
        if entry.tools.len() < MAX_OBSERVED_TOOLS && !entry.tools.iter().any(|item| item == &tool) {
            entry.tools.push(tool.clone());
            entry.tools.sort();
        }
        let state = payload
            .get("tool_call_id")
            .and_then(Value::as_str)
            .and_then(|call| finished.get(call));
        entry.status = match state.map(String::as_str) {
            Some("ok") => McpStatus::Ok,
            Some(_) => McpStatus::Failed,
            None => McpStatus::Running,
        };
    }
    Ok(observed)
}

/// 会话事件尾部（最新在前，`MAX_PANEL_EVENTS` 条）。
fn tail_events(conn: &Connection, session: &str) -> Result<Vec<EventRow>, WhError> {
    let page = events_page(conn, session, Direction::Backward, None, MAX_PANEL_EVENTS)?;
    Ok(page.items)
}

/// 解析事件载荷；非法 JSON 跳过（事件写入侧已有格式约束）。
fn parse_payload(row: &EventRow) -> Option<Value> {
    serde_json::from_str(&row.payload_json).ok()
}

/// 摘要截断：折叠空白并截到上限（只用于展示层字段）。
#[must_use]
pub fn summarize(text: &str) -> String {
    let collapsed: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut summary: String = collapsed.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        summary = summary.chars().take(MAX_SUMMARY_CHARS).collect();
        summary.push('…');
    }
    summary
}

/// 工具名去重集合（保留首次出现顺序；用于测试与投影）。
#[must_use]
pub fn stable_tool_set(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for name in names {
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use whcode_store::apply_migrations;

    fn migrated() -> Connection {
        let conn = Connection::open_in_memory().expect("内存库");
        apply_migrations(&conn).expect("迁移");
        conn
    }

    fn insert_event(conn: &Connection, seq: i64, kind: &str, payload: Value) {
        conn.execute(
            "INSERT INTO events (seq, id, session_id, task_id, request_id, tool_call_id, \
                                 kind, payload_json, ts, persistence_level) \
             VALUES (?, ?, 'S-1', NULL, NULL, NULL, ?, CAST(? AS JSON)::VARIANT, \
                     current_timestamp, 'durable')",
            duckdb::params![seq, format!("E-{seq}"), kind, payload.to_string()],
        )
        .expect("插事件");
    }

    #[test]
    fn subagent_runs_pair_started_and_finished_events() {
        let conn = migrated();
        insert_event(
            &conn,
            1,
            "ToolCallStarted",
            serde_json::json!({
                "tool_name": "spawn_subagent",
                "tool_call_id": "call-1",
                "exec_id": "TX-1",
            }),
        );
        insert_event(
            &conn,
            2,
            "ToolCallFinished",
            serde_json::json!({
                "tool_call_id": "call-1",
                "state": "completed",
                "output_summary": "子任务完成",
                "sub_termination_reason": "model_finished",
                "sub_iterations": 2,
                "duration_ms": 42,
            }),
        );
        let runs = subagent_runs(&conn, Some("S-1")).expect("读取");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, SubagentState::Ok);
        assert_eq!(runs[0].termination.as_deref(), Some("model_finished"));
        assert_eq!(runs[0].iterations, Some(2));
        assert_eq!(runs[0].duration_ms, Some(42));
        assert_eq!(runs[0].output_summary.as_deref(), Some("子任务完成"));
    }

    #[test]
    fn subagent_started_without_finish_is_running() {
        let conn = migrated();
        insert_event(
            &conn,
            1,
            "ToolCallStarted",
            serde_json::json!({
                "tool_name": "spawn_subagent",
                "tool_call_id": "call-1",
            }),
        );
        let runs = subagent_runs(&conn, Some("S-1")).expect("读取");
        assert_eq!(runs[0].state, SubagentState::Running);
    }

    #[test]
    fn mcp_panel_reports_missing_config_as_correct_empty() {
        let conn = migrated();
        let temp = std::env::temp_dir().join(format!("whcode-panels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).expect("建目录");
        let panel = mcp_panel(Some(&temp), &conn, Some("S-1")).expect("读取");
        assert!(!panel.configured);
        assert!(panel.servers.is_empty());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn summarize_is_bounded_and_collapses_whitespace() {
        let long = format!("  a\n b\t{}", "x".repeat(MAX_SUMMARY_CHARS + 10));
        let summary = summarize(&long);
        assert!(summary.chars().count() <= MAX_SUMMARY_CHARS + 1);
        assert!(!summary.contains('\n'));
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn skills_panel_without_workspace_is_empty() {
        assert_eq!(skills_panel(None), SkillsPanel::default());
    }
}
