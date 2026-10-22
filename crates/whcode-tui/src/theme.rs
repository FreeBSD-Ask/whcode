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

use serde::Serialize;

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

/// 渲染主题（配置选择）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Theme {
    /// AMOLED 夜间。
    AmoledNight,
    /// 墨水屏日间。
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

    /// 稳定标识（配置选择）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmoledNight => "amoled-night",
            Self::EinkDay => "eink-day",
            Self::EinkNight => "eink-night",
        }
    }
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
}
