//! 按行重写的终端后端（V-003 实测缺陷修复，2026-10-23）。
//!
//! ## 为什么需要它
//!
//! ratatui 的 crossterm 后端**逐格绝对定位**（每格一次 `MoveTo`），列号由
//! `unicode-width` 计算。Windows 控制台在本机（CP 65001 + 中文字体）把 East Asian
//! **Ambiguous** 宽度字符（`·`、`×`、`—`、`“”` 等）渲染为 **2 格**，而 `unicode-width`
//! 判为 1 格——于是后一格字符被写进该字符的尾格，conhost 把整字擦除，界面出现缺字
//! （实测见 `artifacts/tui-v003-2026-10-23/`：页脚“按键”显示成“键”）。
//!
//! 本后端改为**整行连续输出**：每行只发一次绝对定位 `MoveTo(0, y)`，随后按影子屏内容
//! 顺序写出该行全部字符与样式。常规路径下宽度由终端自己推进，不再依赖本地宽度假设，
//! 因此歧义宽度字符、双宽 CJK 与用户数据中的任意字符都不会错位；行尾空白不补空格，
//! 改用 `EL` 擦到行尾。**溢出行例外**：尾部正文让位、以空格把最右格锚定到最后一列，
//! 且不发 `EL`（详见 `write_row`）。
//!
//! 实测（`artifacts/tui-v003-2026-10-23/width-probe.txt`，CP 65001 + 中文环境）：
//! 光标前进量——ASCII 1 格；CJK 2 格；`·`、`×`、`—`、`“”`、`’`、`°`、`±`、`§`、
//! `●`、`→`、`…` 均为 2 格；制表符（`─│┌┼━`）、块元素（`█▏`）、`✓`、盲文 均为 1 格。
//! 该分布**与 Unicode East Asian Width 不一致**（制表符在 Unicode 里也属 Ambiguous，
//! 却是 1 格），说明它是控制台字体/区域的属性，任何静态表都只能猜。因此本后端**实测**：
//! 在真实终端下（`live`），把未知非 ASCII 字形写到屏幕最后一行、读光标列差得出其在本
//! 终端的前进格数，结果按字形缓存；测试与嵌入方（`with_size`）沿用 ratatui 口径，
//! ASCII 恒按 1 格。
//!
//! 实测值同时用于**溢出行压缩**：ratatui 仍按 1 格排版歧义字符，行在终端里会比影子屏宽；
//! 超出宽度的行保留最右格（通常是右边框）并把它锚定在最后一列，正文从尾部让位，避免
//! 折行污染下一行。
//!
//! ## 代价（正负和解）
//!
//! - 正面：消除整类“宽度假设不一致”缺陷；不引入新依赖；实现只依赖 crossterm 与
//!   ratatui 的公开 API；
//! - 负面：脏行按整行重写，比逐格 diff 多写若干字节（一帧上界 = 行数 × 列数），
//!   且不再享受行内局部更新的带宽优势；
//! - 负面：首次出现的字形要实测（写一个字形 + 读一次光标），有一次性成本；实测缓存有
//!   上界（[`MAX_GLYPHS`]），满了以后新字形回落 ratatui 口径（可能重新出现溢出）；
//! - 负面：溢出行被压缩时会丢失尾部若干格正文（**不**丢最右格）；压缩只是让画面保持
//!   结构完整，被挤掉的列内容不显示；
//! - 回退：旧后端已不再装配，类型仍由 `ratatui-crossterm` 提供；回退需替换 `lib.rs` 里
//!   `Terminal::new` 的构造并补 `CrosstermBackend` 的 import。
//!
//! ## 纪律
//!
//! 影子屏有容量上界（[`MAX_CELLS`]），超限的尺寸被夹到上界，防止异常终端尺寸造成无界内存；
//! 字形宽度缓存有上界（[`MAX_GLYPHS`]）；不使用 `unsafe`；所有权与缓冲全部显式。

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Write};

use crossterm::cursor;
use crossterm::queue;
use crossterm::style::{
    Attribute, Color as CtColor, Print, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{self, Clear, ClearType as CtClearType};
use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};

/// 影子屏的容量上界（格数）。终端报告更大尺寸时按上界夹住并只渲染可见部分。
pub const MAX_CELLS: usize = 500_000;

/// 字形宽度缓存的上界：超出后新字形不再实测，回落 ratatui 口径（防止用户数据撑爆内存）。
pub const MAX_GLYPHS: usize = 1_024;

/// 尺寸探测失败时的兜底（与 `lib.rs` 的 `DEFAULT_SIZE` 语义一致）。
const FALLBACK_SIZE: Size = Size {
    width: 80,
    height: 24,
};

/// 按行重写后端：持有影子屏，脏行整行输出。
pub struct RowBackend<W: Write> {
    writer: W,
    size: Size,
    /// 影子屏：最后一次写出的整屏内容（用于整行重写）。
    rows: Vec<Vec<Cell>>,
    cursor: Position,
    cursor_visible: bool,
    /// 是否跟随真实终端尺寸（`false` = 固定尺寸，供测试与嵌入方显式指定）。
    live: bool,
    /// 字形 → 本终端实测前进格数；未命中时回落 ratatui 口径。
    glyph_widths: HashMap<String, u16>,
}

impl<W: Write> RowBackend<W> {
    /// 以给定写入器创建后端；尺寸取自终端，探测失败时用 80×24。
    pub fn new(writer: W) -> Self {
        let (width, height) =
            terminal::size().unwrap_or((FALLBACK_SIZE.width, FALLBACK_SIZE.height));
        let mut backend = Self::with_size(writer, width, height);
        backend.live = true;
        backend
    }

    /// 以**固定尺寸**创建后端，不探测真实终端（测试与嵌入方使用）。
    #[must_use]
    pub fn with_size(writer: W, width: u16, height: u16) -> Self {
        Self::build(writer, clamp_size(width, height), false)
    }

    fn build(writer: W, size: Size, live: bool) -> Self {
        Self {
            writer,
            size,
            rows: blank_rows(size),
            cursor: Position { x: 0, y: 0 },
            cursor_visible: true,
            live,
            glyph_widths: HashMap::new(),
        }
    }

    /// 标记脏行并整行重写；越界坐标被忽略（不 panic）。
    fn flush_rows(&mut self, mut dirty: BTreeSet<u16>) -> io::Result<()> {
        if dirty.is_empty() {
            return Ok(());
        }
        for row in dirty.iter().copied() {
            self.write_row(row)?;
        }
        dirty.clear();
        Ok(())
    }

    /// 写一整行：一次绝对定位到行首，随后顺序输出字符与样式（行内不再绝对定位）。
    ///
    /// 三个宽度概念必须分开：
    /// - **覆盖跳过**用实测宽度：它决定影子屏里哪些格是双宽字形的尾格，跳过才不会重复输出；
    /// - **行尾空白裁剪**：影子屏把整行填满空格，照原样写出会把歧义字符的膨胀量顶出边界；
    /// - **溢出行压缩**：整行实测宽度超过终端宽度时，保留最右格并把它锚定在最后一列，
    ///   正文从尾部让位——否则终端折行会污染下一行。
    fn write_row(&mut self, row: u16) -> io::Result<()> {
        let row_width = usize::from(self.size.width);
        let mut segments: Vec<(String, Color, Color, Modifier)> = Vec::with_capacity(row_width);
        let mut covered = 0usize;
        for cell in &self.rows[usize::from(row)] {
            if covered > 0 {
                covered -= 1;
                continue;
            }
            let symbol = cell.symbol();
            // 尾格跳过用 ratatui 的布局口径（它决定影子屏里是否留下尾格），与终端实测无关；
            // 实测宽度只用于“整行会占多少列”的核算。
            covered = symbol_width(symbol).saturating_sub(1);
            segments.push((symbol.to_string(), cell.fg, cell.bg, cell.modifier));
        }
        while matches!(segments.last(), Some((symbol, ..)) if symbol == " ") {
            segments.pop();
        }

        let total: usize = segments
            .iter()
            .map(|(symbol, ..)| usize::from(self.console_width(symbol)))
            .sum();
        let mut tail: Option<(String, Color, Color, Modifier)> = None;
        let mut pad = 0usize;
        if total > row_width && segments.len() > 1 {
            let last = segments.pop().expect("行内至少两段");
            let last_width = usize::from(self.console_width(&last.0));
            let mut body = total - last_width;
            while body + last_width > row_width && !segments.is_empty() {
                let dropped = segments.pop().expect("非空");
                body -= usize::from(self.console_width(&dropped.0));
            }
            pad = row_width.saturating_sub(body + last_width);
            tail = Some(last);
        }
        // 写完这一行后光标落在第 `printed` 列（可能正好是行尾）。
        let printed = if tail.is_some() { row_width } else { total };

        queue!(self.writer, cursor::MoveTo(0, row))?;
        self.print_segments(&segments)?;
        if pad > 0 {
            queue!(
                self.writer,
                SetForegroundColor(CtColor::Reset),
                SetBackgroundColor(CtColor::Reset),
                SetAttribute(Attribute::Reset),
                Print(" ".repeat(pad))
            )?;
        }
        if let Some(last) = tail {
            self.print_segments(std::slice::from_ref(&last))?;
        }
        queue!(
            self.writer,
            SetForegroundColor(CtColor::Reset),
            SetBackgroundColor(CtColor::Reset),
            SetAttribute(Attribute::Reset)
        )?;
        // 只有未写满的行才发 EL：光标已在行尾时 EL 会连最后一格一起擦掉（右边框消失）。
        if printed < row_width {
            queue!(self.writer, Clear(CtClearType::UntilNewLine))?;
        }
        Ok(())
    }

    /// 顺序输出若干段，样式变化时才发 SGR。
    fn print_segments(&mut self, segments: &[(String, Color, Color, Modifier)]) -> io::Result<()> {
        let mut last: Option<(Color, Color, Modifier)> = None;
        for (symbol, fg, bg, modifier) in segments {
            let style = (*fg, *bg, *modifier);
            if last != Some(style) {
                if last.is_some_and(|(_, _, modifier)| !modifier.is_empty()) && style.2.is_empty() {
                    queue!(self.writer, SetAttribute(Attribute::Reset))?;
                }
                queue!(
                    self.writer,
                    SetForegroundColor(to_ct_color(style.0)),
                    SetBackgroundColor(to_ct_color(style.1))
                )?;
                for attribute in attributes_of(style.2) {
                    queue!(self.writer, SetAttribute(attribute))?;
                }
                last = Some(style);
            }
            queue!(self.writer, Print(symbol))?;
        }
        Ok(())
    }

    /// 字形在本终端的前进格数：已实测取实测值，否则用 ratatui 口径。
    fn console_width(&self, symbol: &str) -> u16 {
        if symbol.is_ascii() {
            return u16::try_from(symbol.len()).unwrap_or(1);
        }
        self.glyph_widths
            .get(symbol)
            .copied()
            .unwrap_or_else(|| u16::try_from(symbol_width(symbol)).unwrap_or(1))
    }

    /// 收集本轮脏行里出现、尚未实测的非 ASCII 字形；达到 [`MAX_GLYPHS`] 即停止收集。
    fn pending_glyphs(&self, rows: &BTreeSet<u16>) -> Vec<String> {
        let mut pending: Vec<String> = Vec::new();
        'rows: for row in rows {
            for cell in &self.rows[usize::from(*row)] {
                let symbol = cell.symbol();
                if symbol.is_ascii()
                    || self.glyph_widths.contains_key(symbol)
                    || pending.iter().any(|known| known == symbol)
                {
                    continue;
                }
                if self.glyph_widths.len() + pending.len() >= MAX_GLYPHS {
                    break 'rows;
                }
                pending.push(symbol.to_string());
            }
        }
        pending
    }

    /// 实测本轮脏行里出现、尚未实测的非 ASCII 字形。
    ///
    /// 做法：把字形写到屏幕最后一行的连续列位，读光标列差得到前进格数，最后擦掉这一行。
    /// 擦除会破坏该行画面，因此校准结束后**必须**把它并入脏集合重画（`dirty.insert`）。
    /// 只在真实终端（`live`）下进行；测试环境直接用 ratatui 口径。ASCII 恒为 1 格，不实测。
    fn calibrate(&mut self, dirty: &mut BTreeSet<u16>) -> io::Result<()> {
        if !self.live {
            return Ok(());
        }
        let pending = self.pending_glyphs(dirty);
        if pending.is_empty() {
            return Ok(());
        }

        let row = self.size.height.saturating_sub(1);
        let mut column: u16 = 0;
        for symbol in pending {
            let measured = self.measure(symbol.as_str(), row, &mut column)?;
            self.glyph_widths.insert(symbol, measured);
        }
        queue!(
            self.writer,
            cursor::MoveTo(0, row),
            Clear(CtClearType::UntilNewLine)
        )?;
        self.writer.flush()?;
        dirty.insert(row);
        Ok(())
    }

    /// 把单个字形写到 `(column, row)` 并读回前进格数；读不到时回落 ratatui 口径。
    fn measure(&mut self, symbol: &str, row: u16, column: &mut u16) -> io::Result<u16> {
        let estimate = u16::try_from(symbol_width(symbol)).unwrap_or(1);
        // 行尾留 2 格余量，保证 2 格字形也不会折行；放不下就回到行首再测。
        let start = if usize::from(*column) + 3 > usize::from(self.size.width) {
            0
        } else {
            *column
        };
        queue!(self.writer, cursor::MoveTo(start, row), Print(symbol))?;
        self.writer.flush()?;
        let measured = match cursor::position() {
            Ok((x, y)) if y == row => x.saturating_sub(start),
            Ok(_) => 2,
            Err(_) => estimate,
        };
        let measured = if measured == 0 { estimate } else { measured };
        *column = start.saturating_add(measured);
        Ok(measured)
    }

    /// 重新探测尺寸；变化时清屏并重建影子屏（返回是否变化）。
    fn reconcile_size(&mut self) -> io::Result<bool> {
        if !self.live {
            return Ok(false);
        }
        let (width, height) = terminal::size().unwrap_or((self.size.width, self.size.height));
        let size = clamp_size(width, height);
        if size == self.size {
            return Ok(false);
        }
        self.size = size;
        self.rows = blank_rows(size);
        queue!(self.writer, Clear(CtClearType::All))?;
        Ok(true)
    }
}

fn clamp_size(width: u16, height: u16) -> Size {
    let width = width.max(1);
    let height = height.max(1);
    let cells = usize::from(width) * usize::from(height);
    if cells <= MAX_CELLS {
        return Size { width, height };
    }
    let height = u16::try_from(MAX_CELLS / usize::from(width))
        .unwrap_or(1)
        .max(1);
    Size { width, height }
}

fn blank_rows(size: Size) -> Vec<Vec<Cell>> {
    vec![vec![Cell::EMPTY; usize::from(size.width)]; usize::from(size.height)]
}

fn to_ct_color(color: Color) -> CtColor {
    match color {
        Color::Reset => CtColor::Reset,
        Color::Black => CtColor::Black,
        Color::Red => CtColor::DarkRed,
        Color::Green => CtColor::DarkGreen,
        Color::Yellow => CtColor::DarkYellow,
        Color::Blue => CtColor::DarkBlue,
        Color::Magenta => CtColor::DarkMagenta,
        Color::Cyan => CtColor::DarkCyan,
        Color::Gray => CtColor::Grey,
        Color::DarkGray => CtColor::DarkGrey,
        Color::LightRed => CtColor::Red,
        Color::LightGreen => CtColor::Green,
        Color::LightYellow => CtColor::Yellow,
        Color::LightBlue => CtColor::Blue,
        Color::LightMagenta => CtColor::Magenta,
        Color::LightCyan => CtColor::Cyan,
        Color::White => CtColor::White,
        Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        Color::Indexed(index) => CtColor::AnsiValue(index),
    }
}

fn attributes_of(modifier: Modifier) -> Vec<Attribute> {
    let mut attributes = Vec::new();
    for (flag, attribute) in [
        (Modifier::BOLD, Attribute::Bold),
        (Modifier::DIM, Attribute::Dim),
        (Modifier::ITALIC, Attribute::Italic),
        (Modifier::UNDERLINED, Attribute::Underlined),
        (Modifier::SLOW_BLINK, Attribute::SlowBlink),
        (Modifier::RAPID_BLINK, Attribute::RapidBlink),
        (Modifier::REVERSED, Attribute::Reverse),
        (Modifier::HIDDEN, Attribute::Hidden),
        (Modifier::CROSSED_OUT, Attribute::CrossedOut),
    ] {
        if modifier.contains(flag) {
            attributes.push(attribute);
        }
    }
    attributes
}

/// 字形宽度（与 ratatui 布局口径一致：ASCII 直接按字节数，其余交给 `Line::width`）。
fn symbol_width(symbol: &str) -> usize {
    if symbol.is_ascii() {
        return symbol.len();
    }
    ratatui::text::Line::from(symbol).width()
}

impl<W: Write> Backend for RowBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        if self.reconcile_size()? {
            // 尺寸变化后整屏失效：让所有行都重写，避免残留旧画面。
            let all: BTreeSet<u16> = (0..self.size.height).collect();
            self.flush_rows(all)?;
        }
        let mut dirty = BTreeSet::new();
        for (x, y, cell) in content {
            if x >= self.size.width || y >= self.size.height {
                continue;
            }
            self.rows[usize::from(y)][usize::from(x)] = cell.clone();
            dirty.insert(y);
        }
        // 先实测本帧新出现的字形宽度，再按实测值写行；校准会擦掉末行，故由它把该行
        // 放进脏集合，保证本帧末尾重画。
        self.calibrate(&mut dirty)?;
        self.flush_rows(dirty)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        if !self.cursor_visible {
            return Ok(());
        }
        self.cursor_visible = false;
        queue!(self.writer, cursor::Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        if self.cursor_visible {
            return Ok(());
        }
        self.cursor_visible = true;
        queue!(self.writer, cursor::Show)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.cursor = position;
        queue!(self.writer, cursor::MoveTo(position.x, position.y))
    }

    fn clear(&mut self) -> io::Result<()> {
        self.rows = blank_rows(self.size);
        queue!(self.writer, Clear(CtClearType::All))
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.rows = blank_rows(self.size);
        let crossterm_type = match clear_type {
            ClearType::All => CtClearType::All,
            ClearType::AfterCursor => CtClearType::FromCursorDown,
            ClearType::BeforeCursor => CtClearType::FromCursorUp,
            ClearType::CurrentLine => CtClearType::CurrentLine,
            ClearType::UntilNewLine => CtClearType::UntilNewLine,
        };
        queue!(self.writer, Clear(crossterm_type))
    }

    fn size(&self) -> io::Result<Size> {
        // 实时探测：`Terminal` 依靠本方法发现 resize；缓存值只用于影子屏，
        // 在 `draw` 里由 `reconcile_size` 更新，二者不会长期不一致。
        if !self.live {
            return Ok(self.size);
        }
        let (width, height) = terminal::size().unwrap_or((self.size.width, self.size.height));
        Ok(clamp_size(width, height))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size {
                width: 0,
                height: 0,
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_CELLS, MAX_GLYPHS, RowBackend};
    use ratatui::backend::Backend;
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::{Color, Style};
    use std::collections::BTreeSet;

    /// 用固定尺寸构造后端（避免测试依赖真实终端尺寸）。
    fn backend(width: u16, height: u16) -> RowBackend<Vec<u8>> {
        RowBackend::with_size(Vec::new(), width, height)
    }

    /// 按 ratatui 自身的布局口径写入一行（`Buffer::set_string` 会为双宽字符重置尾格），
    /// 与生产路径传入的 `(x, y, &Cell)` 完全一致。
    fn put(backend: &mut RowBackend<Vec<u8>>, row: u16, text: &str, style: Style) {
        let width = backend.size().expect("尺寸").width;
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, row + 1));
        buffer.set_string(0, row, text, style);
        let start = usize::from(row) * usize::from(width);
        let end = start + usize::from(width);
        backend
            .draw(
                buffer.content[start..end]
                    .iter()
                    .enumerate()
                    .map(|(x, cell)| (u16::try_from(x).expect("列号"), row, cell)),
            )
            .expect("draw 应成功");
    }

    /// 去掉 CSI 序列，只留打印出来的字形（用于断言“实际写到屏幕的宽度”）。
    fn printed_text(output: &str) -> String {
        let mut text = String::new();
        let mut chars = output.chars();
        while let Some(ch) = chars.next() {
            if ch != '\u{1b}' {
                text.push(ch);
                continue;
            }
            if chars.next() != Some('[') {
                continue;
            }
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        }
        text
    }

    /// 按“实测宽度”口径计算一段文本占多少格（测试里把 `·` 记为 2 格）。
    fn console_len(text: &str) -> usize {
        text.chars().map(|ch| if ch == '·' { 2 } else { 1 }).sum()
    }

    /// 关键回归：一行里含 `·`（歧义宽度）与 CJK 时，行内不再出现绝对定位，
    /// 整行按原顺序连续写出——终端按自身宽度推进，不会被本地宽度假设打断。
    #[test]
    fn row_is_written_sequentially_with_single_absolute_move() {
        let mut backend = backend(40, 4);
        put(&mut backend, 1, "窗口 · 按键", Style::default());
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        assert!(
            output.contains("窗口 · 按键"),
            "整行文本必须连续出现；实际：{output:?}"
        );
        assert_eq!(
            output.matches("\u{1b}[2;1H").count(),
            1,
            "每行只允许一次绝对定位；实际：{output:?}"
        );
    }

    /// 行尾用 `EL` 擦除，不用空格补齐：补齐按本地宽度估算列数，终端把歧义字符渲染成
    /// 2 格时会越界换行、污染下一行。
    #[test]
    fn row_tail_is_erased_instead_of_padded() {
        let mut backend = backend(10, 2);
        put(&mut backend, 0, "abc", Style::default());
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        assert!(output.contains("\u{1b}[K"), "行尾必须发 EL：{output:?}");
        assert!(
            !output.contains("abc       "),
            "不应用空格填满整行：{output:?}"
        );
    }

    /// 溢出行压缩：实测宽度使整行超出终端时，正文从尾部让位，最右格锚定在最后一列。
    #[test]
    fn overflowing_row_keeps_right_edge_cell() {
        let mut backend = backend(9, 2);
        backend.glyph_widths.insert("·".to_string(), 2);
        put(&mut backend, 0, "abcdefg·h", Style::default());
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        let text = printed_text(&output);
        assert!(text.ends_with('h'), "最右格必须保留：{output:?}");
        assert_eq!(
            console_len(&text),
            9,
            "压缩后整行必须正好等于终端宽度：{output:?}"
        );
    }

    /// 未溢出的行不压缩：所有字形按原顺序原样写出。
    #[test]
    fn fitting_row_is_printed_in_full() {
        let mut backend = backend(12, 2);
        backend.glyph_widths.insert("·".to_string(), 2);
        put(&mut backend, 0, "abcdefg·h", Style::default());
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        assert!(
            printed_text(&output).starts_with("abcdefg·h"),
            "未溢出不应压缩：{output:?}"
        );
    }

    /// 写满整行时不发 `EL`：光标已在行尾，`EL` 会连最后一格（通常是右边框）一起擦掉。
    #[test]
    fn full_row_does_not_emit_erase_line() {
        let mut backend = backend(3, 2);
        put(&mut backend, 0, "abc", Style::default());
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        assert!(
            !output.contains("\u{1b}[K"),
            "整行写满时不应发 EL：{output:?}"
        );
    }

    /// 未实测的非 ASCII 字形按出现顺序收集，ASCII 不收集（恒 1 格）。
    #[test]
    fn pending_glyphs_collects_unmeasured_symbols() {
        let mut backend = backend(8, 2);
        put(&mut backend, 0, "a窗b口", Style::default());
        let pending = backend.pending_glyphs(&BTreeSet::from([0]));
        assert_eq!(pending, vec!["窗".to_string(), "口".to_string()]);
    }

    /// 字形缓存是硬上界：已满时不再收集新字形（否则缓存随用户数据无界增长）。
    #[test]
    fn pending_glyphs_stops_at_glyph_cache_bound() {
        let mut backend = backend(8, 2);
        for index in 0..MAX_GLYPHS {
            backend.glyph_widths.insert(format!("字{index}"), 2);
        }
        put(&mut backend, 0, "窗口", Style::default());
        let pending = backend.pending_glyphs(&BTreeSet::from([0]));
        assert!(pending.is_empty(), "缓存已满时不得再收集：{pending:?}");
    }

    /// 未变化的行不重写（行级 diff 语义）。
    #[test]
    fn unchanged_rows_are_not_rewritten() {
        let mut backend = backend(20, 3);
        put(&mut backend, 0, "第一行", Style::default());
        backend.writer.clear();
        put(&mut backend, 2, "第三行", Style::default());
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        assert!(output.contains("\u{1b}[3;1H"), "变化的行应重写：{output:?}");
        assert!(
            !output.contains("\u{1b}[1;1H"),
            "未变化的第 1 行不应重写：{output:?}"
        );
    }

    /// 主题色经行内 SGR 输出（前景/背景），且同一行只在首格设置一次。
    #[test]
    fn style_is_emitted_once_per_run() {
        let mut backend = backend(20, 2);
        let style = Style::default()
            .fg(Color::Rgb(0, 0, 0))
            .bg(Color::Rgb(255, 255, 255));
        put(&mut backend, 0, "abc", style);
        let output = String::from_utf8(backend.writer.clone()).expect("UTF-8");
        assert_eq!(output.matches("\u{1b}[38;2;0;0;0m").count(), 1);
        assert_eq!(output.matches("\u{1b}[48;2;255;255;255m").count(), 1);
    }

    /// 越界坐标被忽略而不 panic（异常尺寸下的防御）。
    #[test]
    fn out_of_bounds_cells_are_ignored() {
        let mut backend = backend(5, 2);
        let cell = Cell::new("X");
        backend
            .draw(std::iter::once((9u16, 9u16, &cell)))
            .expect("draw 应成功");
        assert!(backend.writer.is_empty(), "越界内容不应写出");
    }

    /// 尺寸夹取：异常大的终端尺寸不得造成无界内存。
    #[test]
    fn oversized_terminal_is_clamped() {
        let size = super::clamp_size(10_000, 10_000);
        assert!(
            usize::from(size.width) * usize::from(size.height) <= MAX_CELLS,
            "夹取后格数必须不超过上界"
        );
    }

    /// 光标位置由后端自持（避免每次查询都 ioctl）。
    #[test]
    fn cursor_position_round_trips() {
        let mut backend = backend(10, 3);
        backend
            .set_cursor_position(Position { x: 2, y: 1 })
            .expect("设置光标");
        assert_eq!(
            backend.get_cursor_position().expect("读取光标"),
            Position { x: 2, y: 1 }
        );
    }
}
