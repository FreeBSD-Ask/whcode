//! # TUI 六视图（T082、FR-601）
//!
//! 会话树、任务、上下文、事件、Spec 状态、diff 预览——六个视图的数据**只读快照**
//! （与 [`crate::render`] 的渲染纪律一致，§5.3）：数据由调用方预先准备为纯数据
//! 结构（**不在帧路径反序列化 JSON**），渲染层只消费。
//!
//! 视图内容（FR-601）：会话树（ID/标题/阶段/状态）、任务（状态/重试）、上下文
//! （预算/游标）、事件（最近流）、Spec 状态（门禁/产物版本）、diff 预览（变更行）。
//!
//! 边界：本模块只定义视图数据与选择逻辑；绘制与脏标记属 [`crate::render`]。

use serde::Serialize;

/// 视图标识（六视图，FR-601）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ViewId {
    /// 会话树。
    Sessions,
    /// 任务。
    Tasks,
    /// 上下文。
    Context,
    /// 事件。
    Events,
    /// Spec 状态。
    Spec,
    /// diff 预览。
    Diff,
}

impl ViewId {
    /// 稳定标识（配置与切换）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sessions => "sessions",
            Self::Tasks => "tasks",
            Self::Context => "context",
            Self::Events => "events",
            Self::Spec => "spec",
            Self::Diff => "diff",
        }
    }
}

/// 会话树条目（只读快照）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionRow {
    /// 会话 ID。
    pub id: String,
    /// 标题。
    pub title: String,
    /// SDD 阶段。
    pub stage: String,
    /// 会话状态。
    pub status: String,
}

/// 任务条目（只读快照）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskRow {
    /// 任务 ID。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 状态（稳定名）。
    pub status: String,
    /// 重试计数。
    pub retry_count: i64,
}

/// 事件条目（只读快照；载荷经准备层处理，帧路径不做反序列化）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventRow {
    /// 工作区序号。
    pub seq: i64,
    /// 事件类型（稳定名）。
    pub kind: String,
    /// 预渲染摘要（准备层生成；帧路径不解析 JSON）。
    pub summary: String,
}

/// diff 预览行（只读快照）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    /// 变更类型（`+`/`-`/` `）。
    pub marker: char,
    /// 行内容。
    pub text: String,
}

/// 视图快照（六视图之一的只读数据；渲染层只消费，不反序列化）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewSnapshot {
    /// 当前视图。
    pub view: ViewId,
    /// 会话树（会话视图）。
    pub sessions: Vec<SessionRow>,
    /// 任务（任务视图）。
    pub tasks: Vec<TaskRow>,
    /// 事件（事件视图）。
    pub events: Vec<EventRow>,
    /// Spec 状态文本（准备层生成）。
    pub spec_status: String,
    /// 上下文摘要（预算/游标）。
    pub context_summary: String,
    /// diff 行（diff 视图）。
    pub diff: Vec<DiffLine>,
}

impl ViewSnapshot {
    /// 空快照（给定视图）。
    #[must_use]
    pub fn empty(view: ViewId) -> Self {
        Self {
            view,
            sessions: Vec::new(),
            tasks: Vec::new(),
            events: Vec::new(),
            spec_status: String::new(),
            context_summary: String::new(),
            diff: Vec::new(),
        }
    }

    /// 渲染为文本行（只读投影；**不反序列化 JSON**——数据已由准备层处理）。
    #[must_use]
    pub fn render_lines(&self) -> Vec<String> {
        match self.view {
            ViewId::Sessions => self
                .sessions
                .iter()
                .map(|row| format!("{} {} [{}] {}", row.id, row.title, row.stage, row.status))
                .collect(),
            ViewId::Tasks => self
                .tasks
                .iter()
                .map(|row| {
                    format!(
                        "{} {} [{}] retry={}",
                        row.id, row.title, row.status, row.retry_count
                    )
                })
                .collect(),
            ViewId::Events => self
                .events
                .iter()
                .map(|row| format!("{} {} {}", row.seq, row.kind, row.summary))
                .collect(),
            ViewId::Spec => vec![self.spec_status.clone()],
            ViewId::Context => vec![self.context_summary.clone()],
            ViewId::Diff => self
                .diff
                .iter()
                .map(|line| format!("{}{}", line.marker, line.text))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_views_render_from_prepared_data() {
        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        snapshot.sessions.push(SessionRow {
            id: "S-1".to_string(),
            title: "会话".to_string(),
            stage: "plan".to_string(),
            status: "active".to_string(),
        });
        let lines = snapshot.render_lines();
        assert_eq!(lines, vec!["S-1 会话 [plan] active"]);

        snapshot.view = ViewId::Diff;
        snapshot.diff.push(DiffLine {
            marker: '+',
            text: "新增行".to_string(),
        });
        assert_eq!(snapshot.render_lines(), vec!["+新增行"]);
    }

    #[test]
    fn view_ids_are_stable() {
        for view in [
            ViewId::Sessions,
            ViewId::Tasks,
            ViewId::Context,
            ViewId::Events,
            ViewId::Spec,
            ViewId::Diff,
        ] {
            assert!(!view.as_str().is_empty());
        }
        assert_eq!(ViewId::Sessions.as_str(), "sessions");
        assert_eq!(ViewId::Diff.as_str(), "diff");
    }
}
