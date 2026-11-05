//! # 视觉 token 单一事实源（FR-739／FR-740，ADR-057 决策 2/3）
//!
//! 本模块是主题色值的**唯一定义处**：`zcode-day`／`zcode-night` 两套主题的全部
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
//! 对照 Web CSS，保证六套主题的 `[data-theme]` 块齐全（FR-739：zcode 系为新增
//! 而非覆盖）。扩展 token（brand/accent/panel/…/terminal 调色板）只有 zcode 系
//! 定义；既有四套保持 FR-713 口径（不引入主题外颜色），B1 对话渲染使用扩展
//! token 时仅对 zcode 系生效。

/// zcode 系主题的完整视觉 token（冻结值，见模块文档）。
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

/// zcode-day（← ZCode `zai-light`，浅色）。
pub const ZCODE_DAY: ThemeTokens = ThemeTokens {
    name: "zcode-day",
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
};

/// zcode-night（← ZCode `zai-dark`，深色；2026-11-06 用户批准为默认主题）。
pub const ZCODE_NIGHT: ThemeTokens = ThemeTokens {
    name: "zcode-night",
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
};

/// zcode-night 终端调色板（冻结）。
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

/// zcode-day 终端调色板（冻结）。
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

/// zcode 系全部主题（契约测试与遍历用；顺序＝`t` 键循环中的出现顺序）。
pub const ZCODE: [ThemeTokens; 2] = [ZCODE_DAY, ZCODE_NIGHT];

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

    /// 冻结值锚定：zcode 两套的 bg/fg/brand 与终端调色板逐值断言
    /// （本测试即冻结记录，改动任何值必须同步 ADR-057 修订记录）。
    #[test]
    fn frozen_values_match_approved_proposal() {
        assert_eq!(ZCODE_DAY.name, "zcode-day");
        assert_eq!(ZCODE_DAY.background, "#f8f8f8");
        assert_eq!(ZCODE_DAY.foreground, "#262626");
        assert_eq!(ZCODE_DAY.brand, "#000000");
        assert_eq!(ZCODE_DAY.accent, "#ebf4ff");
        assert_eq!(ZCODE_DAY.panel, "#ffffff");
        assert_eq!(ZCODE_DAY.sidebar, "#f0f0f0");
        let day_terminal = ZCODE_DAY.terminal.expect("zcode-day 须有终端调色板");
        assert_eq!(day_terminal.green, "#1e8a3e");
        assert_eq!(day_terminal.blue, "#0b7fff");

        assert_eq!(ZCODE_NIGHT.name, "zcode-night");
        assert_eq!(ZCODE_NIGHT.background, "#161616");
        assert_eq!(ZCODE_NIGHT.foreground, "#d4d4d4");
        assert_eq!(ZCODE_NIGHT.brand, "#ffffff");
        assert_eq!(ZCODE_NIGHT.accent, "#001d3d");
        assert_eq!(ZCODE_NIGHT.panel, "#202020");
        assert_eq!(ZCODE_NIGHT.card, "#2b2b2b");
        let night_terminal = ZCODE_NIGHT.terminal.expect("zcode-night 须有终端调色板");
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
        for theme in ZCODE {
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
        let mut names: Vec<&str> = ZCODE.iter().map(|t| t.name).collect();
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
}
