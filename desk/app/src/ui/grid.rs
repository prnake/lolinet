//! 终端的格子：客户端这边的 libghostty-vt 终端、画它、键盘、输入法、鼠标、选区、剪贴板。
//!
//! 主机的帧（[`TermChunk::Frame`]）按顺序 `write` 进自己的终端，[`TermChunk::Resized`] 处换尺寸；
//! 自己从不按视图大小改终端：量出放得下几列几行就 `resize` 告诉主机，主机回 Resized 才换。
//! 会话比视图小（别的客户端定的尺寸）就画在左上角。
//!
//! 画法：一个自定义 Element。每帧 `RenderState::update`，一行一行读格子。字不按行排（CoreText 排一整行
//! 200 个字要 0.3 ms，整屏新字就是 20 ms）：等宽终端里一个字素就是格子上的几个字形，所以每个字素
//! 每种字体只排一次（含回退字体、emoji），记下字形，画的时候 `paint_glyph` 放到格子上。宽字符在两格里居中。
//! 背景按同色合并成矩形，下划线、删除线、框线、方块自己画。读好的行按内容的哈希缓存；没变的行
//! （`row.dirty()` 为假）连格子都不读。
//! 思路参考了 gpui-ghostty（Xuanwo，Apache-2.0）的 `gpui_ghostty_terminal/src/view/mod.rs`，代码是自己写的。
//!
//! 键盘：终端的按键上下文里 Tab、Shift+Tab 用 NoAction 挡掉外层绑定，所有键按物理键位（NSEvent 键码）
//! 交给 KeyEncoder（编码前 `sync`，模式跟着帧走）。⌘ 组合键留给应用（⌘C/⌘V/⌘K/⌘=/⌘-/⌘0/⌘A 是
//! 这里的动作，其余给菜单）。输入法：实现 `EntityInputHandler`；开着中文、日文输入法时 GPUI 先把可打印键
//! 交给输入法，组好的字从 `replace_text_in_range` 来；没开输入法时可打印键在 key_down 里编码、拦下，
//! 系统就不会再 insertText——同一个键不会发两次。死键（没有字的书写键）放给输入法。

use std::{
    cell::Cell,
    collections::HashMap,
    hash::{Hash, Hasher},
    ops::Range,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{
    App, AppContext, Bounds, ClipboardItem, Context, CursorStyle as MouseCursor, Element,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, Font, FontFeatures, FontId, FontStyle, FontWeight, GlobalElementId, GlyphId, Hsla,
    InspectorElementId, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, KeyUpEvent,
    LayoutId, ModifiersChangedEvent, MouseButton as GButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, NoAction, ParentElement, Pixels, Point, Render, ScrollEvent, ShapedLine,
    SharedString, StatefulInteractiveElement, Style, Styled, Subscription, Task, TextAlign,
    TextRun, UTF16Selection, WeakEntity, Window, actions, div, fill, outline, point, px, relative,
    size,
};
use lolidesk_ghostty::{
    Cell as VtCell, CursorStyle, Dirty, Key, KeyAction, KeyEncoder, KeyEvent, Mode, Mods as VtMods,
    MouseAction, MouseButton as VtButton, MouseEncoder, MouseEvent, MouseSize, OptionAsAlt,
    RenderState, Rgb, Scroll, Selection, Terminal, Theme, Underline, Width,
};
use lolidesk_proto::{ClearKind, TermSize, palette};

use crate::{
    keys::{self, ModifierTracker},
    model::Model,
    prefs::FONT_DEFAULT,
    term::{TermChunk, TermPhase, TermSession, TermSessionEvent},
    termtext::{PAD_X, PAD_Y, fit_grid, url_at, wheel_steps, word_range},
    theme::{self, MONO},
    ui::{
        boxdraw,
        dialogs::{self, Form},
    },
};

actions!(
    terminal,
    [
        TermCopy,
        TermPaste,
        TermClear,
        FontBigger,
        FontSmaller,
        FontReset,
        TermSelectAll
    ]
);

/// 终端格子的按键上下文。
pub const GRID_CONTEXT: &str = "Terminal";

/// 终端的快捷键。在 `ui::app::init` 里调。
#[cfg(target_os = "macos")]
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-c", TermCopy, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-v", TermPaste, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-k", TermClear, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-=", FontBigger, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-+", FontBigger, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd--", FontSmaller, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-0", FontReset, Some(GRID_CONTEXT)),
        KeyBinding::new("cmd-a", TermSelectAll, Some(GRID_CONTEXT)),
        // gpui-kit 的 Root 拿 tab / shift-tab 挪焦点：在终端里它们是键
        KeyBinding::new("tab", NoAction, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(GRID_CONTEXT)),
    ]);
}

/// Linux：Ctrl 组合键归终端里的程序，复制粘贴这些用 Ctrl+Shift（和 GNOME 终端一样）；字号用 Ctrl+= / Ctrl+- / Ctrl+0。
#[cfg(not(target_os = "macos"))]
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-c", TermCopy, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-shift-v", TermPaste, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-shift-k", TermClear, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-=", FontBigger, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-+", FontBigger, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl--", FontSmaller, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-0", FontReset, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-shift-a", TermSelectAll, Some(GRID_CONTEXT)),
        // gpui-kit 的 Root 拿 tab / shift-tab 挪焦点、ctrl-c 复制：在终端里它们是键（ctrl-c 是中断）
        KeyBinding::new("tab", NoAction, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-tab", NoAction, Some(GRID_CONTEXT)),
        KeyBinding::new("ctrl-c", NoAction, Some(GRID_CONTEXT)),
    ]);
}

fn rgb_hsla(c: Rgb, alpha: f32) -> Hsla {
    let mut h: Hsla = gpui_kit::Rgba {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: 1.0,
    }
    .into();
    h.a = alpha;
    h
}

/// 猫眼的终端配色：和主机一样（proto::palette）。
pub fn catseye_theme() -> Theme {
    let c = |v: [u8; 3]| Rgb::new(v[0], v[1], v[2]);
    Theme {
        foreground: c(palette::FG),
        background: c(palette::BG),
        cursor: Some(c(palette::CURSOR)),
        palette: palette::BASE16.map(c),
    }
}

/// 客户端的终端和它的渲染状态、编码器。
pub struct Engine {
    pub term: Terminal,
    rs: RenderState,
    keys: KeyEncoder,
    mouse: MouseEncoder,
}

impl Engine {
    pub fn new(cols: u16, rows: u16) -> anyhow::Result<Self> {
        lolidesk_ghostty::load()?;
        let mut term = Terminal::new(cols, rows, Box::new(()))?;
        term.set_theme(&catseye_theme());
        term.set_default_cursor(CursorStyle::Block, true);
        Ok(Self {
            term,
            rs: RenderState::new()?,
            keys: KeyEncoder::new()?,
            mouse: MouseEncoder::new()?,
        })
    }
}

/// 字体量出来的格子大小。
#[derive(Clone)]
struct Metrics {
    font_size: f32,
    scale: f32,
    cell_w: f32,
    cell_h: f32,
    /// 格子顶到基线
    baseline: f32,
    /// 常规、粗、斜、粗斜
    fonts: [Font; 4],
}

impl Metrics {
    fn new(window: &mut Window, font_size: f32) -> Self {
        let scale = window.scale_factor().max(1.0);
        let features = FontFeatures(Arc::new(vec![("calt".into(), 0), ("liga".into(), 0)]));
        let make = |weight: FontWeight, style: FontStyle| Font {
            family: MONO.into(),
            features: features.clone(),
            fallbacks: None,
            weight,
            style,
        };
        let fonts = [
            make(FontWeight::NORMAL, FontStyle::Normal),
            make(FontWeight::BOLD, FontStyle::Normal),
            make(FontWeight::NORMAL, FontStyle::Italic),
            make(FontWeight::BOLD, FontStyle::Italic),
        ];
        let ts = window.text_system();
        let id = ts.resolve_font(&fonts[0]);
        let size = px(font_size);
        let advance = ts
            .advance(id, size, 'M')
            .map(|s| f32::from(s.width))
            .unwrap_or(font_size * 0.6);
        let ascent = f32::from(ts.ascent(id, size));
        let descent = f32::from(ts.descent(id, size)).abs();
        // 格子对齐到设备像素：相邻格子的背景、框线严丝合缝，没有半像素的缝
        let cell_w = boxdraw::snap(advance, scale).max(1.0);
        let cell_h = ((ascent + descent) * scale).ceil() / scale;
        let baseline = (cell_h - ascent - descent) / 2.0 + ascent;
        Self {
            font_size,
            scale,
            cell_w,
            cell_h,
            baseline,
            fonts,
        }
    }
}

/// 一个字形：字体、字形号、相对字素起点的位置、是不是 emoji（彩色，不上色）。
#[derive(Clone, Copy)]
struct GlyphRef {
    font_id: FontId,
    id: GlyphId,
    x: f32,
    y: f32,
    emoji: bool,
}

/// 一个字素排出来的字形和它多宽。
#[derive(Clone)]
struct Shaped {
    glyphs: Rc<[GlyphRef]>,
    width: f32,
}

/// 字素 → 字形。每个字素每种字体只排一次；单个字符的走快路（不分配）。
#[derive(Default)]
struct Glyphs {
    one: HashMap<(char, u8), Shaped>,
    many: HashMap<(String, u8), Shaped>,
}

impl Glyphs {
    fn clear(&mut self) {
        self.one.clear();
        self.many.clear();
    }

    /// 取一个字素的字形；第二个值是这次是不是新排的。
    fn get(&mut self, text: &str, font: u8, m: &Metrics, window: &mut Window) -> (Shaped, bool) {
        let mut it = text.chars();
        let single = match (it.next(), it.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        if let Some(c) = single
            && let Some(s) = self.one.get(&(c, font))
        {
            return (s.clone(), false);
        }
        if single.is_none()
            && let Some(s) = self.many.get(&(text.to_string(), font))
        {
            return (s.clone(), false);
        }
        // 字素见得太多（程序打了一大堆不同的字）就从头来，不让它一直长
        if self.one.len() + self.many.len() > 50_000 {
            self.clear();
        }
        let line = window.text_system().shape_line(
            SharedString::from(text.to_string()),
            px(m.font_size),
            &[TextRun {
                len: text.len(),
                font: m.fonts[font as usize].clone(),
                color: Hsla::default(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
            None,
        );
        let glyphs: Vec<GlyphRef> = line
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs.iter().map(move |g| GlyphRef {
                    font_id: run.font_id,
                    id: g.id,
                    x: f32::from(g.position.x),
                    y: f32::from(g.position.y),
                    emoji: g.is_emoji,
                })
            })
            .collect();
        let shaped = Shaped {
            glyphs: glyphs.into(),
            width: f32::from(line.width()),
        };
        match single {
            Some(c) => self.one.insert((c, font), shaped.clone()),
            None => self.many.insert((text.to_string(), font), shaped.clone()),
        };
        (shaped, true)
    }
}

/// 一行读好的样子（按内容缓存）。
struct RowPaint {
    /// 字：相对行首的 x、颜色、字形
    glyphs: Vec<(f32, Hsla, Rc<[GlyphRef]>)>,
    /// 背景：[起, 止) 列和颜色
    bgs: Vec<(u16, u16, Hsla)>,
    /// 下划线、删除线、上划线：[起, 止) 列、样子、颜色
    decos: Vec<(u16, u16, Deco, Hsla)>,
    /// 每列的字符（宽字符后半格是 '\0'），选词、找网址、光标下的字用
    chars: Vec<char>,
    /// 每列的样式下标（光标下的字用哪个字体）
    bold: Vec<bool>,
    /// 自己画的框线、方块：列、宽、字符、颜色
    boxes: Vec<(u16, u16, char, Hsla)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Deco {
    Underline,
    Double,
    Curly,
    Dotted,
    Dashed,
    Strike,
    Overline,
}

/// 光标怎么画。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorShape {
    Block,
    Bar,
    Underline,
    Hollow,
}

struct CursorPaint {
    x: u16,
    y: u16,
    wide: bool,
    shape: CursorShape,
    color: Hsla,
    /// 块光标下的字（相对格子的 x、字形）
    text: Option<(f32, Rc<[GlyphRef]>)>,
}

/// 一帧要画的东西（prepaint 算好，paint 画）。
pub struct PaintFrame {
    origin: Point<Pixels>,
    scale: f32,
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
    baseline: f32,
    cols: u16,
    rows_n: u16,
    bg: Hsla,
    rows: Vec<(u16, Rc<RowPaint>)>,
    sel: Vec<(u16, u16, u16)>,
    sel_color: Hsla,
    cursor: Option<CursorPaint>,
    marked: Option<(ShapedLine, u16, u16, f32)>,
    scrollbar: Option<(Bounds<Pixels>, Hsla)>,
    bell: f32,
    animate: bool,
}

/// 选区的单位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelMode {
    Char,
    Word,
    Line,
}

/// 正在拖的选区（屏幕坐标：从回滚最上面数）。
#[derive(Clone, Copy, Debug)]
struct SelDrag {
    anchor: (u16, u32),
    head: (u16, u32),
    mode: SelMode,
    rect: bool,
}

/// 上一帧的版面（鼠标换算格子用）。
#[derive(Clone, Copy, Debug, Default)]
struct Layout {
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    cell_w: f32,
    cell_h: f32,
    cols: u16,
    rows: u16,
    cursor: Option<(u16, u16)>,
}

/// 上一帧的开销（测性能用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct GridStats {
    /// 读终端、排字的时间（prepaint）
    pub prepare: Duration,
    /// 重新读了格子的行
    pub rebuilt: usize,
    /// 行缓存里没有、重新做的行
    pub fresh: usize,
    /// 新排的字素（以前没见过的字）
    pub shaped: usize,
    pub rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridEvent {
    /// 终端的标题变了
    Title(String),
}

pub struct TermGrid {
    pub session: Entity<TermSession>,
    model: WeakEntity<Model>,
    pub focus: FocusHandle,
    engine: Result<Engine, String>,
    metrics: Option<Metrics>,
    /// 上一帧各行（按视口里的第几行）
    prev: Vec<Option<Rc<RowPaint>>>,
    /// 排好的行，按内容哈希；值里记着最后一次用到是第几帧
    cache: HashMap<u64, (u64, Rc<RowPaint>)>,
    glyphs: Glyphs,
    frame_no: u64,
    /// 告诉过主机的尺寸
    sent: Option<TermSize>,
    layout: Rc<Cell<Layout>>,
    drag: Option<SelDrag>,
    has_selection: bool,
    /// 程序要鼠标时按着的键
    mouse_down: Option<VtButton>,
    wheel: f32,
    mods: ModifierTracker,
    /// 输入法正在组的字，和组字里选中的范围（UTF-16）
    marked: Option<String>,
    marked_sel: Range<usize>,
    blink_on: bool,
    blink_task: Option<Task<()>>,
    bell_at: Option<Instant>,
    scrolled_at: Option<Instant>,
    title: String,
    stats: GridStats,
    focus_subs: Vec<Subscription>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<GridEvent> for TermGrid {}

impl Focusable for TermGrid {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TermGrid {
    pub fn new(
        session: Entity<TermSession>,
        model: WeakEntity<Model>,
        cx: &mut Context<Self>,
    ) -> Self {
        let size = session.read(cx).size;
        let engine = Engine::new(size.cols, size.rows).map_err(|e| format!("{e:#}"));
        let subs = vec![cx.subscribe(&session, |g: &mut Self, _, ev, cx| match ev {
            TermSessionEvent::Output => g.pull(cx),
            TermSessionEvent::Bell => {
                g.bell_at = Some(Instant::now());
                cx.notify();
            }
            _ => cx.notify(),
        })];
        let mut g = Self {
            session,
            model,
            focus: cx.focus_handle(),
            engine,
            metrics: None,
            prev: Vec::new(),
            cache: HashMap::new(),
            glyphs: Glyphs::default(),
            frame_no: 0,
            sent: None,
            layout: Rc::new(Cell::new(Layout::default())),
            drag: None,
            has_selection: false,
            mouse_down: None,
            wheel: 0.0,
            mods: ModifierTracker::default(),
            marked: None,
            marked_sel: 0..0,
            blink_on: true,
            blink_task: None,
            bell_at: None,
            scrolled_at: None,
            title: String::new(),
            stats: GridStats::default(),
            focus_subs: Vec::new(),
            _subs: subs,
        };
        g.pull(cx);
        g
    }

    /// 终端库没载入时的原因。
    pub fn engine_error(&self) -> Option<&str> {
        self.engine.as_ref().err().map(String::as_str)
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// 测试、快照用：直接拿终端。
    pub fn terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.engine.as_mut().ok().map(|e| &mut e.term)
    }

    /// 上一帧的开销。
    pub fn stats(&self) -> GridStats {
        self.stats
    }

    /// 测试用：上一帧格子 (0, 0) 的左上角、格子宽高、终端几列几行。
    pub fn cell_layout(&self) -> (Point<Pixels>, f32, f32, u16, u16) {
        let l = self.layout.get();
        (l.origin, l.cell_w, l.cell_h, l.cols, l.rows)
    }

    /// 把会话排着的输出喂给终端。
    pub fn pull(&mut self, cx: &mut Context<Self>) {
        let chunks = self.session.update(cx, |s, _| s.drain());
        if chunks.is_empty() {
            return;
        }
        let (cw, ch) = self
            .metrics
            .as_ref()
            .map(|m| (m.cell_w.round() as u32, m.cell_h.round() as u32))
            .unwrap_or((8, 16));
        let Ok(engine) = &mut self.engine else {
            return;
        };
        for c in chunks {
            match c {
                TermChunk::Frame(bytes) => engine.term.write(&bytes),
                TermChunk::Resized { cols, rows } => engine.term.resize(cols, rows, cw, ch),
            }
        }
        let title = engine.term.title();
        if title != self.title {
            self.title = title.clone();
            cx.emit(GridEvent::Title(title));
        }
        cx.notify();
    }

    fn font_size(&self, cx: &App) -> f32 {
        self.model
            .upgrade()
            .map(|m| m.read(cx).prefs.font_size)
            .unwrap_or(FONT_DEFAULT)
    }

    fn focused_subs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_subs.is_empty() {
            return;
        }
        let focus = self.focus.clone();
        self.focus_subs = vec![
            cx.on_focus(&focus, window, |g, _, cx| g.on_focus_change(true, cx)),
            cx.on_blur(&focus, window, |g, _, cx| g.on_focus_change(false, cx)),
        ];
    }

    fn on_focus_change(&mut self, gained: bool, cx: &mut Context<Self>) {
        if let Ok(e) = &self.engine {
            let bytes = lolidesk_ghostty::focus(&e.term, gained);
            self.session.update(cx, |s, _| {
                if gained {
                    s.claim();
                }
                s.input(bytes);
            });
        }
        if gained {
            self.start_blink(cx);
        } else {
            self.blink_task = None;
            self.blink_on = true;
            self.mods.reset();
        }
        cx.notify();
    }

    fn start_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_on = true;
        self.blink_task = Some(cx.spawn(async move |g, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                if g.update(cx, |g, cx| {
                    g.blink_on = !g.blink_on;
                    cx.notify();
                })
                .is_err()
                {
                    break;
                }
            }
        }));
    }

    // ------------------------------------------------------------ 画

    fn prepare(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaintFrame> {
        let font_size = self.font_size(cx);
        let scale = window.scale_factor().max(1.0);
        if self.metrics.as_ref().map(|m| (m.font_size, m.scale)) != Some((font_size, scale)) {
            self.metrics = Some(Metrics::new(window, font_size));
            self.cache.clear();
            self.glyphs.clear();
            self.prev.clear();
        }
        let m = self.metrics.clone()?;
        // 量放得下几列几行，变了告诉主机（主机回 Resized 才真换）
        let (cols, rows) = fit_grid(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            m.cell_w,
            m.cell_h,
        );
        let want = TermSize {
            cols,
            rows,
            cell_w: m.cell_w.round() as u16,
            cell_h: m.cell_h.round() as u16,
        };
        let can_send = {
            let s = self.session.read(cx);
            matches!(s.phase, TermPhase::Live | TermPhase::Opening)
        };
        if can_send && self.sent != Some(want) {
            self.sent = Some(want);
            self.session.update(cx, |s, _| s.resize(want));
        }

        let focused = self.focus.is_focused(window);
        let started = Instant::now();
        let (mut rebuilt, mut fresh, mut shaped) = (0usize, 0usize, 0usize);
        let Ok(engine) = &mut self.engine else {
            return None;
        };
        engine.rs.update(&engine.term).ok()?;
        let full = engine.rs.dirty() == Dirty::Full;
        let colors = engine.rs.colors().clone();
        let vt_cursor = engine.rs.cursor();
        let sb = engine.rs.scrollbar();
        let (tcols, trows) = engine.rs.size();
        self.frame_no += 1;
        let frame_no = self.frame_no;
        if self.prev.len() != trows as usize {
            self.prev = vec![None; trows as usize];
        }
        let origin = bounds.origin + point(px(PAD_X), px(PAD_Y));
        let origin = point(
            px(boxdraw::snap(f32::from(origin.x), m.scale)),
            px(boxdraw::snap(f32::from(origin.y), m.scale)),
        );
        let mut out = Vec::with_capacity(trows as usize);
        let mut sel = Vec::new();
        {
            let mut it = engine.rs.rows();
            while let Some(mut row) = it.next_row() {
                let y = row.y();
                if let Some((a, b)) = row.selection() {
                    sel.push((y, a, b));
                }
                let slot = &mut self.prev[y as usize];
                let paint = match slot {
                    Some(p) if !full && !row.dirty() => p.clone(),
                    _ => {
                        rebuilt += 1;
                        let cells = row.cells();
                        let built = build_row(cells.iter(), &colors.palette, &m);
                        let hash = built.hash;
                        match self.cache.get_mut(&hash) {
                            Some((used, p)) => {
                                *used = frame_no;
                                p.clone()
                            }
                            None => {
                                fresh += 1;
                                let (row, n) = built.resolve(&mut self.glyphs, window, &m);
                                shaped += n;
                                let p = Rc::new(row);
                                self.cache.insert(hash, (frame_no, p.clone()));
                                p
                            }
                        }
                    }
                };
                *slot = Some(paint.clone());
                out.push((y, paint));
            }
        }
        engine.rs.clean();
        // 缓存不让它无限长：留最近几帧用过的
        if self.cache.len() > 8 * trows as usize + 64 {
            self.cache.retain(|_, (used, _)| frame_no - *used < 4);
        }

        let bg = rgb_hsla(colors.background, 1.0);
        let fg = rgb_hsla(colors.foreground, 1.0);
        let cursor_color = colors
            .cursor
            .map(|c| rgb_hsla(c, 1.0))
            .unwrap_or_else(theme::eye);

        // 光标
        let blink_hidden = focused && vt_cursor.blinking && !self.blink_on;
        let cursor = match vt_cursor.viewport {
            Some((cx_, cy)) if vt_cursor.visible && !blink_hidden && self.marked.is_none() => {
                let (x, wide) = if vt_cursor.wide_tail {
                    (cx_.saturating_sub(1), true)
                } else {
                    let row = out.get(cy as usize).map(|(_, p)| p.clone());
                    let wide = row
                        .as_ref()
                        .and_then(|p| p.chars.get(cx_ as usize + 1))
                        .is_some_and(|c| *c == '\0');
                    (cx_, wide)
                };
                let shape = if !focused {
                    CursorShape::Hollow
                } else {
                    match vt_cursor.style {
                        CursorStyle::Block => CursorShape::Block,
                        CursorStyle::Bar => CursorShape::Bar,
                        CursorStyle::Underline => CursorShape::Underline,
                        CursorStyle::BlockHollow => CursorShape::Hollow,
                    }
                };
                // 块光标下的字换成光标字色重画
                let text = (shape == CursorShape::Block)
                    .then(|| out.get(cy as usize).map(|(_, p)| p.clone()))
                    .flatten()
                    .and_then(|p| {
                        let c = *p.chars.get(x as usize)?;
                        if c == ' ' || c == '\0' || boxdraw::is_drawn(c) {
                            return None;
                        }
                        let font = u8::from(p.bold.get(x as usize).copied().unwrap_or(false));
                        let mut buf = [0u8; 4];
                        let (g, _) = self.glyphs.get(c.encode_utf8(&mut buf), font, &m, window);
                        let room = if wide { 2.0 } else { 1.0 } * m.cell_w;
                        Some((((room - g.width) / 2.0).max(0.0), g.glyphs))
                    });
                Some(CursorPaint {
                    x,
                    y: cy,
                    wide,
                    shape,
                    color: cursor_color,
                    text,
                })
            }
            _ => None,
        };

        // 输入法组的字：画在光标那里
        let marked = match (&self.marked, vt_cursor.viewport) {
            (Some(text), Some((x, y))) if !text.is_empty() => {
                let s: SharedString = text.clone().into();
                let line = window.text_system().shape_line(
                    s.clone(),
                    px(m.font_size),
                    &[TextRun {
                        len: s.len(),
                        font: m.fonts[0].clone(),
                        color: fg,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                let w = f32::from(line.width());
                Some((line, x, y, w))
            }
            _ => None,
        };

        // 滚动条：翻到回滚里时显示，回到底后一秒内淡出
        let now = Instant::now();
        let mut animate = false;
        let scrollbar = if sb.total > sb.len && sb.len > 0 {
            let at_bottom = sb.offset + sb.len >= sb.total;
            let alpha = if !at_bottom {
                1.0
            } else {
                match self.scrolled_at {
                    Some(t) if now.duration_since(t) < Duration::from_millis(900) => {
                        animate = true;
                        1.0 - now.duration_since(t).as_secs_f32() / 0.9
                    }
                    _ => 0.0,
                }
            };
            (alpha > 0.0).then(|| {
                let h = f32::from(bounds.size.height) - 8.0;
                let thumb = (h * sb.len as f32 / sb.total as f32).max(24.0);
                let y = 4.0 + (h - thumb) * sb.offset as f32 / (sb.total - sb.len).max(1) as f32;
                let mut c = theme::line3();
                c.a *= alpha * 1.6;
                (
                    Bounds::new(
                        bounds.origin + point(bounds.size.width - px(6.), px(y)),
                        size(px(3.), px(thumb)),
                    ),
                    c,
                )
            })
        } else {
            None
        };

        // 响铃：一闪
        let bell = match self.bell_at {
            Some(t) if now.duration_since(t) < Duration::from_millis(260) => {
                animate = true;
                1.0 - now.duration_since(t).as_secs_f32() / 0.26
            }
            _ => 0.0,
        };

        self.stats = GridStats {
            prepare: started.elapsed(),
            rebuilt,
            fresh,
            shaped,
            rows: out.len(),
        };
        self.layout.set(Layout {
            origin,
            bounds,
            cell_w: m.cell_w,
            cell_h: m.cell_h,
            cols: tcols,
            rows: trows,
            cursor: vt_cursor.viewport,
        });

        Some(PaintFrame {
            origin,
            scale: m.scale,
            font_size: m.font_size,
            baseline: m.baseline,
            cell_w: m.cell_w,
            cell_h: m.cell_h,
            cols: tcols,
            rows_n: trows,
            bg,
            rows: out,
            sel,
            sel_color: {
                let mut c = theme::eye();
                c.a = 0.28;
                c
            },
            cursor,
            marked,
            scrollbar,
            bell,
            animate,
        })
    }

    // ------------------------------------------------------------ 输入

    fn engine_send(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        if let Ok(e) = &mut self.engine {
            if !e.term.at_bottom() {
                e.term.scroll_viewport(Scroll::Bottom);
            }
            if self.has_selection {
                e.term.set_selection(None);
                self.has_selection = false;
                self.drag = None;
            }
        }
        self.blink_on = true;
        self.session.update(cx, |s, _| s.input(bytes));
        cx.notify();
    }

    fn option_as_alt(&self, cx: &App) -> bool {
        self.model
            .upgrade()
            .map(|m| m.read(cx).prefs.option_as_alt)
            .unwrap_or(true)
    }

    /// 编一个键（按下、重复、松开）。
    fn encode_key(
        &mut self,
        ks: &gpui_kit::Keystroke,
        action: KeyAction,
        alt_as_alt: bool,
    ) -> Option<Vec<u8>> {
        let e = self.engine.as_mut().ok()?;
        let m = &ks.modifiers;
        let code = physical_code(&ks.key);
        let key = code.map(Key::from_code).unwrap_or(Key::Unidentified);
        let unshifted = single_char(&ks.key);
        let typed = ks
            .key_char
            .as_deref()
            .filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
        // Option 当 Alt / Ctrl 时，文字用键上的字（编码器按修饰键改）
        let alt_text;
        let text: &str = if (m.alt && alt_as_alt) || m.control {
            alt_text = unshifted.map(|c| c.to_string()).unwrap_or_default();
            &alt_text
        } else {
            typed.unwrap_or("")
        };
        let mut mods = VtMods::NONE;
        if m.shift {
            mods |= VtMods::SHIFT;
        }
        if m.control {
            mods |= VtMods::CTRL;
        }
        if m.alt {
            mods |= VtMods::ALT;
        }
        if m.platform {
            mods |= VtMods::SUPER;
        }
        let mut consumed = VtMods::NONE;
        if typed.is_some() && m.shift {
            consumed |= VtMods::SHIFT;
        }
        if typed.is_some() && m.alt && !alt_as_alt {
            consumed |= VtMods::ALT;
        }
        e.keys.set_option_as_alt(if alt_as_alt {
            OptionAsAlt::Yes
        } else {
            OptionAsAlt::No
        });
        e.keys.sync(&e.term);
        let out = e
            .keys
            .encode(&KeyEvent {
                action,
                key,
                mods,
                consumed_mods: consumed,
                text,
                unshifted,
                composing: false,
            })
            .to_vec();
        Some(out)
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        // 输入法在组字：键归它
        if self.marked.is_some() {
            return;
        }
        // ⌘ 组合键归应用（终端自己的几个在动作里）
        if m.platform || self.engine.is_err() {
            return;
        }
        let alt_as_alt = self.option_as_alt(cx);
        // 死键（书写键却没打出字）：交给输入法组字
        let typed = ks
            .key_char
            .as_deref()
            .is_some_and(|t| !t.is_empty() && !t.chars().any(char::is_control));
        let key = physical_code(&ks.key).map(Key::from_code);
        if !typed && !m.control && !(m.alt && alt_as_alt) && key.is_some_and(is_writing_key) {
            return;
        }
        cx.stop_propagation();
        let action = if ev.is_held {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        };
        if let Some(bytes) = self.encode_key(ks, action, alt_as_alt) {
            self.engine_send(bytes, cx);
        }
    }

    fn key_up(&mut self, ev: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.modifiers.platform || self.marked.is_some() {
            return;
        }
        cx.stop_propagation();
        let alt_as_alt = self.option_as_alt(cx);
        if let Some(bytes) = self.encode_key(&ev.keystroke, KeyAction::Release, alt_as_alt) {
            // 传统模式下松开键什么都不发
            if !bytes.is_empty() {
                self.session.update(cx, |s, _| s.input(bytes));
            }
        }
    }

    fn modifiers_changed(
        &mut self,
        ev: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &ev.modifiers;
        let now = keys::mods(m.shift, m.control, m.alt, m.platform);
        let changes = self.mods.change(keys::current_key_code(), now.0);
        let Ok(e) = &mut self.engine else { return };
        // 只有 Kitty 协议要报所有键时修饰键自己才有编码，传统模式下编出来是空的
        if e.term.kitty_keyboard_flags() & 0b1000 == 0 {
            return;
        }
        e.keys.sync(&e.term);
        let mut out = Vec::new();
        for (code, down) in changes {
            out.extend_from_slice(e.keys.encode(&KeyEvent {
                action: if down {
                    KeyAction::Press
                } else {
                    KeyAction::Release
                },
                key: Key::from_code(code),
                mods: vt_mods(now.0),
                ..KeyEvent::default()
            }));
        }
        if !out.is_empty() {
            self.session.update(cx, |s, _| s.input(out));
        }
    }

    /// 输入法交来的字（或者没开输入法时系统插入的字）。
    fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let Ok(e) = &mut self.engine else { return };
        e.keys.sync(&e.term);
        let encoded = e
            .keys
            .encode(&KeyEvent {
                action: KeyAction::Press,
                key: Key::Unidentified,
                text,
                ..KeyEvent::default()
            })
            .to_vec();
        let bytes = if encoded.is_empty() {
            text.as_bytes().to_vec()
        } else {
            encoded
        };
        self.engine_send(bytes, cx);
    }

    // ------------------------------------------------------------ 动作

    fn selection(&self) -> Option<Selection> {
        let d = self.drag?;
        let ((ax, ay), (hx, hy)) = (d.anchor, d.head);
        let (mut start, mut end) = if (ay, ax) <= (hy, hx) {
            ((ax, ay), (hx, hy))
        } else {
            ((hx, hy), (ax, ay))
        };
        match d.mode {
            SelMode::Char => {}
            SelMode::Line => {
                start.0 = 0;
                end.0 = self.layout.get().cols.saturating_sub(1);
            }
            SelMode::Word => {
                // 两头扩到词边界（同一行在视口里才扩得了）
                let l = self.layout.get();
                let top = self
                    .engine
                    .as_ref()
                    .map(|e| e.term.scrollbar().offset as u32)
                    .unwrap_or(0);
                if let Some(p) = self.row_chars(start.1.saturating_sub(top), l) {
                    start.0 = word_range(&p, start.0 as usize).0 as u16;
                }
                if let Some(p) = self.row_chars(end.1.saturating_sub(top), l) {
                    end.0 = word_range(&p, end.0 as usize).1 as u16;
                }
            }
        }
        Some(Selection {
            start: lolidesk_ghostty::Point {
                x: start.0,
                y: start.1,
            },
            end: lolidesk_ghostty::Point { x: end.0, y: end.1 },
            rectangle: d.rect,
        })
    }

    fn row_chars(&self, y: u32, l: Layout) -> Option<Vec<char>> {
        if y >= l.rows as u32 {
            return None;
        }
        self.prev.get(y as usize)?.as_ref().map(|p| p.chars.clone())
    }

    fn apply_selection(&mut self, cx: &mut Context<Self>) {
        let sel = self.selection();
        if let Ok(e) = &mut self.engine {
            self.has_selection = e.term.set_selection(sel) && sel.is_some();
        }
        cx.notify();
    }

    /// 选中的文字（没有选区是 None）。
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let e = self.engine.as_ref().ok()?;
        e.term.selection_text(&sel).filter(|t| !t.is_empty())
    }

    fn copy(&mut self, _: &TermCopy, _: &mut Window, cx: &mut Context<Self>) {
        // 没有选区时什么都不做（不发给程序）
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn paste(&mut self, _: &TermPaste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            return;
        };
        self.paste_text(&text, window, cx);
    }

    /// 粘贴：程序没开括号粘贴、文字里有换行（会自己跑起来）时先问一声。
    pub fn paste_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Ok(e) = &self.engine else { return };
        if text.is_empty() {
            return;
        }
        if !lolidesk_ghostty::paste_is_safe(text) && !e.term.mode(Mode::BRACKETED_PASTE) {
            let grid = cx.weak_entity();
            let text = text.to_string();
            let form = cx.new(|_| PasteConfirm { grid, text });
            dialogs::open_form(form, window, cx);
            return;
        }
        let bytes = lolidesk_ghostty::paste(&e.term, text);
        self.engine_send(bytes, cx);
    }

    fn paste_confirmed(&mut self, text: &str, cx: &mut Context<Self>) {
        let Ok(e) = &self.engine else { return };
        let bytes = lolidesk_ghostty::paste(&e.term, text);
        self.engine_send(bytes, cx);
    }

    fn clear(&mut self, _: &TermClear, _: &mut Window, cx: &mut Context<Self>) {
        self.session.update(cx, |s, _| s.clear(ClearKind::Screen));
    }

    fn bump(&mut self, delta: Option<f32>, cx: &mut Context<Self>) {
        if let Some(m) = self.model.upgrade() {
            m.update(cx, |m, cx| match delta {
                Some(d) => m.bump_font(d, cx),
                None => {
                    let d = FONT_DEFAULT - m.prefs.font_size;
                    m.bump_font(d, cx)
                }
            });
        }
        cx.notify();
    }

    fn select_all(&mut self, _: &TermSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        let Ok(e) = &self.engine else { return };
        let total = e.term.scrollbar().total.max(1) as u32;
        let cols = e.term.size().0;
        self.drag = Some(SelDrag {
            anchor: (0, 0),
            head: (cols.saturating_sub(1), total - 1),
            mode: SelMode::Char,
            rect: false,
        });
        self.apply_selection(cx);
    }

    // ------------------------------------------------------------ 鼠标

    /// 窗口坐标 → 视口格子（夹在终端里）。
    fn cell_at(&self, pos: Point<Pixels>) -> (u16, u16) {
        let l = self.layout.get();
        let x = (f32::from(pos.x - l.origin.x) / l.cell_w.max(1.0)).floor();
        let y = (f32::from(pos.y - l.origin.y) / l.cell_h.max(1.0)).floor();
        (
            x.clamp(0.0, l.cols.saturating_sub(1) as f32) as u16,
            y.clamp(0.0, l.rows.saturating_sub(1) as f32) as u16,
        )
    }

    fn mouse_tracking(&self, shift: bool) -> bool {
        !shift && self.engine.as_ref().is_ok_and(|e| e.term.mouse_tracking())
    }

    fn mouse_event(
        &mut self,
        action: MouseAction,
        button: Option<VtButton>,
        pos: Point<Pixels>,
        m: &gpui_kit::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let l = self.layout.get();
        let Ok(e) = &mut self.engine else { return };
        let size = MouseSize {
            screen_width: f32::from(l.bounds.size.width) as u32,
            screen_height: f32::from(l.bounds.size.height) as u32,
            cell_width: l.cell_w.round() as u32,
            cell_height: l.cell_h.round() as u32,
            padding_top: PAD_Y as u32,
            padding_bottom: PAD_Y as u32,
            padding_right: PAD_X as u32,
            padding_left: PAD_X as u32,
        };
        e.mouse.sync(&e.term, &size, self.mouse_down.is_some());
        let local = pos - l.bounds.origin;
        let bytes = e
            .mouse
            .encode(&MouseEvent {
                action,
                button,
                mods: vt_mods(keys::mods(m.shift, m.control, m.alt, m.platform).0),
                x: f32::from(local.x),
                y: f32::from(local.y),
            })
            .to_vec();
        if !bytes.is_empty() {
            self.session.update(cx, |s, _| s.input(bytes));
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            self.focus.focus(window, cx);
        } else {
            self.session.update(cx, |s, _| s.claim());
        }
        let (x, y) = self.cell_at(ev.position);
        // ⌘ 点链接：打开（Linux 上是 Ctrl 点）
        let link_click = if cfg!(target_os = "macos") {
            ev.modifiers.platform
        } else {
            ev.modifiers.control
        };
        if link_click && ev.button == GButton::Left {
            if let Some(url) = self.link_at(x, y) {
                cx.open_url(&url);
            }
            return;
        }
        let button = vt_button(ev.button);
        if self.mouse_tracking(ev.modifiers.shift) {
            if let Some(b) = button {
                self.mouse_down = Some(b);
                self.mouse_event(MouseAction::Press, Some(b), ev.position, &ev.modifiers, cx);
            }
            return;
        }
        if ev.button != GButton::Left {
            return;
        }
        let Ok(e) = &self.engine else { return };
        let p = e.term.viewport_to_screen(x, y);
        let mode = match ev.click_count {
            0 | 1 => SelMode::Char,
            2 => SelMode::Word,
            _ => SelMode::Line,
        };
        self.drag = Some(SelDrag {
            anchor: (p.x, p.y),
            head: (p.x, p.y),
            mode,
            rect: ev.modifiers.alt,
        });
        if mode == SelMode::Char {
            // 单击先不选（拖了才选）
            if let Ok(e) = &mut self.engine {
                e.term.set_selection(None);
            }
            self.has_selection = false;
            cx.notify();
        } else {
            self.apply_selection(cx);
        }
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.mouse_tracking(ev.modifiers.shift) {
            let button = ev.pressed_button.and_then(vt_button);
            self.mouse_event(MouseAction::Motion, button, ev.position, &ev.modifiers, cx);
            return;
        }
        if ev.pressed_button != Some(GButton::Left) {
            return;
        }
        let (x, y) = self.cell_at(ev.position);
        let Ok(e) = &self.engine else { return };
        let p = e.term.viewport_to_screen(x, y);
        if let Some(d) = &mut self.drag
            && d.head != (p.x, p.y)
        {
            d.head = (p.x, p.y);
            self.apply_selection(cx);
        }
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(b) = self.mouse_down.take()
            && self.mouse_tracking(ev.modifiers.shift)
        {
            self.mouse_event(
                MouseAction::Release,
                Some(b),
                ev.position,
                &ev.modifiers,
                cx,
            );
        }
    }

    fn link_at(&self, x: u16, y: u16) -> Option<String> {
        let e = self.engine.as_ref().ok()?;
        if let Some(url) = e.term.hyperlink_at(x, y) {
            return Some(url);
        }
        let chars = self.prev.get(y as usize)?.as_ref()?.chars.clone();
        url_at(&chars, x as usize)
    }

    fn scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let l = self.layout.get();
        let d = ev.delta.pixel_delta(px(l.cell_h.max(1.0)));
        let steps = wheel_steps(&mut self.wheel, f32::from(d.y), l.cell_h);
        if steps == 0 {
            return;
        }
        cx.stop_propagation();
        if self.mouse_tracking(ev.modifiers.shift) {
            let b = if steps > 0 {
                VtButton::WheelUp
            } else {
                VtButton::WheelDown
            };
            for _ in 0..steps.unsigned_abs().min(10) {
                self.mouse_event(MouseAction::Press, Some(b), ev.position, &ev.modifiers, cx);
            }
            return;
        }
        let Ok(e) = &mut self.engine else { return };
        if e.term.alt_screen() {
            // 备用屏：滚轮换成方向键（less、vim 能滚）
            let key = if steps > 0 {
                Key::ArrowUp
            } else {
                Key::ArrowDown
            };
            e.keys.sync(&e.term);
            let mut out = Vec::new();
            for _ in 0..steps.unsigned_abs().min(10) {
                out.extend_from_slice(e.keys.encode(&KeyEvent {
                    action: KeyAction::Press,
                    key,
                    ..KeyEvent::default()
                }));
            }
            if !out.is_empty() {
                self.session.update(cx, |s, _| s.input(out));
            }
            return;
        }
        // 主屏：滚本地的视口（回滚都在帧里）。滚轮往上（内容往下走，正值）是往回看
        e.term.scroll_viewport(Scroll::Delta(-(steps as isize)));
        self.scrolled_at = Some(Instant::now());
        cx.notify();
    }
}

/// 这次按键的物理键位：先读 NSEvent 的键码，读不到按键名猜。
fn physical_code(key: &str) -> Option<&'static str> {
    keys::current_key_code()
        .and_then(keys::vk_to_code)
        .or_else(|| keys::key_name_to_code(key))
}

fn single_char(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// 打字的键（字母、数字、标点）：没打出字时是死键，交给输入法。
fn is_writing_key(k: Key) -> bool {
    let code = k.code();
    code.starts_with("Key")
        || code.starts_with("Digit")
        || matches!(
            code,
            "Backquote"
                | "Backslash"
                | "BracketLeft"
                | "BracketRight"
                | "Comma"
                | "Equal"
                | "IntlBackslash"
                | "IntlRo"
                | "IntlYen"
                | "Minus"
                | "Period"
                | "Quote"
                | "Semicolon"
                | "Slash"
        )
}

fn vt_mods(bits: u8) -> VtMods {
    use lolidesk_proto::Mods as P;
    let mut m = VtMods::NONE;
    if bits & P::SHIFT != 0 {
        m |= VtMods::SHIFT;
    }
    if bits & P::CTRL != 0 {
        m |= VtMods::CTRL;
    }
    if bits & P::ALT != 0 {
        m |= VtMods::ALT;
    }
    if bits & P::SUPER != 0 {
        m |= VtMods::SUPER;
    }
    m
}

fn vt_button(b: GButton) -> Option<VtButton> {
    match b {
        GButton::Left => Some(VtButton::Left),
        GButton::Right => Some(VtButton::Right),
        GButton::Middle => Some(VtButton::Middle),
        _ => None,
    }
}

// ---------------------------------------------------------------- 排一行

/// 一行读出来的样子，带内容哈希。字在 text 里，items 是（列、宽、字体、颜色、text 里的范围）。
struct BuiltRow {
    hash: u64,
    text: String,
    items: Vec<(u16, u16, u8, Hsla, Range<u32>)>,
    bgs: Vec<(u16, u16, Hsla)>,
    decos: Vec<(u16, u16, Deco, Hsla)>,
    chars: Vec<char>,
    bold: Vec<bool>,
    boxes: Vec<(u16, u16, char, Hsla)>,
}

/// FNV-1a，够快，只用来认出同样的行。
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
}

fn build_row<'a>(
    cells: impl Iterator<Item = (&'a VtCell, &'a str)>,
    _palette: &[Rgb; 256],
    m: &Metrics,
) -> BuiltRow {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    m.font_size.to_bits().hash(&mut h);
    let mut text = String::new();
    let mut items = Vec::new();
    let mut bgs: Vec<(u16, u16, Hsla)> = Vec::new();
    let mut decos: Vec<(u16, u16, Deco, Hsla)> = Vec::new();
    let mut chars = Vec::new();
    let mut bold = Vec::new();
    let mut boxes = Vec::new();
    for (cell, t) in cells {
        let col = cell.x;
        match cell.width {
            Width::SpacerTail => {
                // 宽字符的后半格：算前一格的
                chars.push('\0');
                bold.push(false);
                0u8.hash(&mut h);
                continue;
            }
            Width::SpacerHead => {
                chars.push(' ');
                bold.push(false);
                1u8.hash(&mut h);
                continue;
            }
            _ => {}
        }
        let w: u16 = if cell.width == Width::Wide { 2 } else { 1 };
        let glyph = if cell.invisible || t.is_empty() {
            " "
        } else {
            t
        };
        let first = glyph.chars().next().unwrap_or(' ');
        chars.push(first);
        bold.push(cell.bold);
        let font = u8::from(cell.bold) | (u8::from(cell.italic) << 1);
        let fg = rgb_hsla(cell.fg, if cell.faint { 0.55 } else { 1.0 });
        glyph.hash(&mut h);
        (col, cell.fg.r, cell.fg.g, cell.fg.b, cell.faint, font).hash(&mut h);
        if glyph.len() <= 3 && boxdraw::is_drawn(first) {
            // 框线、方块自己画
            boxes.push((col, w, first, fg));
        } else if glyph != " " {
            let start = text.len() as u32;
            text.push_str(glyph);
            items.push((col, w, font, fg, start..text.len() as u32));
        }
        if let Some(bg) = cell.bg {
            let c = rgb_hsla(bg, 1.0);
            (bg.r, bg.g, bg.b, col).hash(&mut h);
            match bgs.last_mut() {
                Some((_, end, last)) if *end == col && *last == c => *end = col + w,
                _ => bgs.push((col, col + w, c)),
            }
        }
        let mut deco = |d: Deco, color: Hsla| {
            (d, col, w).hash(&mut h);
            match decos.last_mut() {
                Some((_, end, k, c)) if *end == col && *k == d && *c == color => *end = col + w,
                _ => decos.push((col, col + w, d, color)),
            }
        };
        let line_color = cell.underline_color.map(|c| rgb_hsla(c, 1.0)).unwrap_or(fg);
        match cell.underline {
            Underline::None => {}
            Underline::Single => deco(Deco::Underline, line_color),
            Underline::Dashed => deco(Deco::Dashed, line_color),
            Underline::Double => deco(Deco::Double, line_color),
            Underline::Curly => deco(Deco::Curly, line_color),
            Underline::Dotted => deco(Deco::Dotted, line_color),
        }
        if cell.strikethrough {
            deco(Deco::Strike, fg);
        }
        if cell.overline {
            deco(Deco::Overline, fg);
        }
    }
    BuiltRow {
        hash: h.finish(),
        text,
        items,
        bgs,
        decos,
        chars,
        bold,
        boxes,
    }
}

impl BuiltRow {
    /// 每个字找到字形（没见过的字素排一次）；返回行和新排了几个字素。
    fn resolve(self, glyphs: &mut Glyphs, window: &mut Window, m: &Metrics) -> (RowPaint, usize) {
        let mut shaped = 0;
        let out = self
            .items
            .iter()
            .map(|(col, w, font, fg, r)| {
                let t = &self.text[r.start as usize..r.end as usize];
                let (g, new) = glyphs.get(t, *font, m, window);
                shaped += usize::from(new);
                // 字形比格子窄（宽字符、回退字体）就居中
                let room = *w as f32 * m.cell_w;
                let x = *col as f32 * m.cell_w + ((room - g.width) / 2.0).max(0.0);
                (x, *fg, g.glyphs)
            })
            .collect();
        (
            RowPaint {
                glyphs: out,
                bgs: self.bgs,
                decos: self.decos,
                chars: self.chars,
                bold: self.bold,
                boxes: self.boxes,
            },
            shaped,
        )
    }
}

// ---------------------------------------------------------------- 画一帧

impl PaintFrame {
    fn cell(&self, x: u16, y: u16) -> Point<Pixels> {
        self.origin + point(px(x as f32 * self.cell_w), px(y as f32 * self.cell_h))
    }

    fn span(&self, x0: u16, x1: u16, y: u16) -> Bounds<Pixels> {
        Bounds::new(
            self.cell(x0, y),
            size(px((x1 - x0) as f32 * self.cell_w), px(self.cell_h)),
        )
    }

    fn paint(self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        // 终端的背景（程序可能用 OSC 11 改过），画满整个格子区
        let area = Bounds::new(
            self.origin - point(px(PAD_X), px(PAD_Y)),
            size(
                px(self.cols as f32 * self.cell_w + 2.0 * PAD_X),
                px(self.rows_n as f32 * self.cell_h + 2.0 * PAD_Y),
            ),
        );
        window.paint_quad(fill(area, self.bg));
        // 格子里的东西放在一层里：同一层的图元共用一个绘制顺序，不用一个个插进 GPUI 的边界树
        // （一屏一万多个字形，一个个插要上百毫秒）。层里先画矩形再画字形，底色、选区在字下面。
        window.paint_layer(area, |window| self.paint_cells(window));
        // 光标、输入法、滚动条、响铃在格子上面
        self.paint_overlays(bounds, window, cx);
    }

    fn paint_cells(&self, window: &mut Window) {
        for (y, row) in &self.rows {
            for (x0, x1, c) in &row.bgs {
                window.paint_quad(fill(self.span(*x0, *x1, *y), *c));
            }
        }
        for (y, a, b) in &self.sel {
            window.paint_quad(fill(self.span(*a, b + 1, *y), self.sel_color));
        }
        let font_size = px(self.font_size);
        for (y, row) in &self.rows {
            let base = self.cell(0, *y) + point(px(0.), px(self.baseline));
            for (x, color, glyphs) in &row.glyphs {
                paint_glyphs(
                    base + point(px(*x), px(0.)),
                    glyphs,
                    *color,
                    font_size,
                    window,
                );
            }
        }
        // 线的粗细：字号的 7.5%，至少一个设备像素
        let thick = boxdraw::snap((self.font_size * 0.075).max(1.0 / self.scale), self.scale)
            .max(1.0 / self.scale);
        // 框线、方块
        for (y, row) in &self.rows {
            for (x, w, ch, c) in &row.boxes {
                boxdraw::paint(*ch, self.span(*x, x + w, *y), *c, thick, self.scale, window);
            }
        }
        // 下划线、删除线、上划线
        let snap = |v: Pixels| px(boxdraw::snap(f32::from(v), self.scale));
        for (y, row) in &self.rows {
            for (x0, x1, d, c) in &row.decos {
                let b = self.span(*x0, *x1, *y);
                let base = snap(b.origin.y + b.size.height - px(thick * 2.0));
                let (left, w) = (b.origin.x, b.size.width);
                let line = |y: Pixels| Bounds::new(point(left, y), size(w, px(thick)));
                match d {
                    Deco::Underline => window.paint_quad(fill(line(base), *c)),
                    Deco::Double => {
                        window.paint_quad(fill(line(base - px(thick * 2.0)), *c));
                        window.paint_quad(fill(line(base), *c));
                    }
                    Deco::Curly => boxdraw::curly(
                        f32::from(left),
                        f32::from(left + w),
                        f32::from(base) - thick / 2.0,
                        thick.max(1.0),
                        thick,
                        *c,
                        window,
                    ),
                    Deco::Dotted | Deco::Dashed => {
                        let (on, gap) = if *d == Deco::Dotted {
                            (thick, thick)
                        } else {
                            (thick * 3.0, thick * 2.0)
                        };
                        let mut x = f32::from(left);
                        let end = f32::from(left + w);
                        while x < end {
                            let r = Bounds::new(
                                point(px(x), base),
                                size(px(on.min(end - x)), px(thick)),
                            );
                            window.paint_quad(fill(r, *c));
                            x += on + gap;
                        }
                    }
                    Deco::Strike => {
                        window.paint_quad(fill(line(snap(b.origin.y + b.size.height * 0.5)), *c))
                    }
                    Deco::Overline => window.paint_quad(fill(line(b.origin.y), *c)),
                }
            }
        }
    }

    fn paint_overlays(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        // 光标
        if let Some(c) = &self.cursor {
            let w = if c.wide { 2 } else { 1 };
            let b = self.span(c.x, c.x + w, c.y);
            match c.shape {
                CursorShape::Block => {
                    window.paint_quad(fill(b, c.color));
                    if let Some((x, glyphs)) = &c.text {
                        let o = b.origin + point(px(*x), px(self.baseline));
                        paint_glyphs(o, glyphs, theme::eye_ink(), px(self.font_size), window);
                    }
                }
                CursorShape::Bar => window.paint_quad(fill(
                    Bounds::new(b.origin, size(px(2.), b.size.height)),
                    c.color,
                )),
                CursorShape::Underline => window.paint_quad(fill(
                    Bounds::new(
                        point(b.origin.x, b.origin.y + b.size.height - px(2.)),
                        size(b.size.width, px(2.)),
                    ),
                    c.color,
                )),
                CursorShape::Hollow => {
                    window.paint_quad(outline(b, c.color, gpui_kit::BorderStyle::Solid))
                }
            }
        }
        // 输入法组的字：盖住格子、下面一道线、后面一个竖线光标
        if let Some((line, x, y, w)) = &self.marked {
            let o = self.cell(*x, *y);
            let b = Bounds::new(o, size(px(*w), px(self.cell_h)));
            window.paint_quad(fill(b, self.bg));
            let _ = line.paint(o, px(self.cell_h), TextAlign::Left, None, window, cx);
            window.paint_quad(fill(
                Bounds::new(
                    point(o.x, o.y + px(self.cell_h - 2.0)),
                    size(px(*w), px(1.)),
                ),
                theme::eye(),
            ));
            window.paint_quad(fill(
                Bounds::new(point(o.x + px(*w), o.y), size(px(2.), px(self.cell_h))),
                theme::eye(),
            ));
        }
        if let Some((b, c)) = self.scrollbar {
            window.paint_quad(fill(b, c).corner_radii(px(1.5)));
        }
        if self.bell > 0.0 {
            let mut wash = theme::eye();
            wash.a = 0.07 * self.bell;
            window.paint_quad(fill(bounds, wash));
            let mut line = theme::eye_line();
            line.a *= self.bell;
            window.paint_quad(outline(bounds, line, gpui_kit::BorderStyle::Solid));
        }
        if self.animate {
            window.request_animation_frame();
        }
    }
}

/// 画一个字素的字形；emoji 是彩色的，不上色。
fn paint_glyphs(
    origin: Point<Pixels>,
    glyphs: &[GlyphRef],
    color: Hsla,
    font_size: Pixels,
    window: &mut Window,
) {
    for g in glyphs {
        let o = origin + point(px(g.x), px(g.y));
        let _ = if g.emoji {
            window.paint_emoji(o, g.font_id, g.id, font_size)
        } else {
            window.paint_glyph(o, g.font_id, g.id, font_size, color)
        };
    }
}

/// 画格子的元素：prepaint 时让 TermGrid 读终端、排字，paint 时画，再把输入法接到这里。
struct GridElement {
    grid: Entity<TermGrid>,
}

impl IntoElement for GridElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for GridElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<PaintFrame>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let style = Style {
            size: size(relative(1.).into(), relative(1.).into()),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<PaintFrame> {
        self.grid.update(cx, |g, cx| g.prepare(bounds, window, cx))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        frame: &mut Option<PaintFrame>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(frame) = frame.take() {
            window.with_content_mask(Some(gpui_kit::ContentMask { bounds }), |window| {
                frame.paint(bounds, window, cx)
            });
        }
        let focus = self.grid.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.grid.clone()),
            cx,
        );
    }
}

impl Render for TermGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.focused_subs(window, cx);
        let me = cx.entity();
        div()
            .id("term-grid")
            .key_context(GRID_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .cursor(MouseCursor::IBeam)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::clear))
            .on_action(cx.listener(|g, _: &FontBigger, _, cx| g.bump(Some(1.0), cx)))
            .on_action(cx.listener(|g, _: &FontSmaller, _, cx| g.bump(Some(-1.0), cx)))
            .on_action(cx.listener(|g, _: &FontReset, _, cx| g.bump(None, cx)))
            .on_action(cx.listener(Self::select_all))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_any_mouse_down(cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(GButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(GButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up(GButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(GButton::Left, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::scroll))
            .child(GridElement { grid: me })
    }
}

// ---------------------------------------------------------------- 输入法

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn utf16_to_byte(s: &str, u: usize) -> usize {
    let mut n = 0;
    for (i, c) in s.char_indices() {
        if n >= u {
            return i;
        }
        n += c.len_utf16();
    }
    s.len()
}

impl EntityInputHandler for TermGrid {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.marked.as_deref()?;
        let len = utf16_len(text);
        let r = range.start.min(len)..range.end.min(len);
        *adjusted = Some(r.clone());
        Some(text[utf16_to_byte(text, r.start)..utf16_to_byte(text, r.end)].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: if self.marked.is_some() {
                self.marked_sel.clone()
            } else {
                0..0
            },
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let text = self.marked.as_deref()?;
        let len = utf16_len(text);
        (len > 0).then_some(0..len)
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        self.commit_text(text, cx);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.is_empty() {
            self.marked = None;
        } else {
            let len = utf16_len(text);
            self.marked_sel = selected.unwrap_or(len..len);
            self.marked = Some(text.to_string());
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let l = self.layout.get();
        let (x, y) = l.cursor?;
        // 组字里的偏移按字符数算一格（够让候选窗跟着走）
        let offset = self
            .marked
            .as_deref()
            .map(|t| t[..utf16_to_byte(t, range.start)].chars().count())
            .unwrap_or(0);
        Some(Bounds::new(
            l.origin
                + point(
                    px((x as usize + offset) as f32 * l.cell_w),
                    px(y as f32 * l.cell_h),
                ),
            size(px(l.cell_w), px(l.cell_h)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

// ---------------------------------------------------------------- 粘贴前问一声

/// 「粘贴多行文字？」：程序没开括号粘贴，文字里的换行会让命令自己跑起来。
pub struct PasteConfirm {
    grid: WeakEntity<TermGrid>,
    text: String,
}

impl Form for PasteConfirm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = std::mem::take(&mut self.text);
        if let Some(g) = self.grid.upgrade() {
            g.update(cx, |g, cx| g.paste_confirmed(&text, cx));
        }
        gpui_kit::component::WindowExt::close_dialog(window, cx);
    }
}

impl Render for PasteConfirm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::theme::*;
        let lines = self.text.lines().count();
        let preview: String = self
            .text
            .lines()
            .take(6)
            .map(|l| {
                let l: String = l.chars().take(80).collect();
                format!("{l}\n")
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(T_LG)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("粘贴多行文字？"),
                    )
                    .child(div().text_size(T_XS).text_color(fg2()).child(format!(
                        "有 {lines} 行。程序没开括号粘贴，每个换行都会像按了回车一样马上执行。"
                    ))),
            )
            .child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(R_SM)
                    .bg(ink1())
                    .border_1()
                    .border_color(line2())
                    .font_family(MONO)
                    .text_size(T_XS)
                    .text_color(fg2())
                    .max_h(px(140.))
                    .overflow_hidden()
                    .child(preview.trim_end().to_string()),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        crate::ui::widgets::pill("paste-cancel", "取消", false).on_click(
                            |_, window, cx| {
                                gpui_kit::component::WindowExt::close_dialog(window, cx)
                            },
                        ),
                    )
                    .child(
                        crate::ui::widgets::pill("paste-ok", "粘贴", true)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    ),
            )
    }
}
