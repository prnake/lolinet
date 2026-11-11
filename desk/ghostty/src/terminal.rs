//! 终端：libghostty-vt 的 GhosttyTerminal。程序的输出改它的屏幕、回滚、光标和模式。
//!
//! 回调怎么接、ClearScreen / ClearScrollback、主题照 mygo plugins/terminal/terminal.go、
//! theme.go 和 internal/vt/terminal.go 移植（MIT，Copyright (c) 2026 MyGo contributors）。
//! 没接的：OSC 52 剪贴板（程序写、读剪贴板都不转给谁，读由库自己拒掉）、
//! Kitty 图片（图片上限是 0，程序会退回别的画法）。

use std::{
    ffi::c_void,
    mem::size_of,
    panic::AssertUnwindSafe,
    ptr,
    time::{Duration, Instant},
};

use anyhow::Result;

use crate::{
    ffi::{self, api, Handle, Rgb, Scrollbar},
    render::CursorStyle,
};

/// 终端的回调。都在 [`Terminal::write`]（`write_pty`、`notify`、`bell`、`progress` 还可能在
/// [`Terminal::resize`]、[`Terminal::reset`]）里、同一个线程上同步调；不能再回头调这个终端。
pub trait Handler: Send {
    /// 回给程序的字节：设备属性、光标位置、尺寸、颜色这些查询的回答。只在这次调用里有效。
    fn write_pty(&mut self, _bytes: &[u8]) {}
    /// 程序改了标题（OSC 0 / 2）。在 write 处理完这一段输出之后调。
    fn title(&mut self, _title: &str) {}
    /// shell 报了工作目录（OSC 7 是 file:// URL，OSC 9 / 1337 是路径），原样给。
    fn pwd(&mut self, _pwd: &str) {}
    /// 响铃（BEL）。
    fn bell(&mut self) {}
    /// 桌面通知（OSC 9、OSC 777）。
    fn notify(&mut self, _title: &str, _body: &str) {}
    /// 进度（OSC 9;4）：state 0 去掉、1 进度、2 出错、3 不定、4 暂停；percent 没给就是 None。
    fn progress(&mut self, _state: u8, _percent: Option<u8>) {}
}

/// 什么都不做的回调，客户端那边的终端用（查询由主机的终端回答）。
impl Handler for () {}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Rgb {
        Rgb { r, g, b }
    }

    /// 0xRRGGBB。
    pub const fn hex(v: u32) -> Rgb {
        Rgb {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        }
    }
}

/// 终端的配色：程序用 OSC 10/11/12/4 问到的默认颜色，也可以改（重置回到这里）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    /// 光标颜色；None 用文字的颜色。
    pub cursor: Option<Rgb>,
    /// 16 色：黑红绿黄蓝品青白，再是亮的。
    pub palette: [Rgb; 16],
}

impl Theme {
    /// 深色：VS Code 的调色板。
    pub fn dark() -> Theme {
        Theme {
            foreground: Rgb::hex(0xcccccc),
            background: Rgb::hex(0x1e1e1e),
            cursor: None,
            palette: [
                0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5,
                0x666666, 0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
            ]
            .map(Rgb::hex),
        }
    }

    /// 浅色：VS Code 的调色板。
    pub fn light() -> Theme {
        Theme {
            foreground: Rgb::hex(0x333333),
            background: Rgb::hex(0xffffff),
            cursor: None,
            palette: [
                0x000000, 0xcd3131, 0x00bc00, 0x949800, 0x0451a5, 0xbc05bc, 0x0598bc, 0x555555,
                0x666666, 0xcd3131, 0x14ce14, 0xb5ba00, 0x0451a5, 0xbc05bc, 0x0598bc, 0xa5a5a5,
            ]
            .map(Rgb::hex),
        }
    }

    /// 背景是不是深色。
    pub fn is_dark(&self) -> bool {
        let b = self.background;
        let luma = 299 * u32::from(b.r) + 587 * u32::from(b.g) + 114 * u32::from(b.b);
        luma < 128 * 1000
    }

    /// 256 色：自己的 16 色、xterm 的 6×6×6 立方和 24 级灰。
    pub fn palette256(&self) -> [Rgb; 256] {
        let mut p = [Rgb::default(); 256];
        p[..16].copy_from_slice(&self.palette);
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        for i in 0..216 {
            p[16 + i] = Rgb::new(LEVELS[i / 36], LEVELS[i / 6 % 6], LEVELS[i % 6]);
        }
        for i in 0..24 {
            let v = 8 + 10 * i as u8;
            p[232 + i] = Rgb::new(v, v, v);
        }
        p
    }
}

/// 一个终端模式（GhosttyMode）：DEC 私有模式的号，ANSI 模式另加第 15 位。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Mode(pub u16);

impl Mode {
    /// DEC 私有模式 `CSI ? n h`。
    pub const fn dec(n: u16) -> Mode {
        Mode(n & 0x7fff)
    }
    /// ANSI 模式 `CSI n h`。
    pub const fn ansi(n: u16) -> Mode {
        Mode((n & 0x7fff) | 0x8000)
    }

    /// 应用光标键（DECCKM）。
    pub const CURSOR_KEYS: Mode = Mode::dec(1);
    /// 整屏反色（DECSCNM）。
    pub const REVERSE_COLORS: Mode = Mode::dec(5);
    /// 自动换行（DECAWM）。
    pub const WRAPAROUND: Mode = Mode::dec(7);
    /// X10 鼠标。
    pub const MOUSE_X10: Mode = Mode::dec(9);
    /// 光标可见（DECTCEM）。
    pub const CURSOR_VISIBLE: Mode = Mode::dec(25);
    /// 应用小键盘（DECNKM）。
    pub const KEYPAD_KEYS: Mode = Mode::dec(66);
    /// 退格发 0x08（DECBKM）。
    pub const BACKARROW_KEY: Mode = Mode::dec(67);
    /// 鼠标：按下和松开。
    pub const MOUSE_NORMAL: Mode = Mode::dec(1000);
    /// 鼠标：按着拖动也报。
    pub const MOUSE_BUTTON: Mode = Mode::dec(1002);
    /// 鼠标：什么移动都报。
    pub const MOUSE_ANY: Mode = Mode::dec(1003);
    /// 焦点进出报给程序。
    pub const FOCUS_EVENT: Mode = Mode::dec(1004);
    /// 鼠标坐标用 UTF-8。
    pub const MOUSE_UTF8: Mode = Mode::dec(1005);
    /// 鼠标用 SGR 格式。
    pub const MOUSE_SGR: Mode = Mode::dec(1006);
    /// 备用屏上滚轮变方向键。
    pub const ALT_SCROLL: Mode = Mode::dec(1007);
    /// 鼠标用 URxvt 格式。
    pub const MOUSE_URXVT: Mode = Mode::dec(1015);
    /// 鼠标用 SGR 像素坐标。
    pub const MOUSE_SGR_PIXELS: Mode = Mode::dec(1016);
    /// 有 NumLock 时小键盘不算应用模式。
    pub const NUMLOCK_KEYPAD: Mode = Mode::dec(1035);
    /// Alt 前缀 ESC。
    pub const ALT_ESC_PREFIX: Mode = Mode::dec(1036);
    /// Alt 发 ESC。
    pub const ALT_SENDS_ESC: Mode = Mode::dec(1039);
    /// 备用屏（存光标、清屏）。
    pub const ALT_SCREEN: Mode = Mode::dec(1049);
    /// 括号粘贴。
    pub const BRACKETED_PASTE: Mode = Mode::dec(2004);
    /// 同步输出：程序画完一帧之前别显示。
    pub const SYNC_OUTPUT: Mode = Mode::dec(2026);
    /// 字素簇按宽度占格。
    pub const GRAPHEMES: Mode = Mode::dec(2027);
    /// 深浅色变了告诉程序。
    pub const COLOR_SCHEME: Mode = Mode::dec(2031);
    /// 尺寸变了带内告诉程序。
    pub const IN_BAND_RESIZE: Mode = Mode::dec(2048);
}

/// 视口滚到哪。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scroll {
    /// 滚几行，负的往上。
    Delta(isize),
    /// 回滚最上面。
    Top,
    /// 最下面（跟着屏幕）。
    Bottom,
    /// 让第几行（从回滚最上面数，和 [`Scrollbar::offset`] 一样）成为视口第一行。
    Row(usize),
}

/// 屏幕坐标里的一个格子：y 从回滚最上面数（备用屏上就是屏幕第几行）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point {
    pub x: u16,
    pub y: u32,
}

/// 选区：两个格子（含）；rectangle 是按矩形选。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: Point,
    pub end: Point,
    pub rectangle: bool,
}

/// 程序看到的颜色：OSC 设的盖在默认的上面；没有的是 None。
pub(crate) struct TermColors {
    pub foreground: Option<Rgb>,
    pub background: Option<Rgb>,
    pub cursor: Option<Rgb>,
    pub palette: [Rgb; 256],
}

/// 同步输出最多扣住帧多久。
pub(crate) const HOLD_MAX: Duration = Duration::from_secs(1);

/// 回调用到的状态，地址交给库当 userdata。
struct State {
    handler: Box<dyn Handler>,
    cols: u16,
    rows: u16,
    cell_w: u32,
    cell_h: u32,
    /// 配色是不是深色；没设过主题算深色（同 mygo）。
    dark: Option<bool>,
    /// 程序开始同步输出（模式 2026）的时候。
    held: Option<Instant>,
    title_changed: bool,
    pwd_changed: bool,
    /// 程序重置光标（DECSCUSR 0）回到的样子。
    cursor: (CursorStyle, bool),
}

/// 一个终端。可以换线程（Send），不能两边同时用（不是 Sync）：`&self` 的方法也会动库里的
/// 状态（视口、脏标记）。
pub struct Terminal {
    h: Handle,
    state: *mut State,
}

// SAFETY：句柄和 State 只经 `&self` / `&mut self` 用，Terminal 不是 Sync，同一时刻只在一个
// 线程上；Handler 是 Send；库不把终端绑在线程上。
unsafe impl Send for Terminal {}

impl Terminal {
    /// 新建一个 cols × rows 的终端（会先载入库）。默认：回滚 10 MiB、图片上限 0、
    /// 字素簇模式开、默认光标是闪烁的块、terminfo 名 xterm-256color、格子 8×16 像素。
    pub fn new(cols: u16, rows: u16, handler: Box<dyn Handler>) -> Result<Terminal> {
        let lib = ffi::lib_checked()?;
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut h: Handle = ptr::null_mut();
        // SAFETY：h 是出参，分配器传空用默认的。
        ffi::check(
            unsafe { (lib.api.terminal_new)(ptr::null(), &mut h, cols, rows) },
            "新建终端",
        )?;
        let state = Box::into_raw(Box::new(State {
            handler,
            cols,
            rows,
            cell_w: 8,
            cell_h: 16,
            dark: None,
            held: None,
            title_changed: false,
            pwd_changed: false,
            cursor: (CursorStyle::Block, true),
        }));
        let mut t = Terminal { h, state };
        type Cb = *const c_void;
        t.set_value(ffi::opt::USERDATA, state as Cb);
        t.set_value(ffi::opt::WRITE_PTY, cb_write_pty as Cb);
        t.set_value(ffi::opt::BELL, cb_bell as Cb);
        t.set_value(ffi::opt::TITLE_CHANGED, cb_title_changed as Cb);
        t.set_value(ffi::opt::PWD_CHANGED, cb_pwd_changed as Cb);
        t.set_value(ffi::opt::RENDER_HOLD, cb_render_hold as Cb);
        t.set_value(ffi::opt::SIZE, cb_size as Cb);
        t.set_value(ffi::opt::COLOR_SCHEME, cb_color_scheme as Cb);
        t.set_value(ffi::opt::DEVICE_ATTRIBUTES, cb_device_attributes as Cb);
        t.set_value(
            ffi::opt::DESKTOP_NOTIFICATION,
            cb_desktop_notification as Cb,
        );
        t.set_value(ffi::opt::PROGRESS_REPORT, cb_progress_report as Cb);
        // 程序问 XTGETTCAP TN 时报的名字。
        static TERMINFO: &[u8] = b"xterm-256color";
        let name = ffi::GString {
            ptr: TERMINFO.as_ptr(),
            len: TERMINFO.len(),
        };
        t.set(ffi::opt::TERMINFO_NAME, &name);
        // Kitty 图片一律拒掉，程序退回别的画法（Mirror 不传图片）。
        t.set(ffi::opt::KITTY_IMAGE_STORAGE_LIMIT, &0u64);
        // 字素簇按宽度占格，同 Ghostty：带修饰的、连起来的 emoji 占两格。
        t.set(
            ffi::opt::MODE_DEFAULT,
            &ffi::ModeConfig {
                mode: Mode::GRAPHEMES.0,
                value: true,
            },
        );
        t.set_scrollback_limit(10 << 20);
        t.set_default_cursor(CursorStyle::Block, true);
        t.resize(cols, rows, 8, 16);
        Ok(t)
    }

    pub(crate) fn handle(&self) -> Handle {
        self.h
    }

    fn st(&self) -> &State {
        // SAFETY：state 来自 Box::into_raw，Drop 之前一直有效；回调只在 &mut self 的调用里跑。
        unsafe { &*self.state }
    }

    fn st_mut(&mut self) -> &mut State {
        // SAFETY：同上，&mut self 保证没有别的引用。
        unsafe { &mut *self.state }
    }

    /// 设一个值本身就是指针的选项（回调、userdata）。
    fn set_value(&mut self, opt: i32, value: *const c_void) {
        // SAFETY：h 有效；值的类型就是这个选项要的。
        unsafe { (api().terminal_set)(self.h, opt, value) };
    }

    /// 设一个选项为 value 指着的值；库在调用里拷走。
    fn set<T>(&self, opt: i32, value: &T) -> bool {
        // SAFETY：h 有效；T 是这个选项要的类型（调用处保证），调用期间 value 有效。
        unsafe { (api().terminal_set)(self.h, opt, value as *const T as *const c_void) == 0 }
    }

    /// 读一项数据到 out。
    pub(crate) fn get<T>(&self, data: i32, out: &mut T) -> bool {
        // SAFETY：h 有效；T 是这项数据的输出类型（调用处保证）。
        unsafe { (api().terminal_get)(self.h, data, out as *mut T as *mut c_void) == 0 }
    }

    /// 喂程序的输出（字节流，转义序列可以断在任何地方）。
    pub fn write(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        // SAFETY：h 有效；回调经 userdata 拿 State，此时没有别的引用指着它。
        unsafe { (api().terminal_vt_write)(self.h, data.as_ptr(), data.len()) };
        // 标题和目录在回调返回后才能读，处理完这段再告诉 Handler。
        if std::mem::take(&mut self.st_mut().title_changed) {
            let title = self.title();
            guard(|| self.st_mut().handler.title(&title));
        }
        if std::mem::take(&mut self.st_mut().pwd_changed) {
            let pwd = self.pwd();
            guard(|| self.st_mut().handler.pwd(&pwd));
        }
    }

    /// 改尺寸：格子数，和一个格子的像素（程序问像素尺寸时报，SGR 像素鼠标按它算）。
    /// 主屏会重新排版；同步输出会结束。
    pub fn resize(&mut self, cols: u16, rows: u16, cell_w_px: u32, cell_h_px: u32) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let (cw, ch) = (cell_w_px.max(1), cell_h_px.max(1));
        let st = self.st_mut();
        st.cols = cols;
        st.rows = rows;
        st.cell_w = cw;
        st.cell_h = ch;
        // SAFETY：h 有效。
        let r = unsafe { (api().terminal_resize)(self.h, cols, rows, cw, ch) };
        if let Err(e) = ffi::check(r, "改终端尺寸") {
            tracing::warn!("{e:#}");
        }
    }

    /// 屏幕几列几行。
    pub fn size(&self) -> (u16, u16) {
        let (mut c, mut r) = (0u16, 0u16);
        self.get(ffi::data::COLS, &mut c);
        self.get(ffi::data::ROWS, &mut r);
        (c, r)
    }

    /// 光标在当前屏幕的第几列、第几行。
    pub fn cursor(&self) -> (u16, u16) {
        let (mut x, mut y) = (0u16, 0u16);
        self.get(ffi::data::CURSOR_X, &mut x);
        self.get(ffi::data::CURSOR_Y, &mut y);
        (x, y)
    }

    /// 全部重置（RIS）。
    pub fn reset(&mut self) {
        // SAFETY：h 有效。
        unsafe { (api().terminal_reset)(self.h) };
    }

    /// 回滚最多占多少字节（大约，按页算）；0 是不要回滚。
    pub fn set_scrollback_limit(&mut self, bytes: usize) {
        self.set(ffi::opt::SCROLLBACK_MAX_BYTES, &bytes);
    }

    /// 程序重置光标（DECSCUSR 0）回到的样子；Mirror 也按它判断光标是不是默认的。
    pub fn set_default_cursor(&mut self, style: CursorStyle, blink: bool) {
        let s = style.to_ffi();
        self.set(ffi::opt::DEFAULT_CURSOR_STYLE, &s);
        self.set(ffi::opt::DEFAULT_CURSOR_BLINK, &blink);
        self.st_mut().cursor = (style, blink);
    }

    pub(crate) fn default_cursor(&self) -> (CursorStyle, bool) {
        self.st().cursor
    }

    fn string(&self, data: i32) -> String {
        let mut s = ffi::GString::empty();
        if !self.get(data, &mut s) {
            return String::new();
        }
        // SAFETY：库借出的字符串在终端下次变之前有效，这里马上拷走。
        unsafe { s.to_string_lossy() }
    }

    /// 程序设的标题（OSC 0 / 2）。
    pub fn title(&self) -> String {
        self.string(ffi::data::TITLE)
    }

    /// shell 报的工作目录，原样（OSC 7 是 file:// URL）。
    pub fn pwd(&self) -> String {
        self.string(ffi::data::PWD)
    }

    /// 是不是在备用屏（全屏程序）。
    pub fn alt_screen(&self) -> bool {
        let mut v = 0i32;
        self.get(ffi::data::ACTIVE_SCREEN, &mut v) && v == ffi::screen::ALTERNATE
    }

    /// 回滚里有几行。
    pub fn scrollback_rows(&self) -> usize {
        let mut n = 0usize;
        self.get(ffi::data::SCROLLBACK_ROWS, &mut n);
        n
    }

    /// 视口在回滚里的位置。
    pub fn scrollbar(&self) -> Scrollbar {
        let mut s = Scrollbar::default();
        self.get(ffi::data::SCROLLBAR, &mut s);
        s
    }

    /// 视口是不是跟着屏幕（没往上翻）。
    pub fn at_bottom(&self) -> bool {
        let mut v = true;
        !self.get(ffi::data::VIEWPORT_ACTIVE, &mut v) || v
    }

    /// 滚视口。
    pub fn scroll_viewport(&mut self, to: Scroll) {
        self.scroll(to);
    }

    /// 滚视口；内部读回滚时用，读完放回最下面。
    pub(crate) fn scroll(&self, to: Scroll) {
        let v = match to {
            Scroll::Top => ffi::ScrollViewport {
                tag: ffi::scroll::TOP,
                value: [0; 2],
            },
            Scroll::Bottom => ffi::ScrollViewport {
                tag: ffi::scroll::BOTTOM,
                value: [0; 2],
            },
            Scroll::Delta(d) => ffi::ScrollViewport {
                tag: ffi::scroll::DELTA,
                value: [d as i64 as u64, 0],
            },
            Scroll::Row(r) => ffi::ScrollViewport {
                tag: ffi::scroll::ROW,
                value: [r as u64, 0],
            },
        };
        // SAFETY：h 有效，按值传。
        unsafe { (api().terminal_scroll_viewport)(self.h, v) };
    }

    /// 一个模式开着没有。
    pub fn mode(&self, m: Mode) -> bool {
        let mut c = ffi::ModeConfig {
            mode: m.0,
            value: false,
        };
        self.get(ffi::data::MODE, &mut c) && c.value
    }

    /// 自己开关一个模式（不触发回调）。
    pub fn set_mode(&mut self, m: Mode, on: bool) {
        self.set(
            ffi::opt::MODE,
            &ffi::ModeConfig {
                mode: m.0,
                value: on,
            },
        );
    }

    /// 程序设的 Kitty 键盘协议标志。
    pub fn kitty_keyboard_flags(&self) -> u8 {
        let mut f = 0u8;
        self.get(ffi::data::KITTY_KEYBOARD_FLAGS, &mut f);
        f
    }

    /// 程序要不要鼠标事件。
    pub fn mouse_tracking(&self) -> bool {
        let mut v = false;
        self.get(ffi::data::MOUSE_TRACKING, &mut v) && v
    }

    /// 程序要的鼠标指针形状（OSC 22），GhosttyMouseShape 的值；名字见 [`crate::MOUSE_SHAPES`]。
    pub fn mouse_shape(&self) -> i32 {
        let mut v = 0i32;
        self.get(ffi::data::MOUSE_SHAPE, &mut v);
        v
    }

    /// xterm 的 modifyOtherKeys 2 开着没有（`CSI > 4 ; 2 m`）。库没有直接给，
    /// 拿一个按终端模式设好的键盘编码器编 Ctrl+P 看（同 mygo）。
    pub fn modify_other_keys(&self) -> bool {
        crate::input::modify_other_keys(self)
    }

    /// 程序看到的颜色；defaults 时只要默认的（不算 OSC 改的）。
    pub(crate) fn colors(&self, defaults: bool) -> TermColors {
        use ffi::data::*;
        let base = if defaults {
            [
                COLOR_FOREGROUND_DEFAULT,
                COLOR_BACKGROUND_DEFAULT,
                COLOR_CURSOR_DEFAULT,
                COLOR_PALETTE_DEFAULT,
            ]
        } else {
            [
                COLOR_FOREGROUND,
                COLOR_BACKGROUND,
                COLOR_CURSOR,
                COLOR_PALETTE,
            ]
        };
        let one = |d| {
            let mut c = Rgb::default();
            self.get(d, &mut c).then_some(c)
        };
        let mut palette = [Rgb::default(); 256];
        self.get(base[3], &mut palette);
        TermColors {
            foreground: one(base[0]),
            background: one(base[1]),
            cursor: one(base[2]),
            palette,
        }
    }

    /// 设配色：程序用 OSC 10/11/12/4 问到的默认颜色。开了模式 2031 的程序在深浅变了时会收到
    /// `CSI ? 997 ; 1|2 n`（经 Handler::write_pty）。
    pub fn set_theme(&mut self, theme: &Theme) {
        self.set(ffi::opt::COLOR_FOREGROUND, &theme.foreground);
        self.set(ffi::opt::COLOR_BACKGROUND, &theme.background);
        match &theme.cursor {
            Some(c) => self.set(ffi::opt::COLOR_CURSOR, c),
            // SAFETY：空指针清掉光标颜色，头文件里写明可以。
            None => unsafe {
                (api().terminal_set)(self.h, ffi::opt::COLOR_CURSOR, ptr::null()) == 0
            },
        };
        let palette = theme.palette256();
        self.set(ffi::opt::COLOR_PALETTE, &palette);
        let dark = theme.is_dark();
        let old = self.st_mut().dark.replace(dark);
        if old.is_some_and(|o| o != dark) && self.mode(Mode::COLOR_SCHEME) {
            let reply = if dark {
                b"\x1b[?997;1n"
            } else {
                b"\x1b[?997;2n"
            };
            guard(|| self.st_mut().handler.write_pty(reply));
        }
    }

    /// 屏幕和回滚的纯文本：软换行接起来，行尾空白去掉。
    pub fn text(&self) -> String {
        let mut opts = ffi::FormatterTerminalOptions {
            size: size_of::<ffi::FormatterTerminalOptions>(),
            emit: ffi::format::PLAIN,
            unwrap: true,
            trim: true,
            extra: ffi::FormatterTerminalExtra::default(),
            selection: ptr::null(),
        };
        opts.extra.size = size_of::<ffi::FormatterTerminalExtra>();
        opts.extra.screen.size = size_of::<ffi::FormatterScreenExtra>();
        let api = api();
        let mut f: Handle = ptr::null_mut();
        // SAFETY：f 是出参，h 有效，opts 按值传。
        if unsafe { (api.formatter_terminal_new)(ptr::null(), &mut f, self.h, opts) } != 0 {
            return String::new();
        }
        let (mut p, mut n) = (ptr::null_mut::<u8>(), 0usize);
        // SAFETY：f 刚建好；p、n 是出参。
        let ok = unsafe { (api.formatter_format_alloc)(f, ptr::null(), &mut p, &mut n) } == 0;
        let s = if ok { take_alloc(p, n) } else { String::new() };
        // SAFETY：f 是上面建的。
        unsafe { (api.formatter_free)(f) };
        s
    }

    /// 清屏：清掉回滚和光标那行以上的屏幕，光标那行挪到最上面（同 Ghostty 的 ⌘K、
    /// Kitty 的 clear_terminal to_cursor），shell 的提示符留着。备用屏上不清，返回 false。
    pub fn clear_screen(&mut self) -> bool {
        if self.alt_screen() {
            return false;
        }
        self.set_selection(None);
        self.scroll(Scroll::Bottom);
        let (x, y) = self.cursor();
        let mut seq = String::from("\x1b[3J");
        if y > 0 {
            // 从最上面删掉光标以上的行，再把光标放回它那行（现在是第一行）。
            seq.push_str(&format!("\x1b[H\x1b[{y}M\x1b[1;{}H", x + 1));
        }
        self.write(seq.as_bytes());
        true
    }

    /// 清回滚，屏幕不动。
    pub fn clear_scrollback(&mut self) {
        self.write(b"\x1b[3J");
    }

    /// 同步输出（模式 2026）还扣着帧：程序开始后不到 1 秒、还没结束。
    pub fn render_held(&self) -> bool {
        self.st().held.is_some_and(|s| s.elapsed() < HOLD_MAX) && self.mode(Mode::SYNC_OUTPUT)
    }

    fn grid_ref(&self, p: ffi::Point) -> Option<ffi::GridRef> {
        let mut r = ffi::GridRef::sized();
        // SAFETY：h 有效，r 是出参。
        (unsafe { (api().terminal_grid_ref)(self.h, p, &mut r) } == 0).then_some(r)
    }

    /// 视口里 (x, y) 这格的超链接（OSC 8）。
    pub fn hyperlink_at(&self, x: u16, y: u16) -> Option<String> {
        let r = self.grid_ref(ffi::Point::new(ffi::point::VIEWPORT, x, y as u32))?;
        let api = api();
        let mut buf = vec![0u8; 512];
        let mut n = 0usize;
        // SAFETY：r 刚取的，终端没变；buf 有 len 字节。
        let mut res =
            unsafe { (api.grid_ref_hyperlink_uri)(&r, buf.as_mut_ptr(), buf.len(), &mut n) };
        if res == ffi::OUT_OF_SPACE && n > buf.len() {
            buf.resize(n, 0);
            // SAFETY：同上。
            res = unsafe { (api.grid_ref_hyperlink_uri)(&r, buf.as_mut_ptr(), buf.len(), &mut n) };
        }
        if res != 0 || n == 0 {
            return None;
        }
        buf.truncate(n);
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    /// 视口里的格子换成屏幕坐标（从回滚最上面数）。
    pub fn viewport_to_screen(&self, x: u16, y: u16) -> Point {
        Point {
            x,
            y: (self.scrollbar().offset + y as u64).min(u32::MAX as u64) as u32,
        }
    }

    fn selection(&self, s: &Selection) -> Option<ffi::Selection> {
        let start = self.grid_ref(ffi::Point::new(ffi::point::SCREEN, s.start.x, s.start.y))?;
        let end = self.grid_ref(ffi::Point::new(ffi::point::SCREEN, s.end.x, s.end.y))?;
        Some(ffi::Selection {
            size: size_of::<ffi::Selection>(),
            start,
            end,
            rectangle: s.rectangle,
        })
    }

    /// 选区的文字（屏幕坐标，可以跨回滚）：软换行接起来，行尾空白去掉，同 Ghostty 复制。
    /// 坐标出界时是 None。
    pub fn selection_text(&self, s: &Selection) -> Option<String> {
        let sel = self.selection(s)?;
        let opts = ffi::SelectionFormatOptions {
            size: size_of::<ffi::SelectionFormatOptions>(),
            emit: ffi::format::PLAIN,
            unwrap: true,
            trim: true,
            selection: &sel,
        };
        let (mut p, mut n) = (ptr::null_mut::<u8>(), 0usize);
        // SAFETY：h 有效；sel 的两个格子刚取，终端没变；p、n 是出参。
        let r = unsafe {
            (api().terminal_selection_format_alloc)(self.h, ptr::null(), opts, &mut p, &mut n)
        };
        (r == 0).then(|| take_alloc(p, n))
    }

    /// 设终端的选区（画的时候 [`crate::Row::selection`] 报哪些格子选中了），None 是不选。
    pub fn set_selection(&mut self, s: Option<Selection>) -> bool {
        match s {
            Some(s) => match self.selection(&s) {
                Some(sel) => self.set(ffi::opt::SELECTION, &sel),
                None => false,
            },
            // SAFETY：空指针清掉选区，头文件里写明可以。
            None => unsafe { (api().terminal_set)(self.h, ffi::opt::SELECTION, ptr::null()) == 0 },
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY：先释放终端（之后不会再有回调），再收回 State。
        unsafe {
            (api().terminal_free)(self.h);
            drop(Box::from_raw(self.state));
        }
    }
}

/// 拷出库分配的字节，再还给库。
fn take_alloc(p: *mut u8, n: usize) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY：p 是库刚分配的 n 字节。
    let s = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p, n) }).into_owned();
    // SAFETY：用默认分配器分配的，同样的长度还回去。
    unsafe { (api().free)(ptr::null(), p, n) };
    s
}

/// 调 Handler，panic 了记一笔，不让它穿过 C 的栈。
fn guard(f: impl FnOnce()) {
    if std::panic::catch_unwind(AssertUnwindSafe(f)).is_err() {
        tracing::error!("终端回调 panic 了");
    }
}

/// 从 userdata 拿 State。
fn with_state(ud: *mut c_void, f: impl FnOnce(&mut State)) {
    if ud.is_null() {
        return;
    }
    // SAFETY：ud 是 Terminal::new 交给库的 State，活到 terminal_free 之后；回调只在
    // write / resize / reset 里跑，那时 Rust 这边没有别的引用指着它。
    let st = unsafe { &mut *(ud as *mut State) };
    guard(|| f(st));
}

unsafe extern "C" fn cb_write_pty(_t: Handle, ud: *mut c_void, data: *const u8, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY：库给的 data 有 len 字节，只在这次调用里有效。
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    with_state(ud, |st| st.handler.write_pty(bytes));
}

unsafe extern "C" fn cb_bell(_t: Handle, ud: *mut c_void) {
    with_state(ud, |st| st.handler.bell());
}

unsafe extern "C" fn cb_title_changed(_t: Handle, ud: *mut c_void) {
    with_state(ud, |st| st.title_changed = true);
}

unsafe extern "C" fn cb_pwd_changed(_t: Handle, ud: *mut c_void) {
    with_state(ud, |st| st.pwd_changed = true);
}

unsafe extern "C" fn cb_render_hold(_t: Handle, ud: *mut c_void, held: bool) {
    with_state(ud, |st| st.held = held.then(Instant::now));
}

unsafe extern "C" fn cb_size(_t: Handle, ud: *mut c_void, out: *mut ffi::SizeReportSize) -> bool {
    if out.is_null() {
        return false;
    }
    let mut size = None;
    with_state(ud, |st| {
        size = Some(ffi::SizeReportSize {
            rows: st.rows,
            columns: st.cols,
            cell_width: st.cell_w,
            cell_height: st.cell_h,
        })
    });
    let Some(size) = size else { return false };
    // SAFETY：out 是库给的出参。
    unsafe { *out = size };
    true
}

unsafe extern "C" fn cb_color_scheme(_t: Handle, ud: *mut c_void, out: *mut i32) -> bool {
    if out.is_null() {
        return false;
    }
    let mut dark = true;
    with_state(ud, |st| dark = st.dark.unwrap_or(true));
    // SAFETY：out 是库给的出参（GhosttyColorScheme，int）。
    unsafe {
        *out = if dark {
            ffi::scheme::DARK
        } else {
            ffi::scheme::LIGHT
        }
    };
    true
}

unsafe extern "C" fn cb_device_attributes(
    _t: Handle,
    _ud: *mut c_void,
    out: *mut ffi::DeviceAttributes,
) -> bool {
    if out.is_null() {
        return false;
    }
    // 同 Ghostty：带 ANSI 颜色的 VT220（DA1），型号 1、版本 10（DA2）。
    let mut features = [0u16; 64];
    features[0] = 22;
    let da = ffi::DeviceAttributes {
        primary: ffi::DeviceAttributesPrimary {
            conformance_level: 62,
            features,
            num_features: 1,
        },
        secondary: ffi::DeviceAttributesSecondary {
            device_type: 1,
            firmware_version: 10,
            rom_cartridge: 0,
        },
        tertiary: ffi::DeviceAttributesTertiary { unit_id: 0 },
    };
    // SAFETY：out 是库给的出参。
    unsafe { *out = da };
    true
}

unsafe extern "C" fn cb_desktop_notification(
    _t: Handle,
    ud: *mut c_void,
    n: *const ffi::DesktopNotification,
) {
    if n.is_null() {
        return;
    }
    // SAFETY：库借出的通知和里面的字符串在这次调用里有效。
    let (title, body) = unsafe {
        let n = &*n;
        if n.size < size_of::<ffi::DesktopNotification>() {
            return;
        }
        (n.title.to_string_lossy(), n.body.to_string_lossy())
    };
    with_state(ud, |st| st.handler.notify(&title, &body));
}

unsafe extern "C" fn cb_progress_report(
    _t: Handle,
    ud: *mut c_void,
    p: *const ffi::ProgressReport,
) {
    if p.is_null() {
        return;
    }
    // SAFETY：库借出的，这次调用里有效；先看 size 够不够。
    let p = unsafe {
        if (*p).size < size_of::<ffi::ProgressReport>() {
            return;
        }
        *p
    };
    let percent = (p.progress >= 0).then_some(p.progress as u8);
    with_state(ud, |st| {
        st.handler
            .progress(p.state.clamp(0, u8::MAX as i32) as u8, percent)
    });
}
