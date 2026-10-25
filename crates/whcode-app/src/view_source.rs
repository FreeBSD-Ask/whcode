//! # 前端共用的只读视图模型（T082／FR-601／FR-711）
//!
//! 会话树、任务、上下文、事件、Spec 状态、diff 预览——六个视图的**只读快照**契约。
//! 放在应用层是为了让 TUI 与原生 GUI 消费同一份数据形状：前端之间不得互相依赖
//! （ADR-003/ADR-027），而快照的生成方（装配根）已依赖本 crate。
//!
//! 纪律（§5.3）：快照由调用方在**准备阶段**生成纯数据，帧路径不反序列化 JSON；
//! 本模块只定义数据与选择逻辑，绘制与脏标记属各前端 crate。
//!
//! 视图内容（FR-601）：会话树（ID/标题/阶段/状态）、任务（状态/重试）、上下文
//! （预算/游标）、事件（最近流）、Spec 状态（门禁/产物版本）、diff 预览（变更行）。

use serde::Serialize;
use std::borrow::Cow;

/// 视图总数（切换键 1—6 与之一一对应）。
pub const VIEW_COUNT: usize = 6;

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
    /// 全部视图（固定顺序：切换键 `1`—`6` 依次对应）。
    pub const ALL: [ViewId; VIEW_COUNT] = [
        Self::Sessions,
        Self::Tasks,
        Self::Context,
        Self::Events,
        Self::Spec,
        Self::Diff,
    ];

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

    /// 显示标题（帧标题与切换提示；FR-601 六视图名称）。
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Sessions => "会话",
            Self::Tasks => "任务",
            Self::Context => "上下文",
            Self::Events => "事件",
            Self::Spec => "Spec 状态",
            Self::Diff => "diff 预览",
        }
    }

    /// 文本层键（FR-615）：`title()` 是 zh-CN 的静态投影，渲染层用 I18n 取本地化标题。
    #[must_use]
    pub const fn title_key(self) -> &'static str {
        match self {
            Self::Sessions => "view.sessions",
            Self::Tasks => "view.tasks",
            Self::Context => "view.context",
            Self::Events => "view.events",
            Self::Spec => "view.spec",
            Self::Diff => "view.diff",
        }
    }

    /// 序号（从 1 起；切换键与帧标题 `{n}/{VIEW_COUNT}`）。
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Sessions => 1,
            Self::Tasks => 2,
            Self::Context => 3,
            Self::Events => 4,
            Self::Spec => 5,
            Self::Diff => 6,
        }
    }

    /// 切换键映射：`'1'`—`'6'` → 对应视图；其他键返回 `None`。
    #[must_use]
    pub const fn from_key(key: char) -> Option<ViewId> {
        match key {
            '1' => Some(Self::Sessions),
            '2' => Some(Self::Tasks),
            '3' => Some(Self::Context),
            '4' => Some(Self::Events),
            '5' => Some(Self::Spec),
            '6' => Some(Self::Diff),
            _ => None,
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

/// 列投影行（T194／ADR-041：TUI 列对齐用）。
///
/// 字段借用快照（`Cow::Borrowed`），只有选中标记与数字列产生少量派生值；行数上界由调用方
/// 的 `max` 界定（帧路径取终端行数），因此单帧分配只与终端尺寸相关，不随快照行数增长。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRow<'a> {
    /// 单元格（顺序由视图列定义决定；会话与任务视图首格为选中标记）。
    pub cells: Vec<Cow<'a, str>>,
}

/// 选中标记（ui-design：反显或 `>` 前缀，不依赖颜色）。
const fn selection_marker(selected: bool) -> &'static str {
    if selected { "> " } else { "" }
}

/// 视图快照（六视图之一的只读数据；渲染层只消费，不反序列化）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ViewSnapshot {
    /// 当前视图。
    pub view: ViewId,
    /// 摘要行（准备层生成；非空时作为首行显示，如“会话 S-1 · 总计 12 · 完成 3”）。
    pub summary: String,
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
    /// 会话视图的选中行（`j`/`k` 选择；`None` 表示无选中）。
    pub selected_index: Option<usize>,
}

impl ViewSnapshot {
    /// 空快照（给定视图）。
    #[must_use]
    pub fn empty(view: ViewId) -> Self {
        Self {
            view,
            summary: String::new(),
            sessions: Vec::new(),
            tasks: Vec::new(),
            events: Vec::new(),
            spec_status: String::new(),
            context_summary: String::new(),
            diff: Vec::new(),
            selected_index: None,
        }
    }

    /// 渲染为文本行（完整投影；测试与准备层证据用；**不反序列化 JSON**）。
    #[must_use]
    pub fn render_lines(&self) -> Vec<String> {
        self.render_lines_limited(usize::MAX)
    }

    /// 渲染至多 `max` 行为文本行（**帧路径的投影**：帧成本由终端尺寸界定，与快照
    /// 行数无关——10k 行快照与 200 行快照的帧投影同阶。完整行数由准备层按
    /// “有界尾部”裁剪（V-101），此处只是安全上界）。**不反序列化 JSON**。
    #[must_use]
    pub fn render_lines_limited(&self, max: usize) -> Vec<String> {
        if max == 0 {
            return Vec::new();
        }
        let mut lines: Vec<String> = Vec::new();
        if !self.summary.is_empty() && lines.len() < max {
            lines.push(self.summary.clone());
        }
        let remaining = max - lines.len();
        lines.extend(match self.view {
            ViewId::Sessions => self
                .sessions
                .iter()
                .enumerate()
                .take(remaining)
                .map(|(index, row)| {
                    // 选中态用 `> ` 前缀（ui-design：反显或前缀，不依赖颜色）。
                    let marker = if self.selected_index == Some(index) {
                        "> "
                    } else {
                        ""
                    };
                    format!(
                        "{marker}{} {} [{}] {}",
                        row.id, row.title, row.stage, row.status
                    )
                })
                .collect::<Vec<_>>(),
            ViewId::Tasks => self
                .tasks
                .iter()
                .enumerate()
                .take(remaining)
                .map(|(index, row)| {
                    // 任务选中态（FR-623 写操作作用于选中任务）：`> ` 前缀（不只靠颜色）。
                    let marker = if self.view == ViewId::Tasks && self.selected_index == Some(index)
                    {
                        "> "
                    } else {
                        ""
                    };
                    format!(
                        "{marker}{} {} [{}] retry={}",
                        row.id, row.title, row.status, row.retry_count
                    )
                })
                .collect(),
            ViewId::Events => self
                .events
                .iter()
                .take(remaining)
                .map(|row| format!("{} {} {}", row.seq, row.kind, row.summary))
                .collect(),
            ViewId::Spec => text_lines(&self.spec_status, remaining),
            ViewId::Context => text_lines(&self.context_summary, remaining),
            ViewId::Diff => self
                .diff
                .iter()
                .take(remaining)
                .map(|line| format!("{}{}", line.marker, line.text))
                .collect(),
        });
        lines
    }

    /// 列投影至多 `max` 行（T194／ADR-041：TUI 三区布局的列对齐数据源）。
    ///
    /// 只有会话、任务、事件三视图有列定义；Spec／上下文／diff 返回空投影，由渲染层继续走
    /// [`Self::render_lines_limited`] 的文本行路径。摘要行（`summary`）不参与列投影——TUI 把
    /// 它放在状态栏，避免与主面板首行重复。快照行数不影响单帧分配：`take(max)` 在
    /// 迭代器上裁剪，投影行数恒 ≤ `max`。
    #[must_use]
    pub fn render_columns_limited(&self, max: usize) -> Vec<ColumnRow<'_>> {
        if max == 0 {
            return Vec::new();
        }
        match self.view {
            ViewId::Sessions => self
                .sessions
                .iter()
                .enumerate()
                .take(max)
                .map(|(index, row)| ColumnRow {
                    cells: vec![
                        Cow::Borrowed(selection_marker(self.selected_index == Some(index))),
                        Cow::Borrowed(row.id.as_str()),
                        Cow::Borrowed(row.title.as_str()),
                        Cow::Borrowed(row.stage.as_str()),
                        Cow::Borrowed(row.status.as_str()),
                    ],
                })
                .collect(),
            ViewId::Tasks => self
                .tasks
                .iter()
                .enumerate()
                .take(max)
                .map(|(index, row)| ColumnRow {
                    cells: vec![
                        Cow::Borrowed(selection_marker(self.selected_index == Some(index))),
                        Cow::Borrowed(row.id.as_str()),
                        Cow::Borrowed(row.title.as_str()),
                        Cow::Borrowed(row.status.as_str()),
                        Cow::Owned(row.retry_count.to_string()),
                    ],
                })
                .collect(),
            ViewId::Events => self
                .events
                .iter()
                .take(max)
                .map(|row| ColumnRow {
                    cells: vec![
                        Cow::Owned(row.seq.to_string()),
                        Cow::Borrowed(row.kind.as_str()),
                        Cow::Borrowed(row.summary.as_str()),
                    ],
                })
                .collect(),
            ViewId::Context | ViewId::Spec | ViewId::Diff => Vec::new(),
        }
    }
}

/// 单文本行视图的投影：空文本投影 0 行（帧内显示“暂无数据”，不伪造空行）。
fn text_lines(text: &str, max: usize) -> Vec<String> {
    if max == 0 || text.is_empty() {
        Vec::new()
    } else {
        vec![text.to_string()]
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
        for view in ViewId::ALL {
            assert!(!view.as_str().is_empty());
        }
        assert_eq!(ViewId::Sessions.as_str(), "sessions");
        assert_eq!(ViewId::Diff.as_str(), "diff");
    }

    /// 标题键（FR-615）：六视图各有稳定键，且与 `title()` 同序。
    #[test]
    fn view_title_keys_are_stable() {
        for view in ViewId::ALL {
            assert!(view.title_key().starts_with("view."), "{view:?}");
            assert!(!view.title().is_empty());
        }
        assert_eq!(ViewId::Sessions.title_key(), "view.sessions");
        assert_eq!(ViewId::Diff.title_key(), "view.diff");
    }

    /// 切换键 `1`—`6` 与六视图一一对应（`from_key`/`index`/`ALL` 三者一致）。
    #[test]
    fn switching_keys_map_to_all_six_views() {
        assert_eq!(ViewId::ALL.len(), VIEW_COUNT);
        for (offset, view) in ViewId::ALL.into_iter().enumerate() {
            let key = char::from_digit(u32::try_from(offset + 1).expect("序号在 u32 内"), 10)
                .expect("十进制数字");
            assert_eq!(
                ViewId::from_key(key),
                Some(view),
                "键 {key} 应切换到 {view:?}"
            );
            assert_eq!(view.index(), offset + 1);
            assert!(!view.title().is_empty());
        }
        for other in ['0', '7', 'a', ' ', '\u{4e2d}'] {
            assert_eq!(ViewId::from_key(other), None, "{other:?} 不是视图切换键");
        }
    }

    /// 摘要行 + 会话选中标记：summary 作首行；选中行以 `> ` 前缀（不依赖颜色）。
    #[test]
    fn summary_and_selection_marker_render_in_order() {
        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        snapshot.summary = "工作区 · 会话 2".to_string();
        for (index, title) in ["甲", "乙"].iter().enumerate() {
            snapshot.sessions.push(SessionRow {
                id: format!("S-{}", index + 1),
                title: (*title).to_string(),
                stage: "plan".to_string(),
                status: "active".to_string(),
            });
        }
        snapshot.selected_index = Some(1);
        let lines = snapshot.render_lines();
        assert_eq!(lines[0], "工作区 · 会话 2", "summary 必须是首行");
        assert_eq!(lines[1], "S-1 甲 [plan] active", "未选中行不加前缀");
        assert_eq!(lines[2], "> S-2 乙 [plan] active", "选中行加 > 前缀");

        // 行数上界包含 summary；max=0 投影 0 行。
        assert_eq!(snapshot.render_lines_limited(2).len(), 2);
        assert!(snapshot.render_lines_limited(0).is_empty());

        // 摘要 + 空数据：摘要仍显示（正确的空不是“暂无数据”）。
        let mut tasks = ViewSnapshot::empty(ViewId::Tasks);
        tasks.summary = "会话 S-1 · 总计 0".to_string();
        assert_eq!(tasks.render_lines(), vec!["会话 S-1 · 总计 0"]);
    }

    /// 帧路径投影（Check 83）：`render_lines_limited` 的行数受上界约束，
    /// 且与完整投影前缀一致——帧成本由终端尺寸界定，与快照行数无关。
    #[test]
    fn limited_projection_is_bounded_and_consistent() {
        let mut snapshot = ViewSnapshot::empty(ViewId::Events);
        for seq in 1..=100 {
            snapshot.events.push(EventRow {
                seq,
                kind: "StreamingChunk".to_string(),
                summary: format!("chunk {seq}"),
            });
        }
        let full = snapshot.render_lines();
        assert_eq!(full.len(), 100);
        for max in [0_usize, 1, 22, 24, 99, 100, 200] {
            let limited = snapshot.render_lines_limited(max);
            assert_eq!(limited.len(), full.len().min(max), "max={max}");
            assert_eq!(limited, full[..limited.len()], "前缀必须与完整投影一致");
        }

        // 空文本视图投影 0 行（帧内提示“暂无数据”，不伪造空行）。
        let spec = ViewSnapshot::empty(ViewId::Spec);
        assert!(spec.render_lines().is_empty());
        let mut context = ViewSnapshot::empty(ViewId::Context);
        context.context_summary = "预算 1/2".to_string();
        assert_eq!(
            context.render_lines_limited(10),
            vec!["预算 1/2".to_string()]
        );
        assert!(context.render_lines_limited(0).is_empty());
    }

    /// 列投影（T194）：三视图的列序、选中标记与派生列；文本行视图返回空投影。
    #[test]
    fn column_projection_covers_row_views() {
        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        for (index, title) in ["甲", "乙"].iter().enumerate() {
            snapshot.sessions.push(SessionRow {
                id: format!("S-{}", index + 1),
                title: (*title).to_string(),
                stage: "plan".to_string(),
                status: "active".to_string(),
            });
        }
        snapshot.selected_index = Some(1);
        let rows = snapshot.render_columns_limited(10);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .map(|row| row
                    .cells
                    .iter()
                    .map(|cell| cell.as_ref())
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            vec![
                vec!["", "S-1", "甲", "plan", "active"],
                vec!["> ", "S-2", "乙", "plan", "active"],
            ],
            "会话列序：标记／ID／标题／阶段／状态"
        );

        let mut tasks = ViewSnapshot::empty(ViewId::Tasks);
        tasks.tasks.push(TaskRow {
            id: "T-1".to_string(),
            title: "构建".to_string(),
            status: "failed".to_string(),
            retry_count: 2,
        });
        tasks.selected_index = Some(0);
        let task_rows = tasks.render_columns_limited(10);
        assert_eq!(
            task_rows[0]
                .cells
                .iter()
                .map(|cell| cell.as_ref())
                .collect::<Vec<_>>(),
            vec!["> ", "T-1", "构建", "failed", "2"],
            "重试列必须是数字文本（派生列允许 Owned）"
        );

        let mut events = ViewSnapshot::empty(ViewId::Events);
        events.events.push(EventRow {
            seq: 7,
            kind: "SessionCreated".to_string(),
            summary: "会话 S-1".to_string(),
        });
        let event_rows = events.render_columns_limited(10);
        assert_eq!(
            event_rows[0]
                .cells
                .iter()
                .map(|cell| cell.as_ref())
                .collect::<Vec<_>>(),
            vec!["7", "SessionCreated", "会话 S-1"]
        );

        // 文本行视图（Spec／上下文／diff）无列定义：空投影，渲染层回落文本行路径。
        for view in [ViewId::Spec, ViewId::Context, ViewId::Diff] {
            assert!(
                ViewSnapshot::empty(view)
                    .render_columns_limited(10)
                    .is_empty(),
                "{view:?} 不应有列投影"
            );
        }

        // max=0 投影 0 行（失败路径：上界为 0 时不得产出任何行）。
        assert!(snapshot.render_columns_limited(0).is_empty());
    }

    /// 列投影的有界性（资源中性不变量，ADR-041 决策 8）：万行快照下单帧投影行数恒 ≤ `max`。
    #[test]
    fn column_projection_bounded_by_max() {
        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        for index in 0..10_000 {
            snapshot.sessions.push(SessionRow {
                id: format!("S-{index}"),
                title: "标题".to_string(),
                stage: "plan".to_string(),
                status: "active".to_string(),
            });
        }
        for max in [1_usize, 22, 24, 100] {
            assert_eq!(snapshot.render_columns_limited(max).len(), max, "max={max}");
        }
        // 超过快照行数时按实际行数返回（不伪造空行）。
        assert_eq!(snapshot.render_columns_limited(20_000).len(), 10_000);
    }
}

/// 视图数据源（准备层）：为给定视图准备**只读快照**。
///
/// 实现方必须立即返回纯数据：查询/整理应在调用方的准备阶段完成，不得在快照准备中
/// 执行 LLM 网络 I/O 或长时间工具调用（§5.3 渲染线程纪律）。视图切换、会话选择
/// （`j`/`k`）与显式刷新（`r`）时各准备一次；**帧路径不反序列化 JSON**（快照即纯数据）。
///
/// `session` 为当前选中的会话 ID（`None` = 尚未选择）；会话树视图忽略该参数。
pub trait ViewSource {
    /// 准备指定视图的只读快照（含会话范围）。
    fn snapshot(&self, view: ViewId, session: Option<&str>) -> ViewSnapshot;

    /// 持久化事件游标（FR-624 在线可见）：`None` 表示数据源不可用或不可查——
    /// TUI 仅在游标**推进**时置脏重绘一次；默认 `None`（空数据源与测试夹具兼容）。
    fn cursor(&self, _session: Option<&str>) -> Option<i64> {
        None
    }
}

/// 空数据源：数据源不可用时的回退实现——六视图可切换、可绘制，界面提示“暂无数据”。
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyViewSource;

impl ViewSource for EmptyViewSource {
    fn snapshot(&self, view: ViewId, _session: Option<&str>) -> ViewSnapshot {
        ViewSnapshot::empty(view)
    }
}
