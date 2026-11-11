//! 渲染状态：从终端拷一份视口，给画屏幕的人一行一行、一格一格地读。
//!
//! 照 mygo plugins/terminal/internal/vt/render.go 移植（MIT，Copyright (c) 2026 MyGo contributors），
//! 再加上给 GUI 用的一层：颜色解析好（调色板、默认色、反色），文字放进复用的缓冲区，
//! 画一帧不用每格分配。
//!
//! 用法（每帧）：
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! use lolidesk_ghostty::{RenderState, Terminal, Width};
//! let term = Terminal::new(80, 24, Box::new(()))?;
//! let mut rs = RenderState::new()?; // 留着复用
//! rs.update(&term)?;
//! let bg = rs.colors().background;
//! let mut rows = rs.rows();
//! while let Some(mut row) = rows.next_row() {
//!     if !row.dirty() {
//!         continue; // 上一帧画的还能用（按 row.id() 缓存）
//!     }
//!     let y = row.y();
//!     for (cell, text) in row.cells().iter() {
//!         if cell.width == Width::SpacerTail || cell.width == Width::SpacerHead {
//!             continue;
//!         }
//!         let _ = (cell.x, y, cell.fg, cell.bg.unwrap_or(bg), text); // 画
//!     }
//! }
//! rs.clean();
//! # Ok(()) }
//! ```

use std::{mem::size_of, ptr};

use anyhow::Result;

use crate::{
    ffi::{self, api, Handle, Rgb, RowId, Scrollbar},
    terminal::Terminal,
};

/// 光标的样子。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CursorStyle {
    Bar,
    #[default]
    Block,
    Underline,
    BlockHollow,
}

impl CursorStyle {
    pub(crate) fn to_ffi(self) -> i32 {
        match self {
            CursorStyle::Bar => ffi::cursor_style::BAR,
            CursorStyle::Block => ffi::cursor_style::BLOCK,
            CursorStyle::Underline => ffi::cursor_style::UNDERLINE,
            CursorStyle::BlockHollow => ffi::cursor_style::BLOCK_HOLLOW,
        }
    }

    fn from_visual(v: i32) -> CursorStyle {
        match v {
            ffi::visual_style::BAR => CursorStyle::Bar,
            ffi::visual_style::UNDERLINE => CursorStyle::Underline,
            ffi::visual_style::BLOCK_HOLLOW => CursorStyle::BlockHollow,
            _ => CursorStyle::Block,
        }
    }
}

/// 一帧的光标。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// 在视口里的第几列、第几行；滚到回滚里看不见时是 None。
    pub viewport: Option<(u16, u16)>,
    /// 在宽字符的后半格上。
    pub wide_tail: bool,
    /// 程序让它显示（DECTCEM）。
    pub visible: bool,
    pub blinking: bool,
    /// 程序在读密码。
    pub password: bool,
    pub style: CursorStyle,
}

/// 一帧的颜色：程序设的，没设就是默认的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    /// 光标颜色；None 用文字的。
    pub cursor: Option<Rgb>,
    pub palette: [Rgb; 256],
}

impl Default for Colors {
    fn default() -> Colors {
        Colors {
            foreground: Rgb::default(),
            background: Rgb::default(),
            cursor: None,
            palette: [Rgb::default(); 256],
        }
    }
}

/// 自上次 clean 以来变了多少。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dirty {
    /// 没变，不用画。
    Clean,
    /// 有些行变了（看 [`Row::dirty`]）。
    Partial,
    /// 全变了，整个重画。
    Full,
}

/// 格子多宽。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Width {
    #[default]
    Narrow,
    /// 宽字符，占这格和下一格（下一格是 SpacerTail）。
    Wide,
    /// 宽字符的后半，跳过不画。
    SpacerTail,
    /// 软换行的行尾：放不下的宽字符挪到了下一行，跳过不画。
    SpacerHead,
}

impl Width {
    fn from_raw(v: u8) -> Width {
        match v as i32 {
            ffi::wide::WIDE => Width::Wide,
            ffi::wide::SPACER_TAIL => Width::SpacerTail,
            ffi::wide::SPACER_HEAD => Width::SpacerHead,
            _ => Width::Narrow,
        }
    }
}

/// 下划线的样子。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

impl Underline {
    fn from_ffi(v: i32) -> Underline {
        match v {
            ffi::underline::SINGLE => Underline::Single,
            ffi::underline::DOUBLE => Underline::Double,
            ffi::underline::CURLY => Underline::Curly,
            ffi::underline::DOTTED => Underline::Dotted,
            ffi::underline::DASHED => Underline::Dashed,
            _ => Underline::None,
        }
    }
}

/// 一格，颜色解析好了：调色板和默认色套上，反色（SGR 7）也换好了（`inverse` 还留着说一声）。
/// 文字在 [`Cells::text`] 里。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub x: u16,
    pub width: Width,
    pub fg: Rgb,
    /// 背景；None 是终端的默认背景，不用画。
    pub bg: Option<Rgb>,
    pub underline: Underline,
    /// 下划线颜色；None 用文字的。
    pub underline_color: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    /// 有超链接（OSC 8），地址用 [`Terminal::hyperlink_at`] 取。
    pub hyperlink: bool,
    text: (u32, u32),
}

/// 一格的原始值，从压缩的 GhosttyCell 拆出来（Mirror 和画格子都用）。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RawCell {
    /// 字符，0 是没有；grapheme 说后面还有组合的码点。
    pub codepoint: u32,
    pub grapheme: bool,
    /// 样式在这一行里的编号，0 是默认样式。
    pub style: u16,
    pub wide: u8,
    pub hyperlink: bool,
    /// 没有字、程序填了背景的格子：（是不是调色板色，颜色；调色板色的序号在 r）。
    pub bg: Option<(bool, Rgb)>,
}

impl RawCell {
    pub fn width(&self) -> Width {
        Width::from_raw(self.wide)
    }
}

/// 样式的颜色：没有、调色板第几色、RGB。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StyleColor {
    #[default]
    None,
    Palette(u8),
    Rgb(Rgb),
}

impl StyleColor {
    fn from_ffi(c: ffi::StyleColor) -> StyleColor {
        let b = c.bytes();
        match c.tag {
            ffi::color_tag::PALETTE => StyleColor::Palette(b[0]),
            ffi::color_tag::RGB => StyleColor::Rgb(Rgb::new(b[0], b[1], b[2])),
            _ => StyleColor::None,
        }
    }

    fn resolve(self, palette: &[Rgb; 256]) -> Option<Rgb> {
        match self {
            StyleColor::None => None,
            StyleColor::Palette(i) => Some(palette[i as usize]),
            StyleColor::Rgb(c) => Some(c),
        }
    }
}

/// 一格的样式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CellStyle {
    pub fg: StyleColor,
    pub bg: StyleColor,
    pub underline_color: StyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: i32,
}

impl CellStyle {
    fn from_ffi(s: &ffi::Style) -> CellStyle {
        CellStyle {
            fg: StyleColor::from_ffi(s.fg_color),
            bg: StyleColor::from_ffi(s.bg_color),
            underline_color: StyleColor::from_ffi(s.underline_color),
            bold: s.bold,
            italic: s.italic,
            faint: s.faint,
            blink: s.blink,
            inverse: s.inverse,
            invisible: s.invisible,
            strikethrough: s.strikethrough,
            overline: s.overline,
            underline: s.underline,
        }
    }

    /// 从没有样式设成这个样式的 SGR（同 mygo 的 Style.SGR）。
    pub fn sgr(&self) -> String {
        let underline = match self.underline {
            ffi::underline::NONE => None,
            ffi::underline::SINGLE => Some("4".to_string()),
            n => Some(format!("4:{n}")),
        };
        let mut p: Vec<String> = vec!["0".into()];
        for (on, code) in [(self.bold, "1"), (self.faint, "2"), (self.italic, "3")] {
            if on {
                p.push(code.into());
            }
        }
        p.extend(underline);
        for (on, code) in [
            (self.blink, "5"),
            (self.inverse, "7"),
            (self.invisible, "8"),
            (self.strikethrough, "9"),
            (self.overline, "53"),
        ] {
            if on {
                p.push(code.into());
            }
        }
        for (c, base) in [
            (self.fg, "38"),
            (self.bg, "48"),
            (self.underline_color, "58"),
        ] {
            match c {
                StyleColor::None => {}
                StyleColor::Palette(i) => p.push(format!("{base}:5:{i}")),
                StyleColor::Rgb(c) => p.push(format!("{base}:2::{}:{}:{}", c.r, c.g, c.b)),
            }
        }
        format!("\x1b[{}m", p.join(";"))
    }
}

/// 渲染状态：update 从终端拷视口，之后读它不用碰终端。
pub struct RenderState {
    h: Handle,
    rows_it: Handle,
    cells_h: Handle,
    /// 正读的那行的原始格子。
    raw: Vec<RawCell>,
    /// 这一行见过的样式（编号，样式）。
    styles: Vec<(u16, CellStyle)>,
    cps: Vec<u32>,
    out: Vec<Cell>,
    text: String,
    colors: Colors,
    cursor: Cursor,
    scrollbar: Scrollbar,
}

// SAFETY：句柄只经 &self / &mut self 用，不是 Sync；库不把渲染状态绑在线程上。
unsafe impl Send for RenderState {}

impl RenderState {
    /// 新建一个空的渲染状态（会先载入库）。
    pub fn new() -> Result<RenderState> {
        let api = &ffi::lib_checked()?.api;
        let mut rs = RenderState {
            h: ptr::null_mut(),
            rows_it: ptr::null_mut(),
            cells_h: ptr::null_mut(),
            raw: Vec::new(),
            styles: Vec::new(),
            cps: Vec::new(),
            out: Vec::new(),
            text: String::new(),
            colors: Colors::default(),
            cursor: Cursor::default(),
            scrollbar: Scrollbar::default(),
        };
        // SAFETY：出参；失败时 Drop 只释放建好了的。
        unsafe {
            ffi::check(
                (api.render_state_new)(ptr::null(), &mut rs.h),
                "新建渲染状态",
            )?;
            ffi::check(
                (api.render_state_row_iterator_new)(ptr::null(), &mut rs.rows_it),
                "新建行迭代器",
            )?;
            ffi::check(
                (api.render_state_row_cells_new)(ptr::null(), &mut rs.cells_h),
                "新建格子迭代器",
            )?;
        }
        Ok(rs)
    }

    /// 从终端拷一份视口（带脏标记）；同时记下颜色、光标和滚动条。
    /// 拷完之前读到的行、格子都作废。
    pub fn update(&mut self, t: &Terminal) -> Result<()> {
        let api = api();
        // SAFETY：两个句柄都有效；之前借出的行和格子不再用（&mut self）。
        ffi::check(
            unsafe { (api.render_state_update)(self.h, t.handle()) },
            "更新渲染状态",
        )?;
        self.scrollbar = t.scrollbar();
        let mut c = ffi::RenderColors {
            size: size_of::<ffi::RenderColors>(),
            background: Rgb::default(),
            foreground: Rgb::default(),
            cursor: Rgb::default(),
            cursor_has_value: false,
            palette: [Rgb::default(); 256],
        };
        self.get(ffi::rs_data::COLORS, &mut c);
        self.colors = Colors {
            foreground: c.foreground,
            background: c.background,
            cursor: c.cursor_has_value.then_some(c.cursor),
            palette: c.palette,
        };
        let mut c = ffi::RenderCursor {
            size: size_of::<ffi::RenderCursor>(),
            ..Default::default()
        };
        self.get(ffi::rs_data::CURSOR, &mut c);
        self.cursor = Cursor {
            viewport: c.viewport_has_value.then_some((c.viewport_x, c.viewport_y)),
            wide_tail: c.wide_tail,
            visible: c.visible,
            blinking: c.blinking,
            password: c.password_input,
            style: CursorStyle::from_visual(c.visual_style),
        };
        Ok(())
    }

    fn get<T>(&self, data: i32, out: &mut T) -> bool {
        // SAFETY：h 有效；T 是这项数据的输出类型（调用处保证）。
        unsafe { (api().render_state_get)(self.h, data, out as *mut T as *mut _) == 0 }
    }

    /// 视口几列几行。
    pub fn size(&self) -> (u16, u16) {
        let (mut c, mut r) = (0u16, 0u16);
        self.get(ffi::rs_data::COLS, &mut c);
        self.get(ffi::rs_data::ROWS, &mut r);
        (c, r)
    }

    /// 自上次 clean 以来变了多少。
    pub fn dirty(&self) -> Dirty {
        let mut d = 0i32;
        self.get(ffi::rs_data::DIRTY, &mut d);
        match d {
            ffi::dirty::FALSE => Dirty::Clean,
            ffi::dirty::PARTIAL => Dirty::Partial,
            _ => Dirty::Full,
        }
    }

    /// 画完一帧，清掉整帧和每行的脏标记。
    pub fn clean(&mut self) {
        // SAFETY：h 有效。
        unsafe { (api().render_state_clean)(self.h) };
    }

    /// update 时的颜色。
    pub fn colors(&self) -> &Colors {
        &self.colors
    }

    /// update 时的光标。
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// update 时视口在回滚里的位置（画滚动条用）。
    pub fn scrollbar(&self) -> Scrollbar {
        self.scrollbar
    }

    /// 从上往下读视口的每一行。
    pub fn rows(&mut self) -> Rows<'_> {
        self.begin_rows();
        Rows { rs: self, y: 0 }
    }

    // ---- 给 Mirror 用的底层读法 ----

    pub(crate) fn begin_rows(&mut self) {
        let mut it = self.rows_it;
        self.get(ffi::rs_data::ROW_ITERATOR, &mut it);
        self.rows_it = it;
    }

    pub(crate) fn next_raw_row(&mut self) -> bool {
        // SAFETY：迭代器有效。
        unsafe { (api().render_state_row_iterator_next)(self.rows_it) }
    }

    fn row_get<T>(&self, data: i32, out: &mut T) -> bool {
        // SAFETY：迭代器停在一行上；T 是这项数据的输出类型（调用处保证）。
        unsafe { (api().render_state_row_get)(self.rows_it, data, out as *mut T as *mut _) == 0 }
    }

    pub(crate) fn row_id(&self) -> RowId {
        let mut id = RowId::default();
        self.row_get(ffi::rs_row::ID, &mut id);
        id
    }

    pub(crate) fn row_dirty(&self) -> bool {
        let mut v = false;
        self.row_get(ffi::rs_row::DIRTY, &mut v) && v
    }

    fn raw_row(&self) -> Option<u64> {
        let mut raw = 0u64;
        self.row_get(ffi::rs_row::RAW, &mut raw).then_some(raw)
    }

    pub(crate) fn row_wrapped(&self) -> bool {
        let Some(raw) = self.raw_row() else {
            return false;
        };
        let mut v = false;
        // SAFETY：raw 是这一行的 GhosttyRow 值；输出是 bool。
        unsafe { (api().row_get)(raw, ffi::row::WRAP, &mut v as *mut bool as *mut _) == 0 && v }
    }

    /// 这一行属于 shell 提示符的哪部分（OSC 133）：0 不是，1 提示符开头，2 接续。
    pub(crate) fn row_prompt(&self) -> u8 {
        let Some(raw) = self.raw_row() else {
            return 0;
        };
        let mut v = 0i32;
        // SAFETY：同上；输出是 GhosttyRowSemanticPrompt（int）。
        let ok = unsafe {
            (api().row_get)(raw, ffi::row::SEMANTIC_PROMPT, &mut v as *mut i32 as *mut _)
        };
        match (ok, v) {
            (0, ffi::row_prompt::PROMPT) => 1,
            (0, ffi::row_prompt::PROMPT_CONTINUATION) => 2,
            _ => 0,
        }
    }

    pub(crate) fn row_selection(&self) -> Option<(u16, u16)> {
        let mut s = ffi::RowSelection {
            size: size_of::<ffi::RowSelection>(),
            ..Default::default()
        };
        self.row_get(ffi::rs_row::SELECTION, &mut s)
            .then_some((s.start_x, s.end_x))
    }

    /// 读这一行的原始格子；之后可以按列查样式和字素。
    pub(crate) fn load_raw(&mut self) -> &[RawCell] {
        self.raw.clear();
        self.styles.clear();
        let mut v = ffi::CellsView {
            ptr: ptr::null(),
            len: 0,
        };
        if !self.row_get(ffi::rs_row::CELLS_RAW, &mut v) || v.ptr.is_null() {
            return &self.raw;
        }
        let bits = ffi::lib().cell;
        // SAFETY：库借出的这一行的格子，下次 update 之前有效。
        let cells = unsafe { std::slice::from_raw_parts(v.ptr, v.len) };
        self.raw.extend(cells.iter().map(|&c| {
            let mut out = RawCell {
                style: bits.style_id.of(c) as u16,
                wide: bits.wide.of(c) as u8,
                hyperlink: bits.hyperlink.of(c) != 0,
                ..RawCell::default()
            };
            match bits.tag.of(c) as i32 {
                ffi::content::CODEPOINT | ffi::content::CODEPOINT_GRAPHEME => {
                    out.codepoint = bits.codepoint.of(c) as u32;
                    out.grapheme = bits.tag.of(c) as i32 == ffi::content::CODEPOINT_GRAPHEME;
                }
                ffi::content::BG_COLOR_PALETTE => {
                    out.bg = Some((true, Rgb::new(bits.palette_index.of(c) as u8, 0, 0)));
                }
                _ => {
                    out.bg = Some((
                        false,
                        Rgb::new(bits.r.of(c) as u8, bits.g.of(c) as u8, bits.b.of(c) as u8),
                    ));
                }
            }
            out
        }));
        // 样式和字素按列查要用这一行的格子句柄。
        let mut cells_h = self.cells_h;
        self.row_get(ffi::rs_row::CELLS, &mut cells_h);
        self.cells_h = cells_h;
        &self.raw
    }

    pub(crate) fn raw(&self) -> &[RawCell] {
        &self.raw
    }

    fn select(&self, x: u16) -> bool {
        // SAFETY：格子句柄是 load_raw 填的这一行。
        unsafe { (api().render_state_row_cells_select)(self.cells_h, x) == 0 }
    }

    fn cell_get<T>(&self, data: i32, out: &mut T) -> bool {
        // SAFETY：select 过；T 是这项数据的输出类型（调用处保证）。
        unsafe {
            (api().render_state_row_cells_get)(self.cells_h, data, out as *mut T as *mut _) == 0
        }
    }

    /// 第 x 格的样式（load_raw 之后），同一行里按编号缓存。
    pub(crate) fn cell_style(&mut self, x: u16) -> CellStyle {
        let id = self.raw.get(x as usize).map_or(0, |c| c.style);
        if id == 0 {
            return CellStyle::default();
        }
        if let Some((_, s)) = self.styles.iter().find(|(i, _)| *i == id) {
            return *s;
        }
        let mut s = ffi::Style::sized();
        let style = if self.select(x) && self.cell_get(ffi::rs_cell::STYLE, &mut s) {
            CellStyle::from_ffi(&s)
        } else {
            CellStyle::default()
        };
        self.styles.push((id, style));
        style
    }

    /// 把第 x 格的字素（基本码点和组合的）接到 out 后面。
    pub(crate) fn cell_graphemes(&mut self, x: u16, out: &mut String) {
        if !self.select(x) {
            return;
        }
        let mut n = 0u32;
        if !self.cell_get(ffi::rs_cell::GRAPHEMES_LEN, &mut n) || n == 0 {
            return;
        }
        self.cps.clear();
        self.cps.resize(n as usize, 0);
        // SAFETY：缓冲区有 GRAPHEMES_LEN 个 u32。
        let ok = unsafe {
            (api().render_state_row_cells_get)(
                self.cells_h,
                ffi::rs_cell::GRAPHEMES_BUF,
                self.cps.as_mut_ptr() as *mut _,
            ) == 0
        };
        if ok {
            out.extend(self.cps.iter().filter_map(|&c| char::from_u32(c)));
        }
    }

    /// 把这一行的格子解析成 Cell（颜色、文字）放进 out / text。
    fn resolve_row(&mut self) {
        self.load_raw();
        let mut out = std::mem::take(&mut self.out);
        let mut text = std::mem::take(&mut self.text);
        out.clear();
        text.clear();
        let (fg0, bg0) = (self.colors.foreground, self.colors.background);
        for x in 0..self.raw.len() {
            let raw = self.raw[x];
            let x = x as u16;
            let style = self.cell_style(x);
            let palette = &self.colors.palette;
            let mut fg = style.fg.resolve(palette).unwrap_or(fg0);
            let mut bg = match raw.bg {
                Some((true, c)) => Some(palette[c.r as usize]),
                Some((false, c)) => Some(c),
                None => style.bg.resolve(palette),
            };
            let underline_color = style.underline_color.resolve(palette);
            if style.inverse {
                let f = fg;
                fg = bg.unwrap_or(bg0);
                bg = Some(f);
            }
            let start = text.len() as u32;
            let width = raw.width();
            if !matches!(width, Width::SpacerTail | Width::SpacerHead) {
                if raw.grapheme {
                    self.cell_graphemes(x, &mut text);
                } else if let Some(c) = char::from_u32(raw.codepoint).filter(|&c| c != '\0') {
                    text.push(c);
                }
            }
            out.push(Cell {
                x,
                width,
                fg,
                bg,
                underline: Underline::from_ffi(style.underline),
                underline_color,
                bold: style.bold,
                italic: style.italic,
                faint: style.faint,
                blink: style.blink,
                inverse: style.inverse,
                invisible: style.invisible,
                strikethrough: style.strikethrough,
                overline: style.overline,
                hyperlink: raw.hyperlink,
                text: (start, text.len() as u32),
            });
        }
        self.out = out;
        self.text = text;
    }
}

impl Drop for RenderState {
    fn drop(&mut self) {
        let api = api();
        // SAFETY：只释放建好了的句柄。
        unsafe {
            if !self.cells_h.is_null() {
                (api.render_state_row_cells_free)(self.cells_h);
            }
            if !self.rows_it.is_null() {
                (api.render_state_row_iterator_free)(self.rows_it);
            }
            if !self.h.is_null() {
                (api.render_state_free)(self.h);
            }
        }
    }
}

/// 视口的行，从上往下。
pub struct Rows<'a> {
    rs: &'a mut RenderState,
    y: u16,
}

impl Rows<'_> {
    /// 下一行；没有了是 None。
    pub fn next_row(&mut self) -> Option<Row<'_>> {
        if !self.rs.next_raw_row() {
            return None;
        }
        let y = self.y;
        self.y += 1;
        Some(Row { rs: self.rs, y })
    }
}

/// 视口的一行。
pub struct Row<'a> {
    rs: &'a mut RenderState,
    y: u16,
}

impl Row<'_> {
    /// 视口里第几行。
    pub fn y(&self) -> u16 {
        self.y
    }

    /// 行的身份，滚动时不变（缓存排好的字形用）。
    pub fn id(&self) -> RowId {
        self.rs.row_id()
    }

    /// 自上次 clean 以来变过（保守：滚动后全算变了）。
    pub fn dirty(&self) -> bool {
        self.rs.row_dirty()
    }

    /// 软换行到下一行。
    pub fn wrapped(&self) -> bool {
        self.rs.row_wrapped()
    }

    /// 这一行选中的列（含两头）。
    pub fn selection(&self) -> Option<(u16, u16)> {
        self.rs.row_selection()
    }

    /// 这一行的格子，颜色解析好了；缓冲区复用，下一行读时作废。
    pub fn cells(&mut self) -> Cells<'_> {
        self.rs.resolve_row();
        Cells {
            cells: &self.rs.out,
            text: &self.rs.text,
        }
    }
}

/// 一行的格子和它们的文字。
#[derive(Clone, Copy)]
pub struct Cells<'a> {
    cells: &'a [Cell],
    text: &'a str,
}

impl<'a> Cells<'a> {
    pub fn as_slice(&self) -> &'a [Cell] {
        self.cells
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// 一格的文字（字符加组合码点）；空格子、宽字符后半是空串。
    pub fn text(&self, c: &Cell) -> &'a str {
        &self.text[c.text.0 as usize..c.text.1 as usize]
    }

    /// 一格一格：（格子，文字）。
    pub fn iter(&self) -> impl Iterator<Item = (&'a Cell, &'a str)> + 'a {
        let text = self.text;
        self.cells
            .iter()
            .map(move |c| (c, &text[c.text.0 as usize..c.text.1 as usize]))
    }
}
