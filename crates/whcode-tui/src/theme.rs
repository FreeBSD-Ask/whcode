//! # 渲染主题（T128、FR-612）
//!
//! 三套主题（FR-612，精确色值依据：OLED 仅纯黑关断像素；e-ink 黑→白转换残影最重）：
//!
//! | 主题 | 背景 | 前景 |
//! |---|---|---|
//! | [`AMOLED_NIGHT`] | `#000000`（纯黑） | `#E6E6E6` |
//! | [`EINK_DAY`] | `#FFFFFF`（纯白） | `#000000` |
//! | [`EINK_NIGHT`] | `#000000`（纯黑） | `#FFFFFF` |
//!
//! 主题经配置选择（可见、可改、可撤销）；测试断言背景色精确为纯黑/纯白
//! （墨水屏避免大面积中间灰与渐变，ui-design 纪律）。
//!
//! 边界：色值为常量声明（渲染应用属 `render.rs`，T083/T129）；终端绘制由终端
//! 模拟器承担，WHCode 不直接调度 GPU（FR-613 前端部分）。

use serde::{Deserialize, Serialize};

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

/// 渲染主题（配置选择；序列化用稳定 kebab-case 名，与 [`Theme::as_str`] 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// AMOLED 夜间。
    AmoledNight,
    /// 墨水屏日间（默认：最保守、最省电，ui-design §主题）。
    #[default]
    EinkDay,
    /// 墨水屏夜间。
    EinkNight,
}

impl Theme {
    /// 主题的精确色值（背景，前景）。
    #[must_use]
    pub const fn colors(self) -> (&'static str, &'static str) {
        match self {
            Self::AmoledNight => AMOLED_NIGHT,
            Self::EinkDay => EINK_DAY,
            Self::EinkNight => EINK_NIGHT,
        }
    }

    /// 稳定标识（配置选择与序列化）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmoledNight => "amoled-night",
            Self::EinkDay => "eink-day",
            Self::EinkNight => "eink-night",
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
            other => Err(whcode_domain::WhError::parse_error(format!(
                "未知主题 {other:?}：合法取值为 amoled-night / eink-day / eink-night"
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
        for theme in [Theme::AmoledNight, Theme::EinkDay, Theme::EinkNight] {
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

    #[test]
    fn theme_names_are_stable_for_configuration() {
        assert_eq!(Theme::AmoledNight.as_str(), "amoled-night");
        assert_eq!(Theme::EinkDay.as_str(), "eink-day");
        assert_eq!(Theme::EinkNight.as_str(), "eink-night");
    }

    /// 默认主题为 `eink-day`（ui-design §主题：最保守、最省电）。
    #[test]
    fn default_theme_is_eink_day() {
        assert_eq!(Theme::default(), Theme::EinkDay);
        assert_eq!(Theme::default().colors(), EINK_DAY);
    }

    /// 配置解析：稳定名双向一致；未知名返回可操作的 parse_error。
    #[test]
    fn theme_names_roundtrip_and_reject_unknown() {
        for theme in [Theme::AmoledNight, Theme::EinkDay, Theme::EinkNight] {
            assert_eq!(theme.as_str().parse::<Theme>(), Ok(theme));
        }
        let err = "midnight".parse::<Theme>().expect_err("未知主题必须失败");
        assert_eq!(err.code, whcode_domain::ErrorCode::ParseError);
        assert!(
            err.message.contains("amoled-night"),
            "错误消息须列出合法取值"
        );
    }

    /// 色值解析：三个主题的精确色值均可解析；非法输入不 panic 且返回 `None`。
    #[test]
    fn rgb_parsing_covers_exact_theme_colors() {
        assert_eq!(parse_rgb("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_rgb("#FFFFFF"), Some((255, 255, 255)));
        assert_eq!(parse_rgb("#E6E6E6"), Some((230, 230, 230)));
        for theme in [Theme::AmoledNight, Theme::EinkDay, Theme::EinkNight] {
            let (background, foreground) = theme.colors();
            assert!(parse_rgb(background).is_some(), "{background} 必须可解析");
            assert!(parse_rgb(foreground).is_some(), "{foreground} 必须可解析");
        }
        for invalid in ["", "#", "000000", "#GGGGGG", "#12345", "#1234567"] {
            assert_eq!(parse_rgb(invalid), None, "{invalid:?} 应判为非法");
        }
    }
}
