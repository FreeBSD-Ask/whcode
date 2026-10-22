//! 终端会话抽象与生命周期守卫（tasks.md T024）。
//!
//! `TerminalSession` 把真实终端操作抽象为可替换接口：生产实现 `CrosstermSession` 走
//! alternate screen + raw mode；测试用替身记录调用顺序，从而在不触碰真实终端的前提下验证
//! “退出”与“异常也恢复”（V-003 的逻辑层证据）。

use std::io;

use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use whcode_domain::WhError;

use crate::event::{UiEvent, map_event};
use crate::io_error;

/// 一次终端会话：尺寸探测、进入/恢复、阻塞读事件。
///
/// 实现者负责让 `enter` 与 `restore` 成对使用；配对由 [`TerminalGuard`] 强制。
pub trait TerminalSession {
    /// 探测当前终端尺寸；无法探测（非 TTY）时返回 `None`。
    fn size(&self) -> Option<(u16, u16)>;
    /// 进入会话（alternate screen、raw mode 等）。
    fn enter(&mut self) -> Result<(), WhError>;
    /// 把终端恢复到进入前状态。
    fn restore(&mut self) -> Result<(), WhError>;
    /// 阻塞读取下一个归一化事件；输入流结束返回 `Ok(None)`。
    fn next_event(&mut self) -> Result<Option<UiEvent>, WhError>;
}

/// 生产实现：经 crossterm 操作标准输出。
#[derive(Debug, Default)]
pub struct CrosstermSession {
    entered: bool,
}

impl CrosstermSession {
    /// 创建一个尚未进入的会话。
    #[must_use]
    pub const fn new() -> Self {
        Self { entered: false }
    }
}

impl TerminalSession for CrosstermSession {
    fn size(&self) -> Option<(u16, u16)> {
        crossterm::terminal::size().ok()
    }

    fn enter(&mut self) -> Result<(), WhError> {
        enable_raw_mode().map_err(|err| io_error("启用 raw mode 失败", err))?;
        crossterm::execute!(io::stdout(), EnterAlternateScreen)
            .map_err(|err| io_error("进入备用屏幕失败", err))?;
        self.entered = true;
        Ok(())
    }

    fn restore(&mut self) -> Result<(), WhError> {
        // 离开备用屏幕与关闭 raw mode 都要尝试，避免其一失败时另一状态残留。
        let leave = crossterm::execute!(io::stdout(), LeaveAlternateScreen)
            .map_err(|err| io_error("离开备用屏幕失败", err));
        let raw = disable_raw_mode().map_err(|err| io_error("关闭 raw mode 失败", err));
        self.entered = false;
        leave.and(raw)
    }

    fn next_event(&mut self) -> Result<Option<UiEvent>, WhError> {
        loop {
            match crossterm::event::read() {
                Ok(event) => {
                    if let Some(ui) = map_event(event) {
                        return Ok(Some(ui));
                    }
                    // 与最小闭环无关的事件：跳过，继续阻塞等待下一个。
                }
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(err) => return Err(io_error("读取终端事件失败", err)),
            }
        }
    }
}

/// 终端生命周期守卫：构造即 `enter`，`Drop` 即 `restore`。
///
/// `Drop` 中的恢复失败**不** panic（panic 会掩盖原始错误或触发二次 panic），而是记录到
/// 守卫自身的状态，供 [`TerminalGuard::restore_attempted`] / [`TerminalGuard::restore_succeeded`]
/// 查询。
pub struct TerminalGuard<'a, S: TerminalSession> {
    session: &'a mut S,
    entered: bool,
    restore_attempted: bool,
    restore_succeeded: bool,
}

impl<'a, S: TerminalSession> TerminalGuard<'a, S> {
    /// 进入会话并返回守卫。`enter` 失败即返回 `Err`，此时会话未被进入，也无需恢复。
    pub fn enter(session: &'a mut S) -> Result<Self, WhError> {
        session.enter()?;
        Ok(Self {
            session,
            entered: true,
            restore_attempted: false,
            restore_succeeded: false,
        })
    }

    /// 只读访问底层会话。
    #[must_use]
    pub fn session(&self) -> &S {
        self.session
    }

    /// 可变访问底层会话（供事件循环读取事件）。
    pub fn session_mut(&mut self) -> &mut S {
        self.session
    }

    /// 立即恢复终端并记录结果；幂等——未进入或已尝试过恢复时为无操作。
    pub fn restore_now(&mut self) -> Result<(), WhError> {
        if !self.entered || self.restore_attempted {
            return Ok(());
        }
        self.restore_attempted = true;
        match self.session.restore() {
            Ok(()) => {
                self.restore_succeeded = true;
                self.entered = false;
                Ok(())
            }
            Err(err) => {
                self.restore_succeeded = false;
                Err(err)
            }
        }
    }

    /// 是否已尝试恢复终端。
    #[must_use]
    pub const fn restore_attempted(&self) -> bool {
        self.restore_attempted
    }

    /// 最近一次恢复是否成功（未尝试过时为 `false`）。
    #[must_use]
    pub const fn restore_succeeded(&self) -> bool {
        self.restore_succeeded
    }
}

impl<S: TerminalSession> Drop for TerminalGuard<'_, S> {
    fn drop(&mut self) {
        // 恢复失败不得 panic：结果已由 restore_now 记入自身状态，可经查询接口获知。
        let _ = self.restore_now();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::rc::Rc;

    use super::{TerminalGuard, TerminalSession};
    use crate::event::UiEvent;
    use whcode_domain::{ErrorCode, WhError};

    /// 一次会话调用记录。
    #[derive(Debug, Default)]
    struct Recorder {
        calls: Vec<&'static str>,
    }

    /// 测试替身：把 enter/restore 调用写入共享记录，并可配置恢复失败。
    struct FakeSession {
        recorder: Rc<RefCell<Recorder>>,
        fail_restore: bool,
        size: Option<(u16, u16)>,
        events: VecDeque<UiEvent>,
    }

    impl FakeSession {
        fn new(recorder: Rc<RefCell<Recorder>>) -> Self {
            Self {
                recorder,
                fail_restore: false,
                size: None,
                events: VecDeque::new(),
            }
        }

        fn failing_restore(recorder: Rc<RefCell<Recorder>>) -> Self {
            Self {
                fail_restore: true,
                ..Self::new(recorder)
            }
        }
    }

    impl TerminalSession for FakeSession {
        fn size(&self) -> Option<(u16, u16)> {
            self.size
        }

        fn enter(&mut self) -> Result<(), WhError> {
            self.recorder.borrow_mut().calls.push("enter");
            Ok(())
        }

        fn restore(&mut self) -> Result<(), WhError> {
            self.recorder.borrow_mut().calls.push("restore");
            if self.fail_restore {
                Err(WhError::new(ErrorCode::IoError, "模拟恢复失败"))
            } else {
                Ok(())
            }
        }

        fn next_event(&mut self) -> Result<Option<UiEvent>, WhError> {
            Ok(self.events.pop_front())
        }
    }

    #[test]
    fn guard_enters_then_restores_on_drop() {
        let recorder = Rc::new(RefCell::new(Recorder::default()));
        let mut session = FakeSession::new(Rc::clone(&recorder));
        {
            let guard = TerminalGuard::enter(&mut session).expect("enter 应成功");
            assert!(guard.session().size().is_none());
            assert_eq!(recorder.borrow().calls, vec!["enter"]);
        }
        assert_eq!(
            recorder.borrow().calls,
            vec!["enter", "restore"],
            "Drop 必须恢复终端"
        );
    }

    #[test]
    fn guard_restores_when_body_panics() {
        let recorder = Rc::new(RefCell::new(Recorder::default()));
        let mut session = FakeSession::new(Rc::clone(&recorder));
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _guard = TerminalGuard::enter(&mut session).expect("enter 应成功");
            panic!("模拟渲染循环 panic");
        }));
        assert!(result.is_err(), "闭包应传播 panic");
        assert_eq!(
            recorder.borrow().calls,
            vec!["enter", "restore"],
            "panic 展开也必须恢复终端"
        );
    }

    #[test]
    fn guard_records_failed_restore_without_panicking() {
        let recorder = Rc::new(RefCell::new(Recorder::default()));
        let mut session = FakeSession::failing_restore(Rc::clone(&recorder));
        let mut guard = TerminalGuard::enter(&mut session).expect("enter 应成功");
        let err = guard.restore_now().expect_err("恢复应失败");
        assert_eq!(err.code, ErrorCode::IoError);
        assert!(guard.restore_attempted());
        assert!(!guard.restore_succeeded());
        // 幂等：再次调用不再触发底层 restore。
        assert!(guard.restore_now().is_ok());
        drop(guard);
        assert_eq!(
            recorder.borrow().calls,
            vec!["enter", "restore"],
            "恢复只尝试一次且不 panic"
        );
    }

    #[test]
    fn guard_enter_failure_does_not_restore() {
        struct FailingEnter;
        impl TerminalSession for FailingEnter {
            fn size(&self) -> Option<(u16, u16)> {
                None
            }
            fn enter(&mut self) -> Result<(), WhError> {
                Err(WhError::new(ErrorCode::IoError, "模拟进入失败"))
            }
            fn restore(&mut self) -> Result<(), WhError> {
                panic!("未进入的会话不应被恢复");
            }
            fn next_event(&mut self) -> Result<Option<UiEvent>, WhError> {
                Ok(None)
            }
        }

        let mut session = FailingEnter;
        assert!(TerminalGuard::enter(&mut session).is_err());
    }
}
