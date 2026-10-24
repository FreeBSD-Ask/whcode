//! TUI 逻辑状态与事件处理（tasks.md T024、T082）。
//!
//! 纯逻辑：不触碰终端、不执行 I/O、不访问 `whcode-app`——从而可在无 TTY 环境下断言
//! 退出/尺寸/计数/视图切换语义（V-003 的逻辑层证据）。

use crate::event::UiEvent;
use crate::views::ViewId;

/// 最小 TUI 状态（含当前视图选择，T082）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiState {
    quit: bool,
    cols: u16,
    rows: u16,
    keys_seen: usize,
    view: ViewId,
}

impl TuiState {
    /// 以给定初始尺寸创建状态（尚未退出、尚未按键、视图为会话）。
    #[must_use]
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            quit: false,
            cols,
            rows,
            keys_seen: 0,
            view: ViewId::Sessions,
        }
    }

    /// 处理一个事件：`q` 与 Ctrl-C 请求退出；`1`—`6` 切换视图（导航键，不计入普通
    /// 按键）；`Resize` 更新尺寸；其余字符键仅计数（Enter/Esc/Backspace 由写操作
    /// 模式在 LoopCore 消费，Normal 态落到计数）。
    pub fn handle(&mut self, event: UiEvent) {
        match event {
            UiEvent::Key('q') | UiEvent::CtrlC => self.quit = true,
            UiEvent::Key(key) => match ViewId::from_key(key) {
                Some(view) => self.view = view,
                None => self.keys_seen += 1,
            },
            UiEvent::Enter | UiEvent::Esc | UiEvent::Backspace => self.keys_seen += 1,
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

    /// 已接收的非退出、非导航字符键数量。
    #[must_use]
    pub const fn keys_seen(&self) -> usize {
        self.keys_seen
    }

    /// 当前视图（六视图之一，FR-601）。
    #[must_use]
    pub const fn view(&self) -> ViewId {
        self.view
    }
}

#[cfg(test)]
mod tests {
    use super::TuiState;
    use crate::event::UiEvent;
    use crate::views::ViewId;

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
        assert_eq!(state.view(), ViewId::Sessions, "resize 不应改变视图");
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

    /// T082：`1`—`6` 依次切换到六视图，且导航键不计入普通按键计数。
    #[test]
    fn digit_keys_switch_views_in_fixed_order() {
        let mut state = TuiState::new(80, 24);
        assert_eq!(state.view(), ViewId::Sessions, "默认视图为会话");
        for (offset, view) in ViewId::ALL.into_iter().enumerate() {
            let key = char::from_digit(u32::try_from(offset + 1).expect("序号在 u32 内"), 10)
                .expect("十进制数字");
            state.handle(UiEvent::Key(key));
            assert_eq!(state.view(), view, "键 {key} 应切换到 {view:?}");
        }
        assert_eq!(state.keys_seen(), 0, "导航键不计入普通按键");
        assert!(!state.quit(), "视图切换不应触发退出");
    }

    /// 非导航键（如 `0`、`7`）仍是普通按键：计数且不改变视图。
    #[test]
    fn non_navigation_digits_do_not_switch_views() {
        let mut state = TuiState::new(80, 24);
        state.handle(UiEvent::Key('3'));
        state.handle(UiEvent::Key('0'));
        state.handle(UiEvent::Key('7'));
        assert_eq!(state.view(), ViewId::Context, "仅 3 切换视图");
        assert_eq!(state.keys_seen(), 2, "0 与 7 是普通按键");
    }
}
