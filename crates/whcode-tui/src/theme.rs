//! # 渲染主题（T128、FR-612；FR-739 zcode 系新增）
//!
//! 六套主题。既有四套为铁律精确色值（OLED 仅纯黑关断像素；e-ink 黑→白转换残影最重）：
//!
//! | 主题 | 背景 | 前景 |
//! |---|---|---|
//! | [`AMOLED_NIGHT`] | `#000000`（纯黑） | `#E6E6E6` |
//! | [`EINK_DAY`] | `#FFFFFF`（纯白） | `#000000` |
//! | [`EINK_NIGHT`] | `#000000`（纯黑） | `#FFFFFF` |
//! | [`TERMINAL_GREEN`] | `#000000`（纯黑） | `#00FF00`（2026-10-24 用户新增，CRT 荧光风格） |
//!
//! 2026-11-06 用户批准新增 zcode 系（ADR-057 决策 2，色值冻结于
//! `whcode_domain::theme_tokens`——本模块 bg/fg 直接引用该单一事实源，不复制值）：
//!
//! | 主题 | 背景 | 前景 |
//! |---|---|---|
//! | [`Theme::ZcodeDay`] | `#f8f8f8` | `#262626` |
//! | [`Theme::ZcodeNight`] | `#161616` | `#d4d4d4`（**默认主题**，用户批准切换） |
//!
//! zcode 系无墨水屏/OLED 纯黑承诺（背景允许非纯黑/白）；既有四套铁律不受影响。
//! 主题经配置选择（可见、可改、可撤销）；测试断言既有四套背景精确为纯黑/纯白。
//!
//! 边界：色值为常量声明（渲染应用属 `render.rs`，T083/T129）；终端绘制由终端
//! 模拟器承担，WHCode 不直接调度 GPU（FR-613 前端部分）。

use serde::{Deserialize, Serialize};
use whcode_domain::theme_tokens;

/// 背景纯黑（OLED 关断像素）。
pub const BLACK: &str = "#000000";

/// 背景纯白（e-ink 日间）。
pub const WHITE: &str = "#FFFFFF";

/// AMOLED 夜间：纯黑背景 + `#E6E6E6` 前景。
pub const AMOLED_NIGHT: (&str, &str) = (BLACK, "#E6E6E6");

/// 墨水屏日间：纯白背景 + 黑字。
pub const EINK_DAY: (&str, &str) = (WHITE, BLACK);

/// 墨水屏夜间：纯黑背景 + 白字。
pub const EINK_NIGHT: (&str, &str) = (BLACK, WHITE);

/// 终端绿夜间：纯黑背景（OLED 兼容）+ 荧光绿字（CRT P1 风格）。
pub const TERMINAL_GREEN: (&str, &str) = (BLACK, "#00FF00");

/// 渲染主题（配置选择；序列化用稳定 kebab-case 名，与 [`Theme::as_str`] 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// AMOLED 夜间。
    AmoledNight,
    /// 墨水屏日间（2026-11-06 前的默认；现默认为 zcode-night，用户批准，ADR-057）。
    EinkDay,
    /// 墨水屏夜间。
    EinkNight,
    /// 终端绿夜间（2026-10-24 用户新增；FR-612 三套为下限，GUI 侧 FR-713 同步四套）。
    TerminalGreen,
    /// zcode 日间（FR-739，2026-11-06 用户新增并批准冻结；色值源
    /// `whcode_domain::theme_tokens`）。
    ZcodeDay,
    /// zcode 夜间（FR-739；**默认主题**，2026-11-06 用户批准切换）。
    #[default]
    ZcodeNight,
}

impl Theme {
    /// `t` 键循环顺序（T178.1/FR-625）：六套依配置体系顺序循环，六步回原位。
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::EinkDay => Self::EinkNight,
            Self::EinkNight => Self::AmoledNight,
            Self::AmoledNight => Self::TerminalGreen,
            Self::TerminalGreen => Self::ZcodeDay,
            Self::ZcodeDay => Self::ZcodeNight,
            Self::ZcodeNight => Self::EinkDay,
        }
    }

    /// 主题的精确色值（背景，前景）。zcode 系引用
    /// `whcode_domain::theme_tokens` 冻结值（单一事实源，FR-740）。
    #[must_use]
    pub const fn colors(self) -> (&'static str, &'static str) {
        match self {
            Self::AmoledNight => AMOLED_NIGHT,
            Self::EinkDay => EINK_DAY,
            Self::EinkNight => EINK_NIGHT,
            Self::TerminalGreen => TERMINAL_GREEN,
            Self::ZcodeDay => (
                theme_tokens::ZCODE_DAY.background,
                theme_tokens::ZCODE_DAY.foreground,
            ),
            Self::ZcodeNight => (
                theme_tokens::ZCODE_NIGHT.background,
                theme_tokens::ZCODE_NIGHT.foreground,
            ),
        }
    }

    /// 稳定标识（配置选择与序列化）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmoledNight => "amoled-night",
            Self::EinkDay => "eink-day",
            Self::EinkNight => "eink-night",
            Self::TerminalGreen => "terminal-green",
            Self::ZcodeDay => "zcode-day",
            Self::ZcodeNight => "zcode-night",
        }
    }
}

impl std::str::FromStr for Theme {
    type Err = whcode_domain::WhError;

    /// 从稳定标识解析（配置与 `WHCODE_UI_THEME` 环境变量覆盖用）。
    ///
    /// # Errors
    ///
    /// 未知主题名返回 `parse_error`，消息列出全部合法取值（错误可操作、不责怪用户）。
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name {
            "amoled-night" => Ok(Self::AmoledNight),
            "eink-day" => Ok(Self::EinkDay),
            "eink-night" => Ok(Self::EinkNight),
            "terminal-green" => Ok(Self::TerminalGreen),
            "zcode-day" => Ok(Self::ZcodeDay),
            "zcode-night" => Ok(Self::ZcodeNight),
            other => Err(whcode_domain::WhError::parse_error(format!(
                "未知主题 {other:?}：合法取值为 amoled-night / eink-day / eink-night / \
                 terminal-green / zcode-day / zcode-night"
            ))),
        }
    }
}

/// 把 `#RRGGBB` 解析为 RGB 三元组；非法输入返回 `None`（帧路径不得 panic）。
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

    #[test]
    fn background_colors_are_exactly_pure_black_or_pure_white() {
        // 既有四套为铁律（OLED/墨水屏口径）；zcode 系无此承诺，见下一测试。
        for theme in [
            Theme::AmoledNight,
            Theme::EinkDay,
            Theme::EinkNight,
            Theme::TerminalGreen,
        ] {
            let (background, _foreground) = theme.colors();
            assert!(
                background == BLACK || background == WHITE,
                "主题 {theme:?} 背景必须精确为纯黑/纯白（FR-612），实际 {background}"
            );
        }
        assert_eq!(Theme::AmoledNight.colors(), ("#000000", "#E6E6E6"));
        assert_eq!(Theme::EinkDay.colors(), ("#FFFFFF", "#000000"));
        assert_eq!(Theme::EinkNight.colors(), ("#000000", "#FFFFFF"));
    }

    /// zcode 系精确引用 `theme_tokens` 冻结值（FR-740：本 crate 不复制色值）。
    #[test]
    fn zcode_colors_come_from_domain_single_source() {
        assert_eq!(Theme::ZcodeDay.colors(), ("#f8f8f8", "#262626"));
        assert_eq!(Theme::ZcodeNight.colors(), ("#161616", "#d4d4d4"));
        assert_eq!(
            Theme::ZcodeNight.colors(),
            (
                theme_tokens::ZCODE_NIGHT.background,
                theme_tokens::ZCODE_NIGHT.foreground
            )
        );
    }

    #[test]
    fn theme_names_are_stable_for_configuration() {
        assert_eq!(Theme::AmoledNight.as_str(), "amoled-night");
        assert_eq!(Theme::EinkDay.as_str(), "eink-day");
        assert_eq!(Theme::EinkNight.as_str(), "eink-night");
        assert_eq!(Theme::TerminalGreen.as_str(), "terminal-green");
        assert_eq!(Theme::TerminalGreen.colors(), ("#000000", "#00FF00"));
        assert_eq!(Theme::ZcodeDay.as_str(), "zcode-day");
        assert_eq!(Theme::ZcodeNight.as_str(), "zcode-night");
    }

    /// 默认主题为 `zcode-night`（2026-11-06 用户批准切换，ADR-057/FR-739）。
    #[test]
    fn default_theme_is_zcode_night() {
        assert_eq!(Theme::default(), Theme::ZcodeNight);
        assert_eq!(
            Theme::default().colors(),
            (
                theme_tokens::ZCODE_NIGHT.background,
                theme_tokens::ZCODE_NIGHT.foreground
            )
        );
    }

    /// 配置解析：稳定名双向一致；未知名返回可操作的 parse_error。
    #[test]
    fn theme_names_roundtrip_and_reject_unknown() {
        for theme in [
            Theme::AmoledNight,
            Theme::EinkDay,
            Theme::EinkNight,
            Theme::TerminalGreen,
            Theme::ZcodeDay,
            Theme::ZcodeNight,
        ] {
            assert_eq!(theme.as_str().parse::<Theme>(), Ok(theme));
        }
        let err = "midnight".parse::<Theme>().expect_err("未知主题必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(
            err.message.contains("amoled-night") && err.message.contains("zcode-night"),
            "错误消息须列出全部合法取值"
        );
    }

    /// 色值解析：三个主题的精确色值均可解析；非法输入不 panic 且返回 `None`。
    #[test]
    fn rgb_parsing_covers_exact_theme_colors() {
        assert_eq!(parse_rgb("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_rgb("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(parse_rgb("#E6E6E6"), Some((230, 230, 230)));
        for theme in [
            Theme::AmoledNight,
            Theme::EinkDay,
            Theme::EinkNight,
            Theme::TerminalGreen,
        ] {
            let (background, foreground) = theme.colors();
            assert!(parse_rgb(background).is_some(), "{background} 必须可解析");
            assert!(parse_rgb(foreground).is_some(), "{foreground} 必须可解析");
        }
        for invalid in ["", "#", "000000", "#GGGGGG", "#12345", "#1234567"] {
            assert_eq!(parse_rgb(invalid), None, "{invalid:?} 应判为非法");
        }
    }
}
