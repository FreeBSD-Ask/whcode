//! # whcode-gui
//!
//! 原生桌面 GUI（只读 v1，FR-711—FR-715、ADR-027）。
//!
//! 边界：
//! - 只经 [`whcode_app::view_source`] 的只读快照取数，**不触库**、不绕过应用服务（ADR-003）；
//! - 不内嵌任何浏览器引擎（ADR-026）；渲染由 `iced` 自绘，使用 **tiny-skia 软件光栅**，
//!   默认不创建 GPU 上下文（NFR-212、ADR-027）；
//! - 空闲不重绘：没有定时器与动画，重绘只由输入或窗口事件触发（FR-714）；
//! - v1 只读：破坏性命令不经 GUI（FR-711）。
//!
//! 渲染纪律（§5.3）：帧路径只消费准备阶段生成的纯数据快照，不反序列化 JSON、不查询数据库。

pub mod surface_map;

use std::rc::Rc;

use iced::keyboard::{Key, key};
use iced::widget::{button, column, container, rich_text, row, scrollable, span, text};
use iced::{
    Background, Border, Color, Element, Font, Length, Shadow, Size, Subscription, Task, Theme,
    window,
};
use whcode_app::view_source::{VIEW_COUNT, ViewId, ViewSnapshot, ViewSource};
use whcode_domain::i18n::I18n;

/// 窗口初始逻辑尺寸（与 FR-712 的预算口径一致）。
const WINDOW_SIZE: Size = Size::new(1200.0, 800.0);

/// 内置字体子集（FR-716／ADR-028）：完整字体在 `assets/fonts/source`，
/// 这里只嵌入 `.github/scripts/subset-fonts.py` 生成的子集，避免整份 CJK 字体常驻。
const FONT_INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.subset.ttf");
const FONT_NOTO: &[u8] = include_bytes!("../assets/fonts/NotoSansCJKsc-Regular.subset.otf");
const FONT_JETBRAINS: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.subset.ttf");
const FONT_SARASA: &[u8] = include_bytes!("../assets/fonts/SarasaMonoSC-Regular.subset.ttf");

/// GUI 主题（四套精确色值，FR-612／FR-713；`terminal-green` 为 2026-10-24 用户新增）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuiTheme {
    /// 墨水屏日间：纯白底、纯黑字（默认，残影最小）。
    #[default]
    EinkDay,
    /// 墨水屏夜间：纯黑底、纯白字。
    EinkNight,
    /// AMOLED 夜间：纯黑底、浅灰字（仅纯黑可关断 OLED 像素）。
    AmoledNight,
    /// 终端绿夜间：纯黑底（OLED 兼容纯黑）、荧光绿字（CRT P1 风格，#00FF00）。
    TerminalGreen,
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
            _ => None,
        }
    }

    /// 下一套主题（`t` 键循环；FR-713：切换不得要求重启）。
    #[must_use]
    const fn next(self) -> Self {
        match self {
            Self::EinkDay => Self::EinkNight,
            Self::EinkNight => Self::AmoledNight,
            Self::AmoledNight => Self::TerminalGreen,
            Self::TerminalGreen => Self::EinkDay,
        }
    }

    /// 背景色与前景色（精确 RGB）。
    #[must_use]
    pub const fn colors(self) -> (Color, Color) {
        match self {
            Self::EinkDay => (Color::WHITE, Color::BLACK),
            Self::EinkNight => (Color::BLACK, Color::WHITE),
            Self::AmoledNight => (Color::BLACK, Color::from_rgb(0.902, 0.902, 0.902)),
            Self::TerminalGreen => (Color::BLACK, Color::from_rgb(0.0, 1.0, 0.0)),
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

/// 启动参数。
#[derive(Default)]
pub struct RunOptions {
    /// 主题（FR-612）。
    pub theme: GuiTheme,
    /// 界面文本层（FR-615）：默认 zh-CN + en 回退。
    pub texts: I18n,
}

/// 界面消息（全部由按键或窗口事件产生，无定时器）。
#[derive(Debug, Clone)]
enum Message {
    SelectView(ViewId),
    NextSession,
    PrevSession,
    Refresh,
    CycleTheme,
    Resized(Size),
    Quit,
}

/// 应用状态：只持有只读快照与选择位置。
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
}

impl State {
    fn new(source: Rc<dyn ViewSource>, texts: I18n, theme: GuiTheme) -> Self {
        let snapshot = source.snapshot(ViewId::Sessions, None);
        let session_ids = snapshot
            .sessions
            .iter()
            .map(|row| row.id.clone())
            .collect::<Vec<_>>();
        Self {
            source,
            texts,
            theme,
            view: ViewId::Sessions,
            selected: None,
            session_ids,
            snapshot,
            size: WINDOW_SIZE,
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
            ViewId::Context | ViewId::Spec => 0,
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
    let RunOptions { theme, texts } = options;

    let mut app = iced::application(
        move || State::new(Rc::clone(&source), texts.clone(), theme),
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
    match message {
        Message::SelectView(view) => {
            state.view = view;
            state.prepare();
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
        }
        Message::PrevSession => {
            if !state.session_ids.is_empty() {
                state.selected = Some(state.selected.map_or(0, |index| index.saturating_sub(1)));
                state.view = ViewId::Sessions;
                state.prepare();
            }
        }
        Message::Refresh => {
            state.prepare();
        }
        Message::CycleTheme => state.theme = state.theme.next(),
        Message::Resized(size) => state.size = size,
        Message::Quit => return iced::exit(),
    }
    Task::none()
}

fn subscription(_: &State) -> Subscription<Message> {
    iced::event::listen_raw(handle_event)
}

/// 原始事件 → 消息。`RedrawRequested` 必须返回 `None`（否则会形成无限重绘循环）。
fn handle_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::Resized(size)) => Some(Message::Resized(size)),
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => match key {
            Key::Named(key::Named::Escape) => Some(Message::Quit),
            Key::Character(character) => match character.as_str() {
                "1" | "2" | "3" | "4" | "5" | "6" => {
                    let index = character.parse::<usize>().ok()?.saturating_sub(1);
                    ViewId::ALL.get(index).copied().map(Message::SelectView)
                }
                "j" => Some(Message::NextSession),
                "k" => Some(Message::PrevSession),
                "r" => Some(Message::Refresh),
                "t" => Some(Message::CycleTheme),
                "q" => Some(Message::Quit),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    }
}

/// 侧栏条目样式：只用主题两色（选中反显），不引入灰色、边框或阴影（ADR-041）。
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

fn view(state: &State) -> Element<'_, Message> {
    let items: Vec<Element<'_, Message>> = ViewId::ALL
        .iter()
        .map(|item| sidebar_button(state, *item))
        .collect();
    let sidebar = container(column(items).spacing(4))
        .width(Length::Fixed(200.0))
        .height(Length::Fill);

    let lines = state.snapshot.render_lines_limited(500);
    // diff 预览用等宽字体（拉丁 JetBrains Mono／CJK Sarasa Mono SC），其余用比例字体。
    let mono = state.view == ViewId::Diff;
    let body: Element<'_, Message> = if lines.is_empty() {
        rich_line(state.texts.resolve("tui.empty"), false, 16.0)
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
    }

    #[test]
    fn theme_cycle_returns_to_start_after_four_steps() {
        let mut theme = GuiTheme::EinkDay;
        let mut seen = Vec::new();
        for _ in 0..4 {
            theme = theme.next();
            seen.push(theme.name());
        }
        assert_eq!(
            seen,
            ["eink-night", "amoled-night", "terminal-green", "eink-day"]
        );
        assert_eq!(theme, GuiTheme::EinkDay);
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
        State::new(source, I18n::default(), GuiTheme::EinkDay)
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
            state.title().contains("会话 (1/6)"),
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
}
