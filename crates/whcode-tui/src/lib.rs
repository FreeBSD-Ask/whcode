//! # whcode-tui
//!
//! ratatui/crossterm 终端界面：会话树、任务、上下文、事件、Spec 状态、diff 预览（FR-601）。
//!
//! **当前状态：M0 最小闭环原型已实现（tasks.md T024 / V-003 的逻辑层）。** 已交付：终端
//! 尺寸探测、进入/恢复的生命周期守卫、键盘输入、resize、退出与异常恢复的可测试核心，以及
//! 由主程序装配的真实事件循环。六视图与渲染纪律已交付（T082/T083/T129：[`views`]/
//! [`render`]——只读快照、脏标记重绘、空闲期零重绘、不在帧路径反序列化 JSON）。
//!
//! 渲染纪律（§5.3）：渲染线程只读取不可变快照或轻量状态，**不**执行 LLM 网络 I/O、阻塞
//! 数据库查询或长时间工具调用；以脏标记或事件驱动触发重绘；不在渲染帧路径反序列化完整
//! JSON payload。所有状态变更经 `whcode-app` 命令，本 crate 不得直接触库（ADR-003）。

mod event;
mod render;
mod session;
mod state;
mod views;

use std::io;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use whcode_domain::{ErrorCode, WhError};

pub use event::UiEvent;
pub use render::{RenderEvent, Renderer};
pub use session::{CrosstermSession, TerminalGuard, TerminalSession};
pub use state::TuiState;
pub use views::{DiffLine, EventRow, SessionRow, TaskRow, ViewId, ViewSnapshot};

/// 探测不到尺寸时使用的兜底尺寸（列, 行）。
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// 把 I/O 错误包装为结构化 `WhError`。
pub(crate) fn io_error(action: &str, err: io::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{action}：{err}"))
}

/// 启动最小 TUI 闭环：进入会话、画一帧、按事件循环直至退出（tasks.md T024、FR-601）。
///
/// 终端生命周期由 [`TerminalGuard`] 管理——任何提前返回或 panic 都会触发恢复。
///
/// # Errors
///
/// 进入终端、初始化渲染后端、绘制或读取事件失败时返回 `WhError`（`io_error`）。
pub fn run() -> Result<(), WhError> {
    let mut session = CrosstermSession::new();
    let mut guard = TerminalGuard::enter(&mut session)?;

    let (cols, rows) = guard.session().size().unwrap_or(DEFAULT_SIZE);
    let mut state = TuiState::new(cols, rows);

    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|err| io_error("初始化终端失败", err))?;

    while !state.quit() {
        terminal
            .draw(|frame| draw_frame(frame, &state))
            .map_err(|err| io_error("绘制界面失败", err))?;
        match guard.session_mut().next_event()? {
            Some(event) => state.handle(event),
            // 输入流结束（例如管道关闭）：退出循环，终端交由 guard 恢复。
            None => break,
        }
    }
    Ok(())
}

/// 无终端重放：按顺序把事件喂给 [`TuiState`]，返回最终状态报告（供自动化证据）。
///
/// 初始尺寸未知，置 `(0, 0)`，由 `Resize` 事件更新。循环在 `quit` 置位时停止，因此
/// `events_consumed` 可能小于 `events.len()`——与真实事件循环“退出即停”的语义保持一致。
#[must_use]
pub fn run_scripted(events: &[UiEvent]) -> ScriptReport {
    let mut state = TuiState::new(0, 0);
    let mut events_consumed = 0;
    for event in events {
        state.handle(*event);
        events_consumed += 1;
        if state.quit() {
            break;
        }
    }
    ScriptReport {
        quit: state.quit(),
        cols: state.cols(),
        rows: state.rows(),
        keys_seen: state.keys_seen(),
        events_consumed,
    }
}

/// [`run_scripted`] 的最终状态报告。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptReport {
    /// 结束时是否已请求退出。
    pub quit: bool,
    /// 结束时的列数。
    pub cols: u16,
    /// 结束时的行数。
    pub rows: u16,
    /// 已接收的非退出字符键数量。
    pub keys_seen: usize,
    /// 实际消费的事件数量（`quit` 置位后不再消费）。
    pub events_consumed: usize,
}

/// 画最小一帧：标题、当前状态与退出提示。
fn draw_frame(frame: &mut ratatui::Frame<'_>, state: &TuiState) {
    let lines = vec![
        Line::from(format!(
            "窗口尺寸：{} 列 × {} 行",
            state.cols(),
            state.rows()
        )),
        Line::from(format!("已接收按键（不含退出键）：{}", state.keys_seen())),
        Line::from("按 q 或 Ctrl-C 退出"),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title("whcode TUI —— M0 最小闭环原型");
    frame.render_widget(Paragraph::new(lines).block(block), frame.area());
}

#[cfg(test)]
mod tests {
    use super::{UiEvent, run_scripted};

    #[test]
    fn scripted_run_consumes_all_events_when_not_quitting() {
        let events = [
            UiEvent::Resize {
                cols: 120,
                rows: 40,
            },
            UiEvent::Key('a'),
            UiEvent::Key('b'),
        ];
        let report = run_scripted(&events);
        assert!(!report.quit);
        assert_eq!((report.cols, report.rows), (120, 40));
        assert_eq!(report.keys_seen, 2);
        assert_eq!(
            report.events_consumed,
            events.len(),
            "无退出事件时应消费全部事件"
        );
    }

    #[test]
    fn scripted_run_stops_at_quit_event() {
        let events = [
            UiEvent::Key('a'),
            UiEvent::Key('q'),
            UiEvent::Resize { cols: 10, rows: 5 },
        ];
        let report = run_scripted(&events);
        assert!(report.quit);
        assert_eq!(report.keys_seen, 1);
        assert_eq!(report.events_consumed, 2, "退出后不得继续消费事件");
        assert_eq!((report.cols, report.rows), (0, 0), "退出后的 Resize 不生效");
    }

    #[test]
    fn scripted_run_reports_empty_input_unchanged() {
        let report = run_scripted(&[]);
        assert!(!report.quit);
        assert_eq!(report.events_consumed, 0);
        assert_eq!((report.cols, report.rows, report.keys_seen), (0, 0, 0));
    }
}
