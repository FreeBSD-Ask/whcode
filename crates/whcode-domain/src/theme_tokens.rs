//! # 视觉 token 单一事实源（FR-739／FR-740，ADR-057 决策 2/3）
//!
//! 本模块是主题色值的**唯一定义处**：`whcode-day`／`whcode-night` 两套主题的全部
//! token 在此冻结，GUI（`whcode-gui`）直接消费，Web（`whcode-web`）的
//! `frontend/styles.css` 作为**受校验产物**由契约测试对照本表断言（篡改 CSS
//! 色值即测试失败，同 feature-parity 矩阵模式）。
//!
//! ## 冻结依据与效力
//!
//! - 2026-11-06 用户批准冻结（ADR-057 决策 2：批次 R 提案表，`research/zcode-comparison/domain-M2-ui-experience.md` §7）。
//! - 提案来源：ZCode 基线 `local/zcode-src`@`3c5b2276` 的 `.theme-zai-light`／
//!   `.theme-zai-dark` 块（`packages/ui/src/styles.css:513/677` 起）。
//! - `foreground` 在 ZCode 源码内为 Tailwind 间接引用（neutral-800／neutral-300），
//!   克隆未含 node_modules；本表按 Tailwind v4 官方默认调色板冻结
//!   （neutral-800 = `#262626`、neutral-300 = `#d4d4d4`）。若实机对照发现 ZCode
//!   渲染值不同，走 ADR-057 复审触发修订本表（当前为登记过的决策，非 `[待验证]`）。
//!
//! ## 铁律：既有四套主题不在本表管辖范围
//!
//! `eink-day`／`eink-night`／`amoled-night`／`terminal-green` 的精确色值由
//! AGENTS.md §6 与 `whcode-tui::theme` 的铁律承载（OLED 纯黑、墨水屏真灰口径），
//! **本模块不重定义、不派生它们**；[`LEGACY_THEMES`] 仅作 bg/fg 锚点供契约测试
//! 对照 Web CSS，保证六套主题的 `[data-theme]` 块齐全（FR-739：whcode 系为新增
//! 而非覆盖）。扩展 token（brand/accent/panel/…/terminal 调色板）只有 whcode 系
//! 定义；既有四套保持 FR-713 口径（不引入主题外颜色），B1 对话渲染使用扩展
//! token 时仅对 whcode 系生效。

/// whcode 系主题的完整视觉 token（冻结值，见模块文档）。
pub struct ThemeTokens {
    /// 稳定配置名（kebab-case，与 `ui.theme`／`data-theme` 一致）。
    pub name: &'static str,
    /// 页面/窗口背景。
    pub background: &'static str,
    /// 正文前景。
    pub foreground: &'static str,
    /// 强调/主操作色（zai 块 `--color-brand`）。
    pub brand: &'static str,
    /// 选中底色（zai 块 `--color-accent`）。
    pub accent: &'static str,
    /// 头部条背景（`--color-header`）。
    pub header: &'static str,
    /// 面板背景（`--color-panel`）。
    pub panel: &'static str,
    /// 侧栏背景（`--color-sidebar`）。
    pub sidebar: &'static str,
    /// 卡片背景（`--color-card`）。
    pub card: &'static str,
    /// 常规边框（含透明度的 rgba 串，冻结原样）。
    pub border: &'static str,
    /// 悬停边框。
    pub border_hover: &'static str,
    /// 悬停底色。
    pub hover: &'static str,
    /// 选中底色（次级）。
    pub selected: &'static str,
    /// 弱表面底色。
    pub surface: &'static str,
    /// 弱表面悬停底色。
    pub surface_hover: &'static str,
    /// 终端 8 色（diff/代码块着色用）；`None`＝本主题未冻结终端调色板。
    pub terminal: Option<&'static TerminalPalette>,
    /// 扩展 UI 语义色板（ADR-102；卡片/输入/菜单/浮层/状态各自的表面色）。
    pub ui: &'static UiPalette,
}

/// 终端 8 色调色板（冻结自 ZCode zai 块 `--color-terminal-*`；bright 系未提取，
/// 待 B1 有消费方时随复审补齐）。
pub struct TerminalPalette {
    pub black: &'static str,
    pub red: &'static str,
    pub green: &'static str,
    pub yellow: &'static str,
    pub blue: &'static str,
    pub magenta: &'static str,
    pub cyan: &'static str,
    pub white: &'static str,
}

/// 扩展 UI 语义色板（ADR-102，来源 ZCode `zai-light`／`zai-dark` 的 `--color-*` 核心集）。
///
/// 与 [`ThemeTokens`] 的 14 个基础 token 分离，避免破坏既有契约与 GUI 调用点；
/// 值是 ZCode 原值（`color-mix(... transparent)` 已折算为等效 rgba）。
/// 这些 token 是“界面看得懂”的基础：卡片/输入框/菜单/浮层/状态各有独立表面色，
/// 不再靠背景或前景色反相表达层级。
pub struct UiPalette {
    /// 交替区域背景（`--color-background-alt` 的实色折算）。
    pub background_alt: &'static str,
    /// 选中卡片（`--color-card-selected`）。
    pub card_selected: &'static str,
    /// 浮层/对话框背景（`--color-popover`）。
    pub popover: &'static str,
    /// 浮层头部（`--color-popover-header`）。
    pub popover_header: &'static str,
    /// 输入框默认背景（`--color-input`）。
    pub input: &'static str,
    /// 输入框聚焦背景（`--color-input-focused`）。
    pub input_focused: &'static str,
    /// 菜单表面（`--color-menu`）。
    pub menu: &'static str,
    /// 菜单悬停（`--color-menu-hover`）。
    pub menu_hover: &'static str,
    /// 主操作填充（`--color-primary`）。
    pub primary: &'static str,
    /// 主操作前景（`--color-primary-foreground`）。
    pub primary_foreground: &'static str,
    /// 次级填充（`--color-secondary`）。
    pub secondary: &'static str,
    /// 标签页底色（`--color-tab`）。
    pub tab: &'static str,
    /// 活动标签页（`--color-tab-active`）。
    pub tab_active: &'static str,
    /// 用户消息气泡（`--color-user-message-bubble`）。
    pub user_message_bubble: &'static str,
    /// 次级文字（`--color-foreground-subtle` 折算）。
    pub foreground_subtle: &'static str,
    /// 三级文字（`--color-foreground-subtlest` 折算）。
    pub foreground_subtlest: &'static str,
    /// 反色文字（`--color-foreground-inverse`）。
    pub foreground_inverse: &'static str,
    /// 成功色（`--color-success`）。
    pub success: &'static str,
    /// 成功前景（`--color-success-foreground`）。
    pub success_foreground: &'static str,
    /// 成功弱表面（`--color-success-surface` 折算）。
    pub success_surface: &'static str,
    /// 成功弱表面前景（`--color-success-surface-foreground`）。
    pub success_surface_foreground: &'static str,
    /// 危险色（`--color-destructive`）。
    pub destructive: &'static str,
    /// 危险前景（`--color-destructive-foreground`）。
    pub destructive_foreground: &'static str,
    /// 危险弱表面（`--color-destructive-surface` 折算）。
    pub destructive_surface: &'static str,
    /// 危险弱表面前景（`--color-destructive-surface-foreground`）。
    pub destructive_surface_foreground: &'static str,
    /// 提示条表面（`--color-toast`）。
    pub toast: &'static str,
    /// 工具提示表面（`--color-tooltip`）。
    pub tooltip: &'static str,
    /// 工具提示前景（`--color-tooltip-foreground`）。
    pub tooltip_foreground: &'static str,
    /// 标签/行内码底色（`--color-tag`）。
    pub tag: &'static str,
    /// 询问态弱表面（`--color-interaction-ask-surface`）。
    pub interaction_ask_surface: &'static str,
    /// 询问态前景（`--color-interaction-ask-foreground`）。
    pub interaction_ask_foreground: &'static str,
    /// 确认态弱表面（`--color-interaction-confirmation-surface` 折算）。
    pub interaction_confirmation_surface: &'static str,
    /// 确认态前景（`--color-interaction-confirmation-foreground`）。
    pub interaction_confirmation_foreground: &'static str,
}

/// 视觉规格常量（ADR-102，来源 ZCode `DESIGN.md`：Radius／Typography／Spacing／Elevation）。
///
/// 双端由同一常量派生（GUI 用像素值，Web 写为 CSS 变量并由契约测试断言），
/// 避免“GUI 一个圆角、Web 另一个圆角”的漂移。
pub mod spec {
    /// 圆角（px）：`2xl`／`xl`／`lg`／`md`／`sm`，最小 `sm`。
    pub const RADIUS_2XL: u16 = 16;
    pub const RADIUS_XL: u16 = 12;
    pub const RADIUS_LG: u16 = 8;
    pub const RADIUS_MD: u16 = 6;
    pub const RADIUS_SM: u16 = 4;

    /// UI 字号基准（px）与七级刻度（相对基准的偏移）。
    pub const UI_FONT_BASE: u16 = 14;
    pub const UI_FONT_XL: u16 = UI_FONT_BASE + 4;
    pub const UI_FONT_LG: u16 = UI_FONT_BASE + 2;
    pub const UI_FONT_CAPTION: u16 = UI_FONT_BASE - 1;
    pub const UI_FONT_SM: u16 = UI_FONT_BASE - 2;
    pub const UI_FONT_XS: u16 = UI_FONT_BASE - 4;
    pub const UI_FONT_2XS: u16 = UI_FONT_BASE - 5;

    /// 间距基准（px）：4 的倍数，五档常用节奏。
    pub const SPACE_UNIT: u16 = 4;
    pub const SPACE_1: u16 = 4;
    pub const SPACE_2: u16 = 8;
    pub const SPACE_3: u16 = 12;
    pub const SPACE_4: u16 = 16;
    pub const SPACE_5: u16 = 24;

    /// Elevation 三档（CSS 阴影串；GUI 以边框＋底纹近似，不绘制投影）。
    pub const SHADOW_SM: &str = "0 1px 3px rgb(0 0 0 / 4%)";
    pub const SHADOW_MD: &str = "0 1px 3px rgb(0 0 0 / 4%), 0 6px 18px rgb(0 0 0 / 4%)";
    pub const SHADOW_LG: &str = "0 8px 30px rgb(0 0 0 / 12%)";

    /// 侧栏宽度（px）与对话阅读宽度上限（ZCode 工作区口径）。
    /// 侧栏宽度（px）：取 ZCode `WORKSPACE_SIDEBAR_DEFAULT_WIDTH_PX = 264`。
    pub const SIDEBAR_WIDTH: u16 = 264;
    /// 侧栏最小宽度：ZCode 的 `WORKSPACE_SIDEBAR_MIN_WIDTH_PX` 同为 264（不可更窄），
    /// 故这里 `MIN == WIDTH`——窄窗口靠正文换行适应，不压缩导航。
    pub const SIDEBAR_MIN: u16 = 264;
    pub const SIDEBAR_MAX: u16 = 360;
    pub const CONTENT_MAX_WIDTH: u16 = 880;
}

/// whcode-day（← ZCode `zai-light`，浅色）。
pub const WHCODE_DAY: ThemeTokens = ThemeTokens {
    name: "whcode-day",
    background: "#f8f8f8",
    foreground: "#262626",
    brand: "#000000",
    accent: "#ebf4ff",
    header: "#ffffff",
    panel: "#ffffff",
    sidebar: "#f0f0f0",
    card: "#ffffff",
    border: "rgba(13,13,13,0.10)",
    border_hover: "rgba(13,13,13,0.15)",
    hover: "rgba(13,13,13,0.05)",
    selected: "rgba(13,13,13,0.05)",
    surface: "rgba(13,13,13,0.03)",
    surface_hover: "rgba(13,13,13,0.05)",
    terminal: Some(&TERMINAL_DAY),
    ui: &UI_DAY,
};

/// whcode-night（← ZCode `zai-dark`，深色；2026-11-06 用户批准为默认主题）。
pub const WHCODE_NIGHT: ThemeTokens = ThemeTokens {
    name: "whcode-night",
    background: "#161616",
    foreground: "#d4d4d4",
    brand: "#ffffff",
    accent: "#001d3d",
    header: "#202020",
    panel: "#202020",
    sidebar: "#161616",
    card: "#2b2b2b",
    border: "rgba(255,255,255,0.10)",
    border_hover: "rgba(255,255,255,0.15)",
    hover: "rgba(255,255,255,0.05)",
    selected: "rgba(255,255,255,0.10)",
    surface: "rgba(255,255,255,0.05)",
    surface_hover: "rgba(255,255,255,0.10)",
    terminal: Some(&TERMINAL_NIGHT),
    ui: &UI_NIGHT,
};

/// whcode-day 的扩展语义色板（ZCode `zai-light` 原值；`color-mix` 折算为等效 rgba）。
pub const UI_DAY: UiPalette = UiPalette {
    background_alt: "#ececee",
    card_selected: "#ffffff",
    popover: "#ffffff",
    popover_header: "#f8f8f8",
    input: "#ffffff",
    input_focused: "#ffffff",
    menu: "#ffffff",
    menu_hover: "#f0f0f0",
    primary: "#000000",
    primary_foreground: "#ffffff",
    secondary: "#e6e6e6",
    tab: "#f0f0f0",
    tab_active: "#ffffff",
    user_message_bubble: "rgba(13,13,13,0.05)",
    foreground_subtle: "rgba(38,38,38,0.70)",
    foreground_subtlest: "rgba(38,38,38,0.40)",
    foreground_inverse: "#ffffff",
    success: "#1e8a3e",
    success_foreground: "#ffffff",
    success_surface: "#eaf7ee",
    success_surface_foreground: "#166b32",
    destructive: "#e03131",
    destructive_foreground: "#ffffff",
    destructive_surface: "rgba(224,49,49,0.10)",
    destructive_surface_foreground: "#b42020",
    toast: "#ffffff",
    tooltip: "#f0f0f0",
    tooltip_foreground: "#0d0d0d",
    tag: "#e6e6e6",
    interaction_ask_surface: "#ebf4ff",
    interaction_ask_foreground: "#0066dd",
    interaction_confirmation_surface: "#eaf7ee",
    interaction_confirmation_foreground: "#166b32",
};

/// whcode-night 的扩展语义色板（ZCode `zai-dark` 原值；`color-mix` 折算为等效 rgba）。
pub const UI_NIGHT: UiPalette = UiPalette {
    background_alt: "#1a1a1a",
    card_selected: "#2b2b2b",
    popover: "#2b2b2b",
    popover_header: "#202020",
    input: "#2b2b2b",
    input_focused: "#2b2b2b",
    menu: "#2b2b2b",
    menu_hover: "#363636",
    primary: "#ffffff",
    primary_foreground: "#000000",
    secondary: "#363636",
    tab: "#202020",
    tab_active: "#161616",
    user_message_bubble: "rgba(255,255,255,0.08)",
    foreground_subtle: "rgba(212,212,212,0.60)",
    foreground_subtlest: "rgba(212,212,212,0.30)",
    foreground_inverse: "#000000",
    success: "#46bf72",
    success_foreground: "#000000",
    success_surface: "rgba(70,191,114,0.16)",
    success_surface_foreground: "#87d9a4",
    destructive: "#ff5c5c",
    destructive_foreground: "#ffffff",
    destructive_surface: "rgba(255,92,92,0.16)",
    destructive_surface_foreground: "#ff8585",
    toast: "#2b2b2b",
    tooltip: "#2b2b2b",
    tooltip_foreground: "#f8f8f8",
    tag: "#363636",
    interaction_ask_surface: "#001d3d",
    interaction_ask_foreground: "#80beff",
    interaction_confirmation_surface: "rgba(70,191,114,0.16)",
    interaction_confirmation_foreground: "#87d9a4",
};

/// whcode-night 终端调色板（冻结）。
pub const TERMINAL_NIGHT: TerminalPalette = TerminalPalette {
    black: "#363636",
    red: "#ff5c5c",
    green: "#46bf72",
    yellow: "#ff8a30",
    blue: "#4099ff",
    magenta: "#7b5ce5",
    cyan: "#42c8c8",
    white: "#adadad",
};

/// whcode-day 终端调色板（冻结）。
pub const TERMINAL_DAY: TerminalPalette = TerminalPalette {
    black: "#5c5c5c",
    red: "#e03131",
    green: "#1e8a3e",
    yellow: "#e07b00",
    blue: "#0b7fff",
    magenta: "#9e77ed",
    cyan: "#0aa7a7",
    white: "#adadad",
};

/// whcode 系全部主题（契约测试与遍历用；顺序＝`t` 键循环中的出现顺序）。
pub const WHCODE: [ThemeTokens; 2] = [WHCODE_DAY, WHCODE_NIGHT];

/// 既有四套主题的 bg/fg 锚点（`(配置名, 背景, 前景)`；铁律值，
/// 来源 AGENTS.md §6——本模块只引用不重定义，供 Web CSS 契约对照）。
pub const LEGACY_THEMES: [(&str, &str, &str); 4] = [
    ("eink-day", "#ffffff", "#000000"),
    ("eink-night", "#000000", "#ffffff"),
    ("amoled-night", "#000000", "#e6e6e6"),
    ("terminal-green", "#000000", "#00ff00"),
];

/// 把 `#RRGGBB` 解析为 RGB 三元组；非法输入返回 `None`（渲染路径不得 panic）。
#[must_use]
pub fn parse_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let digits = hex.strip_prefix('#')?;
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |range: std::ops::Range<usize>| u8::from_str_radix(&digits[range], 16).ok();
    Some((channel(0..2)?, channel(2..4)?, channel(4..6)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 冻结值锚定：whcode 两套的 bg/fg/brand 与终端调色板逐值断言
    /// （本测试即冻结记录，改动任何值必须同步 ADR-057 修订记录）。
    #[test]
    fn frozen_values_match_approved_proposal() {
        assert_eq!(WHCODE_DAY.name, "whcode-day");
        assert_eq!(WHCODE_DAY.background, "#f8f8f8");
        assert_eq!(WHCODE_DAY.foreground, "#262626");
        assert_eq!(WHCODE_DAY.brand, "#000000");
        assert_eq!(WHCODE_DAY.accent, "#ebf4ff");
        assert_eq!(WHCODE_DAY.panel, "#ffffff");
        assert_eq!(WHCODE_DAY.sidebar, "#f0f0f0");
        let day_terminal = WHCODE_DAY.terminal.expect("whcode-day 须有终端调色板");
        assert_eq!(day_terminal.green, "#1e8a3e");
        assert_eq!(day_terminal.blue, "#0b7fff");

        assert_eq!(WHCODE_NIGHT.name, "whcode-night");
        assert_eq!(WHCODE_NIGHT.background, "#161616");
        assert_eq!(WHCODE_NIGHT.foreground, "#d4d4d4");
        assert_eq!(WHCODE_NIGHT.brand, "#ffffff");
        assert_eq!(WHCODE_NIGHT.accent, "#001d3d");
        assert_eq!(WHCODE_NIGHT.panel, "#202020");
        assert_eq!(WHCODE_NIGHT.card, "#2b2b2b");
        let night_terminal = WHCODE_NIGHT.terminal.expect("whcode-night 须有终端调色板");
        assert_eq!(night_terminal.green, "#46bf72");
        assert_eq!(night_terminal.blue, "#4099ff");
    }

    /// 既有四套主题锚点与 AGENTS.md §6 一致（铁律不被本模块漂移）。
    #[test]
    fn legacy_anchors_match_agents_md() {
        assert_eq!(LEGACY_THEMES[0], ("eink-day", "#ffffff", "#000000"));
        assert_eq!(LEGACY_THEMES[1], ("eink-night", "#000000", "#ffffff"));
        assert_eq!(LEGACY_THEMES[2], ("amoled-night", "#000000", "#e6e6e6"));
        assert_eq!(LEGACY_THEMES[3], ("terminal-green", "#000000", "#00ff00"));
    }

    /// 全部色值（含 rgba 串的 hex 部分）可被 `parse_rgb` 解析或显式 rgba 形态。
    #[test]
    fn hex_tokens_are_parseable() {
        for theme in WHCODE {
            for hex in [
                theme.background,
                theme.foreground,
                theme.brand,
                theme.accent,
                theme.header,
                theme.panel,
                theme.sidebar,
                theme.card,
            ] {
                assert!(
                    parse_rgb(hex).is_some(),
                    "{}: {hex} 须为合法 #RRGGBB",
                    theme.name
                );
            }
            for rgba in [
                theme.border,
                theme.border_hover,
                theme.hover,
                theme.selected,
                theme.surface,
                theme.surface_hover,
            ] {
                assert!(
                    rgba.starts_with("rgba(") && rgba.ends_with(')'),
                    "{}: {rgba} 须为 rgba(...) 形态",
                    theme.name
                );
            }
            if let Some(terminal) = theme.terminal {
                for hex in [
                    terminal.black,
                    terminal.red,
                    terminal.green,
                    terminal.yellow,
                    terminal.blue,
                    terminal.magenta,
                    terminal.cyan,
                    terminal.white,
                ] {
                    assert!(
                        parse_rgb(hex).is_some(),
                        "{}: {hex} 须为合法 #RRGGBB",
                        theme.name
                    );
                }
            }
        }
        for (_name, background, foreground) in LEGACY_THEMES {
            assert!(parse_rgb(background).is_some());
            assert!(parse_rgb(foreground).is_some());
        }
    }

    /// 配置名稳定且唯一（kebab-case；`data-theme`/`ui.theme` 双消费）。
    #[test]
    fn names_are_unique_kebab_case() {
        let mut names: Vec<&str> = WHCODE.iter().map(|t| t.name).collect();
        names.extend(LEGACY_THEMES.iter().map(|(name, _bg, _fg)| *name));
        assert_eq!(names.len(), 6);
        for name in &names {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{name} 须为 kebab-case"
            );
        }
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "主题名不得重复");
    }

    /// `parse_rgb` 行为与 `whcode_tui::parse_rgb` 口径一致（合法/非法样本）。
    #[test]
    fn rgb_parsing_rejects_malformed_input() {
        assert_eq!(parse_rgb("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_rgb("#d4d4d4"), Some((212, 212, 212)));
        for invalid in ["", "#", "000000", "#GGGGGG", "#12345", "#1234567"] {
            assert_eq!(parse_rgb(invalid), None, "{invalid:?} 应判为非法");
        }
    }

    /// ADR-102：扩展语义色板的冻结锚点（改动任何值须同步 ADR-102 修订记录）。
    #[test]
    fn ui_palette_matches_zcode_zai_blocks() {
        assert_eq!(WHCODE_DAY.ui.card_selected, "#ffffff");
        assert_eq!(WHCODE_DAY.ui.menu_hover, "#f0f0f0");
        assert_eq!(WHCODE_DAY.ui.primary, "#000000");
        assert_eq!(WHCODE_DAY.ui.primary_foreground, "#ffffff");
        assert_eq!(WHCODE_DAY.ui.destructive, "#e03131");
        assert_eq!(WHCODE_DAY.ui.success, "#1e8a3e");

        assert_eq!(WHCODE_NIGHT.ui.card_selected, "#2b2b2b");
        assert_eq!(WHCODE_NIGHT.ui.menu_hover, "#363636");
        assert_eq!(WHCODE_NIGHT.ui.primary, "#ffffff");
        assert_eq!(WHCODE_NIGHT.ui.primary_foreground, "#000000");
        assert_eq!(WHCODE_NIGHT.ui.destructive, "#ff5c5c");
        assert_eq!(WHCODE_NIGHT.ui.success, "#46bf72");
        assert_eq!(WHCODE_NIGHT.ui.interaction_ask_surface, "#001d3d");
    }

    /// ADR-102：全部 hex token 可解析；rgba token 形态合法（渲染路径不得 panic）。
    #[test]
    fn ui_palette_values_are_renderable() {
        for theme in WHCODE {
            let palette = theme.ui;
            for hex in [
                palette.background_alt,
                palette.card_selected,
                palette.popover,
                palette.popover_header,
                palette.input,
                palette.input_focused,
                palette.menu,
                palette.menu_hover,
                palette.primary,
                palette.primary_foreground,
                palette.secondary,
                palette.tab,
                palette.tab_active,
                palette.foreground_inverse,
                palette.success,
                palette.success_foreground,
                palette.success_surface_foreground,
                palette.destructive,
                palette.destructive_foreground,
                palette.destructive_surface_foreground,
                palette.toast,
                palette.tooltip,
                palette.tooltip_foreground,
                palette.tag,
                palette.interaction_ask_surface,
                palette.interaction_ask_foreground,
                palette.interaction_confirmation_foreground,
            ] {
                assert!(
                    parse_rgb(hex).is_some(),
                    "{}: {hex} 须为合法 #RRGGBB",
                    theme.name
                );
            }
            for rgba in [
                palette.user_message_bubble,
                palette.foreground_subtle,
                palette.foreground_subtlest,
                palette.destructive_surface,
            ] {
                assert!(
                    rgba.starts_with("rgba(") && rgba.ends_with(')'),
                    "{}: {rgba} 须为 rgba(...) 形态",
                    theme.name
                );
            }
            // ZCode 的 success/confirmation surface：day 用 hex、night 用 rgba，
            // 按原值分别断言，不强制同形态（否则 night 会被 hex 组误判）。
            let is_day = theme.name == "whcode-day";
            assert_eq!(
                palette.success_surface,
                if is_day {
                    "#eaf7ee"
                } else {
                    "rgba(70,191,114,0.16)"
                }
            );
            assert_eq!(
                palette.interaction_confirmation_surface,
                if is_day {
                    "#eaf7ee"
                } else {
                    "rgba(70,191,114,0.16)"
                }
            );
        }
    }

    /// ADR-102：视觉规格常量与 ZCode `DESIGN.md` 一致（双端派生锚点）。
    #[test]
    fn visual_spec_matches_zcode_design_md() {
        use super::spec::*;
        assert_eq!(RADIUS_2XL, 16);
        assert_eq!(RADIUS_XL, 12);
        assert_eq!(RADIUS_LG, 8);
        assert_eq!(RADIUS_MD, 6);
        assert_eq!(RADIUS_SM, 4);
        assert_eq!(UI_FONT_BASE, 14);
        assert_eq!(UI_FONT_XL, 18);
        assert_eq!(UI_FONT_LG, 16);
        assert_eq!(UI_FONT_CAPTION, 13);
        assert_eq!(UI_FONT_SM, 12);
        assert_eq!(UI_FONT_XS, 10);
        assert_eq!(SPACE_UNIT, 4);
        assert_eq!(
            (SPACE_1, SPACE_2, SPACE_3, SPACE_4, SPACE_5),
            (4, 8, 12, 16, 24)
        );
        // ZCode 侧栏最小宽度即默认宽度（264），故取 `<=`。
        assert_eq!((SIDEBAR_MIN, SIDEBAR_WIDTH, SIDEBAR_MAX), (264, 264, 360));
    }
}
