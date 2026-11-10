//! 客户端侧的终端模型：和主机用同一个 alacritty_terminal。快照回放 + 增量字节，不自己解析任何东西。
//! 模型产生的回写（DA、光标位置这些查询的应答）全部丢掉：主机那份模型已经答过了。

use alacritty_terminal::{
    event::EventListener,
    grid::{Dimensions, Scroll},
    term::{cell::Flags, Config, RenderableContent, Term, TermMode},
    vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb},
};
use lolidesk_proto::palette;

#[derive(Clone, Copy)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

#[derive(Clone, Copy)]
pub struct Noop;
impl EventListener for Noop {}

pub struct Model {
    term: Term<Noop>,
    parser: Processor,
    pub size: Size,
    pub title: String,
    pub bell: bool,
}

impl Model {
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = Size {
            cols: cols.max(2),
            rows: rows.max(1),
        };
        Self {
            term: Term::new(
                Config {
                    scrolling_history: 5000,
                    ..Config::default()
                },
                &size,
                Noop,
            ),
            parser: Processor::new(),
            size,
            title: String::new(),
            bell: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// 主机发来整屏快照：重建模型再回放。
    pub fn snapshot(&mut self, cols: u16, rows: u16, data: &[u8]) {
        *self = Self::new(cols, rows);
        self.feed(data);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = Size {
            cols: cols.max(2),
            rows: rows.max(1),
        };
        if size.cols != self.size.cols || size.rows != self.size.rows {
            self.size = size;
            self.term.resize(size);
        }
    }

    pub fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
    }

    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn show_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::SHOW_CURSOR)
    }

    pub fn content(&self) -> RenderableContent<'_> {
        self.term.renderable_content()
    }

    pub fn cursor_shape(&self) -> CursorShape {
        self.term.cursor_style().shape
    }
}

/// 把模型里的颜色换成 RGB（优先终端程序 OSC 设过的颜色表，其次猫眼调色板）。
pub fn resolve(
    color: Color,
    colors: &alacritty_terminal::term::color::Colors,
    bold: bool,
) -> [u8; 3] {
    let from_rgb = |r: Rgb| [r.r, r.g, r.b];
    match color {
        Color::Spec(rgb) => from_rgb(rgb),
        Color::Indexed(i) => colors[i as usize]
            .map(from_rgb)
            .unwrap_or_else(|| palette::indexed(i)),
        Color::Named(n) => {
            if let Some(c) = colors[n] {
                return from_rgb(c);
            }
            let idx = n as usize;
            match idx {
                0..=7 if bold => palette::indexed(idx as u8 + 8),
                0..=15 => palette::indexed(idx as u8),
                256 => palette::FG,
                257 => palette::BG,
                258 => palette::CURSOR,
                259..=266 => dim(palette::indexed((idx - 259) as u8)),
                267 => palette::FG,
                _ => dim(palette::FG),
            }
        }
    }
}

pub fn dim(c: [u8; 3]) -> [u8; 3] {
    [
        (c[0] as u16 * 2 / 3) as u8,
        (c[1] as u16 * 2 / 3) as u8,
        (c[2] as u16 * 2 / 3) as u8,
    ]
}

/// 一格的最终前景 / 背景（处理反显、加粗提亮、变暗、隐藏）。
pub fn cell_colors(
    fg: Color,
    bg: Color,
    flags: Flags,
    colors: &alacritty_terminal::term::color::Colors,
) -> ([u8; 3], Option<[u8; 3]>) {
    let bold = flags.contains(Flags::BOLD);
    let mut f = resolve(fg, colors, bold);
    let mut b = resolve(bg, colors, false);
    if flags.contains(Flags::DIM) {
        f = dim(f);
    }
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut f, &mut b);
    }
    if flags.contains(Flags::HIDDEN) {
        f = b;
    }
    let bg_is_default =
        matches!(bg, Color::Named(NamedColor::Background)) && !flags.contains(Flags::INVERSE);
    (f, if bg_is_default { None } else { Some(b) })
}
