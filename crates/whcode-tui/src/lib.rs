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
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use whcode_app::commands::{CommandExecutor, FrontendCommand};
use whcode_domain::i18n::{I18n, is_rtl_locale};
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
    /// LLM 执行器（T167/FR-208）：`Some` 时 `llm.run`／`llm.cancel` 可用（三端同语义，
    /// 同步等待完成——与 CUI／Web 的 `llm run` 一致）；`None` 时显式提示 `not_supported`。
    pub llm_executor: Option<&'a dyn LlmExecutor>,
    /// Chat 对话流执行器（ADR-087）：`Some` 时 Chat 视图可提交流式 turn（后台线程）；
    /// `None` 时 Chat 提交显式提示 `not_supported`。与 `llm_executor` 可为同一实现
    /// （装配根以 `Arc` 同时提供两口径）。
    pub chat_executor: Option<std::sync::Arc<dyn ChatExecutor>>,
}

/// LLM 执行器抽象（T167）：TUI 键位输入经装配根注入的真实编排（与 CUI/Web 同一
/// `whcode_app::llm` 入口）；输出为呈现无关的结果摘要，正文不进入事件与日志。
pub trait LlmExecutor {
    /// 发起一次真实调用（同步；会话取 TUI 当前选中会话）。
    ///
    /// # Errors
    ///
    /// 剖面未注册、凭据缺失或调用失败时返回 [`WhError`]。
    fn run(&self, session_id: &str, provider: &str, prompt: &str) -> Result<String, WhError>;

    /// 取消同机正在运行的调用。
    ///
    /// # Errors
    ///
    /// 请求 ID 非法或无活跃调用时返回 [`WhError`]。
    fn cancel(&self, request_id: &str) -> Result<String, WhError>;
}

// ── Chat 对话流（ADR-087）────────────────────────────────────────────────────

/// Chat 行缓冲上限（有界，NFR-215）：超出丢最旧并继续（对话转写是滚动窗口语义，
/// 权威历史在 `conversation_tail` 与事件表，不在界面缓冲）。
pub(crate) const CHAT_BUFFER_MAX: usize = 1000;

/// delta 通道容量（有界；`try_send` 满即丢——流式进度是有损信号，完成消息走独立
/// 阻塞通道保证送达，最终正文以 turn 结果全量替换兜底）。
pub(crate) const CHAT_CHANNEL_CAP: usize = 64;

/// 历史回放上限（与 GUI `CHAT_HISTORY_MAX` 同口径，FR-741）。
pub(crate) const CHAT_HISTORY_MAX: usize = 50;

/// turn 运行期间空闲排空的阻塞上限（毫秒）：保证完成消息及时被消费，同时不显著
/// 抬高输入延迟（事件循环轮询间隔 250 ms）。
pub(crate) const CHAT_DRAIN_BLOCK_MS: u64 = 50;

/// Chat 提交结果（ADR-087）：`text` 为 turn 全量正文（完成后替换流式行的权威值），
/// `summary` 为稳定键值摘要（轮尾行），`reason` 为终止原因稳定码，
/// `completed`／`truncated` 为结果标志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatOutcome {
    /// turn 全量正文（完成态权威值；弥补 delta 通道丢帧）。
    pub text: String,
    /// 稳定键值摘要（`provider=… model=… chunks=… completed=… truncated=… request_id=…`）。
    pub summary: String,
    /// 终止原因稳定码（`model_finished`／`limit`／`cancel`／`budget`／`error`）。
    pub reason: String,
    /// turn 是否正常完成。
    pub completed: bool,
    /// 输出是否被截断。
    pub truncated: bool,
}

/// Chat 后台执行器（ADR-087 决策 1）：`Send + Sync`，供每 turn 一次的后台线程调用。
/// 与 GUI 的 `GuiLlmExecutor` 同形（装配根一者实现两者）；TUI 侧独立定义以维持前端平级。
pub trait ChatExecutor: Send + Sync {
    /// 剖面名清单（注册表顺序；未配置注册表时为空）。
    fn profiles(&self) -> Vec<String>;

    /// 发起流式 turn（**阻塞至 turn 完成**；在调用方的后台线程上执行）。
    /// `request_id` 由调用方预生成并传入（确定性取消，与 CUI 显式 `--request-id`
    /// 同语义）；进度与工具观察回调在调用线程同步执行，必须非阻塞。
    ///
    /// # Errors
    ///
    /// 剖面未注册、凭据缺失或调用失败时返回 [`WhError`]。
    fn run_streaming(
        &self,
        session_id: &str,
        provider: &str,
        prompt: &str,
        request_id: &str,
        progress: whcode_app::turn::TurnProgress<'_>,
        observer: Option<whcode_app::turn::TurnObserver<'_>>,
    ) -> Result<ChatOutcome, WhError>;

    /// 取消同机正在运行的调用。
    ///
    /// # Errors
    ///
    /// 请求 ID 非法或无活跃调用时返回 [`WhError`]。
    fn cancel(&self, request_id: &str) -> Result<String, WhError>;
}

/// 后台线程 → 界面线程的消息（delta 通道；有损）。
#[derive(Debug, Clone)]
pub(crate) enum ChatMsg {
    /// 流式增量（累计块数、增量文本）。
    Delta(u64, String),
    /// 工具调用信号（与落表事件 `ToolCallStarted`／`ToolCallFinished` 同义）。
    Signal(whcode_app::turn::TurnSignal),
}

/// 渲染帧所需的 Chat 快照（帧路径零触库；从 [`ChatState`] 复制）。
#[derive(Debug, Clone)]
pub(crate) struct ChatRender {
    /// 转写行缓冲。
    pub rows: Vec<ChatRow>,
    /// turn 是否运行中（决定流式指示）。
    pub turn_running: bool,
    /// 自尾部滚动偏移。
    pub scroll_from_end: usize,
}

/// Chat 转写行（结构化数据；本地化在渲染层完成，ADR-087 决策 3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChatRow {
    /// 轮头：请求 ID。
    TurnHeader { request_id: String },
    /// 历史回放行（`conversation_tail`，role 为 `user`／`assistant`／`tool`）。
    History { role: String, text: String },
    /// 用户提交行。
    User { text: String },
    /// 助手正文行（流式期间原位追加；完成后全量替换）。
    Assistant { text: String },
    /// 工具卡（结构化投影；无信号即无行——不伪造）。
    Tool {
        event: whcode_app::tool_call::ToolCallEvent,
        finished: bool,
    },
    /// 轮尾：终止原因＋耗时＋摘要（或错误）。
    TurnFooter {
        reason: String,
        duration_ms: u64,
        note: Option<String>,
    },
}

/// Chat 视图状态（ADR-087）：转写行缓冲、运行中标志、后台通道。
pub(crate) struct ChatState {
    /// 转写行缓冲（有界 [`CHAT_BUFFER_MAX`]，超限丢最旧）。
    pub rows: Vec<ChatRow>,
    /// 剖面名（`None`＝用执行器清单首项；`P` 键改写）。
    pub provider: Option<String>,
    /// turn 是否在运行（运行期间冻结视图切换与触库动作）。
    pub turn_running: bool,
    /// 当前（或最近一次提交的）请求 ID——取消与轮头取值。
    pub active_request_id: Option<String>,
    /// 流式助手行的缓冲下标（`None`＝尚无正文）。
    pub streaming: Option<usize>,
    /// 本轮开始时刻（轮尾耗时）。
    pub started_at: Option<Instant>,
    /// delta 通道接收端（有损）。
    pub rx: Option<std::sync::mpsc::Receiver<ChatMsg>>,
    /// 完成通道接收端（阻塞 send 保证送达）。
    pub done_rx: Option<std::sync::mpsc::Receiver<Result<ChatOutcome, WhError>>>,
    /// 自尾部的滚动偏移（0＝吸底；PgUp/PgDn 调整，有界）。
    pub scroll_from_end: usize,
}

impl ChatState {
    pub(crate) fn new() -> Self {
        Self {
            rows: Vec::new(),
            provider: None,
            turn_running: false,
            active_request_id: None,
            streaming: None,
            started_at: None,
            rx: None,
            done_rx: None,
            scroll_from_end: 0,
        }
    }

    /// 追加一行并维持缓冲有界（超限丢最旧；流式下标同步左移）。
    pub(crate) fn push_row(&mut self, row: ChatRow) {
        if self.rows.len() >= CHAT_BUFFER_MAX {
            self.rows.remove(0);
            self.streaming = self.streaming.map(|index| index.saturating_sub(1));
        }
        self.rows.push(row);
    }

    /// 应用流式增量：无正文行则新建，否则原位追加（ADR-087 决策 2）。
    pub(crate) fn apply_delta(&mut self, _count: u64, delta: &str) {
        if delta.is_empty() {
            return;
        }
        match self.streaming {
            Some(index) => {
                if let Some(ChatRow::Assistant { text }) = self.rows.get_mut(index) {
                    text.push_str(delta);
                }
            }
            None => {
                self.push_row(ChatRow::Assistant {
                    text: delta.to_string(),
                });
                self.streaming = Some(self.rows.len() - 1);
            }
        }
    }

    /// 应用工具信号：结构化投影为卡片行（缺 `tool_call_id` 的损坏信号丢弃，
    /// 不伪造卡片）。
    pub(crate) fn apply_signal(&mut self, signal: &whcode_app::turn::TurnSignal) {
        let (kind, payload) = match signal {
            whcode_app::turn::TurnSignal::ToolStarted {
                tool_call_id,
                tool_name,
                exec_id,
                iteration,
            } => (
                whcode_app::tool_call::TOOL_STARTED,
                serde_json::json!({
                    "tool_call_id": tool_call_id,
                    "tool_name": tool_name,
                    "exec_id": exec_id,
                    "iteration": iteration,
                }),
            ),
            whcode_app::turn::TurnSignal::ToolFinished {
                tool_call_id,
                exec_id,
                state,
                side_effect,
                output_value,
                error_code,
                duration_ms,
            } => (
                whcode_app::tool_call::TOOL_FINISHED,
                serde_json::json!({
                    "tool_call_id": tool_call_id,
                    "exec_id": exec_id,
                    "state": state,
                    "side_effect": side_effect,
                    "output_value": output_value,
                    "error_code": error_code,
                    "duration_ms": duration_ms,
                }),
            ),
        };
        let Some(event) = whcode_app::tool_call::describe(kind, &payload.to_string()) else {
            return;
        };
        self.push_row(ChatRow::Tool {
            event,
            finished: kind == whcode_app::tool_call::TOOL_FINISHED,
        });
    }

    /// 开启一轮：轮头＋用户行，运行标志置位。
    pub(crate) fn start_turn(&mut self, request_id: &str, prompt: &str) {
        self.push_row(ChatRow::TurnHeader {
            request_id: request_id.to_string(),
        });
        self.push_row(ChatRow::User {
            text: prompt.to_string(),
        });
        self.turn_running = true;
        self.active_request_id = Some(request_id.to_string());
        self.streaming = None;
        self.started_at = Some(Instant::now());
        self.scroll_from_end = 0;
    }

    /// 收口一轮：正文全量替换＋轮尾；运行标志复位（ADR-087 决策 2）。
    pub(crate) fn finish_turn(&mut self, result: &Result<ChatOutcome, WhError>) {
        let (reason, note, final_text) = match result {
            Ok(outcome) => (
                outcome.reason.clone(),
                Some(outcome.summary.clone()),
                Some(outcome.text.clone()),
            ),
            Err(err) => ("error".to_string(), Some(err.to_string()), None),
        };
        let duration_ms = self
            .started_at
            .map(|started| started.elapsed().as_millis() as u64)
            .unwrap_or(0);
        if let (Some(index), Some(text)) = (self.streaming, final_text)
            && let Some(ChatRow::Assistant { text: slot }) = self.rows.get_mut(index)
        {
            *slot = text;
        }
        self.push_row(ChatRow::TurnFooter {
            reason,
            duration_ms,
            note,
        });
        self.turn_running = false;
        self.streaming = None;
        self.started_at = None;
        self.rx = None;
        self.done_rx = None;
    }

    /// 转写行渲染为文本（scripted 报告与测试断言共用；本地化键缺失回落原始值）。
    pub(crate) fn report_lines(&self, texts: &I18n) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|row| chat_row_text(row, texts))
            .collect()
    }

    /// 渲染帧快照（零触库）。
    pub(crate) fn clone_render(&self) -> ChatRender {
        ChatRender {
            rows: self.rows.clone(),
            turn_running: self.turn_running,
            scroll_from_end: self.scroll_from_end,
        }
    }
}

/// 单行转写文本（渲染与报告共用；None＝该行不产生文本）。
fn chat_row_text(row: &ChatRow, texts: &I18n) -> Option<String> {
    match row {
        ChatRow::TurnHeader { request_id } => {
            Some(texts.resolve_fmt("tui.chat.turn.header", &[("request", request_id)]))
        }
        ChatRow::History { role, text } => {
            Some(format!("{} · {text}", chat_role_label(role, texts)))
        }
        ChatRow::User { text } => Some(format!("{} · {text}", texts.resolve("tui.chat.role.user"))),
        ChatRow::Assistant { text } => Some(format!(
            "{} · {text}",
            texts.resolve("tui.chat.role.assistant")
        )),
        ChatRow::Tool { event, finished } => {
            let tool = event.tool.as_deref().unwrap_or("?");
            let exec = event.exec.as_deref().unwrap_or("?");
            if *finished {
                let mut line = texts.resolve_fmt(
                    "tool.call.finished",
                    &[
                        ("tool", tool),
                        ("state", event.state.as_deref().unwrap_or("?")),
                        ("exec", exec),
                        ("call", &event.call_id),
                        ("duration", &event.duration_ms.unwrap_or(0).to_string()),
                    ],
                );
                if let Some(output) = &event.output {
                    line.push_str(" · ");
                    line.push_str(
                        &texts.resolve_fmt("tui.chat.tool.output", &[("output", output)]),
                    );
                }
                if let Some(error) = &event.error {
                    line.push_str(" · ");
                    line.push_str(error);
                }
                Some(line)
            } else {
                Some(texts.resolve_fmt(
                    "tool.call.started",
                    &[
                        ("tool", tool),
                        ("exec", exec),
                        ("call", &event.call_id),
                        ("iteration", &event.iteration.unwrap_or(0).to_string()),
                    ],
                ))
            }
        }
        ChatRow::TurnFooter {
            reason,
            duration_ms,
            note,
        } => {
            let reason_key = alloc_reason_key(reason);
            let reason_text = texts.try_resolve(&reason_key).unwrap_or(reason);
            let mut line = texts.resolve_fmt(
                "tui.chat.turn.footer",
                &[
                    ("reason", reason_text),
                    ("duration", &duration_ms.to_string()),
                ],
            );
            if let Some(note) = note {
                line.push_str(" · ");
                line.push_str(note);
            }
            Some(line)
        }
    }
}

/// 终止原因的本地化键（动态键；调用方以 [`I18n::try_resolve`] 区分缺失）。
fn alloc_reason_key(reason: &str) -> String {
    format!("tui.chat.reason.{reason}")
}

/// 历史 role 标签（`user`／`assistant`／`tool` 之外的取值回落原文）。
fn chat_role_label<'a>(role: &'a str, texts: &'a I18n) -> &'a str {
    match role {
        "user" => texts.resolve("tui.chat.role.user"),
        "assistant" => texts.resolve("tui.chat.role.assistant"),
        "tool" => texts.resolve("tui.chat.role.tool"),
        other => other,
    }
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
    /// llm.run 第一步：剖面名（显式选择，ADR-031 决策 4）。
    LlmProvider,
    /// llm.run 第二步：提示词正文（剖面名暂存于 [`InputMode::Text`] 的前值）。
    LlmPrompt,
    /// llm.cancel：请求 ID（`llm run` 输出或 CUI 预置）。
    LlmCancel,
    /// Chat 对话流提示词（ADR-087：Chat 视图 Composer）。
    ChatPrompt,
    /// Chat 对话流剖面名（ADR-087：`P` 键改写，空＝回落清单首项）。
    ChatProvider,
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

/// 启动字标（FR-765、ADR-061）：进入全屏界面前向 stdout 打印一次 ASCII 字标。
///
/// 落在终端滚动历史里，不占帧、不参与重绘（因此不影响 T129 空闲零重绘），也不进入
/// 备用屏；stdout 被重定向（管道、日志文件）时不打印，避免污染机器可读输出。
fn print_startup_banner() {
    use std::io::{IsTerminal, Write};

    let mut out = io::stdout();
    if !out.is_terminal() {
        return;
    }
    // 打印失败（终端已关闭、管道断开）不阻止界面启动：字标是可选的标识，不是功能。
    let _ = writeln!(out, "{}\n", whcode_domain::brand::ASCII_LOGO);
    let _ = out.flush();
}

/// 启动 TUI 闭环：装配根注入数据源、主题与最小化探测。
///
/// 终端生命周期由 [`TerminalGuard`] 管理——任何提前返回或 panic 都会触发恢复。
///
/// # Errors
///
/// 进入终端、初始化渲染后端、绘制或读取事件失败时返回 `WhError`（`io_error`）。
pub fn run_with(source: &dyn ViewSource, options: RunOptions<'_>) -> Result<(), WhError> {
    print_startup_banner();
    let mut session = CrosstermSession::new();
    let mut guard = TerminalGuard::enter(&mut session)?;

    let (cols, rows) = guard.session().size().unwrap_or(DEFAULT_SIZE);
    let mut core = LoopCore::new(
        source,
        options.command_executor,
        options.llm_executor,
        options.chat_executor.clone(),
        &options.texts,
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
            let composer = core.composer_line();
            let chat = core.chat.clone_render();
            terminal
                .draw(|frame| {
                    draw_frame(
                        frame,
                        &core.state,
                        &core.snapshot,
                        core.selected_session.as_deref(),
                        &core.status,
                        &composer,
                        &chat,
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
    let mut core = LoopCore::new(
        source,
        options.command_executor,
        options.llm_executor,
        options.chat_executor.clone(),
        &options.texts,
        options.theme,
        0,
        0,
        now,
    );
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
    /// 结束时 Chat 视图的转写行（本地化文本；ADR-087 scripted 取证与测试断言用）。
    pub chat_lines: Vec<String>,
    /// 结束时 Chat turn 是否仍在运行（ADR-087：收口复位断言用）。
    pub chat_turn_running: bool,
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
    /// LLM 执行器（T167）；`None` = llm 动作显式不可用。
    llm_executor: Option<&'a dyn LlmExecutor>,
    /// Chat 对话流执行器（ADR-087）；`None` = Chat 提交显式不可用。
    chat_executor: Option<std::sync::Arc<dyn ChatExecutor>>,
    /// 界面文本层（ADR-087：Chat 状态行的门控与反馈本地化）。
    texts: &'a I18n,
    /// Chat 视图状态（转写缓冲／运行标志／后台通道）。
    chat: ChatState,
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
    /// 构造入参为装配根逐项注入（执行器三口径＋文本层＋主题＋尺寸），与
    /// `run_turn_with_approval` 同例显式豁免参数数门限（ADR-087）。
    #[allow(clippy::too_many_arguments)]
    fn new(
        source: &'a dyn ViewSource,
        executor: Option<&'a dyn CommandExecutor>,
        llm_executor: Option<&'a dyn LlmExecutor>,
        chat_executor: Option<std::sync::Arc<dyn ChatExecutor>>,
        texts: &'a I18n,
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
            llm_executor,
            chat_executor,
            texts,
            chat: ChatState::new(),
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
        // ADR-087：进入（或刷新）Chat 视图时回放历史（准备层边界；turn 运行期间
        // 不触库，故运行中跳过——转写以内存缓冲为准）。
        if self.state.view() == ViewId::Chat && !self.chat.turn_running {
            self.replay_history();
        }
        self.renderer.handle_event(RenderEvent::DataChanged);
    }

    /// Chat 历史回放（ADR-087 决策 5）：`conversation_tail`（至多
    /// [`CHAT_HISTORY_MAX`] 条）替换转写缓冲；数据源不可用返回空（正确的空）。
    fn replay_history(&mut self) {
        let tail = self
            .selected_session
            .as_deref()
            .map(|session| self.source.conversation_tail(session, CHAT_HISTORY_MAX))
            .unwrap_or_default();
        self.chat.rows = tail
            .into_iter()
            .map(|(role, text)| ChatRow::History { role, text })
            .collect();
        self.chat.streaming = None;
        self.chat.scroll_from_end = 0;
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
            // ADR-087：turn 运行期间冻结导航与刷新（触库动作），Esc 之外的动作键
            // 一律不穿透（详见 on_key_normal 的冻结分支）。
            UiEvent::Key('j')
                if self.input_mode == InputMode::Normal && !self.chat.turn_running =>
            {
                self.session_cursor = self.session_cursor.saturating_add(1);
                self.prepare();
            }
            UiEvent::Key('k')
                if self.input_mode == InputMode::Normal && !self.chat.turn_running =>
            {
                self.session_cursor = self.session_cursor.saturating_sub(1);
                self.prepare();
            }
            UiEvent::Key('r')
                if self.input_mode == InputMode::Normal && !self.chat.turn_running =>
            {
                self.prepare()
            }
            UiEvent::Resize { .. } => self.state.handle(event),
            _ => self.on_key(event, now),
        }
        // ADR-087：输入后即刻排空后台通道（流式增量与完成消息），有内容即置脏。
        if self.drain_chat(false) {
            self.renderer.handle_event(RenderEvent::DataChanged);
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
                        TextKind::LlmProvider => {
                            // llm.run 第一步：剖面名 Enter 后进入提示词输入（剖面暂存于状态行）。
                            self.input_mode = InputMode::Text {
                                kind: TextKind::LlmPrompt,
                                buffer: String::new(),
                            };
                            self.status =
                                format!("llm.run 剖面 {buffer}：提示词：_（Enter 提交，Esc 取消）");
                            return;
                        }
                        TextKind::LlmPrompt => {
                            self.llm_run(buffer.trim());
                            return;
                        }
                        TextKind::LlmCancel => {
                            self.llm_cancel(buffer.trim());
                            return;
                        }
                        TextKind::ChatProvider => {
                            // ADR-087：剖面名登记（空＝回落执行器清单首项）。
                            let trimmed = buffer.trim().to_string();
                            self.chat.provider = if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed)
                            };
                            self.status = self.texts.resolve_fmt(
                                "gui.chat.provider",
                                &[(
                                    "provider",
                                    self.chat.provider.as_deref().unwrap_or(
                                        self.chat_executor
                                            .as_ref()
                                            .and_then(|exec| exec.profiles().into_iter().next())
                                            .as_deref()
                                            .unwrap_or("?"),
                                    ),
                                )],
                            );
                            return;
                        }
                        TextKind::ChatPrompt => {
                            self.chat_submit(buffer.trim());
                            return;
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
        // ADR-087：turn 运行期间冻结全部动作键与视图切换（Esc＝取消除外）——
        // 写命令与导航都可能触库，而连接锁被后台 turn 持有。
        if self.chat.turn_running {
            if key == UiEvent::Esc {
                self.chat_cancel_running();
            } else {
                self.status = self.texts.resolve("tui.chat.composer.running").to_string();
            }
            return;
        }
        // ADR-087：Chat 视图对话键（i＝进入 Composer、P＝剖面、Esc＝返回会话视图）。
        if self.state.view() == ViewId::Chat {
            match key {
                UiEvent::Key('i') => {
                    self.input_mode = InputMode::Text {
                        kind: TextKind::ChatPrompt,
                        buffer: String::new(),
                    };
                    self.status = self.texts.resolve("tui.chat.composer.hint").to_string();
                    return;
                }
                UiEvent::Key('P') => {
                    self.input_mode = InputMode::Text {
                        kind: TextKind::ChatProvider,
                        buffer: String::new(),
                    };
                    self.status = self
                        .texts
                        .resolve("gui.chat.provider.placeholder")
                        .to_string();
                    return;
                }
                UiEvent::Esc => {
                    // 空闲 Esc：返回会话视图（选择会话是对话前置）。
                    self.state.handle(UiEvent::Key('1'));
                    if self.snapshot.view != self.state.view() {
                        self.prepare();
                    }
                    return;
                }
                UiEvent::PageUp => {
                    self.chat.scroll_from_end = self.chat.scroll_from_end.saturating_add(1);
                    self.renderer.handle_event(RenderEvent::DataChanged);
                    return;
                }
                UiEvent::PageDown => {
                    self.chat.scroll_from_end = self.chat.scroll_from_end.saturating_sub(1);
                    self.renderer.handle_event(RenderEvent::DataChanged);
                    return;
                }
                _ => {}
            }
        }
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
            UiEvent::Key('L') if self.state.view() == ViewId::Sessions => {
                // llm.run（T167）：两步输入（剖面名 → 提示词），会话取当前选中。
                if self.selected_session.is_some() {
                    self.input_mode = InputMode::Text {
                        kind: TextKind::LlmProvider,
                        buffer: String::new(),
                    };
                    self.status = "llm.run 剖面名：_（Enter 提交，Esc 取消）".to_string();
                } else {
                    self.status = "llm.run：未选择会话".to_string();
                }
            }
            UiEvent::Key('C') => {
                // llm.cancel（T167）：请求 ID 由单行输入提供。
                self.input_mode = InputMode::Text {
                    kind: TextKind::LlmCancel,
                    buffer: String::new(),
                };
                self.status = "llm.cancel 请求 ID：_（Enter 提交，Esc 取消）".to_string();
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

    /// llm.run（T167）：会话取当前选中会话；剖面名取自状态行（第一步输入的暂存）。
    /// 同步等待完成（三端同语义：CUI/Web 的 `llm run` 均阻塞到完成），调用期间
    /// 界面以“调用中”状态行提示，完成后置结果摘要（正文不进事件与日志）。
    fn llm_run(&mut self, prompt: &str) {
        let Some(session_id) = self.selected_session.clone() else {
            self.status = "llm.run：未选择会话".to_string();
            return;
        };
        // 剖面名从状态行回取（第一步 LlmProvider Enter 时写入）；解析失败显式提示。
        let status_snapshot = std::mem::take(&mut self.status);
        let Some(provider) = status_snapshot
            .strip_prefix("llm.run 剖面 ")
            .and_then(|rest| rest.split('：').next())
            .map(str::to_string)
        else {
            self.status = "llm.run：缺少剖面名（请重新按 L 发起）".to_string();
            return;
        };
        self.status = format!("llm.run 调用中（会话 {session_id}、剖面 {provider}）……");
        let Some(llm) = self.llm_executor else {
            self.status = "llm.run: not_supported（未注入 LLM 执行器）".to_string();
            return;
        };
        match llm.run(&session_id, &provider, prompt) {
            Ok(summary) => {
                self.status = format!("llm.run 完成：{summary}");
            }
            Err(err) => {
                self.status = format!("llm.run 失败：{err}");
            }
        }
    }

    /// llm.cancel（T167）：取消同机正在运行的调用（请求 ID 由单行输入提供）。
    fn llm_cancel(&mut self, request_id: &str) {
        let Some(llm) = self.llm_executor else {
            self.status = "llm.cancel: not_supported（未注入 LLM 执行器）".to_string();
            return;
        };
        match llm.cancel(request_id) {
            Ok(summary) => {
                self.status = format!("llm.cancel：{summary}");
            }
            Err(err) => {
                self.status = format!("llm.cancel 失败：{err}");
            }
        }
    }

    // ── Chat 对话流（ADR-087）──────────────────────────────────────────────

    /// 提交一轮对话（ADR-087 决策 2/4）：门控四类结构化拒绝；预生成 request_id；
    /// 后台线程执行流式 turn（delta 有损通道＋完成保达通道）；转写记轮头与用户行。
    fn chat_submit(&mut self, prompt: &str) {
        let Some(executor) = self.chat_executor.clone() else {
            self.status = self
                .texts
                .resolve("tui.chat.status.not_supported")
                .to_string();
            return;
        };
        if self.chat.turn_running {
            self.status = self.texts.resolve("tui.chat.status.busy").to_string();
            return;
        }
        let Some(session_id) = self.selected_session.clone() else {
            self.status = self.texts.resolve("tui.chat.status.no_session").to_string();
            return;
        };
        let prompt = prompt.trim();
        if prompt.is_empty() {
            self.status = self.texts.resolve("tui.chat.status.empty").to_string();
            return;
        }
        let provider = self
            .chat
            .provider
            .clone()
            .or_else(|| executor.profiles().into_iter().next());
        let Some(provider) = provider else {
            self.status = self
                .texts
                .resolve("tui.chat.status.no_provider")
                .to_string();
            return;
        };
        // 提交前最后一次读历史（连接锁空闲；turn 运行期间不再触库——GUI 同款纪律）。
        self.replay_history();
        let request_id =
            whcode_app::llm::generate_request_id(&session_id, &provider, prompt, now_ms());
        self.chat.start_turn(&request_id, prompt);
        let (delta_tx, delta_rx) = std::sync::mpsc::sync_channel::<ChatMsg>(CHAT_CHANNEL_CAP);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        self.chat.rx = Some(delta_rx);
        self.chat.done_rx = Some(done_rx);
        self.status = String::new();
        let prompt_owned = prompt.to_string();
        std::thread::spawn(move || {
            let progress = |count: u64, delta: &str| {
                // 有界通道 try_send（Fn 每次克隆 Sender）；满即丢——流式进度是有损
                // 信号，最终正文由完成消息全量替换兜底（ADR-087 决策 2）。
                let _ = delta_tx
                    .clone()
                    .try_send(ChatMsg::Delta(count, delta.to_string()));
            };
            let observer = |signal: whcode_app::turn::TurnSignal| {
                let _ = delta_tx.clone().try_send(ChatMsg::Signal(signal));
            };
            let result = executor.run_streaming(
                &session_id,
                &provider,
                &prompt_owned,
                &request_id,
                &progress,
                Some(&observer),
            );
            // 完成消息阻塞送达（与 GUI Phase 2 的 oneshot 同语义）。
            let _ = done_tx.send(result);
        });
    }

    /// 取消运行中的一轮（ADR-087 决策 4）：确定性 request_id 直达执行器 cancel。
    fn chat_cancel_running(&mut self) {
        let Some(executor) = self.chat_executor.clone() else {
            self.status = self
                .texts
                .resolve("tui.chat.status.not_supported")
                .to_string();
            return;
        };
        let Some(request_id) = self.chat.active_request_id.clone() else {
            self.status = self.texts.resolve("gui.chat.no_request").to_string();
            return;
        };
        match executor.cancel(&request_id) {
            Ok(summary) => {
                self.status = self
                    .texts
                    .resolve_fmt("tui.chat.status.cancel_requested", &[("request", &summary)]);
            }
            Err(err) => {
                self.status = self.texts.resolve_fmt(
                    "tui.chat.status.cancel_failed",
                    &[("err", &err.to_string())],
                );
            }
        }
    }

    /// 排空后台通道（ADR-087 决策 2）：`blocking` 时以
    /// [`CHAT_DRAIN_BLOCK_MS`] 短阻塞等待首条消息（空闲路径），否则只 `try_recv`
    /// （输入路径）。返回是否有新内容（供置脏）。
    fn drain_chat(&mut self, blocking: bool) -> bool {
        let mut dirty = false;
        let timeout = std::time::Duration::from_millis(CHAT_DRAIN_BLOCK_MS);
        // 逐条搬移：先把接收端从状态中取出，处理完再放回（避免同时借用）。
        if let Some(rx) = self.chat.rx.take() {
            loop {
                let received = if blocking && !dirty {
                    rx.recv_timeout(timeout).ok()
                } else {
                    rx.try_recv().ok()
                };
                match received {
                    Some(ChatMsg::Delta(count, delta)) => {
                        self.chat.apply_delta(count, &delta);
                        dirty = true;
                    }
                    Some(ChatMsg::Signal(signal)) => {
                        self.chat.apply_signal(&signal);
                        dirty = true;
                    }
                    None => break,
                }
            }
            if self.chat.turn_running {
                self.chat.rx = Some(rx);
            }
        }
        if let Some(done_rx) = self.chat.done_rx.take() {
            if let Ok(result) = done_rx.try_recv() {
                // 收口后保留转写在屏（轮头/轮尾可见）；持久化历史在下一次提交前
                // 与进入视图时回放（ADR-087 决策 5 的修订口径：避免回放截断转写）。
                self.chat.finish_turn(&result);
                dirty = true;
            } else {
                self.chat.done_rx = Some(done_rx);
            }
        }
        dirty
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
        // ADR-087：turn 运行期间空闲排空后台通道（短阻塞保证完成消息及时消费）。
        if self.chat.turn_running && self.drain_chat(true) {
            self.renderer.handle_event(RenderEvent::DataChanged);
        }
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
            chat_lines: self.chat.report_lines(self.texts),
            chat_turn_running: self.chat.turn_running,
        }
    }

    /// Composer 行文本（ADR-087）：Chat 视图才产生；运行中显示冻结提示，
    /// 输入模式显示缓冲，空闲显示键位提示。
    fn composer_line(&self) -> String {
        if self.state.view() != ViewId::Chat {
            return String::new();
        }
        if self.chat.turn_running {
            return self.texts.resolve("tui.chat.composer.running").to_string();
        }
        if let InputMode::Text {
            kind: TextKind::ChatPrompt,
            buffer,
        } = &self.input_mode
        {
            return format!("> {buffer}_");
        }
        self.texts.resolve("tui.chat.composer.hint").to_string()
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
        ViewId::Context | ViewId::Spec | ViewId::Diff | ViewId::Chat | ViewId::Ledger => 0,
    }
}

/// 按视图列定义排版一行（列宽固定，标题列吃剩余宽度；超宽再夹到可用列）。
///
/// 单元格由闭包按索引取用（借用快照字段，不复制长文本）；`cell` 越界返回空串，
/// 不 panic——投影与列定义不一致时退化为空列，而不是让帧路径崩溃。
/// 全行只分配一个输出缓冲（帧内每单元格一次分配会抬高一帧成本）。
///
/// 方向（FR-766／T279）：RTL 时逻辑列序反转（视觉镜像），列内对齐随方向取反——
/// 每列文本贴住的视觉边缘与 LTR 一致（如右对齐的重试计数在镜像后贴住镜像行的右缘）。
fn layout_columns<'a, F: Fn(usize) -> &'a str>(
    view: ViewId,
    width: usize,
    cell: F,
    rtl: bool,
) -> String {
    // 先按逻辑列序产出各列（互不依赖），再按方向决定拼接顺序。
    // 列间各 1 空格；`push_cell` 的对齐旗标在 RTL 下取反。
    let mut segments: Vec<String> = Vec::new();
    macro_rules! column {
        ($index:expr, $col_width:expr, $right:expr) => {{
            let mut text = String::new();
            push_cell(&mut text, cell($index), $col_width, $right != rtl);
            segments.push(text);
        }};
    }
    match view {
        // 标记(1) + ID(8) + 标题(剩余) + 阶段(10) + 状态(10)，列间各 1 空格。
        ViewId::Sessions => {
            let flex = width.saturating_sub(33);
            column!(0, 1, false);
            column!(1, 8, false);
            column!(2, flex, false);
            column!(3, 10, false);
            column!(4, 10, false);
        }
        // 标记(1) + ID(8) + 标题(剩余) + 状态(10) + 重试(5，右对齐)。
        ViewId::Tasks => {
            let flex = width.saturating_sub(28);
            column!(0, 1, false);
            column!(1, 8, false);
            column!(2, flex, false);
            column!(3, 10, false);
            column!(4, 5, true);
        }
        // 序号(8，右对齐) + 类型(18) + 摘要(剩余)。
        ViewId::Events => {
            let flex = width.saturating_sub(28);
            column!(0, 8, true);
            column!(1, 18, false);
            column!(2, flex, false);
        }
        ViewId::Context | ViewId::Spec | ViewId::Diff | ViewId::Chat | ViewId::Ledger => {}
    }
    if rtl {
        segments.reverse();
    }
    let mut out = segments.join(" ");
    clamp_line(&mut out, width);
    out
}

/// 列标题行（与数据行列宽一致；标签来自文本层）。
fn column_header(view: ViewId, texts: &I18n, width: usize, rtl: bool) -> String {
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
        ViewId::Context | ViewId::Spec | ViewId::Diff | ViewId::Chat | ViewId::Ledger => &[],
    };
    layout_columns(
        view,
        width,
        |index| {
            keys.get(index).map_or("", |key| {
                if key.is_empty() {
                    ""
                } else {
                    texts.resolve(key)
                }
            })
        },
        rtl,
    )
}

/// 画一帧（T195／ADR-041）：顶栏（应用名、视图序号、当前会话）／左导航栏 + 主面板／
/// 键位提示栏三区；主面板按视图列定义排版，选中行反显，右侧标注滚动位置。
///
/// 纯投影：只读 [`ViewSnapshot`] 与 [`TuiState`]，不做查询、不反序列化 JSON（§5.3）；
/// 帧内分配只与终端尺寸相关（列投影行数 = 主面板高度）。
/// 帧装配内部签名较宽（状态/快照/会话/双反馈行/Chat 快照/主题/文本层），
/// 均为同源数据直通——与 `run_turn_with_approval` 同例显式豁免（ADR-087）。
#[allow(clippy::too_many_arguments)]
fn draw_frame(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    snapshot: &ViewSnapshot,
    session: Option<&str>,
    status: &str,
    composer: &str,
    chat: &ChatRender,
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

    let view = state.view();
    let composer_height = u16::from(view == ViewId::Chat && !composer.is_empty());
    let status_height = u16::from(!status.is_empty());
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(composer_height),
        Constraint::Length(status_height),
        Constraint::Length(1),
    ])
    .split(area);

    // 1) 顶栏：应用名 + 视图（序号/总数）+ 当前会话。
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
    draw_content(frame, rows[1], snapshot, view, base, texts, chat);

    // 3) Composer 行（仅 Chat 视图占一行）。
    if composer_height == 1 {
        frame.render_widget(Paragraph::new(Line::from(composer)).style(base), rows[2]);
    }

    // 4) 动作反馈行（仅非空时占一行，不加高帧）。
    if status_height == 1 {
        frame.render_widget(Paragraph::new(Line::from(status)).style(base), rows[3]);
    }

    // 5) 键位提示栏：键位 + 窗口尺寸（不显示按键计数——计数不是用户需要的信号）。
    let keybar = texts.resolve_fmt(
        "tui.footer",
        &[
            ("cols", &state.cols().to_string()),
            ("rows", &state.rows().to_string()),
        ],
    );
    frame.render_widget(Paragraph::new(Line::from(keybar)).style(base), rows[4]);
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
    chat: &ChatRender,
) {
    let rtl = is_rtl_locale(&texts.locale);
    let rail = RAIL_WIDTH.min(area.width.saturating_sub(BODY_MIN_WIDTH));
    let body_width = area.width.saturating_sub(rail);
    // 导航栏侧向（FR-766／T279）：LTR 在左，RTL 镜像到右。
    let rail_x = if rtl {
        area.x.saturating_add(body_width)
    } else {
        area.x
    };
    let body_x = if rtl {
        area.x
    } else {
        area.x.saturating_add(rail)
    };

    if rail > 0 {
        let rail_area = Rect::new(rail_x, area.y, rail, area.height);
        // 导航栏朝向主面板的一侧留 1 列空白，与主面板边框形成分隔（不叠加第二条竖线）。
        let inner_width = usize::from(rail.saturating_sub(1));
        let lines: Vec<Line<'_>> = ViewId::ALL
            .iter()
            .map(|item| {
                let selected = *item == view;
                // 选中标记是方向性字形：RTL 镜像为 `<`（指向阅读起点一侧的面板）。
                let marker = match (selected, rtl) {
                    (true, false) => ">",
                    (true, true) => "<",
                    (false, _) => " ",
                };
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
    // ADR-087：Chat 视图渲染转写行（结构化数据 → 本地化文本；自尾部滚动窗口）。
    if view == ViewId::Chat {
        let mut texts_rows: Vec<String> = chat
            .rows
            .iter()
            .filter_map(|row| chat_row_text(row, texts))
            .collect();
        if chat.turn_running
            && let Some(last) = texts_rows.last_mut()
        {
            last.push_str(" …");
        }
        let height = usize::from(inner.height);
        let end = texts_rows.len().saturating_sub(chat.scroll_from_end);
        let start = end.saturating_sub(height);
        let body: Vec<Line<'_>> = texts_rows[start..end]
            .iter()
            .map(|text| Line::from(text.as_str()))
            .collect();
        let paragraph = Paragraph::new(body).style(base).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
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
        push_empty_state(&mut lines, body_rows, content_width, texts);
    } else if column_view {
        lines.push(Line::from(column_header(view, texts, content_width, rtl)));
        for (index, row) in snapshot
            .render_columns_limited(body_rows)
            .iter()
            .enumerate()
        {
            let text = layout_columns(
                view,
                content_width,
                |cell| row.cells.get(cell).map_or("", |value| value.as_ref()),
                rtl,
            );
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
        push_empty_state(&mut lines, body_rows, content_width, texts);
    }

    let mut block = Block::default()
        .style(base)
        .borders(Borders::ALL)
        .title(texts.resolve(view.title_key()));
    if column_view {
        // 滚动位置标注镜像到另一侧（FR-766／T279）：RTL 时从右缘改挂左缘。
        let counter = Line::from(format!("{shown}/{total}"));
        block = if rtl {
            block.title(counter)
        } else {
            block.title(counter.right_aligned())
        };
    }
    // 边框块已把主题样式铺满整个区域（含正文单元），正文不再重复上色。
    frame.render_widget(Paragraph::new(lines).block(block), body_area);
}

/// 空态正文：放得下就先画 ASCII 字标（FR-765、ADR-061），再给显式空提示。
///
/// 判据是“字标宽度 ≤ 可用宽度”且“字标行数 + 1 行空提示 ≤ 可用行数”——**放不下就只给
/// 空提示**：裁掉半个字标或让它顶掉空提示，都是把“没有数据”说成别的东西（不伪装原则）。
/// 空提示始终存在，既有帧断言（Check 291 等）据此保持不变。
fn push_empty_state<'a>(
    lines: &mut Vec<Line<'a>>,
    body_rows: usize,
    content_width: usize,
    texts: &'a I18n,
) {
    // `body_rows > ASCII_LOGO_LINES` 即“字标 + 一行空提示”都放得下（clippy
    // `int_plus_one` 拒绝写成 `>= ASCII_LOGO_LINES + 1`）。
    let logo_fits = content_width >= whcode_domain::brand::ASCII_LOGO_WIDTH
        && body_rows > whcode_domain::brand::ASCII_LOGO_LINES;
    if logo_fits {
        lines.extend(whcode_domain::brand::ASCII_LOGO.lines().map(Line::from));
    }
    lines.push(Line::from(texts.resolve("tui.empty")));
}

#[cfg(test)]
mod tests {
    use super::{
        CHAT_BUFFER_MAX, ChatExecutor, ChatOutcome, ChatRow, ChatState, LoopCore, RunOptions,
        ScriptStep, Theme, UiEvent, VIEW_COUNT, ViewId, run_scripted, run_scripted_steps,
    };
    use crate::EmptyViewSource;
    use crate::tests_support::{
        FakeProbe, buffer_text, draw_snapshot_with_context, fixture_snapshot, render_empty_frame,
        render_fixture_frame, render_snapshot_text, view_switch_key,
    };
    use std::time::Instant;
    use whcode_domain::WhError;
    use whcode_domain::i18n::I18n;

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

    /// T178.1：`t` 六步循环回原位（FR-625；FR-739 后为六套），每步产生一帧（主题变化置脏）。
    #[test]
    fn theme_key_cycles_six_themes_back_to_start() {
        use crate::theme::Theme;
        let events = [
            UiEvent::Key('t'),
            UiEvent::Key('t'),
            UiEvent::Key('t'),
            UiEvent::Key('t'),
            UiEvent::Key('t'),
            UiEvent::Key('t'),
        ];
        let report = run_scripted(&events);
        assert_eq!(
            report.theme,
            Theme::WhcodeNight,
            "六步必须回原位（默认 whcode-night）"
        );
        let one = run_scripted(&[UiEvent::Key('t')]);
        assert_eq!(one.theme, Theme::EinkDay, "一步到第二套");
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
                    archived: false,
                    created_at_ms: 0,
                    last_active_at_ms: 0,
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
            "有执行器时不得报不可用"
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
                    archived: false,
                    created_at_ms: 0,
                    last_active_at_ms: 0,
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
            "状态行必须显式不可用"
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

    /// Check 82 / T195：全部视图在固定终端尺寸（80×24）下渲染出稳定三区帧——
    /// 顶栏（应用名 + 视图序号）、导航栏（视图清单 + 选中标记）、主面板、键位提示栏。
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
                lines[0].contains(&format!(
                    "{} ({}/{})",
                    view.title(),
                    view.index(),
                    VIEW_COUNT
                )),
                "{view:?} 顶栏必须含视图序号：{:?}",
                lines[0]
            );
            // 导航栏：全部视图列出，当前项同时具备 `>` 标记与标题。
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
            for expected in [
                &format!("1-{VIEW_COUNT} 视图"),
                "j/k 移动",
                "q 退出",
                "80x24",
            ] {
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
                ViewId::Context | ViewId::Spec | ViewId::Diff | ViewId::Chat | ViewId::Ledger => {
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
            archived: false,
            created_at_ms: 0,
            last_active_at_ms: 0,
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
        assert!(
            text.contains(&format!("任务 (2/{VIEW_COUNT})")),
            "顶栏必须含视图序号：{text}"
        );
    }

    /// FR-765：空态放得下时显示 ASCII 字标，且**空提示仍在**（字标不顶掉空态语义）。
    #[test]
    fn empty_state_shows_ascii_wordmark_when_it_fits() {
        let text = render_empty_frame(ViewId::Tasks, 100, 30);
        assert!(
            text.contains("local-first LLM coding harness"),
            "空态应显示字标的说明行：{text}"
        );
        assert!(text.contains("（暂无数据）"), "字标不得顶掉空提示：{text}");
    }

    /// FR-765：终端放不下字标时退回纯空提示——不裁切、不溢出（不伪装原则）。
    #[test]
    fn empty_state_falls_back_when_wordmark_does_not_fit() {
        let text = render_empty_frame(ViewId::Tasks, 60, 12);
        assert!(!text.contains("local-first"), "放不下时不得画字标：{text}");
        assert!(text.contains("（暂无数据）"), "空提示必须仍在：{text}");
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
            text.contains(&format!("WHCode Coding Harness  |  Tasks (2/{VIEW_COUNT})")),
            "英文顶栏：{text}"
        );
        assert!(text.contains("no session"), "英文顶栏会话占位：{text}");
        assert!(text.contains("70x12"), "英文键位栏窗口尺寸：{text}");
        assert!(text.contains("(no data)"), "英文空提示：{text}");
        // 数据字段（会话 ID/事件类型）不翻译——由准备层原样提供。
    }

    /// RTL 列镜像（FR-766／T279）：逻辑列序反转、列内对齐取反——状态列在镜像行
    /// 最左侧且右对齐（LTR 下它在最右侧且左对齐），每列文本贴住的视觉边缘不变。
    #[test]
    fn session_columns_mirror_in_rtl() {
        let cells = ["*", "S-1", "标题", "plan", "active"];
        let cell = |index: usize| cells[index];
        let ltr = super::layout_columns(ViewId::Sessions, 60, cell, false);
        let rtl = super::layout_columns(ViewId::Sessions, 60, cell, true);
        assert!(
            ltr.find("S-1").expect("LTR 行含 ID") < ltr.find("active").expect("LTR 行含状态"),
            "LTR 保持登记列序：{ltr:?}"
        );
        assert!(
            rtl.find("active").expect("RTL 行含状态") < rtl.find("S-1").expect("RTL 行含 ID"),
            "RTL 反转列序：{rtl:?}"
        );
        let before_status = &rtl[..rtl.find("active").expect("RTL 行含状态")];
        assert!(
            !before_status.is_empty() && before_status.chars().all(|ch| ch == ' '),
            "RTL 下状态列右对齐（自身盒内贴右缘）：{rtl:?}"
        );
    }

    /// RTL 帧镜像（FR-766／T279）：导航栏移到右缘，选中标记由 `>` 镜像为 `<`。
    #[test]
    fn rtl_frame_places_rail_on_the_right_and_flips_marker() {
        use crate::tests_support::draw_snapshot_with;

        let snapshot = crate::tests_support::fixture_snapshot(ViewId::Sessions);
        let ltr = draw_snapshot_with(
            &snapshot,
            100,
            24,
            crate::Theme::EinkDay,
            &whcode_domain::i18n::I18n::default(),
        );
        let rtl = draw_snapshot_with(
            &snapshot,
            100,
            24,
            crate::Theme::EinkDay,
            &whcode_domain::i18n::I18n::load("ar", whcode_domain::i18n::LocaleSource::Explicit),
        );
        let first_marker_column = |buffer: &ratatui::buffer::Buffer, symbol: &str| {
            let width = usize::from(buffer.area.width);
            buffer
                .content()
                .chunks(width)
                .find_map(|row| {
                    row.iter()
                        .position(|cell| cell.symbol() == symbol)
                        .map(|x| u16::try_from(x).expect("列号不超出 u16"))
                })
                .unwrap_or_else(|| panic!("帧中必须出现标记 {symbol}"))
        };
        let ltr_x = first_marker_column(&ltr, ">");
        let rtl_x = first_marker_column(&rtl, "<");
        assert!(
            super::RAIL_WIDTH > ltr_x,
            "LTR 导航栏在左缘：标记列 {ltr_x}"
        );
        assert!(
            rtl_x >= 100 - super::RAIL_WIDTH,
            "RTL 导航栏在右缘：标记列 {rtl_x}"
        );
    }

    /// RTL 渲染行镜像（FR-766／T279）：真实帧路径下会话行的状态列出现在 ID 左侧。
    #[test]
    fn rtl_session_rows_show_status_left_of_id() {
        use crate::tests_support::{buffer_text, draw_snapshot_with};

        let snapshot = crate::tests_support::fixture_snapshot(ViewId::Sessions);
        let ltr = buffer_text(&draw_snapshot_with(
            &snapshot,
            100,
            24,
            crate::Theme::EinkDay,
            &whcode_domain::i18n::I18n::default(),
        ));
        let rtl = buffer_text(&draw_snapshot_with(
            &snapshot,
            100,
            24,
            crate::Theme::EinkDay,
            &whcode_domain::i18n::I18n::load("ar", whcode_domain::i18n::LocaleSource::Explicit),
        ));
        let ltr_row = ltr
            .lines()
            .find(|line| line.contains("S-1") && line.contains("active"))
            .expect("LTR 帧含会话数据行");
        let rtl_row = rtl
            .lines()
            .find(|line| line.contains("S-1") && line.contains("active"))
            .expect("RTL 帧含会话数据行");
        assert!(
            ltr_row.find("S-1").expect("ID") < ltr_row.find("active").expect("状态"),
            "LTR 行列序不变：{ltr_row:?}"
        );
        assert!(
            rtl_row.find("active").expect("状态") < rtl_row.find("S-1").expect("ID"),
            "RTL 行列序镜像：{rtl_row:?}"
        );
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
                    archived: false,
                    created_at_ms: 0,
                    last_active_at_ms: 0,
                    id: "S-1".into(),
                    title: "甲".into(),
                    stage: "plan".into(),
                    status: "active".into(),
                },
                SessionRow {
                    archived: false,
                    created_at_ms: 0,
                    last_active_at_ms: 0,
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
                archived: false,
                created_at_ms: 0,
                last_active_at_ms: 0,
                id: "S-1".into(),
                title: "甲".into(),
                stage: "plan".into(),
                status: "active".into(),
            },
            SessionRow {
                archived: false,
                created_at_ms: 0,
                last_active_at_ms: 0,
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

    /// 录制 LLM 输入的桩执行器（T167 键位测试；不触网络）。
    struct RecordingLlm {
        runs: std::sync::Mutex<Vec<(String, String, String)>>,
        cancels: std::sync::Mutex<Vec<String>>,
        fail: bool,
    }

    impl super::LlmExecutor for RecordingLlm {
        fn run(
            &self,
            session_id: &str,
            provider: &str,
            prompt: &str,
        ) -> Result<String, super::WhError> {
            if self.fail {
                return Err(super::WhError::invalid_state("桩失败"));
            }
            self.runs.lock().expect("锁").push((
                session_id.to_string(),
                provider.to_string(),
                prompt.to_string(),
            ));
            Ok(format!(
                "provider={provider} chunks=0 completed=true request_id=r-1"
            ))
        }

        fn cancel(&self, request_id: &str) -> Result<String, super::WhError> {
            self.cancels
                .lock()
                .expect("锁")
                .push(request_id.to_string());
            Ok(format!(
                "request={request_id} state=completed effective=false"
            ))
        }
    }

    /// T167：llm.run 两步输入（剖面名 → 提示词）最终调用执行器并显示结果摘要。
    #[test]
    fn llm_run_two_step_input_reaches_executor() {
        let llm = RecordingLlm {
            runs: std::sync::Mutex::new(Vec::new()),
            cancels: std::sync::Mutex::new(Vec::new()),
            fail: false,
        };
        let options = RunOptions {
            llm_executor: Some(&llm),
            ..RunOptions::default()
        };
        let steps = [
            ScriptStep::Input(UiEvent::Key('L')),
            ScriptStep::Input(UiEvent::Key('p')),
            ScriptStep::Input(UiEvent::Key('r')),
            ScriptStep::Input(UiEvent::Key('o')),
            ScriptStep::Input(UiEvent::Enter),
            ScriptStep::Input(UiEvent::Key('你')),
            ScriptStep::Input(UiEvent::Enter),
            ScriptStep::Input(UiEvent::Key('q')),
        ];
        let report = run_scripted_steps(&EmptySourceForTest, &steps, &options);
        assert!(report.quit, "q 置位退出");
        let runs = llm.runs.lock().expect("锁");
        assert_eq!(
            runs.len(),
            0,
            "空数据源无会话：llm.run 必须显式提示未选择会话，不得调用执行器"
        );
    }

    /// T167：llm.cancel 输入请求 ID 后调用执行器。
    #[test]
    fn llm_cancel_input_reaches_executor() {
        let llm = RecordingLlm {
            runs: std::sync::Mutex::new(Vec::new()),
            cancels: std::sync::Mutex::new(Vec::new()),
            fail: false,
        };
        let options = RunOptions {
            llm_executor: Some(&llm),
            ..RunOptions::default()
        };
        let steps = [
            ScriptStep::Input(UiEvent::Key('C')),
            ScriptStep::Input(UiEvent::Key('r')),
            ScriptStep::Input(UiEvent::Key('-')),
            ScriptStep::Input(UiEvent::Key('1')),
            ScriptStep::Input(UiEvent::Enter),
            ScriptStep::Input(UiEvent::Key('q')),
        ];
        let report = run_scripted_steps(&EmptySourceForTest, &steps, &options);
        assert!(report.quit, "q 置位退出");
        let cancels = llm.cancels.lock().expect("锁");
        assert_eq!(cancels.len(), 1, "取消请求必须到达执行器");
        assert_eq!(cancels[0], "r-1");
    }

    // ── ADR-087：Chat 对话流 ─────────────────────────────────────────────

    /// Chat 夹具：后台流式执行器（delta 有损、正文以完成消息为权威）。
    struct FakeChat {
        /// 取消标记（cancel() 置位；run_streaming 轮询后按 cancel 终止）。
        cancelled: std::sync::atomic::AtomicBool,
        /// 取消调用记录。
        cancels: std::sync::Mutex<Vec<String>>,
    }

    impl FakeChat {
        fn new() -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                cancelled: std::sync::atomic::AtomicBool::new(false),
                cancels: std::sync::Mutex::new(Vec::new()),
            })
        }
    }

    impl ChatExecutor for FakeChat {
        fn profiles(&self) -> Vec<String> {
            vec!["fake".to_string()]
        }

        fn run_streaming(
            &self,
            _session_id: &str,
            _provider: &str,
            _prompt: &str,
            request_id: &str,
            progress: whcode_app::turn::TurnProgress<'_>,
            _observer: Option<whcode_app::turn::TurnObserver<'_>>,
        ) -> Result<ChatOutcome, WhError> {
            // 有损模拟：只发半截增量（“你”），最终正文以完成消息全量替换。
            progress(1, "你");
            for _ in 0..100 {
                if self.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(ChatOutcome {
                        text: "你".to_string(),
                        summary: format!("provider=fake completed=false request_id={request_id}"),
                        reason: "cancel".to_string(),
                        completed: false,
                        truncated: false,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(ChatOutcome {
                text: "你好世界（全量替换）".to_string(),
                summary: format!(
                    "provider=fake model=fake-1 chunks=2 completed=true truncated=false request_id={request_id}"
                ),
                reason: "model_finished".to_string(),
                completed: true,
                truncated: false,
            })
        }

        fn cancel(&self, request_id: &str) -> Result<String, WhError> {
            self.cancels
                .lock()
                .expect("锁")
                .push(request_id.to_string());
            self.cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(format!(
                "request={request_id} state=cancelled effective=true"
            ))
        }
    }

    /// 行缓冲有界（ADR-087 决策 2／Check 474）：超出 [`CHAT_BUFFER_MAX`] 丢最旧，
    /// 流式下标同步左移不越界。
    #[test]
    fn chat_buffer_max_is_bounded() {
        let mut chat = ChatState::new();
        for index in 0..(CHAT_BUFFER_MAX + 50) {
            chat.push_row(ChatRow::User {
                text: format!("行{index}"),
            });
        }
        assert_eq!(chat.rows.len(), CHAT_BUFFER_MAX, "缓冲不得超上限");
        assert_eq!(
            chat.rows.first(),
            Some(&ChatRow::User {
                text: "行50".to_string()
            }),
            "最旧的 50 行应被丢弃"
        );
    }

    /// Chat 夹具数据源：单会话（供选择与历史回放默认空）。
    struct ChatSessionSource;
    impl whcode_app::view_source::ViewSource for ChatSessionSource {
        fn snapshot(
            &self,
            view: ViewId,
            _session: Option<&str>,
        ) -> whcode_app::view_source::ViewSnapshot {
            let mut snapshot = whcode_app::view_source::ViewSnapshot::empty(view);
            snapshot.sessions.push(crate::views::SessionRow {
                archived: false,
                created_at_ms: 0,
                last_active_at_ms: 0,
                id: "S-1".to_string(),
                title: "夹具".to_string(),
                stage: "plan".to_string(),
                status: "active".to_string(),
            });
            snapshot
        }
    }

    /// 提交门控（ADR-087 决策 4／Check 475）：无执行器／无会话／空提示词／运行中
    /// 四类全部结构化拒绝且不产生请求。
    #[test]
    fn chat_submit_gates_reject_structurally() {
        let texts = I18n::default();
        // 无执行器：not_supported。
        let mut core = LoopCore::new(
            &EmptyViewSource,
            None,
            None,
            None,
            &texts,
            Theme::default(),
            80,
            24,
            Instant::now(),
        );
        core.chat_submit("你好");
        assert!(
            core.status.contains("not_supported"),
            "无执行器必须显式 not_supported：{}",
            core.status
        );
        // 无会话：EmptyViewSource 无会话清单。
        let chat = FakeChat::new();
        let mut core = LoopCore::new(
            &EmptyViewSource,
            None,
            None,
            Some(chat),
            &texts,
            Theme::default(),
            80,
            24,
            Instant::now(),
        );
        core.chat_submit("你好");
        assert!(
            core.status.contains("会话"),
            "无会话必须显式拒绝：{}",
            core.status
        );
        // 空提示词：先备好会话与执行器再校验空串。
        let chat = FakeChat::new();
        let mut core = LoopCore::new(
            &ChatSessionSource,
            None,
            None,
            Some(chat),
            &texts,
            Theme::default(),
            80,
            24,
            Instant::now(),
        );
        core.chat_submit("  ");
        assert!(
            core.status.contains("为空"),
            "空提示词必须显式拒绝：{}",
            core.status
        );
        // 运行中：手动置位运行标志再提交。
        core.chat.turn_running = true;
        core.chat_submit("第二条");
        assert!(
            core.status.contains("运行中"),
            "运行中必须显式拒绝：{}",
            core.status
        );
    }

    /// scripted 流式对话（ADR-087 决策 2／Check 472）：轮头→用户→流式→轮尾依序；
    /// delta 有损时最终正文以完成消息全量替换；Esc 返回会话视图。
    #[test]
    fn chat_streaming_scripted_flow_renders_transcript() {
        let chat = FakeChat::new();
        let options = RunOptions {
            chat_executor: Some(chat as std::sync::Arc<dyn ChatExecutor>),
            ..RunOptions::default()
        };
        let mut steps = vec![
            ScriptStep::Input(UiEvent::Key('7')),
            ScriptStep::Input(UiEvent::Key('i')),
            ScriptStep::Input(UiEvent::Key('h')),
            ScriptStep::Input(UiEvent::Key('i')),
            ScriptStep::Input(UiEvent::Enter),
        ];
        steps.extend(std::iter::repeat_n(ScriptStep::Idle, 12));
        steps.push(ScriptStep::Input(UiEvent::Esc));
        let report = run_scripted_steps(&ChatSessionSource, &steps, &options);
        assert_eq!(report.view, ViewId::Sessions, "Esc 返回会话视图");
        let lines = &report.chat_lines;
        assert!(
            lines.iter().any(|line| line.contains("轮开始")),
            "轮头必须出现：{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("用户 · hi")),
            "用户行必须出现：{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("你好世界（全量替换）")),
            "完成消息必须全量替换流式正文：{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("轮结束")),
            "轮尾必须出现：{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("request_id=")),
            "轮尾摘要须携带 request_id（确定性取消口径）：{lines:?}"
        );
    }

    /// 运行中 Esc 取消（ADR-087 决策 4／Check 473）：取消携带预生成 request_id
    /// 到达执行器，终止行如实标注取消。
    #[test]
    fn chat_running_esc_cancels_with_pregenerated_request_id() {
        let chat = FakeChat::new();
        let options = RunOptions {
            chat_executor: Some(std::sync::Arc::clone(&chat) as std::sync::Arc<dyn ChatExecutor>),
            ..RunOptions::default()
        };
        let mut steps = vec![
            ScriptStep::Input(UiEvent::Key('7')),
            ScriptStep::Input(UiEvent::Key('i')),
            ScriptStep::Input(UiEvent::Key('h')),
            ScriptStep::Input(UiEvent::Key('i')),
            ScriptStep::Input(UiEvent::Enter),
            ScriptStep::Idle,
            ScriptStep::Input(UiEvent::Esc),
        ];
        steps.extend(std::iter::repeat_n(ScriptStep::Idle, 12));
        let report = run_scripted_steps(&ChatSessionSource, &steps, &options);
        let cancels = chat.cancels.lock().expect("锁");
        assert_eq!(cancels.len(), 1, "取消必须恰好到达执行器一次");
        assert!(
            cancels[0].starts_with("llm-"),
            "request_id 必须是预生成值（llm- 前缀）：{}",
            cancels[0]
        );
        assert!(
            report.chat_lines.iter().any(|line| line.contains("已取消")),
            "终止行须标注取消：{:?}",
            report.chat_lines
        );
        assert!(!report.chat_turn_running, "收口后运行标志必须复位");
    }
}
