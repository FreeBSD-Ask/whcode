//! # 品牌标识的终端形态（FR-765、ADR-061）
//!
//! 位图主标志（`assets/brand/logo.svg`）与派生图标（GUI 窗口图标、Web favicon）由
//! `.github/scripts/make-brand-assets.py` 从同一母版生成；**终端形态不能由位图机械
//! 降采样得到**——1080×1080 的古籍版面降采样后只剩噪点（ADR-061 已知限制），因此
//! 这里放的是**手工字形表**拼出的 ASCII 字标。
//!
//! 纪律：
//! - 纯 ASCII（可打印字符，无制表符、无宽字符），因此在任何终端、任何等宽字体下宽度
//!   可预测，不需要 `unicode-width` 参与；
//! - 行宽 ≤ [`ASCII_LOGO_WIDTH`]，行数 == [`ASCII_LOGO_LINES`]：TUI 空态据此判断是否
//!   放得下，放不下就退回纯文本空提示（不裁切、不溢出）；
//! - 无颜色、无动画：颜色由调用方的主题样式决定（FR-612/FR-613）。

/// ASCII 字标的最大行宽（字符数，含行尾空白已裁剪后的实际宽度）。
pub const ASCII_LOGO_WIDTH: usize = 47;
/// ASCII 字标的总行数（6 行字形 + 1 空行 + 1 行说明）。
pub const ASCII_LOGO_LINES: usize = 8;

/// ASCII 字标：`WHCODE` 块字形 + 一行说明（语言中立，不参与 i18n）。
pub const ASCII_LOGO: &str = "\
#     # #     #  ######  ###### #####   #######
#  #  # ####### #       #     # #     #
# # # # #     # #       #     # #     # #####
##   ## #     # #       #     # #     # #
##   ## #     # #       #     # #     # #
#     # #     #  ######  ###### #####   #######

         local-first LLM coding harness";

#[cfg(test)]
mod tests {
    use super::*;

    /// 字标是纯 ASCII：任何终端与等宽字体下宽度可预测（不需要宽字符处理）。
    #[test]
    fn ascii_logo_is_plain_ascii() {
        assert!(
            ASCII_LOGO
                .chars()
                .all(|c| c.is_ascii() && !c.is_ascii_control() || c == '\n'),
            "字标只允许可打印 ASCII 与换行"
        );
        assert!(
            !ASCII_LOGO.contains('\t'),
            "字标不得含制表符（宽度随终端漂移）"
        );
    }

    /// 尺寸契约：行数与最大行宽与常量一致，TUI 据此决定是否放得下。
    #[test]
    fn ascii_logo_matches_declared_dimensions() {
        let lines: Vec<&str> = ASCII_LOGO.lines().collect();
        assert_eq!(
            lines.len(),
            ASCII_LOGO_LINES,
            "行数必须等于 ASCII_LOGO_LINES"
        );
        let widest = lines.iter().map(|line| line.chars().count()).max();
        assert_eq!(
            widest,
            Some(ASCII_LOGO_WIDTH),
            "最宽行必须等于 ASCII_LOGO_WIDTH（否则空态放不下时会溢出）"
        );
    }

    /// 字标必须认得出是 WHCode：字形行含 `#` 笔画，说明行含产品定位关键词。
    #[test]
    fn ascii_logo_identifies_the_product() {
        let mut lines = ASCII_LOGO.lines();
        assert!(
            lines.next().is_some_and(|line| line.contains('#')),
            "首行必须是字形笔画"
        );
        assert!(
            ASCII_LOGO.contains("local-first"),
            "字标说明行须写明本地优先定位"
        );
    }
}
