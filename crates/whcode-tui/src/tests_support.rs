//! TUI 测试支撑：视图夹具、假探测源与固定尺寸帧渲染（单元测试与证据采集共用）。
//!
//! 仅在测试构建中编译（`#[cfg(test)]`）。这里的夹具数据是**准备层数据的等价物**，
//! 用于在固定终端尺寸下复现六视图帧（Check 82 的可执行证据形式）；不含任何真实数据。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::views::{DiffLine, EventRow, SessionRow, TaskRow};
use whcode_domain::i18n::I18n;

use crate::{Theme, TuiState, UiEvent, ViewId, ViewSnapshot, draw_frame};

/// 假最小化探测源：按序返回预置结果（耗尽后返回 `None`＝探测失败），并记录调用次数。
pub struct FakeProbe {
    results: RefCell<VecDeque<Option<bool>>>,
    calls: Cell<usize>,
}

impl FakeProbe {
    /// 以结果序列创建假探测源。
    pub fn new(results: impl IntoIterator<Item = Option<bool>>) -> Self {
        Self {
            results: RefCell::new(results.into_iter().collect()),
            calls: Cell::new(0),
        }
    }

    /// 探测闭包（经 `&dyn Fn() -> Option<bool>` 注入运行选项）。
    pub fn closure(&self) -> impl Fn() -> Option<bool> + '_ {
        move || {
            self.calls.set(self.calls.get() + 1);
            self.results.borrow_mut().pop_front().unwrap_or(None)
        }
    }

    /// 实际调用次数。
    pub fn calls(&self) -> usize {
        self.calls.get()
    }
}

/// 视图切换键（`1`—`6`）。
pub fn view_switch_key(view: ViewId) -> char {
    char::from_digit(u32::try_from(view.index()).expect("序号在 u32 内"), 10).expect("十进制数字")
}

/// 六视图的代表性准备层数据（渲染证据夹具）。
pub fn fixture_snapshot(view: ViewId) -> ViewSnapshot {
    let mut snapshot = ViewSnapshot::empty(view);
    match view {
        ViewId::Sessions => {
            snapshot.sessions = vec![
                SessionRow {
                    id: "S-1".into(),
                    title: "实现会话树".into(),
                    stage: "plan".into(),
                    status: "active".into(),
                },
                SessionRow {
                    id: "S-2".into(),
                    title: "修复分页游标".into(),
                    stage: "implement".into(),
                    status: "idle".into(),
                },
                SessionRow {
                    id: "S-3".into(),
                    title: "归档旧会话".into(),
                    stage: "specify".into(),
                    status: "archived".into(),
                },
            ];
        }
        ViewId::Tasks => {
            snapshot.tasks = vec![
                TaskRow {
                    id: "T-101".into(),
                    title: "补齐索引".into(),
                    status: "running".into(),
                    retry_count: 0,
                },
                TaskRow {
                    id: "T-102".into(),
                    title: "重试导出".into(),
                    status: "failed".into(),
                    retry_count: 2,
                },
            ];
        }
        ViewId::Context => {
            snapshot.context_summary = "预算 4096/8192 token · 游标 seq=128 · 压缩 2 次".into();
        }
        ViewId::Events => {
            snapshot.events = vec![
                EventRow {
                    seq: 126,
                    kind: "StreamingChunk".into(),
                    summary: "chunk 1/3".into(),
                },
                EventRow {
                    seq: 127,
                    kind: "ToolCall".into(),
                    summary: "run tests".into(),
                },
                EventRow {
                    seq: 128,
                    kind: "GateResult".into(),
                    summary: "pass".into(),
                },
            ];
        }
        ViewId::Spec => {
            snapshot.spec_status = "阶段：design · 门禁 4/4 通过 · 产物 v3".into();
        }
        ViewId::Diff => {
            snapshot.diff = vec![
                DiffLine {
                    marker: '+',
                    text: "新增行：视图接线".into(),
                },
                DiffLine {
                    marker: '-',
                    text: "删除行：旧占位符".into(),
                },
                DiffLine {
                    marker: ' ',
                    text: "上下文行".into(),
                },
            ];
        }
    }
    snapshot
}

/// 绘制快照到固定尺寸缓冲（**真实帧路径**：有界投影 + `draw_frame`；不含文本提取，
/// 可作帧预算测量的对象）。文本层用默认 `zh-CN`。
pub fn draw_snapshot(snapshot: &ViewSnapshot, cols: u16, rows: u16, theme: Theme) -> Buffer {
    draw_snapshot_with(snapshot, cols, rows, theme, &I18n::default())
}

/// 以指定文本层绘制（FR-615 双语取证）。
pub fn draw_snapshot_with(
    snapshot: &ViewSnapshot,
    cols: u16,
    rows: u16,
    theme: Theme,
    texts: &I18n,
) -> Buffer {
    let backend = TestBackend::new(cols, rows);
    let mut terminal = Terminal::new(backend).expect("测试终端");
    let mut state = TuiState::new(cols, rows);
    state.handle(UiEvent::Key(view_switch_key(snapshot.view)));
    let lines = snapshot.render_lines_limited(usize::from(rows));
    terminal
        .draw(|frame| draw_frame(frame, &state, &lines, theme, texts))
        .expect("绘制测试帧");
    terminal.backend().buffer().clone()
}

/// 把缓冲单元提取为文本（逐行修剪行尾空白）——“截图”的可执行形式。
///
/// 宽字符（CJK/全角）在缓冲中占两个单元，第二个是被 `reset` 的占位单元；
/// 按符号显示宽度前进可跳过占位单元，避免在重建文本中插入终端上不存在的空隙。
pub fn buffer_text(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width);
    let mut out = String::new();
    for row in buffer.content().chunks(width) {
        let mut line = String::new();
        let mut x = 0;
        while x < row.len() {
            let symbol = row[x].symbol();
            line.push_str(symbol);
            x += ratatui::text::Line::from(symbol).width().max(1);
        }
        while line.ends_with(' ') {
            line.pop();
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// 以给定快照渲染固定尺寸帧文本（截图取证用；含文本提取，非帧路径）。
pub fn render_snapshot_text(snapshot: &ViewSnapshot, cols: u16, rows: u16) -> String {
    buffer_text(&draw_snapshot(snapshot, cols, rows, Theme::EinkDay))
}

/// 以夹具数据渲染某视图的固定尺寸帧文本。
pub fn render_fixture_frame(view: ViewId, cols: u16, rows: u16) -> String {
    render_snapshot_text(&fixture_snapshot(view), cols, rows)
}

/// 以空数据源渲染某视图的固定尺寸帧文本。
pub fn render_empty_frame(view: ViewId, cols: u16, rows: u16) -> String {
    render_snapshot_text(&ViewSnapshot::empty(view), cols, rows)
}

/// 渲染某视图并返回原始缓冲（主题色值断言用）。
pub fn frame_buffer(view: ViewId, cols: u16, rows: u16, theme: Theme) -> Buffer {
    draw_snapshot(&ViewSnapshot::empty(view), cols, rows, theme)
}
