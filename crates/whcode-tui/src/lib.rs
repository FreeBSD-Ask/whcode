//! # whcode-tui
//!
//! ratatui/crossterm 终端界面：会话树、任务、上下文、事件、Spec 状态、diff 预览（FR-601）。
//!
//! **当前状态：六视图与渲染纪律已接入真实运行循环（T082/T083/T129/T135 接线）。**
//! 已交付并接线：终端尺寸探测、进入/恢复的生命周期守卫、键盘输入（含 `1`—`6` 视图
//! 切换）、resize、退出与异常恢复；六视图（[`views`]）经 [`ViewSource`] 准备层供给
//! 只读快照；[`Renderer`] 脏标记门控重绘（空闲期零重绘，T129）；窗口最小化探测
//! （[`RunOptions::minimize_probe`]，T135/ADR-021）在最小化期间跳过绘制、还原后
//! 单次全量重绘；主题精确色值（[`theme`]，FR-612）经 [`RunOptions::theme`] 应用。
//!
//! 渲染纪律（§5.3）：渲染线程只读取不可变快照或轻量状态，**不**执行 LLM 网络 I/O、阻塞
//! 数据库查询或长时间工具调用；以脏标记或事件驱动触发重绘；不在渲染帧路径反序列化完整
//! JSON payload。所有状态变更经 `whcode-app` 命令，本 crate 不得直接触库（ADR-003）。
//!
//! 数据接线边界（诚实标注）：本 crate 只交付**视图与渲染接线**；六视图的业务数据由装配根
//! 经 [`ViewSource`] 注入。`whcode-bin` 打开工作区库后优先注入 `StoreViewSource`（真实
//! 数据）；仅当库打开失败时回退 [`EmptyViewSource`] 并打印提示，此时帧内显式显示
//! “暂无数据”。
//!
//! 渲染后端（2026-10-23）：不用 crossterm 后端的逐格绝对定位，改用
//! [`rowbackend::RowBackend`] 的**整行连续输出**。原因是 V-003 实测发现 Windows 控制台
//! 把 East Asian Ambiguous 宽度字符（`·`、`×`、`—` 等）渲染为 2 格，而 `unicode-width`
//! 判为 1 格，逐格定位会把后续字符写进尾格而擦掉整字（界面上“按键”显示成“键”）。
//! 详见 `docs/guides/tui.md` §7 与 `artifacts/tui-v003-2026-10-23/`。

mod event;
mod render;
mod rowbackend;
mod session;
mod state;
pub mod surface_map;
mod theme;

/// 只读视图模型已下沉到应用层（T150）：TUI 与原生 GUI 共用同一份快照契约。
pub use whcode_app::view_source as views;

#[cfg(test)]
mod tests_support;

use std::io;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use whcode_app::commands::{CommandExecutor, FrontendCommand};
use whcode_domain::i18n::I18n;
use whcode_domain::{ErrorCode, WhError};

use crate::rowbackend::RowBackend;

pub use event::UiEvent;
pub use render::{RenderEvent, Renderer};
pub use session::{CrosstermSession, PollOutcome, TerminalGuard, TerminalSession};
pub use state::TuiState;
pub use theme::Theme;
pub use views::{DiffLine, EventRow, SessionRow, TaskRow, VIEW_COUNT, ViewId, ViewSnapshot};

/// 探测不到尺寸时使用的兜底尺寸（列, 行）。
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// 事件轮询超时：空闲唤醒粒度。无事件时也周期性回到循环头（心跳与低频探测都在此处）。
const POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// 输入静默阈值：键/resize 事件本身即证明窗口可见；静默 ≥ 此值后才值得探测最小化。
const QUIET_BEFORE_PROBE: Duration = Duration::from_secs(1);

/// 最小化探测的最小间隔（ADR-021：≥ 5s——探测成本 200—500ms/次，不得高频调用）；
/// 检测到最小化后回落此值，以尽快发现还原。
const MINIMIZE_PROBE_INTERVAL: Duration = Duration::from_secs(5);

/// 最小化探测的退避上限：连续“未最小化”结果后最多 60s 探一次
/// （≈ 0.5% 单核占用的上界，NFR-205 空闲 CPU 预算之内；空闲无待绘制帧时完全不探测）。
const MAX_PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// 无终端重放模式的模拟时钟步长（每步 1s；使 ≥1s/≥5s 之类阈值在重放中可复现）。
const SCRIPT_STEP: Duration = Duration::from_secs(1);

/// 把 I/O 错误包装为结构化 `WhError`。
pub(crate) fn io_error(action: &str, err: io::Error) -> WhError {
    WhError::new(ErrorCode::IoError, format!("{action}：{err}"))
}

/// 视图数据源与空数据源由应用层定义（T150），此处转出以维持既有调用点。
pub use whcode_app::view_source::{EmptyViewSource, ViewSource};

/// TUI 运行选项。
#[derive(Default)]
pub struct RunOptions<'a> {
    /// 渲染主题（FR-612）；默认 `eink-day`（ui-design 默认值：最保守、最省电）。
    pub theme: Theme,
    /// 界面文本层（FR-615）：默认 `zh-CN` + `en` 回退（`I18n::default()`）。
    pub texts: I18n,
    /// 窗口最小化探测（T135/FR-617）：`Some` 时由事件循环低频调用
    /// （仅在有**待绘制帧**且输入静默 ≥1s 时；间隔 5s→60s 退避，ADR-021）。
    /// `None` = 不探测（门控保持“未最小化”，行为与接线前一致）。
    pub minimize_probe: Option<&'a dyn Fn() -> Option<bool>>,
    /// 写命令执行器（FR-623/T177）：`Some` 时会话/任务写操作可用；`None` 时写动作
    /// 显式提示 `not_supported`（不静默无响应）。
    pub command_executor: Option<&'a dyn CommandExecutor>,
}

/// TUI 写操作输入模式（T177）：Normal 为只读浏览；Text 收集单行输入
/// （new/rename）；Confirm 持有待确认命令（delete/purge 两段确认，取消不改状态）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum InputMode {
    Normal,
    /// 单行输入：提示键（新增/重命名）与缓冲。
    Text {
        kind: TextKind,
        buffer: String,
    },
    /// 待确认的破坏性命令（预览已执行且显示在状态行；二次确认才执行）。
    Confirm {
        command: FrontendCommand,
        label: String,
    },
}

/// 单行输入的用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextKind {
    /// 新建会话标题。
    SessionNew,
    /// 重命名当前选中会话。
    SessionRename,
}

/// TUI 游标检查间隔（FR-624 量化判据）：≥1000 ms，低频有界。
pub const CURSOR_CHECK_INTERVAL: Duration = Duration::from_millis(1000);

/// 清理预览的默认截止窗口（30 天，毫秒）——TUI purge 预览取“早于 30 天”。
const PURGE_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// 当前 Unix 毫秒（写命令时钟注入；失败回退 0——命令会因时间倒挂被保护逻辑拒，
/// 不 panic）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 启动 TUI 闭环：默认数据源（空）、默认主题、不探测最小化（tasks.md T024、FR-601）。
///
/// # Errors
///
/// 进入终端、初始化渲染后端、绘制或读取事件失败时返回 `WhError`（`io_error`）。
pub fn run() -> Result<(), WhError> {
    run_with(&EmptyViewSource, RunOptions::default())
}

/// 启动 TUI 闭环：装配根注入数据源、主题与最小化探测。
///
/// 终端生命周期由 [`TerminalGuard`] 管理——任何提前返回或 panic 都会触发恢复。
///
/// # Errors
///
/// 进入终端、初始化渲染后端、绘制或读取事件失败时返回 `WhError`（`io_error`）。
pub fn run_with(source: &dyn ViewSource, options: RunOptions<'_>) -> Result<(), WhError> {
    let mut session = CrosstermSession::new();
    let mut guard = TerminalGuard::enter(&mut session)?;

    let (cols, rows) = guard.session().size().unwrap_or(DEFAULT_SIZE);
    let mut core = LoopCore::new(
        source,
        options.command_executor,
        options.theme,
        cols,
        rows,
        Instant::now(),
    );

    let mut terminal = Terminal::new(RowBackend::new(io::stdout()))
        .map_err(|err| io_error("初始化终端失败", err))?;

    while !core.state.quit() {
        let now = Instant::now();
        // 低频最小化探测（ADR-021：非帧路径；仅待绘制帧 + 输入静默时）。
        if let Some(probe) = options.minimize_probe {
            core.maybe_probe(probe, now);
        }
        // 脏标记门控：仅脏且未最小化时绘制（T083/T129/T135）。
        if let Some(mut lines) = core.render_frame() {
            if !core.status.is_empty() {
                lines.push(core.status.clone());
            }
            let theme = core.theme;
            terminal
                .draw(|frame| draw_frame(frame, &core.state, &lines, theme, &options.texts))
                .map_err(|err| io_error("绘制界面失败", err))?;
        }
        match guard.session_mut().poll_event(POLL_TIMEOUT)? {
            PollOutcome::Event(event) => core.on_input(event, Instant::now()),
            PollOutcome::Timeout => core.on_idle(Instant::now()),
            // 输入流结束（例如管道关闭）：退出循环，终端交由 guard 恢复。
            PollOutcome::Eof => break,
        }
    }
    Ok(())
}

/// 无终端重放的一个步骤（供自动化证据；与真实循环共用 [`LoopCore`] 逻辑）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptStep {
    /// 输入事件（键盘/resize；收到输入即证明窗口可见，直接置脏绘制且不触发探测）。
    Input(UiEvent),
    /// 数据变更通知（模拟应用服务数据通道：**不**视为输入——需经最小化探测门控）。
    DataChanged,
    /// 空闲心跳（一次轮询超时；不置脏——T129）。
    Idle,
}

/// 无终端重放：按步骤驱动与真实循环相同的核心逻辑，返回最终报告（供自动化证据）。
///
/// 时间模型：初始时刻为创建时，每步前进 [`SCRIPT_STEP`]（1s）；每步之后执行
/// “最小化探测判断 → 渲染判断”，与真实循环的循环体顺序一致。`quit` 置位后
/// 不再消费剩余步骤（与真实循环“退出即停”语义一致）。
#[must_use]
pub fn run_scripted_steps(
    source: &dyn ViewSource,
    steps: &[ScriptStep],
    options: &RunOptions<'_>,
) -> ScriptReport {
    let mut now = Instant::now();
    let mut core = LoopCore::new(source, options.command_executor, options.theme, 0, 0, now);
    let mut events_consumed = 0;
    let mut steps_consumed = 0;
    for step in steps {
        if core.state.quit() {
            break;
        }
        now += SCRIPT_STEP;
        match step {
            ScriptStep::Input(event) => {
                core.on_input(*event, now);
                events_consumed += 1;
            }
            ScriptStep::DataChanged => core.on_data_changed(),
            ScriptStep::Idle => core.on_idle(now),
        }
        steps_consumed += 1;
        if let Some(probe) = options.minimize_probe {
            core.maybe_probe(probe, now);
        }
        let _ = core.render_frame();
    }
    core.report(events_consumed, steps_consumed)
}

/// 无终端重放（输入事件便捷入口）：等价于把每个事件作为 [`ScriptStep::Input`]。
#[must_use]
pub fn run_scripted(events: &[UiEvent]) -> ScriptReport {
    let steps: Vec<ScriptStep> = events
        .iter()
        .map(|event| ScriptStep::Input(*event))
        .collect();
    run_scripted_steps(&EmptyViewSource, &steps, &RunOptions::default())
}

/// [`run_scripted_steps`] 的最终状态报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptReport {
    /// 结束时是否已请求退出。
    pub quit: bool,
    /// 结束时的列数。
    pub cols: u16,
    /// 结束时的行数。
    pub rows: u16,
    /// 已接收的非退出、非导航字符键数量。
    pub keys_seen: usize,
    /// 实际消费的输入事件数量（`quit` 置位后不再消费）。
    pub events_consumed: usize,
    /// 实际消费的重放步骤数量（含输入/数据变更/空闲）。
    pub steps_consumed: usize,
    /// 结束时的当前视图。
    pub view: ViewId,
    /// 结束时会话清单条数（准备层返回）。
    pub sessions: usize,
    /// 结束时会话选择序号（`j`/`k` 选择）。
    pub session_cursor: usize,
    /// 结束时会话选择（会话 ID；空清单为 `None`）。
    pub selected_session: Option<String>,
    /// 实际产生的重绘次数（渲染门控放行的帧数）。
    pub redraws: usize,
    /// 结束时的当前主题（T178.1 `t` 循环）。
    pub theme: Theme,
    /// 最近一次动作反馈行（T177 写操作/取消提示；空为无）。
    pub status: String,
    /// 最小化期间跳过绘制的累计次数（FR-617）。
    pub skipped_while_minimized: usize,
    /// 最小化探测的实际调用次数（T135/ADR-021 成本证据）。
    pub probes: usize,
    /// 结束时是否处于最小化门控。
    pub minimized: bool,
}

/// 事件循环核心：状态 + 渲染器 + 视图快照 + 会话选择 + 最小化探测时序（无终端依赖）。
///
/// 真实循环（[`run_with`]）与无终端重放（[`run_scripted_steps`]）共用本结构——
/// 保证重放证据与真实行为同源，而不是另写一套仿真。
struct LoopCore<'a> {
    state: TuiState,
    renderer: Renderer,
    snapshot: ViewSnapshot,
    source: &'a dyn ViewSource,
    /// 当前主题（T178.1）：`t` 键四套循环，绘制使用当前主题而非固定 RunOptions 值。
    theme: Theme,
    /// 写命令执行器（FR-623/T177）；`None` = 写动作显式不可用。
    executor: Option<&'a dyn CommandExecutor>,
    /// 会话清单（每次准备时刷新；供 `j`/`k` 选择与边界钳制）。
    sessions: Vec<SessionRow>,
    /// 会话选择序号（`usize::MAX` 等越界值在准备时钳制为 0）。
    session_cursor: usize,
    /// 当前选中的会话 ID（由 `sessions[session_cursor]` 解析）。
    selected_session: Option<String>,
    /// 任务视图选中序号（`R`/`A` 作用于选中任务，T177）。
    task_cursor: usize,
    /// 输入模式（只读浏览 / 单行输入 / 破坏性确认）。
    input_mode: InputMode,
    /// 动作反馈行（追加在帧尾；空则不占行）。
    status: String,
    /// 最近一次观测的持久化游标（FR-624）：推进才置脏。
    last_cursor: Option<i64>,
    /// 上次游标检查时刻（间隔 [`CURSOR_CHECK_INTERVAL`]）。
    last_cursor_check: Instant,
    last_input_at: Instant,
    last_probe_at: Instant,
    probe_interval: Duration,
    probes: usize,
    redraws: usize,
    skipped: usize,
}

impl<'a> LoopCore<'a> {
    fn new(
        source: &'a dyn ViewSource,
        executor: Option<&'a dyn CommandExecutor>,
        theme: Theme,
        cols: u16,
        rows: u16,
        now: Instant,
    ) -> Self {
        let state = TuiState::new(cols, rows);
        let last_cursor = source.cursor(None);
        let mut core = Self {
            snapshot: ViewSnapshot::empty(state.view()),
            state,
            renderer: Renderer::new(),
            source,
            theme,
            executor,
            sessions: Vec::new(),
            session_cursor: 0,
            selected_session: None,
            task_cursor: 0,
            input_mode: InputMode::Normal,
            status: String::new(),
            last_cursor,
            last_cursor_check: now,
            last_input_at: now,
            last_probe_at: now,
            probe_interval: MINIMIZE_PROBE_INTERVAL,
            probes: 0,
            redraws: 0,
            skipped: 0,
        };
        core.prepare();
        core
    }

    /// 重新准备快照（准备层）：刷新会话清单 → 钳制选择 → 取当前视图快照 → 置脏。
    ///
    /// 查询发生在这里（视图切换/会话选择/显式刷新的边界），帧路径只消费快照（§5.3）。
    fn prepare(&mut self) {
        let sessions_snapshot = self.source.snapshot(ViewId::Sessions, None);
        self.sessions = sessions_snapshot.sessions.clone();
        if self.sessions.is_empty() {
            self.session_cursor = 0;
        } else if self.session_cursor >= self.sessions.len() {
            self.session_cursor = self.sessions.len() - 1;
        }
        self.selected_session = self
            .sessions
            .get(self.session_cursor)
            .map(|row| row.id.clone());
        self.snapshot = if self.state.view() == ViewId::Sessions {
            sessions_snapshot
        } else {
            self.source
                .snapshot(self.state.view(), self.selected_session.as_deref())
        };
        // 选中序号按视图归属：会话视图选中会话行；任务视图选中任务行（T177 写操作目标）。
        self.snapshot.selected_index = if self.state.view() == ViewId::Tasks {
            let max = self.snapshot.tasks.len();
            if max == 0 {
                None
            } else {
                if self.task_cursor >= max {
                    self.task_cursor = max - 1;
                }
                Some(self.task_cursor)
            }
        } else {
            Some(self.session_cursor)
        };
        self.renderer.handle_event(RenderEvent::DataChanged);
    }

    /// 输入事件（键/resize）：更新状态并置脏；收到输入即证明窗口可见——若此前处于
    /// 最小化门控则按“还原”记账（还原后单次全量重绘，FR-617）。视图切换时重新
    /// 准备快照（准备层允许做同步整理，但不得做 LLM/长工具调用，§5.3）。
    ///
    /// 导航键（`j`/`k` 选择、`r` 刷新）与写操作键（T177）由本层消费，不计入普通按键计数。
    fn on_input(&mut self, event: UiEvent, now: Instant) {
        self.last_input_at = now;
        if self.renderer.is_minimized() {
            self.renderer.handle_event(RenderEvent::Restored);
        }
        match event {
            UiEvent::Key('j') if self.input_mode == InputMode::Normal => {
                self.session_cursor = self.session_cursor.saturating_add(1);
                self.prepare();
            }
            UiEvent::Key('k') if self.input_mode == InputMode::Normal => {
                self.session_cursor = self.session_cursor.saturating_sub(1);
                self.prepare();
            }
            UiEvent::Key('r') if self.input_mode == InputMode::Normal => self.prepare(),
            UiEvent::Resize { .. } => self.state.handle(event),
            _ => self.on_key(event, now),
        }
        self.renderer.handle_event(RenderEvent::DataChanged);
    }

    /// 写操作与模式键（T177）：Text 收集输入、Confirm 二段确认、Normal 触发动作。
    /// 无执行器时动作显式提示 `not_supported`（不静默无响应，FR-620）。
    fn on_key(&mut self, event: UiEvent, now: Instant) {
        // Ctrl-C 在任何模式都请求退出（安全出口不因模式屏蔽）。
        if event == UiEvent::CtrlC {
            self.state.handle(event);
            return;
        }
        let mode = std::mem::replace(&mut self.input_mode, InputMode::Normal);
        match (mode, event) {
            (InputMode::Normal, event) => self.on_key_normal(event, now),
            (InputMode::Text { kind, mut buffer }, event) => match event {
                UiEvent::Enter => {
                    if buffer.trim().is_empty() {
                        self.status = "输入为空：已取消".to_string();
                        return;
                    }
                    let command = match kind {
                        TextKind::SessionNew => FrontendCommand::SessionNew {
                            title: buffer.trim().to_string(),
                            model: None,
                            now_ms: now_ms(),
                        },
                        TextKind::SessionRename => {
                            let Some(session_id) = self.selected_session.clone() else {
                                self.status = "重命名：未选择会话".to_string();
                                return;
                            };
                            FrontendCommand::SessionRename {
                                session_id,
                                title: buffer.trim().to_string(),
                                now_ms: now_ms(),
                            }
                        }
                    };
                    self.execute(command, "会话写入");
                }
                UiEvent::Backspace => {
                    buffer.pop();
                    self.status = format!("输入：{buffer}_（Enter 提交，Esc 取消）");
                    self.input_mode = InputMode::Text { kind, buffer };
                }
                UiEvent::Esc => {
                    self.status = "已取消".to_string();
                }
                UiEvent::Key(ch) if !ch.is_control() && self.buffer_room(&buffer) => {
                    buffer.push(ch);
                    self.status = format!("输入：{buffer}_（Enter 提交，Esc 取消）");
                    self.input_mode = InputMode::Text { kind, buffer };
                }
                other => {
                    self.state.handle(other);
                    self.input_mode = InputMode::Text { kind, buffer };
                }
            },

            (InputMode::Confirm { command, label }, event) => match event {
                UiEvent::Key('y') | UiEvent::Key('Y') => {
                    self.execute(command, &label);
                }
                other => {
                    self.status = format!("{label} 已取消（状态不变）");
                    self.state.handle(other);
                }
            },
        }
    }

    /// 单行输入缓冲上限（有界输入，NFR-215）。
    fn buffer_room(&self, buffer: &str) -> bool {
        buffer.chars().count() < 200
    }

    /// 经门面执行写命令（FR-623）：成功即重新准备快照（在线可见同步）；失败写状态行。
    fn execute(&mut self, command: FrontendCommand, label: &str) {
        let Some(executor) = self.executor else {
            self.status = format!("{label}: not_supported（未注入写命令执行器——装配根未提供）");
            return;
        };
        match executor.execute(command) {
            Ok(_) => {
                self.status = format!("{label} 完成");
                self.prepare();
            }
            Err(err) => {
                self.status = format!("{label} 失败：{err}");
            }
        }
    }

    /// Normal 模式动作键（T177）：会话视图 n/m/d/A，任务视图 R/X，任意视图 p/t。
    /// 键位避开既有证据键（q/1—6/j/k/r 与 B-05 计数键 a/b，ui-design）。
    fn on_key_normal(&mut self, key: UiEvent, now: Instant) {
        let _ = now;
        match key {
            UiEvent::Key('n') if self.state.view() == ViewId::Sessions => {
                self.input_mode = InputMode::Text {
                    kind: TextKind::SessionNew,
                    buffer: String::new(),
                };
                self.status = "新会话标题：_（Enter 提交，Esc 取消）".to_string();
            }
            UiEvent::Key('m') if self.state.view() == ViewId::Sessions => {
                if self.selected_session.is_some() {
                    self.input_mode = InputMode::Text {
                        kind: TextKind::SessionRename,
                        buffer: String::new(),
                    };
                    self.status = "新标题：_（Enter 提交，Esc 取消）".to_string();
                } else {
                    self.status = "重命名：未选择会话".to_string();
                }
            }
            UiEvent::Key('d') if self.state.view() == ViewId::Sessions => {
                let Some(session_id) = self.selected_session.clone() else {
                    self.status = "删除：未选择会话".to_string();
                    return;
                };
                self.request_confirm(
                    FrontendCommand::SessionDelete {
                        session_id,
                        confirm: true,
                        now_ms: now_ms(),
                    },
                    "删除会话",
                );
            }
            UiEvent::Key('A') if self.state.view() == ViewId::Sessions => {
                let Some(session_id) = self.selected_session.clone() else {
                    self.status = "归档：未选择会话".to_string();
                    return;
                };
                self.request_confirm(
                    FrontendCommand::SessionArchive {
                        session_id,
                        archived: true,
                        confirm: true,
                        now_ms: now_ms(),
                    },
                    "归档会话",
                );
            }
            UiEvent::Key('R') if self.state.view() == ViewId::Tasks => {
                self.task_action(false);
            }
            UiEvent::Key('X') if self.state.view() == ViewId::Tasks => {
                self.task_action(true);
            }
            UiEvent::Key('t') => {
                // 主题运行时循环（T178.1/FR-625）：四套依序循环，不重启。
                self.theme = self.theme.next();
                self.status = format!("主题：{}", self.theme.as_str());
                self.renderer.handle_event(RenderEvent::DataChanged);
            }
            UiEvent::Key('p') => {
                self.request_confirm(
                    FrontendCommand::PurgeExecute {
                        before_ms: now_ms()
                            .saturating_sub(u64::try_from(PURGE_WINDOW_MS).unwrap_or(0))
                            .min(i64::MAX as u64) as i64,
                        confirm: true,
                        now_ms: now_ms(),
                    },
                    "清理 30 天前会话",
                );
            }
            other => {
                self.state.handle(other);
                if self.snapshot.view != self.state.view() {
                    self.prepare();
                }
            }
        }
    }

    /// 任务视图写动作（retry/abort 作用于选中任务）。
    fn task_action(&mut self, abort: bool) {
        let task_id = self
            .snapshot
            .tasks
            .get(self.task_cursor)
            .map(|row| row.id.clone());
        let Some(task_id) = task_id else {
            self.status = "任务动作：无选中任务".to_string();
            return;
        };
        let (command, label) = if abort {
            (
                FrontendCommand::TaskAbort {
                    task_id,
                    now_ms: now_ms(),
                },
                "中止任务",
            )
        } else {
            (
                FrontendCommand::TaskRetry {
                    task_id,
                    now_ms: now_ms(),
                },
                "重试任务",
            )
        };
        self.execute(command, label);
    }

    /// 两段确认第一步：执行预览段（confirm=false——不写库），通过后才进入待确认。
    fn request_confirm(&mut self, command: FrontendCommand, label: &str) {
        let preview = match &command {
            FrontendCommand::SessionDelete {
                session_id, now_ms, ..
            } => FrontendCommand::SessionDelete {
                session_id: session_id.clone(),
                confirm: false,
                now_ms: *now_ms,
            },
            FrontendCommand::SessionArchive {
                session_id,
                archived,
                now_ms,
                ..
            } => FrontendCommand::SessionArchive {
                session_id: session_id.clone(),
                archived: *archived,
                confirm: false,
                now_ms: *now_ms,
            },
            FrontendCommand::PurgeExecute {
                before_ms, now_ms, ..
            } => FrontendCommand::PurgePreview {
                before_ms: *before_ms,
                now_ms: *now_ms,
            },
            other => {
                // 非破坏性命令不进确认流（防御式；当前调用点不会到达）。
                self.execute(other.clone(), label);
                return;
            }
        };
        let Some(executor) = self.executor else {
            self.status = format!("{label}: not_supported（未注入写命令执行器）");
            return;
        };
        match executor.execute(preview) {
            Ok(outcome) => {
                self.status = match outcome {
                    whcode_app::CommandOutcome::SessionDeletePreview(preview) => format!(
                        "{label} 预览：将删 {} 个会话（事件 {}）——再按 y 确认，其他键取消",
                        preview.selected_sessions.len(),
                        preview.counts.events
                    ),
                    whcode_app::CommandOutcome::PurgePreview(preview) => format!(
                        "{label} 预览：将删 {} 个会话（事件 {}）——再按 y 确认，其他键取消",
                        preview.selected_sessions.len(),
                        preview.counts.events
                    ),
                    whcode_app::CommandOutcome::SessionArchivePreview {
                        session,
                        target_archived,
                    } => format!(
                        "{label} 预览：{} 当前{} → {}——再按 y 确认，其他键取消",
                        session.id,
                        if session.archived {
                            "已归档"
                        } else {
                            "活跃"
                        },
                        if target_archived {
                            "已归档"
                        } else {
                            "活跃"
                        }
                    ),
                    _ => format!("{label} 预览完成——再按 y 确认，其他键取消"),
                };
                self.input_mode = InputMode::Confirm {
                    command,
                    label: label.to_string(),
                };
            }
            Err(err) => {
                self.status = format!("{label} 预览失败：{err}");
            }
        }
    }

    /// 数据变更通知（模拟应用服务数据通道；本 crate 尚无数据通道，暂由重放步骤注入）。
    fn on_data_changed(&mut self) {
        self.renderer.handle_event(RenderEvent::DataChanged);
    }

    /// 空闲心跳（FR-624/T178）：低频检查持久化游标（间隔 [`CURSOR_CHECK_INTERVAL`]，
    /// 查询 O(1)），仅游标推进时经 prepare 置脏重绘一次；游标未变不置脏（T129）。
    fn on_idle(&mut self, now: Instant) {
        if now.duration_since(self.last_cursor_check) >= CURSOR_CHECK_INTERVAL {
            self.last_cursor_check = now;
            let cursor = self.source.cursor(self.selected_session.as_deref());
            if cursor.is_some() && cursor != self.last_cursor {
                self.last_cursor = cursor;
                self.prepare();
            }
        }
        self.renderer.handle_event(RenderEvent::IdleTick);
    }

    /// 最小化探测（T135/ADR-021）：仅在有**待绘制帧**且输入静默 ≥1s 时调用；
    /// 连续“未最小化”结果按指数退避（5s→60s 上限），检测到最小化后回落 5s。
    ///
    /// 成本纪律：空闲（无待绘制帧）与连续输入期间**不探测**——无待绘制帧时探测
    /// 不改变任何行为，只会白烧 200—500ms；输入连续时窗口必然可见。
    fn maybe_probe(&mut self, probe: &dyn Fn() -> Option<bool>, now: Instant) {
        if !self.renderer.is_dirty() {
            return;
        }
        if now.duration_since(self.last_input_at) < QUIET_BEFORE_PROBE {
            return;
        }
        if now.duration_since(self.last_probe_at) < self.probe_interval {
            return;
        }
        self.last_probe_at = now;
        self.probes += 1;
        match probe() {
            Some(true) => {
                self.renderer.handle_event(RenderEvent::Minimized);
                // 已最小化：心跳回落到最小间隔，尽快发现还原（还原后单次全量重绘）。
                self.probe_interval = MINIMIZE_PROBE_INTERVAL;
            }
            Some(false) => {
                if self.renderer.is_minimized() {
                    self.renderer.handle_event(RenderEvent::Restored);
                }
                self.probe_interval = (self.probe_interval * 2).min(MAX_PROBE_INTERVAL);
            }
            None => {
                // 平台不支持或探测失败：不改变门控状态（不伪装），按退避继续低频复核。
                self.probe_interval = (self.probe_interval * 2).min(MAX_PROBE_INTERVAL);
            }
        }
    }

    /// 尝试渲染一帧：返回待绘制文本行；最小化期间跳过并计入累计（FR-617）。
    fn render_frame(&mut self) -> Option<Vec<String>> {
        let was_minimized = self.renderer.is_minimized();
        let frame = self.renderer.render(&self.snapshot, self.frame_limit());
        match frame {
            Some(lines) => {
                self.redraws += 1;
                Some(lines)
            }
            None => {
                if was_minimized && self.renderer.is_dirty() {
                    self.skipped += 1; // 最小化期间的待绘制帧被跳过（关键事件已记账）。
                }
                None
            }
        }
    }

    /// 帧路径行数上界（Check 83 帧预算）：终端行数——帧成本由终端尺寸界定；
    /// 脚本模式初始尺寸未知（0）时按兜底尺寸。
    fn frame_limit(&self) -> usize {
        match self.state.rows() {
            0 => usize::from(DEFAULT_SIZE.1),
            rows => usize::from(rows),
        }
    }

    fn report(&self, events_consumed: usize, steps_consumed: usize) -> ScriptReport {
        ScriptReport {
            quit: self.state.quit(),
            cols: self.state.cols(),
            rows: self.state.rows(),
            keys_seen: self.state.keys_seen(),
            events_consumed,
            steps_consumed,
            view: self.state.view(),
            sessions: self.sessions.len(),
            session_cursor: self.session_cursor,
            selected_session: self.selected_session.clone(),
            redraws: self.redraws,
            theme: self.theme,
            status: self.status.clone(),
            skipped_while_minimized: self.skipped,
            probes: self.probes,
            minimized: self.renderer.is_minimized(),
        }
    }
}

/// 画一帧：标题（视图名/序号）、视图文本、页脚（尺寸/按键/快捷键提示），
/// 应用主题精确色值（FR-612）与文本层（FR-615）。纯投影：不做查询、不反序列化 JSON（§5.3）。
fn draw_frame(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    lines: &[String],
    theme: Theme,
    texts: &I18n,
) {
    let (background, foreground) = theme.colors();
    let style = match (theme::parse_rgb(background), theme::parse_rgb(foreground)) {
        (Some(bg), Some(fg)) => Style::default()
            .fg(Color::Rgb(fg.0, fg.1, fg.2))
            .bg(Color::Rgb(bg.0, bg.1, bg.2)),
        _ => Style::default(), // 常量色值恒合法；防御式回退到终端默认配色。
    };

    let view = state.view();
    let title = texts.resolve_fmt(
        "tui.title",
        &[
            ("view", texts.resolve(view.title_key())),
            ("index", &view.index().to_string()),
            ("count", &VIEW_COUNT.to_string()),
        ],
    );
    let block = Block::default()
        .style(style)
        .borders(Borders::ALL)
        .title(title);
    let area = frame.area();
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
    let body: Vec<Line<'_>> = if lines.is_empty() {
        // 数据源未装配（或视图为空）：显式提示，不伪造内容（不伪装原则）。
        vec![Line::from(texts.resolve("tui.empty"))]
    } else {
        lines.iter().map(|line| Line::from(line.as_str())).collect()
    };
    frame.render_widget(Paragraph::new(body).style(style), chunks[0]);
    let footer = texts.resolve_fmt(
        "tui.footer",
        &[
            ("cols", &state.cols().to_string()),
            ("rows", &state.rows().to_string()),
            ("keys", &state.keys_seen().to_string()),
        ],
    );
    frame.render_widget(Paragraph::new(Line::from(footer)).style(style), chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::{RunOptions, ScriptStep, UiEvent, ViewId, run_scripted, run_scripted_steps};
    use crate::tests_support::{
        FakeProbe, fixture_snapshot, render_empty_frame, render_fixture_frame, view_switch_key,
    };

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
        assert_eq!(report.view, ViewId::Sessions, "默认视图为会话");
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

    /// T178.1：`t` 四步循环回原位（FR-625），每步产生一帧（主题变化置脏）。
    #[test]
    fn theme_key_cycles_four_themes_back_to_start() {
        use crate::theme::Theme;
        let events = [
            UiEvent::Key('t'),
            UiEvent::Key('t'),
            UiEvent::Key('t'),
            UiEvent::Key('t'),
        ];
        let report = run_scripted(&events);
        assert_eq!(report.theme, Theme::EinkDay, "四步必须回原位");
        let one = run_scripted(&[UiEvent::Key('t')]);
        assert_eq!(one.theme, Theme::EinkNight, "一步到第二套");
    }

    /// T178.2：游标未变空闲零重绘（首帧除外）。
    #[test]
    fn idle_steps_without_cursor_change_do_not_redraw() {
        use whcode_app::view_source::EmptyViewSource;
        let steps = [ScriptStep::Idle; 5];
        let report = run_scripted_steps(&EmptyViewSource, &steps, &RunOptions::default());
        assert_eq!(report.redraws, 1, "游标未变：仅初始帧，空闲零重绘");
    }

    /// T178.2：游标推进触发一帧重绘（FR-624 在线可见）。
    #[test]
    fn cursor_advance_triggers_single_redraw() {
        use std::cell::Cell;
        struct AdvancingSource(Cell<i64>);
        impl whcode_app::view_source::ViewSource for AdvancingSource {
            fn snapshot(
                &self,
                _view: ViewId,
                _session: Option<&str>,
            ) -> whcode_app::view_source::ViewSnapshot {
                whcode_app::view_source::ViewSnapshot::empty(ViewId::Sessions)
            }
            fn cursor(&self, _session: Option<&str>) -> Option<i64> {
                self.0.set(self.0.get() + 1);
                Some(self.0.get())
            }
        }
        let source = AdvancingSource(Cell::new(0));
        let steps = [ScriptStep::Idle; 5];
        let report = run_scripted_steps(&source, &steps, &RunOptions::default());
        assert!(
            report.redraws > 1,
            "游标每次检查都推进：空闲检查必须触发重绘（实际 {}）",
            report.redraws
        );
    }

    /// T177/FR-623：`A` 归档走两段确认——先预览（confirm=false），`y` 后才执行（confirm=true）。
    #[test]
    fn archive_requires_preview_then_confirm() {
        use std::cell::RefCell;
        use whcode_app::SessionRow;
        use whcode_app::commands::{CommandExecutor, CommandOutcome, FrontendCommand};
        use whcode_domain::WhError;

        struct RecordingExecutor(RefCell<Vec<FrontendCommand>>);
        impl CommandExecutor for RecordingExecutor {
            fn execute(&self, command: FrontendCommand) -> Result<CommandOutcome, WhError> {
                let row = SessionRow {
                    id: "S-1".to_string(),
                    title: "夹具".to_string(),
                    created_at_ms: 0,
                    last_active_at_ms: 0,
                    model: None,
                    archived: matches!(
                        &command,
                        FrontendCommand::SessionArchive {
                            confirm: true,
                            archived: true,
                            ..
                        }
                    ),
                    status: "active".to_string(),
                    stage: "plan".to_string(),
                };
                let outcome = match &command {
                    FrontendCommand::SessionArchive {
                        archived,
                        confirm: false,
                        ..
                    } => CommandOutcome::SessionArchivePreview {
                        session: SessionRow {
                            archived: false,
                            ..row.clone()
                        },
                        target_archived: *archived,
                    },
                    FrontendCommand::SessionArchive { .. } => CommandOutcome::Session(row),
                    other => panic!("归档流程不应执行其他命令：{other:?}"),
                };
                self.0.borrow_mut().push(command);
                Ok(outcome)
            }
        }
        struct OneSessionSource;
        impl whcode_app::view_source::ViewSource for OneSessionSource {
            fn snapshot(
                &self,
                view: ViewId,
                _session: Option<&str>,
            ) -> whcode_app::view_source::ViewSnapshot {
                let mut snapshot = whcode_app::view_source::ViewSnapshot::empty(view);
                snapshot.sessions.push(crate::views::SessionRow {
                    id: "S-1".to_string(),
                    title: "夹具".to_string(),
                    stage: "plan".to_string(),
                    status: "active".to_string(),
                });
                snapshot
            }
        }

        let executor = RecordingExecutor(RefCell::new(Vec::new()));
        let options = RunOptions {
            command_executor: Some(&executor),
            ..RunOptions::default()
        };
        let report = run_scripted_steps(
            &OneSessionSource,
            &[
                ScriptStep::Input(UiEvent::Key('A')),
                ScriptStep::Input(UiEvent::Key('y')),
            ],
            &options,
        );
        let sent = executor.0.borrow();
        assert_eq!(sent.len(), 2, "先预览再执行：{sent:?}");
        assert!(
            matches!(
                sent[0],
                FrontendCommand::SessionArchive {
                    confirm: false,
                    archived: true,
                    ..
                }
            ),
            "第一步必须是归档预览：{:?}",
            sent[0]
        );
        assert!(
            matches!(
                sent[1],
                FrontendCommand::SessionArchive {
                    confirm: true,
                    archived: true,
                    ..
                }
            ),
            "确认后才执行：{:?}",
            sent[1]
        );
        assert!(
            !report.status.contains("not_supported"),
            "有执行器时不得报不可用：{:?}",
            report.status
        );
    }

    /// T177：无执行器时写动作显式 `not_supported`，不静默无响应（FR-620）。
    /// 用带一条会话的夹具源触发删除路径（空源会先命中“未选择会话”）。
    #[test]
    fn write_action_without_executor_reports_not_supported() {
        struct OneSessionSource;
        impl whcode_app::view_source::ViewSource for OneSessionSource {
            fn snapshot(
                &self,
                view: ViewId,
                _session: Option<&str>,
            ) -> whcode_app::view_source::ViewSnapshot {
                let mut snapshot = whcode_app::view_source::ViewSnapshot::empty(view);
                snapshot.sessions.push(crate::views::SessionRow {
                    id: "S-1".to_string(),
                    title: "夹具".to_string(),
                    stage: "plan".to_string(),
                    status: "active".to_string(),
                });
                snapshot
            }
        }
        let report = run_scripted_steps(
            &OneSessionSource,
            &[ScriptStep::Input(UiEvent::Key('d'))],
            &RunOptions::default(),
        );
        assert!(
            report.status.contains("not_supported"),
            "状态行必须显式不可用：{:?}",
            report.status
        );
    }

    #[test]
    fn scripted_run_reports_empty_input_unchanged() {
        let report = run_scripted(&[]);
        assert!(!report.quit);
        assert_eq!(report.events_consumed, 0);
        assert_eq!((report.cols, report.rows, report.keys_seen), (0, 0, 0));
        assert_eq!(report.redraws, 0, "无步骤不绘制");
    }

    /// T082：`1`—`6` 视图切换进入真实循环逻辑（重放报告给出最终视图）。
    #[test]
    fn scripted_run_switches_views_with_digit_keys() {
        for view in ViewId::ALL {
            let steps = [ScriptStep::Input(UiEvent::Key(view_switch_key(view)))];
            let report = run_scripted_steps(&EmptySourceForTest, &steps, &RunOptions::default());
            assert_eq!(report.view, view, "键应切换到 {view:?}");
            assert_eq!(report.keys_seen, 0, "导航键不计入普通按键");
        }
    }

    /// T083/T129：空闲（无事件）零重绘——100 个空闲步骤不增加重绘计数。
    #[test]
    fn scripted_idle_steps_produce_zero_redraws() {
        let single = [ScriptStep::Input(UiEvent::Key('a'))];
        let baseline = run_scripted_steps(&EmptySourceForTest, &single, &RunOptions::default());
        assert_eq!(baseline.redraws, 1, "一次输入绘制一帧");

        let mut with_idle = vec![ScriptStep::Input(UiEvent::Key('a'))];
        with_idle.extend(std::iter::repeat_n(ScriptStep::Idle, 100));
        let report = run_scripted_steps(&EmptySourceForTest, &with_idle, &RunOptions::default());
        assert_eq!(
            report.redraws, baseline.redraws,
            "100 个空闲步骤必须零重绘（T129）"
        );
        assert_eq!(report.probes, 0, "无探测源时不得探测");
        assert_eq!(report.steps_consumed, 101);
    }

    /// T135：数据变更 + 静默 → 探测到最小化则跳过绘制；还原后单次全量重绘（FR-617）。
    ///
    /// 时间线（每步 1s；探测间隔 5s）：第 12 步数据变更触发探测 1（最小化）；
    /// 第 17 步探测 2（仍最小化）；第 22 步探测 3（已还原 → 单次全量重绘）。
    #[test]
    fn scripted_minimize_gates_redraws_and_restores_once() {
        let probe = FakeProbe::new([Some(true), Some(true), Some(false)]);
        let probe_fn = probe.closure();
        let options = RunOptions {
            minimize_probe: Some(&probe_fn),
            ..RunOptions::default()
        };
        let mut steps = vec![ScriptStep::DataChanged];
        steps.extend(std::iter::repeat_n(ScriptStep::Idle, 10));
        steps.push(ScriptStep::DataChanged);
        steps.extend(std::iter::repeat_n(ScriptStep::Idle, 10));
        let report = run_scripted_steps(&EmptySourceForTest, &steps, &options);

        assert_eq!(report.probes, 3, "仅在有待绘制帧且静默时探测");
        assert_eq!(report.redraws, 2, "最小化期间不重绘；还原后单次全量重绘");
        assert!(!report.minimized, "第三次探测报告已还原");
        assert_eq!(
            report.skipped_while_minimized, 10,
            "第 12—21 步共 10 次渲染尝试在最小化期间被跳过（每次尝试计数，与 Renderer 同语义）"
        );
    }

    /// T135 成本纪律：无待绘制帧（空闲）时**不探测**——空闲成本为零。
    #[test]
    fn scripted_idle_without_pending_frames_never_probes() {
        let probe = FakeProbe::new([Some(true)]);
        let probe_fn = probe.closure();
        let options = RunOptions {
            minimize_probe: Some(&probe_fn),
            ..RunOptions::default()
        };
        let steps: Vec<ScriptStep> = std::iter::repeat_n(ScriptStep::Idle, 200).collect();
        let report = run_scripted_steps(&EmptySourceForTest, &steps, &options);
        assert_eq!(report.probes, 0);
        assert_eq!(report.redraws, 1, "仅初始帧绘制一次");
        assert_eq!(probe.calls(), 0);
    }

    /// T135 成本纪律：连续输入（静默 < 1s）期间**不探测**——输入即证明窗口可见。
    #[test]
    fn scripted_continuous_input_never_probes() {
        let probe = FakeProbe::new([Some(true)]);
        let probe_fn = probe.closure();
        let options = RunOptions {
            minimize_probe: Some(&probe_fn),
            ..RunOptions::default()
        };
        let steps: Vec<ScriptStep> =
            std::iter::repeat_n(ScriptStep::Input(UiEvent::Key('a')), 30).collect();
        let report = run_scripted_steps(&EmptySourceForTest, &steps, &options);
        assert_eq!(report.probes, 0);
        assert_eq!(report.redraws, 30, "每个输入各绘制一帧");
        assert_eq!(probe.calls(), 0);
    }

    /// T135：输入事件证明窗口可见——最小化判定被输入“还原”并单次全量重绘。
    #[test]
    fn scripted_input_proves_window_visible_and_restores() {
        let probe = FakeProbe::new([Some(true)]);
        let probe_fn = probe.closure();
        let options = RunOptions {
            minimize_probe: Some(&probe_fn),
            ..RunOptions::default()
        };
        let mut steps = vec![ScriptStep::DataChanged];
        steps.extend(std::iter::repeat_n(ScriptStep::Idle, 11));
        steps.push(ScriptStep::DataChanged); // 第 13 步：触发探测 → 最小化，帧被跳过
        steps.push(ScriptStep::Input(UiEvent::Key('a'))); // 第 14 步：输入 → 还原 + 全量重绘
        let report = run_scripted_steps(&EmptySourceForTest, &steps, &options);
        assert!(!report.minimized, "输入后门控必须解除");
        assert_eq!(report.redraws, 2, "初始帧 + 还原后的单次全量重绘");
        assert_eq!(report.probes, 1);
        assert_eq!(report.skipped_while_minimized, 1);
    }

    /// Check 82：六视图在固定终端尺寸（80×24）下渲染出稳定文本帧（截图取证的可执行形式）。
    #[test]
    fn six_views_render_at_fixed_terminal_size() {
        for view in ViewId::ALL {
            let text = render_fixture_frame(view, 80, 24);
            println!("===== TUI VIEW: {} =====", view.as_str());
            print!("{text}");
            assert!(
                text.contains(&format!(
                    "旺皇大模型执行框架 —— {}（{}/6）",
                    view.title(),
                    view.index()
                )),
                "{view:?} 帧必须含视图标题"
            );
            assert!(text.contains("1—6 视图"), "{view:?} 帧必须含快捷键提示");
            assert!(text.contains("q 退出"), "{view:?} 帧必须含退出提示");
            for line in fixture_snapshot(view).render_lines() {
                assert!(
                    line.is_empty() || text.contains(line.as_str()),
                    "{view:?} 帧必须包含准备层数据行 {line:?}"
                );
            }
            // 尺寸固定：恰好 24 行（多余列按终端裁剪语义承载，不越界的断言在数据行上）。
            assert_eq!(text.lines().count(), 24, "{view:?} 帧必须为 24 行");
        }
    }

    /// FR-612：主题色值实际进入渲染单元（背景精确为纯黑/纯白）。
    #[test]
    fn theme_colors_are_applied_to_cells() {
        use ratatui::style::Color;

        use crate::tests_support::frame_buffer;
        let eink = frame_buffer(ViewId::Sessions, 40, 10, crate::Theme::EinkDay);
        let amoled = frame_buffer(ViewId::Sessions, 40, 10, crate::Theme::AmoledNight);
        assert_eq!(eink.content()[0].bg, Color::Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(eink.content()[0].fg, Color::Rgb(0, 0, 0));
        assert_eq!(amoled.content()[0].bg, Color::Rgb(0, 0, 0));
        assert_eq!(amoled.content()[0].fg, Color::Rgb(0xE6, 0xE6, 0xE6));
    }

    /// 空数据源帧：显式“暂无数据”，不伪造内容。
    #[test]
    fn empty_source_renders_explicit_placeholder() {
        let text = render_empty_frame(ViewId::Tasks, 60, 12);
        assert!(text.contains("（暂无数据）"), "空视图必须显式提示：{text}");
        assert!(text.contains("任务（2/6）"));
    }

    /// FR-615：英文文本层进入渲染帧（标题/页脚/空提示），数据字段不翻译。
    #[test]
    fn english_texts_render_in_frame() {
        use whcode_domain::i18n::{I18n, LocaleSource};

        use crate::tests_support::{buffer_text, draw_snapshot_with};
        let texts = I18n::load("en", LocaleSource::Explicit);
        let buffer = draw_snapshot_with(
            &super::ViewSnapshot::empty(ViewId::Tasks),
            70,
            12,
            crate::Theme::EinkDay,
            &texts,
        );
        let text = buffer_text(&buffer);
        assert!(text.contains("whcode — Tasks (2/6)"), "英文标题：{text}");
        assert!(text.contains("Window 70×12"), "英文页脚：{text}");
        assert!(text.contains("(no data)"), "英文空提示：{text}");
        // 数据字段（会话 ID/事件类型）不翻译——由准备层原样提供。
    }

    /// 会话选择与刷新（批次 1）：`j`/`k` 移动并重新准备；`r` 重新准备；导航键不计入按键。
    #[test]
    fn scripted_session_selection_and_refresh() {
        use std::cell::RefCell;

        use super::{SessionRow, ViewSource};

        struct RecordingSource {
            sessions: Vec<SessionRow>,
            calls: RefCell<Vec<(ViewId, Option<String>)>>,
        }

        impl ViewSource for RecordingSource {
            fn snapshot(&self, view: ViewId, session: Option<&str>) -> super::ViewSnapshot {
                self.calls
                    .borrow_mut()
                    .push((view, session.map(str::to_string)));
                let mut snapshot = super::ViewSnapshot::empty(view);
                if view == ViewId::Sessions {
                    snapshot.sessions = self.sessions.clone();
                } else {
                    snapshot.summary =
                        format!("会话 {} · {}", session.unwrap_or("(未选择)"), view.as_str());
                }
                snapshot
            }
        }

        let source = RecordingSource {
            sessions: vec![
                SessionRow {
                    id: "S-1".into(),
                    title: "甲".into(),
                    stage: "plan".into(),
                    status: "active".into(),
                },
                SessionRow {
                    id: "S-2".into(),
                    title: "乙".into(),
                    stage: "implement".into(),
                    status: "idle".into(),
                },
            ],
            calls: RefCell::new(Vec::new()),
        };
        let steps = [
            ScriptStep::Input(UiEvent::Key('2')),
            ScriptStep::Input(UiEvent::Key('j')),
            ScriptStep::Input(UiEvent::Key('k')),
            ScriptStep::Input(UiEvent::Key('r')),
        ];
        let report = run_scripted_steps(&source, &steps, &RunOptions::default());
        assert_eq!(report.view, ViewId::Tasks);
        assert_eq!(report.sessions, 2);
        assert_eq!(report.session_cursor, 0, "k 回到首个会话");
        assert_eq!(report.selected_session.as_deref(), Some("S-1"));
        assert_eq!(report.keys_seen, 0, "j/k/r 是导航键，不计入普通按键");
        let calls = source.calls.borrow();
        assert_eq!(
            calls.as_slice(),
            [
                // 构造时准备一次（会话清单）。
                (ViewId::Sessions, None),
                // 切到任务视图：会话清单 + 任务快照（会话 S-1）。
                (ViewId::Sessions, None),
                (ViewId::Tasks, Some("S-1".to_string())),
                // j → S-2；k → S-1；r → 重取 S-1。
                (ViewId::Sessions, None),
                (ViewId::Tasks, Some("S-2".to_string())),
                (ViewId::Sessions, None),
                (ViewId::Tasks, Some("S-1".to_string())),
                (ViewId::Sessions, None),
                (ViewId::Tasks, Some("S-1".to_string())),
            ],
            "准备调用序列：切换/选择/刷新各触发一次会话清单 + 视图快照"
        );
    }

    /// 会话选择越界钳制：连续 `j` 停在最后一个会话；空清单 selected 为 None。
    #[test]
    fn session_selection_clamps_at_bounds() {
        use super::{SessionRow, ViewSource};

        struct FixedSource(Vec<SessionRow>);
        impl ViewSource for FixedSource {
            fn snapshot(&self, view: ViewId, _session: Option<&str>) -> super::ViewSnapshot {
                let mut snapshot = super::ViewSnapshot::empty(view);
                snapshot.sessions = self.0.clone();
                snapshot
            }
        }

        let two = FixedSource(vec![
            SessionRow {
                id: "S-1".into(),
                title: "甲".into(),
                stage: "plan".into(),
                status: "active".into(),
            },
            SessionRow {
                id: "S-2".into(),
                title: "乙".into(),
                stage: "plan".into(),
                status: "active".into(),
            },
        ]);
        let steps: Vec<ScriptStep> =
            std::iter::repeat_n(ScriptStep::Input(UiEvent::Key('j')), 5).collect();
        let report = run_scripted_steps(&two, &steps, &RunOptions::default());
        assert_eq!(report.session_cursor, 1, "越界必须钳制到最后一个会话");
        assert_eq!(report.selected_session.as_deref(), Some("S-2"));

        let empty = FixedSource(Vec::new());
        let report = run_scripted_steps(&empty, &steps, &RunOptions::default());
        assert_eq!(report.sessions, 0);
        assert_eq!(report.session_cursor, 0);
        assert_eq!(report.selected_session, None, "空清单无选择（正确的空）");
    }

    /// Check 83（帧预算）：帧渲染分位数归档到 `artifacts/tui-frame-budget.json`。
    ///
    /// 测量纪律（§5.2 同族）：固定夹具（10k 行事件快照）、预热 5 次、采样 50 次，
    /// 报告微秒分位数；对比的是 **UI 自定候选预算（16 ms）**——它不属于 NFR-201—210，
    /// 因此**不随 ADR-025 冻结**，仅作回归参照（冻结须另立 ADR 并附同夹具前后对比）。
    ///
    /// 两段测量：
    /// 1. **帧路径**（`render_lines_limited(rows)` + 绘制到 80×24 缓冲）——帧成本由
    ///    终端尺寸界定：10k 行快照与有界快照同阶（这是帧预算断言的对象）；
    /// 2. **完整投影参照**（`render_lines()` 全部 10k 行）——仅作参照记录：若把完整
    ///    历史带进帧路径，单帧即达毫秒级，故有界尾部必须由准备层（V-101）裁剪。
    #[test]
    fn frame_budget_is_measured_and_archived() {
        use std::time::Instant;

        use super::{EventRow, ViewSnapshot};
        use crate::tests_support::draw_snapshot;

        const WARMUP: usize = 5;
        const SAMPLES: usize = 50;
        const CANDIDATE_P95_BUDGET_US: u128 = 16_000;

        let mut snapshot = ViewSnapshot::empty(ViewId::Events);
        snapshot.events = (1..=10_000)
            .map(|seq| EventRow {
                seq,
                kind: "StreamingChunk".into(),
                summary: format!("chunk {seq}/10000"),
            })
            .collect();

        let percentile = |samples: &[u128], p: usize| -> u128 {
            let index = ((SAMPLES - 1) * p) / 100;
            samples[index]
        };

        // 1) 帧路径（有界投影 + 绘制；与真实循环同路径）。
        for _ in 0..WARMUP {
            let _ = draw_snapshot(&snapshot, 80, 24, crate::Theme::EinkDay);
        }
        let mut frame_samples: Vec<u128> = Vec::with_capacity(SAMPLES);
        for _ in 0..SAMPLES {
            let started = Instant::now();
            let _ = draw_snapshot(&snapshot, 80, 24, crate::Theme::EinkDay);
            frame_samples.push(started.elapsed().as_micros());
        }
        frame_samples.sort_unstable();
        let (p50, p95, p99) = (
            percentile(&frame_samples, 50),
            percentile(&frame_samples, 95),
            percentile(&frame_samples, 99),
        );
        assert!(p50 <= p95 && p95 <= p99, "分位数必须单调");
        eprintln!(
            "TUI 帧渲染（事件视图 10k 行快照，80×24，有界投影）：p50 {p50} µs / \
             p95 {p95} µs / p99 {p99} µs（UI 自定候选预算 p95 ≤ {CANDIDATE_P95_BUDGET_US} µs；\
             不在 NFR-201—210 内，不随 ADR-025 冻结）"
        );
        assert!(
            p95 <= CANDIDATE_P95_BUDGET_US,
            "帧渲染 p95 {p95} µs 超出候选预算 {CANDIDATE_P95_BUDGET_US} µs（Check 83）；\
             须走性能归因流程（NFR-216）"
        );

        // 2) 完整投影参照（不进帧路径；记录为何准备层必须有界裁剪）。
        let started = Instant::now();
        let full = snapshot.render_lines();
        let full_projection_us = started.elapsed().as_micros();
        assert_eq!(full.len(), 10_000, "参照测量必须覆盖完整 10k 行");
        drop(full);
        eprintln!(
            "完整投影参照（10k 行全量格式化，不进帧路径）：{full_projection_us} µs/次——\
             准备层按有界尾部（V-101）裁剪，帧路径只做限行投影"
        );

        let payload = serde_json::json!({
            "task": "T083/T129",
            "fixture": { "view": "events", "rows": 10_000, "terminal": "80x24" },
            "measurement": {
                "warmup": WARMUP,
                "samples": SAMPLES,
                "unit": "microseconds",
                "clock": "std::time::Instant",
                "scope": "frame_path = render_lines_limited(rows) + draw_frame -> TestBackend buffer (含主题应用；不含文本提取)",
                "profile": if cfg!(debug_assertions) { "debug（未优化，数量级偏悲观；release 运行约低一个数量级）" } else { "release" }
            },
            "p50_us": p50,
            "p95_us": p95,
            "p99_us": p99,
            "reference_full_projection_us": full_projection_us,
            "budget": {
                "candidate_p95_us": CANDIDATE_P95_BUDGET_US,
                "frozen": false,
                "note": "UI 自定候选预算：不属于 NFR-201—210，不随 ADR-025 冻结；本报告仅作回归参照"
            },
            "platform": "windows-amd64",
            "date": "2026-10-22"
        });
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../artifacts/tui-frame-budget.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&payload).expect("序列化报告"),
        )
        .expect("写入帧预算报告");
        eprintln!("帧预算报告已写入 {}", path.display());
    }

    /// 测试用空数据源（与 [`super::EmptyViewSource`] 等价，独立声明以固定证据语义）。
    struct EmptySourceForTest;

    impl super::ViewSource for EmptySourceForTest {
        fn snapshot(&self, view: ViewId, _session: Option<&str>) -> super::ViewSnapshot {
            super::ViewSnapshot::empty(view)
        }
    }
}
