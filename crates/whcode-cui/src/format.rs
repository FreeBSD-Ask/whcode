//! # CUI 记录式排版（T196／ADR-041）
//!
//! 终端族排版：标签列 + 值列的对齐记录、多记录成表、显式空态；仅在交互终端下允许
//! **非颜色**的 ANSI 强调（粗体／弱化）。管道、`NO_COLOR`、`TERM=dumb` 与 `--json`
//! 一律纯 ASCII（无转义符、无制表符线框），保证脚本与日志可重现解析。
//!
//! 边界（ADR-041 决策 3、ADR-032 `CUI_NO_THEME`）：本模块**不输出颜色**，不提供主题
//! 选择；只改字重与排版，不触碰 JSON 信封与退出码。
//!
//! 宽度表与 `whcode-tui` 同名函数刻意重复：前端之间不得互相依赖（ADR-003/027），
//! 两处宽度表只服务各自的排版，合并会引入跨前端依赖。

use std::io::IsTerminal;

/// 标签列宽（显示列；CJK 记 2 列）。
pub const LABEL_WIDTH: usize = 12;

/// 排版风格：是否允许非颜色 ANSI 强调。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Styler {
    emphasis: bool,
}

impl Styler {
    /// 纯文本风格（无任何转义序列）。
    #[must_use]
    pub const fn plain() -> Self {
        Self { emphasis: false }
    }

    /// 按显式开关构造（测试与嵌入方用）。
    #[must_use]
    pub const fn with_emphasis(emphasis: bool) -> Self {
        Self { emphasis }
    }

    /// 从环境检测：stdout 是交互终端、未设置 `NO_COLOR`、`TERM` 不为 `dumb`。
    ///
    /// 检测失败一律按“非交互”处理——宁可少强调，也不把转义序列写进管道。
    #[must_use]
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let dumb_terminal = std::env::var("TERM").is_ok_and(|term| term == "dumb");
        let interactive = std::io::stdout().is_terminal();
        Self::with_emphasis(interactive && !no_color && !dumb_terminal)
    }

    /// 是否启用强调。
    #[must_use]
    pub const fn emphasis(self) -> bool {
        self.emphasis
    }

    /// 粗体（仅强调开启时加转义；文本原样返回）。
    fn bold(self, text: &str) -> String {
        if self.emphasis {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// 对齐记录：可选标题行 + `标签（12 列）+ 值` 的多行文本。
    ///
    /// 空标签表示整行原文（用于分隔或自由文本行）；空值只输出标签，不补齐尾部空格。
    #[must_use]
    pub fn record(self, title: Option<&str>, rows: &[(&str, &str)]) -> String {
        let mut lines: Vec<String> = Vec::with_capacity(rows.len() + usize::from(title.is_some()));
        if let Some(title) = title {
            lines.push(self.bold(title));
        }
        for (label, value) in rows {
            if label.is_empty() {
                lines.push((*value).to_string());
                continue;
            }
            if value.is_empty() {
                lines.push(label.to_string());
                continue;
            }
            lines.push(format!("{}{}", pad_right(label, LABEL_WIDTH), value));
        }
        lines.join("\n")
    }

    /// 对齐表格：表头 + 逐行单元格（列宽取表头与内容的最大显示宽度）。
    ///
    /// 最后一列不补尾部空格（避免日志里出现不可见尾随空白）；表头仅在强调开启时加粗。
    #[must_use]
    pub fn table(self, headers: &[&str], rows: &[Vec<String>]) -> String {
        let columns = headers.len();
        if columns == 0 {
            return String::new();
        }
        let mut widths: Vec<usize> = headers.iter().map(|header| display_width(header)).collect();
        for row in rows {
            for (index, cell) in row.iter().enumerate().take(columns) {
                if let Some(width) = widths.get_mut(index) {
                    *width = (*width).max(display_width(cell));
                }
            }
        }

        let mut lines: Vec<String> = Vec::with_capacity(rows.len() + 1);
        let header_cells: Vec<&str> = headers.to_vec();
        lines.push(self.bold(&join_row(&header_cells, &widths)));
        for row in rows {
            let cells: Vec<&str> = (0..columns)
                .map(|index| row.get(index).map_or("", String::as_str))
                .collect();
            lines.push(join_row(&cells, &widths));
        }
        lines.join("\n")
    }
}

/// 拼接一行单元格：除最后一列外按列宽右补空格，列间两个空格。
fn join_row(cells: &[&str], widths: &[usize]) -> String {
    let mut out = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            out.push_str("  ");
        }
        let is_last = index + 1 == cells.len();
        if is_last {
            out.push_str(cell);
        } else {
            out.push_str(&pad_right(cell, widths.get(index).copied().unwrap_or(0)));
        }
    }
    out
}

/// 字符显示列宽：CJK／全角／常见表情记 2 列，其余 1 列。
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

/// 文本显示列宽。
fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

/// 右补空格到精确显示列宽（超宽不截断：CLI 允许折行，截断会丢信息）。
fn pad_right(text: &str, width: usize) -> String {
    let used = display_width(text);
    let mut out = text.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(used)));
    out
}

#[cfg(test)]
mod tests {
    use super::{LABEL_WIDTH, Styler, display_width};

    /// 记录对齐：CJK 标签按显示列补齐到标签列宽（不是按字节或字符数）。
    #[test]
    fn record_aligns_labels_by_display_width() {
        let text = Styler::plain().record(
            Some("会话 S-1"),
            &[("标题", "实现会话树"), ("状态", "active"), ("", "尾注")],
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "会话 S-1");
        for line in &lines[1..=2] {
            let label_end = line.find(' ').unwrap_or(line.len());
            let label = &line[..label_end];
            assert!(
                display_width(label) < LABEL_WIDTH,
                "值必须从第 {LABEL_WIDTH} 列开始：{line:?}"
            );
        }
        assert_eq!(lines[3], "尾注", "空标签行原样输出");
        assert!(!text.contains('\u{1b}'), "纯文本风格不得含转义序列");
    }

    /// 强调开关：仅强调开启时输出粗体转义，且**从不输出颜色**（SGR 仅 1/2）。
    #[test]
    fn emphasis_emits_only_weight_attributes() {
        let plain = Styler::plain().record(Some("标题"), &[("键", "值")]);
        assert!(!plain.contains('\u{1b}'));

        let emphasis = Styler::with_emphasis(true).record(Some("标题"), &[("键", "值")]);
        assert!(emphasis.contains("\u{1b}[1m"), "强调必须使用粗体");
        assert!(
            !emphasis.contains("\u{1b}[3"),
            "不得输出颜色（30—37/90—97）"
        );
        assert!(
            !emphasis.contains("\u{1b}[4"),
            "不得输出背景色（40—47/100—107）"
        );
        assert!(!emphasis.contains('\r'), "不得输出游标定位序列");
    }

    /// 表格：CJK 单元格按显示列对齐，最后一列不补尾随空格。
    #[test]
    fn table_aligns_cjk_and_trims_last_column() {
        let rows = vec![
            vec![
                "S-1".to_string(),
                "实现会话树".to_string(),
                "active".to_string(),
            ],
            vec!["S-2".to_string(), "短".to_string(), "idle".to_string()],
        ];
        let text = Styler::plain().table(&["ID", "标题", "状态"], &rows);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "表头 + 两行数据");
        let status_column = lines[0].find("状态").expect("表头含状态列");
        for line in &lines[1..] {
            let status = if line.contains("active") {
                "active"
            } else {
                "idle"
            };
            assert!(line.find(status).is_some(), "状态列必须可见：{line:?}");
        }
        // 两行数据的状态列必须起始于同一显示列。
        let start = |line: &str, needle: &str| display_width(&line[..line.find(needle).unwrap()]);
        assert_eq!(
            start(lines[1], "active"),
            start(lines[2], "idle"),
            "状态列必须对齐：{text}"
        );
        assert!(status_column > 0);
        for line in &lines[1..] {
            assert!(!line.ends_with(' '), "最后一列不得补尾随空格：{line:?}");
        }
    }

    /// 空表：只有表头一行（调用方负责替换为显式空态文案）。
    #[test]
    fn empty_table_keeps_header_only() {
        let text = Styler::plain().table(&["ID", "标题"], &[]);
        assert_eq!(text, "ID  标题");
    }
}
