//! # whcode-gui
//!
//! 原生桌面 GUI（FR-711—FR-715、ADR-027；交互对等见 ADR-056）。
//!
//! 边界：
//! - 取数只经 [`whcode_app::view_source`] 的只读快照，**不触库**、不绕过应用服务（ADR-003）；
//! - 不内嵌任何浏览器引擎（ADR-026）；渲染由 `iced` 自绘，使用 **tiny-skia 软件光栅**，
//!   默认不创建 GPU 上下文（NFR-212、ADR-027）；
//! - 空闲不重绘：没有定时器与动画，重绘只由输入或窗口事件触发（FR-714）；
//! - 写命令与 LLM 对话（ADR-056）：经装配根注入的 [`CommandExecutor`] 与
//!   [`GuiLlmExecutor`]，语义与 TUI 三端一致——`run_turn` 同步等待完成（执行期间界面
//!   阻塞，与 TUI 同）；破坏性命令预览→确认两段式；执行器未接入时显式提示
//!   `not_supported`（不静默无响应）。
//!
//! 渲染纪律（§5.3）：帧路径只消费准备阶段生成的纯数据快照，不反序列化 JSON、不查询数据库。

pub mod surface_map;

use std::rc::Rc;

use iced::futures::StreamExt;
use iced::futures::channel::mpsc;
use iced::futures::channel::oneshot;
use iced::keyboard::{Key, key};
use iced::widget::{button, column, container, rich_text, row, scrollable, span, text, text_input};
use iced::{
    Background, Border, Color, Element, Font, Length, Shadow, Size, Subscription, Task, Theme,
    window,
};
use whcode_app::commands::{CommandExecutor, FrontendCommand};
use whcode_app::view_source::{VIEW_COUNT, ViewId, ViewSnapshot, ViewSource};
use whcode_domain::WhError;
use whcode_domain::i18n::I18n;

/// 窗口初始逻辑尺寸（与 FR-712 的预算口径一致）。
const WINDOW_SIZE: Size = Size::new(1200.0, 800.0);

/// 内置字体子集（FR-716／ADR-028）：完整字体在 `assets/fonts/source`，
/// 这里只嵌入 `.github/scripts/subset-fonts.py` 生成的子集，避免整份 CJK 字体常驻。
const FONT_INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.subset.ttf");
const FONT_NOTO: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.subset.otf");
const FONT_JETBRAINS: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.subset.ttf");
const FONT_SARASA: &[u8] = include_bytes!("../assets/fonts/SarasaMonoSC-Regular.subset.ttf");

/// GUI 主题（六套：既有四套铁律精确色值 FR-612／FR-713；zcode 系 FR-739 为
/// 2026-11-06 用户新增并批准冻结，色值源 `whcode_domain::theme_tokens`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuiTheme {
    /// 墨水屏日间：纯白底、纯黑字（残影最小）。
    EinkDay,
    /// 墨水屏夜间：纯黑底、纯白字。
    EinkNight,
    /// AMOLED 夜间：纯黑底、浅灰字（仅纯黑可关断 OLED 像素）。
    AmoledNight,
    /// 终端绿夜间：纯黑底（OLED 兼容纯黑）、荧光绿字（CRT P1 风格，#00FF00）。
    TerminalGreen,
    /// zcode 日间（FR-739，色值单一事实源 `theme_tokens`）。
    ZcodeDay,
    /// zcode 夜间（**默认**，2026-11-06 用户批准切换；ADR-057/FR-739）。
    #[default]
    ZcodeNight,
}

impl GuiTheme {
    /// 配置名（与 `ui.theme` 取值一致）。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::EinkDay => "eink-day",
            Self::EinkNight => "eink-night",
            Self::AmoledNight => "amoled-night",
            Self::TerminalGreen => "terminal-green",
            Self::ZcodeDay => "zcode-day",
            Self::ZcodeNight => "zcode-night",
        }
    }

    /// 按配置名解析；未知取值返回 `None`（调用方决定回退或报错）。
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "eink-day" => Some(Self::EinkDay),
            "eink-night" => Some(Self::EinkNight),
            "amoled-night" => Some(Self::AmoledNight),
            "terminal-green" => Some(Self::TerminalGreen),
            "zcode-day" => Some(Self::ZcodeDay),
            "zcode-night" => Some(Self::ZcodeNight),
            _ => None,
        }
    }

    /// 下一套主题（`t` 键循环；FR-713：切换不得要求重启；FR-739 后为六套）。
    #[must_use]
    const fn next(self) -> Self {
        match self {
            Self::EinkDay => Self::EinkNight,
            Self::EinkNight => Self::AmoledNight,
            Self::AmoledNight => Self::TerminalGreen,
            Self::TerminalGreen => Self::ZcodeDay,
            Self::ZcodeDay => Self::ZcodeNight,
            Self::ZcodeNight => Self::EinkDay,
        }
    }

    /// 背景色与前景色（精确 RGB）。既有四套保持铁律字面值；zcode 系从
    /// `whcode_domain::theme_tokens` 冻结值解析（FR-740 单一事实源）。
    #[must_use]
    pub fn colors(self) -> (Color, Color) {
        match self {
            Self::EinkDay => (Color::WHITE, Color::BLACK),
            Self::EinkNight => (Color::BLACK, Color::WHITE),
            Self::AmoledNight => (Color::BLACK, Color::from_rgb(0.902, 0.902, 0.902)),
            Self::TerminalGreen => (Color::BLACK, Color::from_rgb(0.0, 1.0, 0.0)),
            Self::ZcodeDay => zcode_colors(whcode_domain::theme_tokens::ZCODE_DAY),
            Self::ZcodeNight => zcode_colors(whcode_domain::theme_tokens::ZCODE_NIGHT),
        }
    }

    /// iced 主题：调色板全部映射到主题色，避免出现框架默认蓝。
    fn iced(self) -> Theme {
        let (background, foreground) = self.colors();
        Theme::custom(
            self.name(),
            iced::theme::Palette {
                background,
                text: foreground,
                primary: foreground,
                success: foreground,
                warning: foreground,
                danger: foreground,
            },
        )
    }

    /// 滚动条样式：轨道取背景色、滑块取前景色（FR-713：不引入主题外的灰色）。
    fn scrollbar(self) -> scrollable::Style {
        let (background, foreground) = self.colors();
        let rail = scrollable::Rail {
            background: Some(Background::Color(background)),
            border: Border::default(),
            scroller: scrollable::Scroller {
                background: Background::Color(foreground),
                border: Border::default(),
            },
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(background),
                border: Border::default(),
                shadow: Shadow::default(),
                icon: foreground,
            },
        }
    }
}

/// GUI 侧 LLM 执行器（ADR-056；语义与 `whcode_tui::LlmExecutor` 一致）。
///
/// 由装配根实现（包装 `TuiLlmExecutor`）：GUI 不直接依赖 `whcode-bin`，经此 trait 反转。
/// `Send + Sync` 是 Phase 2（后台执行）的硬要求：`run_with_progress` 在独立线程运行
/// （独立线程 + `Task::stream` 有损进度通道），界面线程仅经 `cancel` 取消。
pub trait GuiLlmExecutor: Send + Sync {
    /// 发起一次真实调用（**阻塞至 turn 完成**；在调用方的后台线程上执行，不冻结界面）。
    /// 会话取 GUI 当前选中会话；`progress` 逐块回调累计块数（回调必须非阻塞、有损可接受）。
    ///
    /// # Errors
    ///
    /// 剖面未注册、凭据缺失或调用失败时返回 [`WhError`]。
    fn run_with_progress(
        &self,
        session_id: &str,
        provider: &str,
        prompt: &str,
        progress: whcode_app::turn::TurnProgress<'_>,
    ) -> Result<String, WhError>;

    /// 取消同机正在运行的调用（取消标记目录跨线程生效；不触库、不需要连接锁）。
    ///
    /// # Errors
    ///
    /// 请求 ID 非法或无活跃调用时返回 [`WhError`]。
    fn cancel(&self, request_id: &str) -> Result<String, WhError>;

    /// 可选剖面名清单（注册表顺序；对话视图显示与默认选择用）。
    fn profiles(&self) -> Vec<String>;
}

/// 会话操作面板的当前状态（ADR-056；破坏性命令两段式）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum OpMode {
    /// 新建会话：输入即标题。
    SessionNew,
    /// 重命名会话：输入即新标题。
    SessionRename,
    /// 归档/恢复预览已出，待确认（`archived` 为目标状态）。
    SessionArchive { archived: bool },
    /// 删除预览已出，待确认。
    SessionDelete,
    /// 清理预览已出，待确认（`before_ms` 冻结于预览时点）。
    Purge { before_ms: i64 },
}

/// 启动参数。
#[derive(Default)]
pub struct RunOptions {
    /// 主题（FR-612）。
    pub theme: GuiTheme,
    /// 界面文本层（FR-615）：默认 zh-CN + en 回退。
    pub texts: I18n,
    /// 写命令执行器（FR-623/ADR-056）：`Some` 时会话/任务/清理写操作可用；`None` 时
    /// 显式提示 `not_supported`（不静默无响应，与 TUI 同口径）。
    pub command_executor: Option<Box<dyn CommandExecutor>>,
    /// LLM 执行器（ADR-056 Phase 2）：`Some` 时对话视图可用（后台线程执行）；`None`
    /// 时显式提示 `not_supported`。`Arc` 以便克隆进阻塞线程。
    pub llm_executor: Option<std::sync::Arc<dyn GuiLlmExecutor>>,
}

/// 界面消息（按键、窗口事件与控件回调产生；无定时器）。
#[derive(Debug, Clone)]
enum Message {
    SelectView(ViewId),
    NextSession,
    PrevSession,
    Refresh,
    CycleTheme,
    Resized(Size),
    // —— 对话（ADR-056）——
    /// 剖面名输入。
    ChatProviderChanged(String),
    /// 提示词输入。
    ChatInputChanged(String),
    /// 提交一轮对话（Phase 2：后台线程执行 `run_turn_with_progress`，界面不冻结）。
    ChatSubmit,
    /// turn 完成（后台线程 → oneshot → `Task::stream` 送达；`turn_running` 解除）。
    ChatDone(Result<String, WhError>),
    /// 流式进度（累计块数；有损——通道满即丢弃）。
    ChatProgress(u64, String),
    /// 取消正在运行的调用（按请求 ID）。
    ChatCancel,
    // —— 会话/任务/清理操作（ADR-056）——
    /// 操作面板输入（标题／任务 ID）。
    OpInputChanged(String),
    /// 打开操作面板（新建／重命名；破坏性命令先进入预览态）。
    OpStart(OpMode),
    /// 破坏性命令：先预览（confirm=false，不写库）。
    OpPreview,
    /// 破坏性命令：确认执行（confirm=true）。
    OpConfirm,
    /// 关闭操作面板（不执行任何写操作）。
    OpCancel,
    /// 任务重试（按下标索引 `snapshot.tasks`）。
    TaskRetry(usize),
    /// 任务中止（按下标索引）。
    TaskAbort(usize),
    /// Esc：有面板时取消面板，否则退出。
    EscapeKey,
    /// 字符键（门控在 update：对话视图／操作面板打开时忽略全局快捷键）。
    Key(char),
}

/// 应用状态：只读快照 + 交互状态（对话与写操作，ADR-056）。
struct State {
    source: Rc<dyn ViewSource>,
    texts: I18n,
    /// 主题（同时用于 iced 主题与滚动条样式）。
    theme: GuiTheme,
    view: ViewId,
    /// 会话选择位置（`None` = 未选择）。
    selected: Option<usize>,
    /// 会话 ID清单（准备层给出，`j`/`k` 在其中移动）。
    session_ids: Vec<String>,
    /// 当前视图快照。
    snapshot: ViewSnapshot,
    /// 当前窗口逻辑尺寸（状态栏显示，FR-712 口径）。
    size: Size,
    /// 写命令执行器（`None` = 装配未接入，写操作显式提示）。
    executor: Option<Rc<dyn CommandExecutor>>,
    /// LLM 执行器（`None` = 装配未接入，对话显式提示）。`Arc`：ChatSubmit 时克隆进
    /// 后台线程（Phase 2）。
    llm: Option<std::sync::Arc<dyn GuiLlmExecutor>>,
    /// turn 运行中（后台线程持有连接锁）：期间除进度/完成/窗口事件外的一切交互
    /// 冻结在当前画面（数据锁被占用，冻结优于阻塞等待）。
    turn_running: bool,
    /// 当前 turn 的累计块数（进度回调经有损通道送达）。
    chat_progress: u64,
    /// 当前 turn 的流式正文缓冲（FR-741 逐块增量呈现；有界——
    /// `CHAT_STREAMING_CAP_BYTES` 截断，turn 输出预算 1 MiB 为天然上界）。
    chat_streaming: String,
    /// 对话视图：剖面名（默认取执行器剖面清单首项）。
    chat_provider: String,
    /// 对话视图：提示词输入。
    chat_input: String,
    /// 对话视图：最近一次结果摘要或错误（不复制模型正文）。
    chat_last: Option<String>,
    /// 操作面板模式（`None` = 关闭）。
    op_mode: Option<OpMode>,
    /// 操作面板输入（标题／任务 ID）。
    op_input: String,
    /// 操作面板反馈行（预览结果或错误）。
    op_feedback: Option<String>,
}

impl State {
    fn new(
        source: Rc<dyn ViewSource>,
        texts: I18n,
        theme: GuiTheme,
        executor: Option<Rc<dyn CommandExecutor>>,
        llm: Option<std::sync::Arc<dyn GuiLlmExecutor>>,
    ) -> Self {
        let snapshot = source.snapshot(ViewId::Sessions, None);
        let session_ids = snapshot
            .sessions
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>();
        let chat_provider = llm
            .as_ref()
            .and_then(|llm| llm.profiles().into_iter().next())
            .unwrap_or_default();
        Self {
            source,
            texts,
            theme,
            view: ViewId::Sessions,
            selected: None,
            session_ids,
            snapshot,
            size: WINDOW_SIZE,
            executor,
            llm,
            turn_running: false,
            chat_progress: 0,
            chat_streaming: String::new(),
            chat_provider,
            chat_input: String::new(),
            chat_last: None,
            op_mode: None,
            op_input: String::new(),
            op_feedback: None,
        }
    }

    /// 以当前选择重新准备快照（准备层；帧路径不查询）。
    fn prepare(&mut self) {
        self.snapshot = self
            .source
            .snapshot(self.view, self.selected_session().as_deref());
    }

    fn selected_session(&self) -> Option<String> {
        self.selected
            .and_then(|index| self.session_ids.get(index))
            .cloned()
    }

    /// 帧标题（与 TUI 同一词条：`tui.title`）。
    fn title(&self) -> String {
        self.texts.resolve_fmt(
            "tui.title",
            &[
                ("app", self.texts.resolve("app.title")),
                ("view", self.texts.resolve(self.view.title_key())),
                ("index", &self.view.index().to_string()),
                ("count", &VIEW_COUNT.to_string()),
            ],
        )
    }

    /// 顶栏会话行：未选择时显示显式占位（不渲染空串）。
    fn session_label(&self) -> String {
        match self.selected_session() {
            Some(id) => self
                .texts
                .resolve_fmt("cui.record.session", &[("session", &id)]),
            None => self.texts.resolve("gui.session.none").to_string(),
        }
    }

    /// KPI 条（T199）：优先使用准备层摘要（任务视图含总计/完成/阻塞），否则给出可得计数。
    ///
    /// 不新增查询：计数只来自已准备的快照与会话清单——帧路径不触库（§5.3）。
    fn kpi(&self) -> String {
        if !self.snapshot.summary.is_empty() {
            return self.snapshot.summary.clone();
        }
        self.texts.resolve_fmt(
            "gui.kpi.count",
            &[
                ("sessions", &self.session_ids.len().to_string()),
                ("rows", &self.snapshot_rows().to_string()),
            ],
        )
    }

    /// 当前视图的数据条数（文本视图无行数概念，记 0）。
    fn snapshot_rows(&self) -> usize {
        match self.view {
            ViewId::Sessions => self.snapshot.sessions.len(),
            ViewId::Tasks => self.snapshot.tasks.len(),
            ViewId::Events => self.snapshot.events.len(),
            ViewId::Diff => self.snapshot.diff.len(),
            ViewId::Context | ViewId::Spec | ViewId::Chat => 0,
        }
    }

    /// 状态栏：键位提示 + 窗口尺寸 + 当前主题（与 TUI 同一批词条）。
    fn footer(&self) -> String {
        let hints = self.texts.resolve_fmt(
            "tui.footer",
            &[
                ("cols", &format!("{:.0}", self.size.width)),
                ("rows", &format!("{:.0}", self.size.height)),
            ],
        );
        let theme = self
            .texts
            .resolve_fmt("gui.status.theme", &[("theme", self.theme.name())]);
        format!("{hints}    {theme}")
    }
}

/// 启动 GUI 闭环。
///
/// `source` 由装配根构造（只读快照生成方），生命周期须覆盖整个应用；
/// 调用方负责在无交互环境时不要进入本函数。
///
/// # Errors
///
/// 窗口创建或渲染后端初始化失败时返回 `iced::Error`。
pub fn run(source: Box<dyn ViewSource>, options: RunOptions) -> iced::Result {
    let source: Rc<dyn ViewSource> = Rc::from(source);
    let RunOptions {
        theme,
        texts,
        command_executor,
        llm_executor,
    } = options;
    // 执行器以 Rc 共享进状态闭包（iced 的状态构造器要求可重复调用；Rc 克隆即可）。
    let executor_shared: Option<Rc<dyn CommandExecutor>> = command_executor.map(Rc::from);
    let llm_shared: Option<std::sync::Arc<dyn GuiLlmExecutor>> = llm_executor;

    let mut app = iced::application(
        move || {
            State::new(
                Rc::clone(&source),
                texts.clone(),
                theme,
                executor_shared.clone(),
                llm_shared.clone(),
            )
        },
        update,
        view,
    )
    .title(|state: &State| state.title())
    .theme(|state: &State| state.theme.iced())
    .subscription(subscription)
    .window(window::Settings {
        size: WINDOW_SIZE,
        position: window::Position::Centered,
        ..window::Settings::default()
    })
    .default_font(Font::with_name("Inter"));
    for bytes in [FONT_INTER, FONT_NOTO, FONT_JETBRAINS, FONT_SARASA] {
        app = app.font(bytes.to_vec());
    }
    app.run()
}

/// 字符是否属于 CJK 区（汉字、假名、CJK 标点与全角符号）。
fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2E80..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
    )
}

/// 四类文本的字体（FR-716）：比例／等宽 × 拉丁／CJK。
fn font_for(cjk: bool, mono: bool) -> Font {
    match (mono, cjk) {
        (false, false) => Font::with_name("Inter"),
        (false, true) => Font::with_name("Noto Sans CJK SC"),
        (true, false) => Font::with_name("JetBrains Mono"),
        (true, true) => Font::with_name("Sarasa Mono SC"),
    }
}

/// 按“拉丁 / CJK”切分文本，保证每段显式使用对应字体（不依赖回退顺序）。
fn split_runs(line: &str) -> Vec<(bool, &str)> {
    let mut runs: Vec<(bool, &str)> = Vec::new();
    let mut start = 0;
    let mut current: Option<bool> = None;
    for (index, ch) in line.char_indices() {
        let cjk = is_cjk(ch);
        match current {
            Some(value) if value == cjk => {}
            Some(value) => {
                runs.push((value, &line[start..index]));
                start = index;
                current = Some(cjk);
            }
            None => current = Some(cjk),
        }
    }
    if let Some(value) = current {
        runs.push((value, &line[start..]));
    }
    runs
}

/// 一行富文本：按字符类别分段指定字体。
fn rich_line(line: &str, mono: bool, size: f32) -> Element<'static, Message> {
    let spans: Vec<text::Span<'static, (), Font>> = split_runs(line)
        .into_iter()
        .map(|(cjk, run)| span(run.to_string()).font(font_for(cjk, mono)).size(size))
        .collect();
    rich_text(spans).into()
}

fn update(state: &mut State, message: Message) -> Task<Message> {
    // turn 运行中：后台线程持有连接锁——除进度/完成/窗口事件外的一切消息冻结在
    // 当前画面（任何触库操作都会阻塞等待锁，冻结优于卡死；完成后自动刷新）。
    if state.turn_running {
        match message {
            Message::ChatProgress(count, delta) => {
                state.chat_progress = count;
                append_streaming(&mut state.chat_streaming, &delta);
            }
            Message::ChatDone(result) => {
                state.turn_running = false;
                state.chat_progress = 0;
                state.chat_streaming.clear();
                match result {
                    Ok(summary) => {
                        state.chat_last = Some(summary);
                        state.prepare();
                    }
                    Err(err) => {
                        state.chat_last =
                            Some(format!("{}: {err}", state.texts.resolve("gui.chat.error")));
                    }
                }
            }
            Message::Resized(size) => state.size = size,
            _ => {}
        }
        return Task::none();
    }
    match message {
        Message::ChatProgress(_, _) | Message::ChatDone(_) => {
            // turn 未在运行（完成后送达的尾消息），无状态可更新。
        }
        Message::SelectView(view) => {
            state.view = view;
            state.prepare();
            return Task::none();
        }
        Message::NextSession => {
            if !state.session_ids.is_empty() {
                state.selected = Some(
                    state
                        .selected
                        .map_or(0, |index| (index + 1).min(state.session_ids.len() - 1)),
                );
                state.view = ViewId::Sessions;
                state.prepare();
            }
            return Task::none();
        }
        Message::PrevSession => {
            if !state.session_ids.is_empty() {
                state.selected = Some(state.selected.map_or(0, |index| index.saturating_sub(1)));
                state.view = ViewId::Sessions;
                state.prepare();
            }
            return Task::none();
        }
        Message::Refresh => {
            state.prepare();
            return Task::none();
        }
        Message::CycleTheme => {
            state.theme = state.theme.next();
            return Task::none();
        }
        Message::Resized(size) => {
            state.size = size;
            return Task::none();
        }
        Message::Key(key) => {
            // 全局快捷键仅在「无文本输入场景」生效：对话视图与操作面板打开时，
            // 键盘由文本输入控件接管（数字与 j/k/r/t/q 是内容而非命令）。
            if state.op_mode.is_some() || state.view == ViewId::Chat {
                return Task::none();
            }
            match key {
                '1' | '2' | '3' | '4' | '5' | '6' | '7' => {
                    let index = (key.to_digit(10).unwrap_or(1) as usize).saturating_sub(1);
                    if let Some(view) = ViewId::ALL.get(index).copied() {
                        let _ = update(state, Message::SelectView(view));
                    }
                    return Task::none();
                }
                'j' => {
                    let _ = update(state, Message::NextSession);
                    return Task::none();
                }
                'k' => {
                    let _ = update(state, Message::PrevSession);
                    return Task::none();
                }
                'r' => {
                    let _ = update(state, Message::Refresh);
                    return Task::none();
                }
                't' => {
                    let _ = update(state, Message::CycleTheme);
                    return Task::none();
                }
                'q' => return iced::exit(),
                _ => return Task::none(),
            }
        }
        Message::EscapeKey => {
            if state.op_mode.take().is_some() {
                state.op_input.clear();
                state.op_feedback = None;
            } else {
                return iced::exit();
            }
        }
        Message::ChatProviderChanged(value) => state.chat_provider = value,
        Message::ChatInputChanged(value) => state.chat_input = value,
        Message::ChatSubmit => return chat_submit(state),
        Message::ChatCancel => chat_cancel(state),
        Message::OpInputChanged(value) => state.op_input = value,
        Message::OpStart(mode) => {
            state.op_mode = Some(mode);
            state.op_input.clear();
            state.op_feedback = None;
        }
        Message::OpPreview => op_preview(state),
        Message::OpConfirm => op_confirm(state),
        Message::OpCancel => {
            state.op_mode = None;
            state.op_input.clear();
            state.op_feedback = None;
        }
        Message::TaskRetry(index) => task_action(state, index, false),
        Message::TaskAbort(index) => task_action(state, index, true),
    }
    Task::none()
}

/// 对话提交（ADR-056）：同步 `run_turn`——执行期间界面阻塞，与 TUI 同语义。
/// 对话提交（ADR-056 Phase 2）：后台线程执行 `run_turn_with_progress`，界面不冻结。
/// 对话流式缓冲上限（FR-741 有界性；与 turn 单次输出预算 1 MiB 同量级——
/// 达到上限即停止追加，块计数照常，截断点落在字符边界）。
const CHAT_STREAMING_CAP_BYTES: usize = 1024 * 1024;
/// 对话正文渲染行数上限（视图有界；超限以省略行“…”标注）。
const CHAT_RENDER_MAX_LINES: usize = 400;

/// 追加流式增量到缓冲（有界：超 `CHAT_STREAMING_CAP_BYTES` 截断并忽略后续）。
fn append_streaming(buffer: &mut String, delta: &str) {
    if buffer.len() >= CHAT_STREAMING_CAP_BYTES {
        return;
    }
    buffer.push_str(delta);
    if buffer.len() > CHAT_STREAMING_CAP_BYTES {
        let mut cut = CHAT_STREAMING_CAP_BYTES;
        while !buffer.is_char_boundary(cut) {
            cut -= 1;
        }
        buffer.truncate(cut);
    }
}

/// FR-741 markdown 最小集（标题／无序列表／代码围栏／行内码）的行级投影——
/// 纯函数可测；不引入第三方渲染器（零新增依赖、渲染预算可控），最小集之外的
/// 语法按普通文本呈现（显式口径，不伪造完整 markdown 支持）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MdLineKind {
    Heading,
    Bullet,
    Text,
    Code,
}

/// 一行投影：`kind` 决定前缀与字号，`segs` 内 `bool`＝行内码（等宽字体）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct MdLine {
    kind: MdLineKind,
    segs: Vec<(bool, String)>,
}

/// 行内切分：把 `` `code` `` 段与普通段分开（按反引号配对，奇数段为行内码）。
fn split_inline_code(line: &str) -> Vec<(bool, String)> {
    let mut parts = Vec::new();
    for (index, part) in line.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        parts.push((index % 2 == 1, part.to_string()));
    }
    if parts.is_empty() {
        parts.push((false, String::new()));
    }
    parts
}

/// 把对话正文投影为行列表（FR-741 最小集；空行产出空 Text 行保结构）。
fn md_project(text: &str) -> Vec<MdLine> {
    let mut lines = Vec::new();
    let mut in_fence = false;
    for raw in text.lines() {
        if raw.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(MdLine {
                kind: MdLineKind::Code,
                segs: vec![(true, raw.to_string())],
            });
            continue;
        }
        let trimmed = raw.trim_start();
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            lines.push(MdLine {
                kind: MdLineKind::Heading,
                segs: split_inline_code(trimmed[hashes..].trim_start()),
            });
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            lines.push(MdLine {
                kind: MdLineKind::Bullet,
                segs: split_inline_code(rest),
            });
            continue;
        }
        lines.push(MdLine {
            kind: MdLineKind::Text,
            segs: split_inline_code(raw),
        });
    }
    lines
}

/// 投影为视图元素（有界：最多 `CHAT_RENDER_MAX_LINES` 行，超限以“…”行标注）。
fn md_elements(text: &str) -> Vec<Element<'static, Message>> {
    let mut elements = Vec::new();
    for line in md_project(text).into_iter().take(CHAT_RENDER_MAX_LINES) {
        let size = match line.kind {
            MdLineKind::Heading => 18.0,
            _ => 14.0,
        };
        let mut spans: Vec<text::Span<'static, (), Font>> = Vec::new();
        if line.kind == MdLineKind::Bullet {
            spans.push(
                span("• ".to_string())
                    .font(font_for(false, false))
                    .size(size),
            );
        }
        if line.segs.is_empty() {
            spans.push(
                span(" ".to_string())
                    .font(font_for(false, false))
                    .size(size),
            );
        }
        for (mono, part) in line.segs {
            spans.push(span(part).font(font_for(false, mono)).size(size));
        }
        elements.push(rich_text(spans).into());
    }
    if md_project(text).len() > CHAT_RENDER_MAX_LINES {
        elements.push(rich_line("…", false, 13.0));
    }
    elements
}

/// 进度经有界通道 `try_send` 送达（满即丢弃——进度是有损信号）；完成消息经 oneshot
/// 保证送达。执行期间 `turn_running` 冻结其余交互。
fn chat_submit(state: &mut State) -> Task<Message> {
    let Some(llm) = state.llm.clone() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.not_supported").to_string());
        return Task::none();
    };
    if state.chat_input.trim().is_empty() {
        state.chat_last = Some(state.texts.resolve("gui.chat.empty").to_string());
        return Task::none();
    }
    let Some(session) = state.selected_session() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.none").to_string());
        return Task::none();
    };
    let provider = if state.chat_provider.trim().is_empty() {
        llm.profiles().into_iter().next().unwrap_or_default()
    } else {
        state.chat_provider.trim().to_string()
    };
    let prompt = std::mem::take(&mut state.chat_input);
    state.turn_running = true;
    state.chat_progress = 0;
    state.chat_streaming.clear();
    let (progress_tx, progress_rx) = mpsc::channel::<Message>(8);
    let (done_tx, done_rx) = oneshot::channel::<Result<String, WhError>>();
    std::thread::spawn(move || {
        let progress = |count: u64, delta: &str| {
            // 有界通道 try_send（&mut self，故每次克隆 Sender 保持 Fn）；满即丢弃
            // （进度是有损信号；完成消息走 oneshot 保证送达）。
            let mut sender = progress_tx.clone();
            let _ = sender.try_send(Message::ChatProgress(count, delta.to_string()));
        };
        let result = llm.run_with_progress(&session, &provider, &prompt, &progress);
        let _ = done_tx.send(result);
    });
    let done_stream = iced::futures::stream::once(async move {
        match done_rx.await {
            Ok(result) => Message::ChatDone(result),
            Err(_) => Message::ChatDone(Err(WhError::invalid_state(
                "对话完成通道关闭（后台线程提前终止）",
            ))),
        }
    });
    Task::stream(progress_rx.chain(done_stream))
}

/// 取消正在运行的调用（请求 ID 取上次摘要中的 `request_id=…`，与 TUI 同口径）。
fn chat_cancel(state: &mut State) {
    let Some(llm) = state.llm.as_ref() else {
        state.chat_last = Some(state.texts.resolve("gui.chat.not_supported").to_string());
        return;
    };
    let request_id = state
        .chat_last
        .as_deref()
        .and_then(|text| text.rsplit("request_id=").next())
        .unwrap_or("")
        .trim()
        .to_string();
    if request_id.is_empty() {
        state.chat_last = Some(state.texts.resolve("gui.chat.no_request").to_string());
        return;
    }
    match llm.cancel(&request_id) {
        Ok(summary) => state.chat_last = Some(summary),
        Err(err) => {
            state.chat_last = Some(format!("{}: {err}", state.texts.resolve("gui.chat.error")));
        }
    }
}

/// 会话/任务/清理写命令统一入口：Capability 与确认语义由 `execute_command` 承担。
fn execute(state: &mut State, command: FrontendCommand) {
    let Some(executor) = state.executor.as_ref() else {
        state.op_feedback = Some(state.texts.resolve("gui.op.not_supported").to_string());
        return;
    };
    match executor.execute(command) {
        Ok(outcome) => {
            state.op_feedback = Some(op_outcome_text(&outcome));
            state.op_mode = None;
            state.op_input.clear();
            // 写后刷新（FR-624 手动刷新语义的对齐：本端写入后本地立即可见）。
            state.prepare();
        }
        Err(err) => {
            state.op_feedback = Some(format!("{}: {err}", state.texts.resolve("gui.op.error")))
        }
    }
}

/// 操作面板预览（破坏性命令 confirm=false；不写库、不写审计）。
fn op_preview(state: &mut State) {
    let Some(mode) = state.op_mode.clone() else {
        return;
    };
    let Some(executor) = state.executor.as_ref() else {
        state.op_feedback = Some(state.texts.resolve("gui.op.not_supported").to_string());
        return;
    };
    let Some(session) = state.selected_session() else {
        state.op_feedback = Some(state.texts.resolve("gui.chat.none").to_string());
        return;
    };
    let now = now_ms();
    let outcome = match mode {
        OpMode::SessionArchive { archived } => executor.execute(FrontendCommand::SessionArchive {
            session_id: session,
            archived,
            confirm: false,
            now_ms: now,
        }),
        OpMode::SessionDelete => executor.execute(FrontendCommand::SessionDelete {
            session_id: session,
            confirm: false,
            now_ms: now,
        }),
        OpMode::Purge { before_ms } => executor.execute(FrontendCommand::PurgePreview {
            before_ms,
            now_ms: now,
        }),
        OpMode::SessionNew | OpMode::SessionRename => return,
    };
    match outcome {
        Ok(result) => {
            // 预览不关面板：下一步 OpConfirm 才执行（FR-623 两段式）。
            state.op_feedback = Some(op_outcome_text(&result));
        }
        Err(err) => {
            state.op_feedback = Some(format!("{}: {err}", state.texts.resolve("gui.op.error")))
        }
    }
}

/// 操作面板确认执行（confirm=true）。
fn op_confirm(state: &mut State) {
    let Some(mode) = state.op_mode.clone() else {
        return;
    };
    let now = now_ms();
    let command = match mode {
        OpMode::SessionNew => {
            let title = std::mem::take(&mut state.op_input);
            FrontendCommand::SessionNew {
                title,
                model: None,
                now_ms: now,
            }
        }
        OpMode::SessionRename => {
            let title = std::mem::take(&mut state.op_input);
            FrontendCommand::SessionRename {
                session_id: state.selected_session().unwrap_or_default(),
                title,
                now_ms: now,
            }
        }
        OpMode::SessionArchive { archived } => FrontendCommand::SessionArchive {
            session_id: state.selected_session().unwrap_or_default(),
            archived,
            confirm: true,
            now_ms: now,
        },
        OpMode::SessionDelete => FrontendCommand::SessionDelete {
            session_id: state.selected_session().unwrap_or_default(),
            confirm: true,
            now_ms: now,
        },
        OpMode::Purge { before_ms } => FrontendCommand::PurgeExecute {
            before_ms,
            confirm: true,
            now_ms: now,
        },
    };
    execute(state, command);
}

/// 任务操作（retry/abort；按下标索引快照任务行）。
fn task_action(state: &mut State, index: usize, abort: bool) {
    let Some(task) = state.snapshot.tasks.get(index) else {
        return;
    };
    let task_id = task.id.clone();
    let now = now_ms();
    let command = if abort {
        FrontendCommand::TaskAbort {
            task_id,
            now_ms: now,
        }
    } else {
        FrontendCommand::TaskRetry {
            task_id,
            now_ms: now,
        }
    };
    execute(state, command);
}

/// `CommandOutcome` 的人可读摘要（不复制模型正文；事件已入库）。
#[allow(clippy::match_same_arms)]
fn op_outcome_text(outcome: &whcode_app::commands::CommandOutcome) -> String {
    match outcome {
        whcode_app::commands::CommandOutcome::Session(row) => {
            format!("session={} title={}", row.id, row.title)
        }
        whcode_app::commands::CommandOutcome::SessionArchivePreview {
            session,
            target_archived,
        } => format!(
            "preview: session={} -> archived={}",
            session.id, target_archived
        ),
        whcode_app::commands::CommandOutcome::SessionDeletePreview(preview) => {
            format!("preview: sessions={}", preview.counts.sessions)
        }
        whcode_app::commands::CommandOutcome::SessionDeleted(result) => {
            format!("deleted_sessions={}", result.deleted_sessions.len())
        }
        whcode_app::commands::CommandOutcome::Task(row) => {
            format!("task={} status={}", row.id, row.status.as_str())
        }
        whcode_app::commands::CommandOutcome::PurgePreview(preview) => {
            format!("preview: cutoff_ms={}", preview.cutoff_ms)
        }
        whcode_app::commands::CommandOutcome::PurgeExecuted(result) => {
            format!(
                "purged: sessions={} tasks={}",
                result.deleted.sessions, result.deleted.tasks
            )
        }
    }
}

/// 毫秒时钟（与 TUI 的 now_ms 同口径：系统墙钟）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|delta| delta.as_millis() as u64)
        .unwrap_or(0)
}

fn subscription(_: &State) -> Subscription<Message> {
    iced::event::listen_raw(handle_event)
}

/// 原始事件 → 消息。`RedrawRequested` 必须返回 `None`（否则会形成无限重绘循环）。
///
/// 字符键统一以 [`Message::Key`] 送入 update——全局快捷键的门控在 update 内按状态裁决
/// （对话视图输入中／操作面板打开时停用，由文本输入控件接管键盘）；`listen_raw` 只收
/// fn 指针，无法闭包捕获状态。
fn handle_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => match key {
            Key::Named(key::Named::Escape) => Some(Message::EscapeKey),
            Key::Character(character) => Some(Message::Key(character.chars().next()?)),
            _ => None,
        },
        _ => None,
    }
}

/// 侧栏条目样式：只用主题两色（选中反显），不引入灰色、边框或阴影（ADR-041）。
/// zcode 系色值解析（FR-740 单一事实源）：从 `theme_tokens` 冻结 hex 构建 iced 颜色。
/// 冻结值合法性由 `whcode-domain::theme_tokens` 测试保证，此处只做不可失败解析的
/// 显式收口（失败即冻结值被改动，属编译期常量缺陷，panic 信息可定位）。
fn zcode_colors(tokens: whcode_domain::theme_tokens::ThemeTokens) -> (Color, Color) {
    let parse = |hex: &str| {
        let (r, g, b) = whcode_domain::theme_tokens::parse_rgb(hex)
            .expect("theme_tokens 冻结色值必须为合法 #RRGGBB（模块测试保证）");
        Color::from_rgb8(r, g, b)
    };
    (parse(tokens.background), parse(tokens.foreground))
}

fn sidebar_style(theme: GuiTheme, selected: bool) -> button::Style {
    let (background, foreground) = theme.colors();
    button::Style {
        background: Some(Background::Color(if selected {
            foreground
        } else {
            background
        })),
        text_color: if selected { background } else { foreground },
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 侧栏条目：可点击切换视图（只读消息），保留 `1—6` 键位。
fn sidebar_button(state: &State, item: ViewId) -> Element<'_, Message> {
    let selected = state.view == item;
    let label = format!("{} {}", item.index(), state.texts.resolve(item.title_key()));
    let theme = state.theme;
    button(rich_line(&label, false, 16.0))
        .on_press(Message::SelectView(item))
        .width(Length::Fill)
        .padding([6, 8])
        .style(move |_theme, _status| sidebar_style(theme, selected))
        .into()
}

/// 只读正文（既有渲染路径；Chat 视图不经过这里）。
fn read_body(state: &State, mono: bool) -> Element<'_, Message> {
    let lines = state.snapshot.render_lines_limited(500);
    if lines.is_empty() {
        rich_line(state.texts.resolve("tui.empty"), mono, 16.0)
    } else {
        scrollable(
            column(
                lines
                    .into_iter()
                    .map(|line| rich_line(line.as_str(), mono, 16.0))
                    .collect::<Vec<_>>(),
            )
            .spacing(4),
        )
        .style(move |_, _| state.theme.scrollbar())
        .height(Length::Fill)
        .into()
    }
}

/// 对话视图（ADR-056）：剖面 + 提示词 + 发送/取消 + 最近结果。
fn chat_panel(state: &State) -> Element<'_, Message> {
    let provider_input = text_input(
        state.texts.resolve("gui.chat.provider.placeholder"),
        state.chat_provider.as_str(),
    )
    .on_input(Message::ChatProviderChanged)
    .padding(6)
    .width(Length::Fill);
    let provider_label = state.texts.resolve("gui.chat.provider");
    let input = text_input(
        state.texts.resolve("gui.chat.prompt.placeholder"),
        state.chat_input.as_str(),
    )
    .on_input(Message::ChatInputChanged)
    .on_submit(Message::ChatSubmit)
    .padding(6)
    .width(Length::Fill);
    // turn 运行中：发送禁用（面板开新 turn 会与连接锁竞争）、取消保持可用
    // （取消经标记目录跨线程生效，不触库）；状态行呈现运行进度或最近结果。
    let send = button(rich_line(state.texts.resolve("gui.chat.send"), false, 14.0))
        .on_press_maybe((!state.turn_running && state.llm.is_some()).then_some(Message::ChatSubmit))
        .padding(6);
    let cancel = button(rich_line(
        state.texts.resolve("gui.chat.cancel"),
        false,
        14.0,
    ))
    .on_press_maybe(state.turn_running.then_some(Message::ChatCancel))
    .padding(6);
    // 正文区（FR-741）：运行中＝进度行＋流式增量（markdown 最小集投影，有界）；
    // 完成后＝最近结果投影；未选会话＝提示。历史消息列表属 T250 后续片。
    let mut body: Vec<Element<'_, Message>> = if state.turn_running {
        let mut items = vec![rich_line(
            state
                .texts
                .resolve_fmt(
                    "gui.chat.progress",
                    &[("chunks", &state.chat_progress.to_string())],
                )
                .as_str(),
            false,
            13.0,
        )];
        items.extend(md_elements(state.chat_streaming.as_str()));
        items
    } else {
        match state.chat_last.as_deref() {
            Some(text) => md_elements(text),
            None => vec![rich_line(state.texts.resolve("gui.chat.hint"), false, 14.0)],
        }
    };
    body.push(rich_line(
        state.texts.resolve("gui.chat.history.hint"),
        false,
        13.0,
    ));
    column![
        rich_line(provider_label, false, 14.0),
        provider_input,
        row![input, send, cancel].spacing(8),
        scrollable(column(body)).height(Length::Fill),
    ]
    .spacing(8)
    .into()
}

/// 会话操作按钮行（Sessions 视图；执行器未接入时按钮显式禁用提示走反馈行）。
fn op_buttons(state: &State) -> Element<'_, Message> {
    let enabled = state.executor.is_some();
    let item = |label: &str, message: Message| -> Element<'_, Message> {
        let theme = state.theme;
        let button = button(rich_line(label, false, 13.0)).padding([4, 8]);
        if enabled {
            button.on_press(message).style(move |_t, _s| {
                let (background, foreground) = theme.colors();
                button::Style {
                    background: Some(Background::Color(background)),
                    text_color: foreground,
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: true,
                }
            })
        } else {
            button.style(move |_t, _s| {
                let (background, foreground) = theme.colors();
                button::Style {
                    background: Some(Background::Color(background)),
                    text_color: foreground,
                    border: Border::default(),
                    shadow: Shadow::default(),
                    snap: true,
                }
            })
        }
        .into()
    };
    row![
        item(
            state.texts.resolve("gui.op.new"),
            Message::OpStart(OpMode::SessionNew)
        ),
        item(
            state.texts.resolve("gui.op.rename"),
            Message::OpStart(OpMode::SessionRename)
        ),
        item(
            state.texts.resolve("gui.op.archive"),
            Message::OpStart(OpMode::SessionArchive { archived: true })
        ),
        item(
            state.texts.resolve("gui.op.restore"),
            Message::OpStart(OpMode::SessionArchive { archived: false })
        ),
        item(
            state.texts.resolve("gui.op.delete"),
            Message::OpStart(OpMode::SessionDelete)
        ),
        item(
            state.texts.resolve("gui.op.purge"),
            Message::OpStart(OpMode::Purge {
                before_ms: purge_cutoff_ms()
            })
        ),
    ]
    .spacing(6)
    .into()
}

/// 清理截止：默认早于 30 天（预览面板可改天数——op_input 即天数文本）。
fn purge_cutoff_ms() -> i64 {
    now_ms() as i64 - 30 * 24 * 60 * 60 * 1000
}

/// 操作面板（op_mode 激活时渲染在正文上方）：输入 + 预览/确认/取消 + 反馈行。
fn op_panel(state: &State) -> Element<'_, Message> {
    let mode = state.op_mode.clone().unwrap_or(OpMode::SessionNew);
    let needs_input = matches!(
        mode,
        OpMode::SessionNew | OpMode::SessionRename | OpMode::Purge { .. }
    );
    let destructive = matches!(
        mode,
        OpMode::SessionArchive { .. } | OpMode::SessionDelete | OpMode::Purge { .. }
    );
    let placeholder = match mode {
        OpMode::SessionNew | OpMode::SessionRename => state.texts.resolve("gui.op.input.title"),
        OpMode::Purge { .. } => state.texts.resolve("gui.op.input.days"),
        _ => "",
    };
    let mut panel = column![];
    if needs_input {
        panel = panel.push(
            text_input(placeholder, state.op_input.as_str())
                .on_input(Message::OpInputChanged)
                .on_submit(Message::OpConfirm)
                .padding(6)
                .width(Length::Fill),
        );
    }
    let mut actions = row![].spacing(6);
    if destructive {
        actions = actions.push(
            button(rich_line(
                state.texts.resolve("gui.op.preview"),
                false,
                13.0,
            ))
            .on_press(Message::OpPreview)
            .padding([4, 8]),
        );
        actions = actions.push(
            button(rich_line(
                state.texts.resolve("gui.op.confirm"),
                false,
                13.0,
            ))
            .on_press(Message::OpConfirm)
            .padding([4, 8]),
        );
    } else {
        actions = actions.push(
            button(rich_line(
                state.texts.resolve("gui.op.confirm"),
                false,
                13.0,
            ))
            .on_press(Message::OpConfirm)
            .padding([4, 8]),
        );
    }
    actions = actions.push(
        button(rich_line(state.texts.resolve("gui.op.cancel"), false, 13.0))
            .on_press(Message::OpCancel)
            .padding([4, 8]),
    );
    panel = panel.push(actions);
    if let Some(feedback) = state.op_feedback.as_deref() {
        panel = panel.push(rich_line(feedback, false, 13.0));
    }
    panel.spacing(6).into()
}

/// 任务行按钮（Tasks 视图）：每行 retry/abort（与 TUI 的 R/X 同语义，ADR-056）。
fn task_buttons(state: &State) -> Element<'_, Message> {
    let mut rows = column![].spacing(2);
    for (index, task) in state.snapshot.tasks.iter().take(50).enumerate() {
        let label = format!("{} {} {}", task.id, task.status.as_str(), task.title);
        let retry = button(rich_line(
            state.texts.resolve("gui.task.retry"),
            false,
            12.0,
        ))
        .on_press(Message::TaskRetry(index))
        .padding([2, 6]);
        let abort = button(rich_line(
            state.texts.resolve("gui.task.abort"),
            false,
            12.0,
        ))
        .on_press(Message::TaskAbort(index))
        .padding([2, 6]);
        rows = rows.push(row![rich_line(label.as_str(), false, 13.0), retry, abort,].spacing(6));
    }
    if state.snapshot.tasks.is_empty() {
        rows = rows.push(rich_line(state.texts.resolve("tui.empty"), false, 13.0));
    }
    scrollable(rows.height(Length::Fill)).into()
}

fn view(state: &State) -> Element<'_, Message> {
    let items: Vec<Element<'_, Message>> = ViewId::ALL
        .iter()
        .map(|item| sidebar_button(state, *item))
        .collect();
    let sidebar = container(column(items).spacing(4))
        .width(Length::Fixed(200.0))
        .height(Length::Fill);

    let body: Element<'_, Message> = match state.view {
        ViewId::Chat => chat_panel(state),
        ViewId::Sessions if state.op_mode.is_some() => {
            column![op_panel(state), read_body(state, false)]
                .spacing(8)
                .into()
        }
        ViewId::Sessions => column![op_buttons(state), read_body(state, false)]
            .spacing(8)
            .into(),
        ViewId::Tasks => column![task_buttons(state), read_body(state, true)]
            .spacing(8)
            .into(),
        _ => read_body(state, state.view == ViewId::Diff),
    };

    // 顶栏：视图标题（20）+ 当前会话（13）；其下为 KPI 条与正文；底部状态栏。
    let header = column![
        rich_line(state.texts.resolve(state.view.title_key()), false, 20.0),
        rich_line(state.session_label().as_str(), false, 13.0),
    ]
    .spacing(4);

    let main = column![
        header,
        rich_line(state.kpi().as_str(), false, 13.0),
        body,
        rich_line(state.footer().as_str(), false, 13.0),
    ]
    .spacing(12)
    .width(Length::Fill);

    container(row![sidebar, main].spacing(16))
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_names_round_trip_and_unknown_is_rejected() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
            GuiTheme::TerminalGreen,
            GuiTheme::ZcodeDay,
            GuiTheme::ZcodeNight,
        ] {
            assert_eq!(GuiTheme::parse(theme.name()), Some(theme));
        }
        assert_eq!(GuiTheme::parse("solarized"), None);
    }

    #[test]
    fn theme_colors_are_exact() {
        assert_eq!(GuiTheme::EinkDay.colors(), (Color::WHITE, Color::BLACK));
        assert_eq!(GuiTheme::EinkNight.colors(), (Color::BLACK, Color::WHITE));
        let (background, foreground) = GuiTheme::AmoledNight.colors();
        assert_eq!(background, Color::BLACK);
        assert!((foreground.r - 0.902).abs() < f32::EPSILON);
        assert_eq!(foreground.r, foreground.g);
        assert_eq!(foreground.g, foreground.b);
        // terminal-green：OLED 兼容纯黑底 + 荧光绿字（#00FF00，绿通道满幅）。
        let (background, foreground) = GuiTheme::TerminalGreen.colors();
        assert_eq!(background, Color::BLACK);
        assert_eq!(foreground.r, 0.0);
        assert!((foreground.g - 1.0).abs() < f32::EPSILON);
        assert_eq!(foreground.b, 0.0);
        // zcode 系：从 theme_tokens 冻结值解析（FR-739/740，2026-11-06 用户批准）。
        let (background, foreground) = GuiTheme::ZcodeDay.colors();
        assert_eq!(background, Color::from_rgb8(0xf8, 0xf8, 0xf8));
        assert_eq!(foreground, Color::from_rgb8(0x26, 0x26, 0x26));
        let (background, foreground) = GuiTheme::ZcodeNight.colors();
        assert_eq!(background, Color::from_rgb8(0x16, 0x16, 0x16));
        assert_eq!(foreground, Color::from_rgb8(0xd4, 0xd4, 0xd4));
    }

    #[test]
    fn theme_cycle_returns_to_start_after_six_steps() {
        let mut theme = GuiTheme::EinkDay;
        let mut seen = Vec::new();
        for _ in 0..6 {
            theme = theme.next();
            seen.push(theme.name());
        }
        assert_eq!(
            seen,
            [
                "eink-night",
                "amoled-night",
                "terminal-green",
                "zcode-day",
                "zcode-night",
                "eink-day"
            ]
        );
        assert_eq!(theme, GuiTheme::EinkDay);
    }

    /// 默认主题为 zcode-night（2026-11-06 用户批准切换，ADR-057/FR-739）。
    #[test]
    fn default_theme_is_zcode_night() {
        assert_eq!(GuiTheme::default(), GuiTheme::ZcodeNight);
        let (background, _foreground) = GuiTheme::default().colors();
        assert_eq!(background, Color::from_rgb8(0x16, 0x16, 0x16));
    }

    #[test]
    fn scrollbar_uses_only_theme_colors() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
        ] {
            let (background, foreground) = theme.colors();
            let style = theme.scrollbar();
            let rail = style.vertical_rail;
            assert_eq!(rail.background, Some(Background::Color(background)));
            assert_eq!(rail.scroller.background, Background::Color(foreground));
        }
    }

    #[test]
    fn runs_split_latin_and_cjk() {
        assert_eq!(
            split_runs("会话 Sessions 1/6 核对"),
            vec![(true, "会话"), (false, " Sessions 1/6 "), (true, "核对")]
        );
        assert!(split_runs("").is_empty());
        assert_eq!(split_runs("sessions"), vec![(false, "sessions")]);
        assert_eq!(split_runs("会话"), vec![(true, "会话")]);
    }

    #[test]
    fn font_selection_covers_four_categories() {
        assert_eq!(font_for(false, false), Font::with_name("Inter"));
        assert_eq!(font_for(true, false), Font::with_name("Noto Sans CJK SC"));
        assert_eq!(font_for(false, true), Font::with_name("JetBrains Mono"));
        assert_eq!(font_for(true, true), Font::with_name("Sarasa Mono SC"));
    }

    #[test]
    fn cjk_ranges_cover_punctuation_and_fullwidth() {
        assert!(is_cjk('会'));
        assert!(is_cjk('，'));
        assert!(is_cjk('（'));
        assert!(!is_cjk('S'));
        // 通用标点（U+2014）由 Inter/Noto 的拉丁子集覆盖，不按 CJK 切段。
        assert!(!is_cjk('—'));
    }

    /// 空数据源状态（T199 顶栏/KPI/状态栏取证；不触库）。
    fn empty_state() -> State {
        let source: Rc<dyn ViewSource> = Rc::new(whcode_app::view_source::EmptyViewSource);
        State::new(source, I18n::default(), GuiTheme::EinkDay, None, None)
    }

    /// T199：窗口标题与网络层同源词条（应用名 + 视图序号），状态栏含主题名。
    #[test]
    fn title_and_status_bar_use_shared_text_layer() {
        let state = empty_state();
        assert!(
            state.title().contains("旺皇大模型执行框架"),
            "标题须含应用名：{}",
            state.title()
        );
        assert!(
            state.title().contains("会话 (1/7)"),
            "标题须含视图序号：{}",
            state.title()
        );
        assert_eq!(
            state.session_label(),
            "未选择会话",
            "未选择时必须有显式占位"
        );
        assert!(
            state.footer().contains("eink-day"),
            "状态栏须显示主题名：{}",
            state.footer()
        );
        assert!(
            state.footer().contains("1200x800"),
            "状态栏须显示窗口尺寸：{}",
            state.footer()
        );
    }

    /// T199：KPI 条优先准备层摘要（任务视图含总计/完成/阻塞），否则回落可得计数。
    #[test]
    fn kpi_prefers_prepared_summary_then_counts() {
        let mut state = empty_state();
        assert!(
            state.kpi().contains("会话 0"),
            "空态 KPI 须给出可得计数：{}",
            state.kpi()
        );
        state.snapshot.summary = "会话 S-1 · 总计 3 · 完成 2 · 阻塞 1".to_string();
        assert_eq!(state.kpi(), state.snapshot.summary);
    }

    /// T199：侧栏选中态只用主题两色（反显），不引入灰色、边框或阴影。
    #[test]
    fn sidebar_selection_inverts_theme_colors_only() {
        for theme in [
            GuiTheme::EinkDay,
            GuiTheme::EinkNight,
            GuiTheme::AmoledNight,
            GuiTheme::TerminalGreen,
        ] {
            let (background, foreground) = theme.colors();
            let selected = sidebar_style(theme, true);
            assert_eq!(selected.background, Some(Background::Color(foreground)));
            assert_eq!(selected.text_color, background);
            let normal = sidebar_style(theme, false);
            assert_eq!(normal.background, Some(Background::Color(background)));
            assert_eq!(normal.text_color, foreground);
            assert_eq!(normal.border, Border::default(), "侧栏不得加边框");
            assert_eq!(normal.shadow, Shadow::default(), "侧栏不得加阴影");
        }
    }

    /// Phase 2 测试桩：记录调用的 LLM 执行器（不触库）。
    struct StubLlm {
        fail: bool,
    }
    impl GuiLlmExecutor for StubLlm {
        fn run_with_progress(
            &self,
            _session_id: &str,
            _provider: &str,
            _prompt: &str,
            progress: whcode_app::turn::TurnProgress<'_>,
        ) -> Result<String, WhError> {
            progress(1, "你");
            progress(2, "好");
            if self.fail {
                return Err(WhError::invalid_state("stub 失败"));
            }
            Ok("provider=stub chunks=2 completed=true request_id=R-1".to_string())
        }

        fn cancel(&self, _request_id: &str) -> Result<String, WhError> {
            Ok("cancelled".to_string())
        }

        fn profiles(&self) -> Vec<String> {
            vec!["stub".to_string()]
        }
    }

    fn interactive_state(fail: bool) -> State {
        let source: Rc<dyn ViewSource> = Rc::new(whcode_app::view_source::EmptyViewSource);
        let mut state = State::new(
            source,
            I18n::default(),
            GuiTheme::EinkDay,
            None,
            Some(std::sync::Arc::new(StubLlm { fail })),
        );
        // 预置选中会话（对话提交的前置）。
        state.session_ids = vec!["S-1".to_string()];
        state.selected = Some(0);
        state.chat_input = "提示词".to_string();
        state
    }

    /// Phase 2：ChatDone 正确解除 turn_running 并落结果（成功路径）。
    #[test]
    fn chat_done_clears_running_and_records_summary() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        let _ = update(
            &mut state,
            Message::ChatDone(Ok("provider=stub chunks=2 completed=true".to_string())),
        );
        assert!(!state.turn_running, "完成后必须解除运行态");
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("provider=stub")),
            "完成后必须呈现摘要：{:?}",
            state.chat_last
        );
    }

    /// Phase 2：ChatDone 错误路径同样解除运行态（不把失败伪装成成功）。
    #[test]
    fn chat_done_error_clears_running_and_records_error() {
        let mut state = interactive_state(true);
        state.turn_running = true;
        let _ = update(
            &mut state,
            Message::ChatDone(Err(WhError::invalid_state("失败"))),
        );
        assert!(!state.turn_running, "失败后必须解除运行态");
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("失败")),
            "失败必须可见：{:?}",
            state.chat_last
        );
    }

    /// FR-741：进度增量逐块累积到流式缓冲；完成后清空（正文落 chat_last）。
    #[test]
    fn chat_progress_appends_streaming_and_done_clears_it() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        let _ = update(&mut state, Message::ChatProgress(1, "你".to_string()));
        let _ = update(&mut state, Message::ChatProgress(2, "好".to_string()));
        assert_eq!(state.chat_progress, 2);
        assert_eq!(state.chat_streaming, "你好");
        let _ = update(
            &mut state,
            Message::ChatDone(Ok("provider=stub chunks=2".to_string())),
        );
        assert!(state.chat_streaming.is_empty(), "完成后缓冲必须清空");
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("provider=stub")),
        );
    }

    /// FR-741 有界性：流式缓冲超限截断且截断点落在字符边界（不得 panic）。
    #[test]
    fn streaming_buffer_is_bounded_on_char_boundary() {
        let mut buffer = String::new();
        let cjk_chunk = "你好世界".repeat(64);
        for _ in 0..((CHAT_STREAMING_CAP_BYTES / cjk_chunk.len()) + 8) {
            append_streaming(&mut buffer, &cjk_chunk);
        }
        assert!(
            buffer.len() <= CHAT_STREAMING_CAP_BYTES,
            "缓冲必须有界：{}",
            buffer.len()
        );
        assert!(buffer.is_char_boundary(buffer.len()));
    }

    /// FR-741 markdown 最小集投影：标题／无序列表／代码围栏／行内码；其余按普通文本。
    #[test]
    fn md_project_covers_minimal_markdown() {
        let text =
            "# 标题\n\n- 项目 `code` 一\n* 项目二\n```rust\nlet a = 1;\n```\n普通 `行内` 文本";
        let lines = md_project(text);
        assert_eq!(lines.len(), 6, "空行产出空 Text 行：{lines:?}");
        assert_eq!(lines[0].kind, MdLineKind::Heading);
        assert_eq!(lines[0].segs, vec![(false, "标题".to_string())]);
        assert_eq!(lines[1].kind, MdLineKind::Text);
        assert_eq!(lines[2].kind, MdLineKind::Bullet);
        assert_eq!(
            lines[2].segs,
            vec![
                (false, "项目 ".to_string()),
                (true, "code".to_string()),
                (false, " 一".to_string())
            ]
        );
        assert_eq!(lines[3].kind, MdLineKind::Bullet);
        assert_eq!(lines[4].kind, MdLineKind::Code);
        assert_eq!(lines[4].segs, vec![(true, "let a = 1;".to_string())]);
        assert_eq!(lines[5].kind, MdLineKind::Text);
    }

    /// FR-741 有界渲染：超过 CHAT_RENDER_MAX_LINES 行时以“…”行标注。
    #[test]
    fn md_elements_caps_rendered_lines() {
        let text = "- 项\n".repeat(CHAT_RENDER_MAX_LINES + 25);
        let elements = md_elements(text.as_str());
        assert_eq!(
            elements.len(),
            CHAT_RENDER_MAX_LINES + 1,
            "渲染行必须封顶并以省略行标注"
        );
    }

    /// Phase 2：turn 运行中冻结交互（视图切换/刷新被忽略），进度可见。
    #[test]
    fn turn_running_freezes_interactions_and_shows_progress() {
        let mut state = interactive_state(false);
        state.turn_running = true;
        state.chat_progress = 7;
        let _ = update(&mut state, Message::SelectView(ViewId::Tasks));
        let _ = update(&mut state, Message::Refresh);
        let _ = update(&mut state, Message::ChatProgress(9, String::new()));
        assert_eq!(state.view, ViewId::Sessions, "运行中不得切换视图");
        assert_eq!(state.chat_progress, 9, "进度必须更新");
        assert!(state.turn_running, "进度不得解除运行态");
    }

    /// Phase 2：ChatSubmit 设运行态；无执行器/无会话/空提示词显式提示且不进入运行态。
    #[test]
    fn chat_submit_guards_are_explicit() {
        let mut state = interactive_state(false);
        state.llm = None;
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("not_supported")),
            "无执行器必须显式提示：{:?}",
            state.chat_last
        );

        let mut state = interactive_state(false);
        state.chat_input = "  ".to_string();
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("为空")),
            "空提示词必须显式提示：{:?}",
            state.chat_last
        );

        let mut state = interactive_state(false);
        state.selected = None;
        let _ = update(&mut state, Message::ChatSubmit);
        assert!(
            state
                .chat_last
                .as_deref()
                .is_some_and(|text| text.contains("未选择会话")),
            "无会话必须显式提示：{:?}",
            state.chat_last
        );
    }
}
