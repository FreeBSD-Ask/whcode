//! 终端事件的最小抽象（tasks.md T024）。
//!
//! 逻辑层（`crate::state::TuiState`）只依赖本枚举，与真实终端解耦；由 `crate::session` 把
//! crossterm 事件映射到本类型，使键盘/resize 语义可在无终端环境下测试。

/// 归一化后的 UI 事件（最小闭环所需子集）。
///
/// 只覆盖最小闭环所需的三类事件：普通字符键、Ctrl-C 与窗口尺寸变化；鼠标、焦点、粘贴等
/// 暂不处理（映射时跳过）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    /// 一个可打印字符键（不含 Ctrl 组合）。
    Key(char),
    /// Ctrl-C（中断请求）。
    CtrlC,
    /// 终端尺寸变化。
    Resize {
        /// 新的列数。
        cols: u16,
        /// 新的行数。
        rows: u16,
    },
}

/// 把 crossterm 事件映射为 `UiEvent`。
///
/// 与最小闭环无关的事件（鼠标、焦点、粘贴、未映射按键）以及按键**释放**事件返回 `None`，
/// 由调用方跳过——忽略释放可避免一次按键被计数两次。
pub(crate) fn map_event(event: crossterm::event::Event) -> Option<UiEvent> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};

    match event {
        Event::Key(key) => {
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                return None;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c' | 'C'))
            {
                return Some(UiEvent::CtrlC);
            }
            match key.code {
                KeyCode::Char(c) => Some(UiEvent::Key(c)),
                _ => None,
            }
        }
        Event::Resize(cols, rows) => Some(UiEvent::Resize { cols, rows }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use super::{UiEvent, map_event};

    #[test]
    fn maps_printable_key() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(map_event(event), Some(UiEvent::Key('a')));
    }

    #[test]
    fn maps_control_c() {
        let event = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(map_event(event), Some(UiEvent::CtrlC));
    }

    #[test]
    fn maps_resize() {
        assert_eq!(
            map_event(Event::Resize(120, 40)),
            Some(UiEvent::Resize {
                cols: 120,
                rows: 40
            })
        );
    }

    #[test]
    fn ignores_key_release_and_other_events() {
        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        assert_eq!(map_event(release), None, "按键释放不应产生事件");
        assert_eq!(map_event(Event::FocusGained), None, "焦点事件无关最小闭环");
        let arrow = Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(map_event(arrow), None, "未映射按键不应产生事件");
    }
}
