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

use std::rc::Rc;

use iced::keyboard::{Key, key};
use iced::widget::{column, container, row, scrollable, text};
use iced::{
    Background, Border, Color, Element, Font, Length, Shadow, Size, Subscription, Task, Theme,
    window,
};
use whcode_app::view_source::{VIEW_COUNT, ViewId, ViewSnapshot, ViewSource};
use whcode_domain::i18n::I18n;

/// 窗口初始逻辑尺寸（与 FR-712 的预算口径一致）。
const WINDOW_SIZE: Size = Size::new(1200.0, 800.0);

/// 中文字体候选族名（按顺序取系统里第一个可用的；不把字体字节读进进程）。
///
/// 说明（FR-712）：微软雅黑等系统字体**不可再分发**，因此不能把子集化字体随包发布；
/// 改为按族名交给系统字体栈按需加载，进程不常驻整份字体字节。
const FONT_FAMILIES: [(&str, &str); 2] = [
    (r"C:\Windows\Fonts\msyh.ttc", "Microsoft YaHei"),
    (r"C:\Windows\Fonts\simhei.ttf", "SimHei"),
];

/// GUI 主题（三套精确色值，FR-612／FR-713）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GuiTheme {
    /// 墨水屏日间：纯白底、纯黑字（默认，残影最小）。
    #[default]
    EinkDay,
    /// 墨水屏夜间：纯黑底、纯白字。
    EinkNight,
    /// AMOLED 夜间：纯黑底、浅灰字（仅纯黑可关断 OLED 像素）。
    AmoledNight,
}

impl GuiTheme {
    /// 配置名（与 `ui.theme` 取值一致）。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::EinkDay => "eink-day",
            Self::EinkNight => "eink-night",
            Self::AmoledNight => "amoled-night",
        }
    }

    /// 按配置名解析；未知取值返回 `None`（调用方决定回退或报错）。
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "eink-day" => Some(Self::EinkDay),
            "eink-night" => Some(Self::EinkNight),
            "amoled-night" => Some(Self::AmoledNight),
            _ => None,
        }
    }

    /// 下一套主题（`t` 键循环；FR-713：切换不得要求重启）。
    #[must_use]
    const fn next(self) -> Self {
        match self {
            Self::EinkDay => Self::EinkNight,
            Self::EinkNight => Self::AmoledNight,
            Self::AmoledNight => Self::EinkDay,
        }
    }

    /// 背景色与前景色（精确 RGB）。
    #[must_use]
    pub const fn colors(self) -> (Color, Color) {
        match self {
            Self::EinkDay => (Color::WHITE, Color::BLACK),
            Self::EinkNight => (Color::BLACK, Color::WHITE),
            Self::AmoledNight => (Color::BLACK, Color::from_rgb(0.902, 0.902, 0.902)),
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
    /// 已按键数（页脚显示；导航键不计入）。
    keys: u32,
    /// 当前窗口逻辑尺寸（页脚显示，FR-712 口径）。
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
            keys: 0,
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
                ("view", self.texts.resolve(self.view.title_key())),
                ("index", &self.view.index().to_string()),
                ("count", &VIEW_COUNT.to_string()),
            ],
        )
    }

    /// 页脚（与 TUI 同一词条：`tui.footer`）。
    fn footer(&self) -> String {
        self.texts.resolve_fmt(
            "tui.footer",
            &[
                ("cols", &format!("{:.0}", self.size.width)),
                ("rows", &format!("{:.0}", self.size.height)),
                ("keys", &self.keys.to_string()),
            ],
        )
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
    let family = pick_font_family();

    iced::application(
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
    .default_font(Font::with_name(family))
    .run()
}

/// 选择系统中文字体族名：取第一个存在的字体文件对应的族名，都不存在时回落雅黑。
fn pick_font_family() -> &'static str {
    for (path, family) in FONT_FAMILIES {
        if std::path::Path::new(path).is_file() {
            return family;
        }
    }
    "Microsoft YaHei"
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
            state.keys = state.keys.saturating_add(1);
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

fn view(state: &State) -> Element<'_, Message> {
    let sidebar = column(
        ViewId::ALL
            .iter()
            .map(|view| {
                let marker = if state.view == *view { "> " } else { "  " };
                text!("{}{}", marker, state.texts.resolve(view.title_key()))
                    .size(18)
                    .into()
            })
            .collect::<Vec<Element<'_, Message>>>(),
    )
    .spacing(8)
    .width(Length::Fixed(180.0));

    let lines = state.snapshot.render_lines_limited(500);
    let body: Element<'_, Message> = if lines.is_empty() {
        text(state.texts.resolve("tui.empty")).size(16).into()
    } else {
        scrollable(
            column(
                lines
                    .into_iter()
                    .map(|line| text(line).size(16).into())
                    .collect::<Vec<_>>(),
            )
            .spacing(4),
        )
        .style(move |_, _| state.theme.scrollbar())
        .height(Length::Fill)
        .into()
    };

    let main = column![body, container(text(state.footer()).size(13)).padding(6)]
        .spacing(8)
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
    }

    #[test]
    fn theme_cycle_returns_to_start_after_three_steps() {
        let mut theme = GuiTheme::EinkDay;
        let mut seen = Vec::new();
        for _ in 0..3 {
            theme = theme.next();
            seen.push(theme.name());
        }
        assert_eq!(seen, ["eink-night", "amoled-night", "eink-day"]);
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
}
