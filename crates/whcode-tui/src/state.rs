//! TUI 逻辑状态与事件处理（tasks.md T024）。
//!
//! 纯逻辑：不触碰终端、不执行 I/O、不访问 `whcode-app`——从而可在无 TTY 环境下断言
//! 退出/尺寸/计数语义（V-003 的逻辑层证据）。

use crate::event::UiEvent;

/// 最小 TUI 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiState {
    quit: bool,
    cols: u16,
    rows: u16,
    keys_seen: usize,
}

impl TuiState {
    /// 以给定初始尺寸创建状态（尚未退出、尚未按键）。
    #[must_use]
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            quit: false,
            cols,
            rows,
            keys_seen: 0,
        }
    }

    /// 处理一个事件：`q` 与 Ctrl-C 请求退出，`Resize` 更新尺寸，其余字符键仅计数。
    pub fn handle(&mut self, event: UiEvent) {
        match event {
            UiEvent::Key('q') | UiEvent::CtrlC => self.quit = true,
            UiEvent::Key(_) => self.keys_seen += 1,
            UiEvent::Resize { cols, rows } => {
                self.cols = cols;
                self.rows = rows;
            }
        }
    }

    /// 是否已请求退出。
    #[must_use]
    pub const fn quit(&self) -> bool {
        self.quit
    }

    /// 当前列数。
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// 当前行数。
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// 已接收的非退出字符键数量。
    #[must_use]
    pub const fn keys_seen(&self) -> usize {
        self.keys_seen
    }
}

#[cfg(test)]
mod tests {
    use super::TuiState;
    use crate::event::UiEvent;

    #[test]
    fn key_q_requests_quit() {
        let mut state = TuiState::new(80, 24);
        state.handle(UiEvent::Key('q'));
        assert!(state.quit());
        assert_eq!(state.keys_seen(), 0, "退出键不计入普通按键");
    }

    #[test]
    fn ctrl_c_requests_quit() {
        let mut state = TuiState::new(80, 24);
        state.handle(UiEvent::CtrlC);
        assert!(state.quit());
        assert_eq!(state.keys_seen(), 0);
    }

    #[test]
    fn resize_updates_dimensions() {
        let mut state = TuiState::new(80, 24);
        state.handle(UiEvent::Resize {
            cols: 200,
            rows: 50,
        });
        assert_eq!(state.cols(), 200);
        assert_eq!(state.rows(), 50);
        assert!(!state.quit(), "resize 不应触发退出");
    }

    #[test]
    fn other_keys_only_count_without_quitting() {
        let mut state = TuiState::new(80, 24);
        state.handle(UiEvent::Key('a'));
        state.handle(UiEvent::Key('b'));
        assert_eq!(state.keys_seen(), 2);
        assert!(!state.quit());
        assert_eq!((state.cols(), state.rows()), (80, 24), "按键不改变尺寸");
    }
}
