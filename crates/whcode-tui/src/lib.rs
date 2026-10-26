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
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
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
        if core.render_frame().is_some() {
            let theme = core.theme;
            terminal
                .draw(|frame| {
                    draw_frame(
                        frame,
                        &core.state,
                        &core.snapshot,
                        core.selected_session.as_deref(),
                        &core.status,
                        theme,
                        &options.texts,
                    )
                })
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

    /// 尝试渲染一帧（门控）：返回 `Some(())` 表示本帧应绘制，`None` 表示空闲或最小化
    /// 跳过（FR-617）。帧内容由 [`draw_frame`] 从快照组装——门控与内容装配分离，
    /// 投影只发生一次。
    fn render_frame(&mut self) -> Option<()> {
        let was_minimized = self.renderer.is_minimized();
        let frame = self.renderer.render();
        match frame {
            Some(()) => {
                self.redraws += 1;
                Some(())
            }
            None => {
                if was_minimized && self.renderer.is_dirty() {
                    self.skipped += 1; // 最小化期间的待绘制帧被跳过（关键事件已记账）。
                }
                None
            }
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

/// 导航栏宽度（列）：固定 14 列，80 列终端下主面板仍有 ≥ 60 列。
const RAIL_WIDTH: u16 = 14;

/// 主面板最小宽度（列）：不足时先压缩导航栏，再按可用列截断正文（不横向溢出）。
const BODY_MIN_WIDTH: u16 = 20;

/// 三区布局的最小终端尺寸（列, 行）；更小时退化为无 chrome 的正文，避免零面积区域。
const CHROME_MIN: (u16, u16) = (24, 6);

/// 主面板最窄可用列数（列宽为 0 时正文只剩边框）。
const MIN_BODY_CONTENT: usize = 8;

/// 字符显示列宽：CJK／全角／常见表情记 2 列，其余 1 列。
///
/// 这是**排版宽度**，只用于本文件内的截断与补位；终端实际前进量由 `RowBackend`
/// 整行输出承担（歧义宽度字形不在此表内断言）。表外的歧义字形（`·`、`×`、`—`）
/// 不在新增装饰中使用（ADR-041 决策 4）。
fn char_width(ch: char) -> usize {
    match u32::from(ch) {
        0x1100..=0x115F
        | 0x2E80..=0x303E
        | 0x3041..=0x33FF
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// 文本显示列宽（按 [`char_width`] 求和；测试与诊断用）。
#[cfg(test)]
fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// 空格填充（避免 `" ".repeat(n)` 的临时 String 分配）。
fn push_spaces(out: &mut String, count: usize) {
    const SPACES: &str = "                                                                ";
    let mut remaining = count;
    while remaining > 0 {
        let take = remaining.min(SPACES.len());
        out.push_str(&SPACES[..take]);
        remaining -= take;
    }
}

/// 追加一个单元格：按显示列截断、以 `~` 收尾、补齐到精确宽度（右对齐可选）。
///
/// 直接写入行缓冲，**不产生中间 String**——帧内每个单元格一次分配会显著抬高
/// 每帧成本（ADR-041 资源中性口径：帧预算相对基线不劣化 20%）。
/// ASCII 走长度直通路径（不逐字查宽度表），CJK 走显示列路径。
fn push_cell(out: &mut String, text: &str, width: usize, right_aligned: bool) {
    if width == 0 {
        return;
    }
    if text.is_ascii() {
        if text.len() <= width {
            let pad = width - text.len();
            if right_aligned {
                push_spaces(out, pad);
            }
            out.push_str(text);
            if !right_aligned {
                push_spaces(out, pad);
            }
        } else {
            let take = width.saturating_sub(1);
            if right_aligned {
                push_spaces(out, width.saturating_sub(take + 1));
            }
            out.push_str(&text[..take]);
            out.push('~');
        }
        return;
    }
    let mut used = 0usize;
    let mut end = text.len();
    let mut truncated = false;
    for (index, ch) in text.char_indices() {
        let cell = char_width(ch);
        if used + cell > width {
            end = index;
            truncated = true;
            break;
        }
        used += cell;
    }
    let mut visible = &text[..end];
    if truncated {
        // 留 1 列给截断标记：必要时回退字符，直到能容纳标记。
        while used + 1 > width {
            match visible.char_indices().next_back() {
                Some((index, ch)) => {
                    used -= char_width(ch);
                    visible = &visible[..index];
                }
                None => break,
            }
        }
    }
    let pad = width.saturating_sub(used + usize::from(truncated));
    if right_aligned {
        push_spaces(out, pad);
    }
    out.push_str(visible);
    if truncated {
        out.push('~');
    }
    if !right_aligned {
        push_spaces(out, pad);
    }
}

/// 左对齐单元格（截断 + 右补空格到精确列宽）。
fn fit_cell(text: &str, width: usize) -> String {
    let mut out = String::with_capacity(width);
    push_cell(&mut out, text, width, false);
    out
}

/// 行宽夹取（原地）：超过可用列时按显示列截断并以 `~` 收尾。
fn clamp_line(line: &mut String, width: usize) {
    if line.is_ascii() {
        if line.len() <= width {
            return;
        }
        let take = width.saturating_sub(1);
        line.truncate(take);
        line.push('~');
        return;
    }
    let mut used = 0usize;
    let mut end = line.len();
    let mut truncated = false;
    for (index, ch) in line.char_indices() {
        let cell = char_width(ch);
        if used + cell > width {
            end = index;
            truncated = true;
            break;
        }
        used += cell;
    }
    if !truncated {
        return;
    }
    line.truncate(end);
    while used + 1 > width {
        match line.pop() {
            Some(ch) => used -= char_width(ch),
            None => break,
        }
    }
    line.push('~');
}

/// 该视图是否有列定义（会话／任务／事件）。
const fn has_columns(view: ViewId) -> bool {
    matches!(view, ViewId::Sessions | ViewId::Tasks | ViewId::Events)
}

/// 视图数据总条数（滚动位置的分母；无列视图为 0）。
fn view_total(view: ViewId, snapshot: &ViewSnapshot) -> usize {
    match view {
        ViewId::Sessions => snapshot.sessions.len(),
        ViewId::Tasks => snapshot.tasks.len(),
        ViewId::Events => snapshot.events.len(),
        ViewId::Context | ViewId::Spec | ViewId::Diff => 0,
    }
}

/// 按视图列定义排版一行（列宽固定，标题列吃剩余宽度；超宽再夹到可用列）。
///
/// 单元格由闭包按索引取用（借用快照字段，不复制长文本）；`cell` 越界返回空串，
/// 不 panic——投影与列定义不一致时退化为空列，而不是让帧路径崩溃。
/// 全行只分配一个输出缓冲（帧内每单元格一次分配会抬高一帧成本）。
fn layout_columns<'a, F: Fn(usize) -> &'a str>(view: ViewId, width: usize, cell: F) -> String {
    let mut out = String::with_capacity(width + 4);
    match view {
        // 标记(1) + ID(8) + 标题(剩余) + 阶段(10) + 状态(10)，列间各 1 空格。
        ViewId::Sessions => {
            let flex = width.saturating_sub(33);
            push_cell(&mut out, cell(0), 1, false);
            out.push(' ');
            push_cell(&mut out, cell(1), 8, false);
            out.push(' ');
            push_cell(&mut out, cell(2), flex, false);
            out.push(' ');
            push_cell(&mut out, cell(3), 10, false);
            out.push(' ');
            push_cell(&mut out, cell(4), 10, false);
        }
        // 标记(1) + ID(8) + 标题(剩余) + 状态(10) + 重试(5，右对齐)。
        ViewId::Tasks => {
            let flex = width.saturating_sub(28);
            push_cell(&mut out, cell(0), 1, false);
            out.push(' ');
            push_cell(&mut out, cell(1), 8, false);
            out.push(' ');
            push_cell(&mut out, cell(2), flex, false);
            out.push(' ');
            push_cell(&mut out, cell(3), 10, false);
            out.push(' ');
            push_cell(&mut out, cell(4), 5, true);
        }
        // 序号(8，右对齐) + 类型(18) + 摘要(剩余)。
        ViewId::Events => {
            let flex = width.saturating_sub(28);
            push_cell(&mut out, cell(0), 8, true);
            out.push(' ');
            push_cell(&mut out, cell(1), 18, false);
            out.push(' ');
            push_cell(&mut out, cell(2), flex, false);
        }
        ViewId::Context | ViewId::Spec | ViewId::Diff => {}
    }
    clamp_line(&mut out, width);
    out
}

/// 列标题行（与数据行列宽一致；标签来自文本层）。
fn column_header(view: ViewId, texts: &I18n, width: usize) -> String {
    let keys: &[&str] = match view {
        ViewId::Sessions => &[
            "",
            "tui.col.id",
            "tui.col.title",
            "tui.col.stage",
            "tui.col.status",
        ],
        ViewId::Tasks => &[
            "",
            "tui.col.id",
            "tui.col.title",
            "tui.col.status",
            "tui.col.retry",
        ],
        ViewId::Events => &["tui.col.seq", "tui.col.kind", "tui.col.summary"],
        ViewId::Context | ViewId::Spec | ViewId::Diff => &[],
    };
    layout_columns(view, width, |index| {
        keys.get(index).map_or("", |key| {
            if key.is_empty() {
                ""
            } else {
                texts.resolve(key)
            }
        })
    })
}

/// 画一帧（T195／ADR-041）：顶栏（应用名、视图序号、当前会话）／左导航栏 + 主面板／
/// 键位提示栏三区；主面板按视图列定义排版，选中行反显，右侧标注滚动位置。
///
/// 纯投影：只读 [`ViewSnapshot`] 与 [`TuiState`]，不做查询、不反序列化 JSON（§5.3）；
/// 帧内分配只与终端尺寸相关（列投影行数 = 主面板高度）。
fn draw_frame(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    snapshot: &ViewSnapshot,
    session: Option<&str>,
    status: &str,
    theme: Theme,
    texts: &I18n,
) {
    let (background, foreground) = theme.colors();
    let base = match (theme::parse_rgb(background), theme::parse_rgb(foreground)) {
        (Some(bg), Some(fg)) => Style::default()
            .fg(Color::Rgb(fg.0, fg.1, fg.2))
            .bg(Color::Rgb(bg.0, bg.1, bg.2)),
        _ => Style::default(), // 常量色值恒合法；防御式回退到终端默认配色。
    };
    let area = frame.area();

    // 极窄/极矮：退化为纯正文（仍显式提示空态），避免三区被压成零面积。
    if area.width < CHROME_MIN.0 || area.height < CHROME_MIN.1 {
        let lines = snapshot.render_lines_limited(usize::from(area.height.max(1)));
        let body: Vec<Line<'_>> = if lines.is_empty() {
            vec![Line::from(texts.resolve("tui.empty"))]
        } else {
            lines.iter().map(|line| Line::from(line.as_str())).collect()
        };
        frame.render_widget(Paragraph::new(body).style(base), area);
        return;
    }

    let status_height = u16::from(!status.is_empty());
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(status_height),
        Constraint::Length(1),
    ])
    .split(area);

    // 1) 顶栏：应用名 + 视图（序号/总数）+ 当前会话。
    let view = state.view();
    let header = texts.resolve_fmt(
        "tui.title",
        &[
            ("app", texts.resolve("app.title")),
            ("view", texts.resolve(view.title_key())),
            ("index", &view.index().to_string()),
            ("count", &VIEW_COUNT.to_string()),
        ],
    );
    let session_label = session.unwrap_or_else(|| texts.resolve("tui.header.none"));
    frame.render_widget(
        Paragraph::new(Line::from(format!("{header}  |  {session_label}"))).style(base),
        rows[0],
    );

    // 2) 中部：导航栏 + 主面板。
    draw_content(frame, rows[1], snapshot, view, base, texts);

    // 3) 动作反馈行（仅非空时占一行，不加高帧）。
    if status_height == 1 {
        frame.render_widget(Paragraph::new(Line::from(status)).style(base), rows[2]);
    }

    // 4) 键位提示栏：键位 + 窗口尺寸（不显示按键计数——计数不是用户需要的信号）。
    let keybar = texts.resolve_fmt(
        "tui.footer",
        &[
            ("cols", &state.cols().to_string()),
            ("rows", &state.rows().to_string()),
        ],
    );
    frame.render_widget(Paragraph::new(Line::from(keybar)).style(base), rows[3]);
}

/// 中部区域：左导航栏（固定宽度，选中项 `>` + 反显）+ 主面板（单线边框、列标题、
/// 列对齐正文、右侧滚动位置）。
fn draw_content(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    snapshot: &ViewSnapshot,
    view: ViewId,
    base: Style,
    texts: &I18n,
) {
    let rail = RAIL_WIDTH.min(area.width.saturating_sub(BODY_MIN_WIDTH));
    let body_x = area.x.saturating_add(rail);
    let body_width = area.width.saturating_sub(rail);

    if rail > 0 {
        let rail_area = Rect::new(area.x, area.y, rail, area.height);
        // 导航栏右侧留 1 列空白，与主面板左边框形成分隔（不叠加第二条竖线）。
        let inner_width = usize::from(rail.saturating_sub(1));
        let lines: Vec<Line<'_>> = ViewId::ALL
            .iter()
            .map(|item| {
                let selected = *item == view;
                let marker = if selected { ">" } else { " " };
                let text = fit_cell(
                    &format!(
                        "{marker}{} {}",
                        item.index(),
                        texts.resolve(item.title_key())
                    ),
                    inner_width,
                );
                let style = if selected {
                    base.add_modifier(Modifier::REVERSED)
                } else {
                    base
                };
                Line::from(text).style(style)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines).style(base), rail_area);
    }

    if body_width == 0 {
        return;
    }
    let body_area = Rect::new(body_x, area.y, body_width, area.height);
    let inner = Rect::new(
        body_area.x.saturating_add(1),
        body_area.y.saturating_add(1),
        body_area.width.saturating_sub(2),
        body_area.height.saturating_sub(2),
    );
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let content_width = usize::from(inner.width).max(MIN_BODY_CONTENT);
    let column_view = has_columns(view);

    // 列视图：摘要行单列一行（文本视图的摘要已含在文本行首，不重复）。
    let summary =
        (column_view && !snapshot.summary.is_empty()).then_some(snapshot.summary.as_str());
    let total = view_total(view, snapshot);
    // 正确空态：列视图无数据时不画空表头，只显示显式空提示（不把“无数据”伪造成
    // “有列但空”）。文本视图的空态由文本行路径与下方兜底处理。
    let empty = column_view && total == 0;
    let chrome = u16::from(summary.is_some()) + u16::from(column_view && !empty);
    let body_rows = usize::from(inner.height.saturating_sub(chrome));

    let mut lines: Vec<Line<'_>> = Vec::with_capacity(body_rows + usize::from(chrome));
    if let Some(summary) = summary {
        lines.push(Line::from(summary));
    }

    let mut shown = 0usize;
    if empty {
        lines.push(Line::from(texts.resolve("tui.empty")));
    } else if column_view {
        lines.push(Line::from(column_header(view, texts, content_width)));
        for (index, row) in snapshot
            .render_columns_limited(body_rows)
            .iter()
            .enumerate()
        {
            let text = layout_columns(view, content_width, |cell| {
                row.cells.get(cell).map_or("", |value| value.as_ref())
            });
            let style = if snapshot.selected_index == Some(index) {
                base.add_modifier(Modifier::REVERSED)
            } else {
                base
            };
            lines.push(Line::from(text).style(style));
            shown += 1;
        }
    } else {
        lines.extend(
            snapshot
                .render_lines_limited(body_rows)
                .into_iter()
                .map(Line::from),
        );
    }

    if lines.is_empty() {
        // 数据源未装配（或视图为空）：显式提示，不伪造内容（不伪装原则）。
        lines.push(Line::from(texts.resolve("tui.empty")));
    }

    let mut block = Block::default()
        .style(base)
        .borders(Borders::ALL)
        .title(texts.resolve(view.title_key()));
    if column_view {
        block = block.title(Line::from(format!("{shown}/{total}")).right_aligned());
    }
    // 边框块已把主题样式铺满整个区域（含正文单元），正文不再重复上色。
    frame.render_widget(Paragraph::new(lines).block(block), body_area);
}

#[cfg(test)]
mod tests {
    use super::{RunOptions, ScriptStep, UiEvent, ViewId, run_scripted, run_scripted_steps};
    use crate::tests_support::{
        FakeProbe, buffer_text, draw_snapshot_with_context, fixture_snapshot, render_empty_frame,
        render_fixture_frame, render_snapshot_text, view_switch_key,
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

    /// Check 82 / T195：六视图在固定终端尺寸（80×24）下渲染出稳定三区帧——
    /// 顶栏（应用名 + 视图序号）、导航栏（六视图 + 选中标记）、主面板、键位提示栏。
    #[test]
    fn six_views_render_at_fixed_terminal_size() {
        for view in ViewId::ALL {
            let text = render_fixture_frame(view, 80, 24);
            println!("===== TUI VIEW: {} =====", view.as_str());
            print!("{text}");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines.len(), 24, "{view:?} 帧必须为 24 行");

            // 顶栏：应用名 + 视图（序号/总数）。
            assert!(
                lines[0].contains("旺皇大模型执行框架"),
                "{view:?} 顶栏必须含应用名：{:?}",
                lines[0]
            );
            assert!(
                lines[0].contains(&format!("{} ({}/6)", view.title(), view.index())),
                "{view:?} 顶栏必须含视图序号：{:?}",
                lines[0]
            );
            // 导航栏：六视图全部列出，当前项同时具备 `>` 标记与标题。
            for item in ViewId::ALL {
                assert!(
                    text.contains(item.title()),
                    "{view:?} 导航栏必须列出 {item:?}"
                );
            }
            assert!(
                text.contains(&format!(">{} {}", view.index(), view.title())),
                "{view:?} 导航栏选中项必须以 > 标记"
            );
            // 键位提示栏（末行）：键位与窗口尺寸。
            let last = lines.last().copied().unwrap_or_default();
            for expected in ["1-6 视图", "j/k 移动", "q 退出", "80x24"] {
                assert!(
                    last.contains(expected),
                    "{view:?} 键位提示栏缺少 {expected:?}：{last:?}"
                );
            }

            // 数据字段可见：列视图逐字段断言，文本视图断言整行。
            let snapshot = fixture_snapshot(view);
            match view {
                ViewId::Sessions => {
                    for row in &snapshot.sessions {
                        assert!(text.contains(&row.id), "{view:?} 缺 ID {}", row.id);
                        assert!(text.contains(&row.title), "{view:?} 缺标题 {}", row.title);
                        assert!(text.contains(&row.status), "{view:?} 缺状态 {}", row.status);
                    }
                }
                ViewId::Tasks => {
                    for row in &snapshot.tasks {
                        assert!(text.contains(&row.id), "{view:?} 缺 ID {}", row.id);
                        assert!(text.contains(&row.status), "{view:?} 缺状态 {}", row.status);
                    }
                }
                ViewId::Events => {
                    for row in &snapshot.events {
                        assert!(text.contains(&row.kind), "{view:?} 缺类型 {}", row.kind);
                    }
                }
                ViewId::Context | ViewId::Spec | ViewId::Diff => {
                    for line in snapshot.render_lines() {
                        assert!(
                            text.contains(line.as_str()),
                            "{view:?} 帧必须包含准备层数据行 {line:?}"
                        );
                    }
                }
            }
        }
    }

    /// T195：顶栏会话、动作反馈行与键位提示栏各占一行，反馈行位于键位栏之上。
    #[test]
    fn context_and_status_bars_occupy_their_own_rows() {
        use whcode_domain::i18n::I18n;

        let buffer = draw_snapshot_with_context(
            &fixture_snapshot(ViewId::Sessions),
            80,
            24,
            crate::Theme::EinkDay,
            &I18n::default(),
            Some("S-2"),
            "删除会话 完成",
        );
        let text = buffer_text(&buffer);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 24, "三区 + 反馈行必须正好 24 行");
        assert!(
            lines[0].contains("S-2"),
            "顶栏必须显示当前会话：{:?}",
            lines[0]
        );
        assert!(
            lines[22].contains("删除会话 完成"),
            "反馈行在键位提示栏之上：{:?}",
            lines[22]
        );
        assert!(
            lines[23].contains("q 退出"),
            "键位提示栏必须是末行：{:?}",
            lines[23]
        );
    }

    /// T195：CJK 标题按显示列截断——任何一行都不得越过终端列宽，超长以 `~` 收尾。
    #[test]
    fn cjk_columns_are_truncated_within_terminal_width() {
        use super::{SessionRow, ViewSnapshot};

        let mut snapshot = ViewSnapshot::empty(ViewId::Sessions);
        snapshot.sessions.push(SessionRow {
            id: "S-1".into(),
            title: "超长标题".repeat(20),
            stage: "implement".into(),
            status: "active".into(),
        });
        snapshot.selected_index = Some(0);
        let text = render_snapshot_text(&snapshot, 60, 10);
        for line in text.lines() {
            assert!(
                super::display_width(line) <= 60,
                "行宽越过 60 列：{line:?}（{} 列）",
                super::display_width(line)
            );
        }
        assert!(text.contains('~'), "超长标题必须截断并以 ~ 收尾");
        assert!(text.contains("S-1"), "ID 列必须保留");
    }

    /// T195 降级路径：窄/矮终端退化为纯正文，不产生零面积区域，也不 panic。
    #[test]
    fn narrow_terminal_falls_back_to_plain_body() {
        let text = render_empty_frame(ViewId::Context, 20, 5);
        assert_eq!(text.lines().count(), 5);
        assert!(text.contains("（暂无数据）"), "空态必须显式：{text}");
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
        // T195：正文单元（边框内）同样带主题色——块样式铺满区域，正文不再重复上色。
        let body_cell = 5 * 40 + 20;
        assert_eq!(eink.content()[body_cell].bg, Color::Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(eink.content()[body_cell].fg, Color::Rgb(0, 0, 0));
        assert_eq!(amoled.content()[body_cell].bg, Color::Rgb(0, 0, 0));
        assert_eq!(amoled.content()[body_cell].fg, Color::Rgb(0xE6, 0xE6, 0xE6));
        // Check 291：四套主题都必须精确进入渲染单元，不能只测其中两套。
        let night = frame_buffer(ViewId::Sessions, 40, 10, crate::Theme::EinkNight);
        assert_eq!(night.content()[0].bg, Color::Rgb(0, 0, 0));
        assert_eq!(night.content()[0].fg, Color::Rgb(0xFF, 0xFF, 0xFF));
        let green = frame_buffer(ViewId::Sessions, 40, 10, crate::Theme::TerminalGreen);
        assert_eq!(green.content()[0].bg, Color::Rgb(0, 0, 0));
        assert_eq!(green.content()[0].fg, Color::Rgb(0, 0xFF, 0));
    }

    /// Check 291：导航栏选中项同时具备 `>` 标记与反显；未选中项不得反显。
    #[test]
    fn selected_nav_item_has_marker_and_reverse_video() {
        use ratatui::style::Modifier;
        use whcode_domain::i18n::I18n;

        let buffer = draw_snapshot_with_context(
            &fixture_snapshot(ViewId::Sessions),
            80,
            24,
            crate::Theme::EinkDay,
            &I18n::default(),
            None,
            "",
        );
        let width = usize::from(buffer.area.width);
        // 顶栏占第 0 行；中部第 1 行是导航栏第一项（Sessions，当前选中）。
        let selected = &buffer.content()[width];
        assert_eq!(selected.symbol(), ">", "选中项必须以 > 标记");
        assert!(
            selected.modifier.contains(Modifier::REVERSED),
            "选中项必须反显：{:?}",
            selected.modifier
        );
        let other = &buffer.content()[2 * width];
        assert_ne!(other.symbol(), ">", "未选中项不得带选中标记");
        assert!(
            !other.modifier.contains(Modifier::REVERSED),
            "未选中项不得反显：{:?}",
            other.modifier
        );
    }

    /// Check 291：列视图的 ID／状态列在多行之间与表头对齐（显示列宽口径）。
    #[test]
    fn columns_align_with_header_across_rows() {
        use super::display_width;

        let text = render_fixture_frame(ViewId::Sessions, 80, 24);
        let lines: Vec<&str> = text.lines().collect();
        let header = lines
            .iter()
            .find(|line| line.contains("ID") && line.contains("标题"))
            .unwrap_or_else(|| panic!("缺少表头：{text}"));
        let id_start = display_width(&header[..header.find("ID").expect("表头 ID")]);
        let status_start = display_width(&header[..header.find("状态").expect("表头状态")]);

        let mut rows = 0usize;
        for id in ["S-1", "S-2", "S-3"] {
            let row = lines
                .iter()
                .find(|line| line.contains(id))
                .unwrap_or_else(|| panic!("缺少数据行 {id}：{text}"));
            assert_eq!(
                display_width(&row[..row.find(id).expect("数据 ID")]),
                id_start,
                "{id} 的 ID 列未与表头对齐"
            );
            assert_eq!(
                display_width(
                    &row[..row
                        .find("active")
                        .or_else(|| row.find("idle"))
                        .or_else(|| row.find("archived"))
                        .expect("状态值")]
                ),
                status_start,
                "{id} 的状态列未与表头对齐"
            );
            rows += 1;
        }
        assert_eq!(rows, 3, "夹具三行都必须参与对齐断言");
    }

    /// Check 291：极矮终端（高度低于三区下限）退化为纯正文且空态显式。
    #[test]
    fn very_short_terminal_falls_back_to_plain_body() {
        for rows in [1u16, 2, 5] {
            let text = render_empty_frame(ViewId::Context, 80, rows);
            assert_eq!(
                text.lines().count(),
                usize::from(rows),
                "极矮帧必须等于请求行数"
            );
            assert!(text.contains("（暂无数据）"), "极矮帧仍须显式空态：{text}");
        }
    }

    /// 空数据源帧：显式“暂无数据”，不伪造内容。
    #[test]
    fn empty_source_renders_explicit_placeholder() {
        let text = render_empty_frame(ViewId::Tasks, 60, 12);
        assert!(text.contains("（暂无数据）"), "空视图必须显式提示：{text}");
        assert!(text.contains("任务 (2/6)"), "顶栏必须含视图序号：{text}");
    }

    /// FR-615：英文文本层进入渲染帧（顶栏/键位栏/空提示），数据字段不翻译。
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
        assert!(
            text.contains("WHCode Coding Harness  |  Tasks (2/6)"),
            "英文顶栏：{text}"
        );
        assert!(text.contains("no session"), "英文顶栏会话占位：{text}");
        assert!(text.contains("70x12"), "英文键位栏窗口尺寸：{text}");
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
            "task": "T083/T129/T195",
            "fixture": { "view": "events", "rows": 10_000, "terminal": "80x24" },
            "measurement": {
                "warmup": WARMUP,
                "samples": SAMPLES,
                "unit": "microseconds",
                "clock": "std::time::Instant",
                "scope": "frame_path = draw_frame（三区布局：顶栏 + 导航栏 + 列投影主面板 + 键位栏）-> TestBackend buffer（含主题应用；不含文本提取）",
                "profile": if cfg!(debug_assertions) { "debug（未优化，数量级偏悲观；release 运行约低一个数量级）" } else { "release" }
            },
            "p50_us": p50,
            "p95_us": p95,
            "p99_us": p99,
            "reference_full_projection_us": full_projection_us,
            "budget": {
                "candidate_p95_us": CANDIDATE_P95_BUDGET_US,
                "frozen": false,
                "note": "UI 自定候选预算：不属于 NFR-201—210，不随 ADR-025 冻结；本报告仅作回归参照。T195 改为三区布局后同机 debug 实测 p95 约 2 000—2 400 µs（改造前单面板参照同机约 1 000—1 500 µs，debug 噪声 ±30%），绝对预算不变、余量仍 ≥ 84%"
            },
            "platform": "windows-amd64",
            "date": "2026-10-25"
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
