//! libghostty-vt 的 C 接口：找库、dlopen、函数表、结构体，载入时拿库自带的类型清单核对。
//!
//! 移植自 mygo plugins/terminal/internal/vt（MIT，Copyright (c) 2026 MyGo contributors）。
//! 结构体照 Ghostty befcdfd2c3a1 的 C 头文件写；清单（ghostty_type_json）里的大小、偏移、
//! 枚举值对不上就不载入，免得别的版本的库写坏内存。GhosttyCell 是压成 64 位的值，
//! 各字段在哪几位不归 ABI 管，按清单给的位来拆。
//!
//! 这个模块之外不直接碰裸指针：别的模块只经 `api()` 调函数，每处 `unsafe` 写明为什么成立。

use std::{
    ffi::{c_char, c_void, CStr},
    mem::{offset_of, size_of},
    path::PathBuf,
    sync::OnceLock,
};

use anyhow::{anyhow, Result};
use libloading::Library;
use serde_json::Value;

/// 库给的不透明句柄（GhosttyTerminal、GhosttyRenderState……）。
pub(crate) type Handle = *mut c_void;

/// 库的文件名。
pub(crate) const LIB_NAME: &str = if cfg!(target_os = "macos") {
    "libghostty-vt.dylib"
} else {
    "libghostty-vt.so"
};

/// 指定库路径的环境变量。
pub(crate) const ENV: &str = "LOLIDESK_GHOSTTY_VT";

macro_rules! api {
    ($( fn $name:ident($($arg:ty),* $(,)?) $(-> $ret:ty)?; )*) => {
        /// 用到的 C 函数，符号名是 `ghostty_` 加字段名。
        pub(crate) struct Api {
            $( pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?, )*
        }

        impl Api {
            /// 取出所有符号。
            ///
            /// SAFETY：调用方保证 lib 是 libghostty-vt，各符号的签名就是这里写的
            /// （载入后再拿类型清单核对结构体）。
            unsafe fn resolve(lib: &Library) -> Result<Api, String> {
                Ok(Api { $( $name: {
                    let sym = concat!("ghostty_", stringify!($name), "\0");
                    // SAFETY：见上；函数指针拷出来用，库一直不卸载。
                    *unsafe { lib.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(sym.as_bytes()) }
                        .map_err(|e| format!("缺符号 ghostty_{}：{e}", stringify!($name)))?
                }, )* })
            }
        }
    };
}

api! {
    fn type_json() -> *const c_char;
    fn free(*const c_void, *mut u8, usize);

    fn terminal_new(*const c_void, *mut Handle, u16, u16) -> i32;
    fn terminal_free(Handle);
    fn terminal_reset(Handle);
    fn terminal_resize(Handle, u16, u16, u32, u32) -> i32;
    fn terminal_set(Handle, i32, *const c_void) -> i32;
    fn terminal_get(Handle, i32, *mut c_void) -> i32;
    fn terminal_vt_write(Handle, *const u8, usize);
    fn terminal_scroll_viewport(Handle, ScrollViewport);
    fn terminal_grid_ref(Handle, Point, *mut GridRef) -> i32;
    fn terminal_selection_format_alloc(Handle, *const c_void, SelectionFormatOptions, *mut *mut u8, *mut usize) -> i32;
    fn grid_ref_hyperlink_uri(*const GridRef, *mut u8, usize, *mut usize) -> i32;
    fn row_get(u64, i32, *mut c_void) -> i32;

    fn formatter_terminal_new(*const c_void, *mut Handle, Handle, FormatterTerminalOptions) -> i32;
    fn formatter_format_alloc(Handle, *const c_void, *mut *mut u8, *mut usize) -> i32;
    fn formatter_free(Handle);

    fn render_state_new(*const c_void, *mut Handle) -> i32;
    fn render_state_free(Handle);
    fn render_state_update(Handle, Handle) -> i32;
    fn render_state_get(Handle, i32, *mut c_void) -> i32;
    fn render_state_clean(Handle) -> i32;
    fn render_state_row_iterator_new(*const c_void, *mut Handle) -> i32;
    fn render_state_row_iterator_free(Handle);
    fn render_state_row_iterator_next(Handle) -> bool;
    fn render_state_row_get(Handle, i32, *mut c_void) -> i32;
    fn render_state_row_cells_new(*const c_void, *mut Handle) -> i32;
    fn render_state_row_cells_free(Handle);
    fn render_state_row_cells_select(Handle, u16) -> i32;
    fn render_state_row_cells_get(Handle, i32, *mut c_void) -> i32;

    fn key_encoder_new(*const c_void, *mut Handle) -> i32;
    fn key_encoder_free(Handle);
    fn key_encoder_setopt(Handle, i32, *const c_void);
    fn key_encoder_setopt_from_terminal(Handle, Handle);
    fn key_encoder_encode(Handle, Handle, *mut c_char, usize, *mut usize) -> i32;
    fn key_event_new(*const c_void, *mut Handle) -> i32;
    fn key_event_free(Handle);
    fn key_event_set_action(Handle, i32);
    fn key_event_set_key(Handle, i32);
    fn key_event_set_mods(Handle, u16);
    fn key_event_set_consumed_mods(Handle, u16);
    fn key_event_set_composing(Handle, bool);
    fn key_event_set_utf8(Handle, *const c_char, usize);
    fn key_event_set_unshifted_codepoint(Handle, u32);

    fn mouse_encoder_new(*const c_void, *mut Handle) -> i32;
    fn mouse_encoder_free(Handle);
    fn mouse_encoder_setopt(Handle, i32, *const c_void);
    fn mouse_encoder_setopt_from_terminal(Handle, Handle);
    fn mouse_encoder_reset(Handle);
    fn mouse_encoder_encode(Handle, Handle, *mut c_char, usize, *mut usize) -> i32;
    fn mouse_event_new(*const c_void, *mut Handle) -> i32;
    fn mouse_event_free(Handle);
    fn mouse_event_set_action(Handle, i32);
    fn mouse_event_set_button(Handle, i32);
    fn mouse_event_clear_button(Handle);
    fn mouse_event_set_mods(Handle, u16);
    fn mouse_event_set_position(Handle, MousePosition);

    fn focus_encode(i32, *mut c_char, usize, *mut usize) -> i32;
    fn paste_encode(*mut c_char, usize, bool, *mut c_char, usize, *mut usize) -> i32;
    fn paste_is_safe(*const c_char, usize) -> bool;
}

/// 载入好的库。
pub(crate) struct Lib {
    pub api: Api,
    pub cell: CellBits,
    pub path: PathBuf,
    _lib: Library,
}

static LIB: OnceLock<Result<Lib, String>> = OnceLock::new();

/// 找到并载入库，只载一次；之后的调用都报第一次的结果。
pub(crate) fn lib_checked() -> Result<&'static Lib> {
    LIB.get_or_init(open).as_ref().map_err(|e| anyhow!("{e}"))
}

/// 载入好的库：只在已经有了终端、渲染状态或编码器的地方用，那时库一定载入了。
pub(crate) fn lib() -> &'static Lib {
    match LIB.get() {
        Some(Ok(lib)) => lib,
        _ => panic!("libghostty-vt 还没载入"),
    }
}

/// 载入好的库的函数表。
pub(crate) fn api() -> &'static Api {
    &lib().api
}

fn open() -> Result<Lib, String> {
    let path = library_path().map_err(|e| format!("{e:#}"))?;
    let bad = |e: String| format!("{} 不是这里绑定的 libghostty-vt：{e}", path.display());
    // SAFETY：载入一个 C 库只跑它的初始化代码，libghostty-vt 没有别的副作用。
    let lib =
        unsafe { Library::new(&path) }.map_err(|e| format!("载入 {} 失败：{e}", path.display()))?;
    // SAFETY：签名照 Ghostty befcdfd2c3a1 的头文件写，下面再拿清单核对结构体。
    let api = unsafe { Api::resolve(&lib) }.map_err(bad)?;
    // SAFETY：ghostty_type_json 返回静态的、以 NUL 结尾的字符串。
    let json = unsafe {
        let p = (api.type_json)();
        if p.is_null() {
            return Err(bad("没有类型清单".into()));
        }
        CStr::from_ptr(p)
    };
    let json = json
        .to_str()
        .map_err(|e| bad(format!("类型清单不是 UTF-8：{e}")))?;
    let cell = check_manifest(json).map_err(bad)?;
    tracing::debug!(path = %path.display(), "载入 libghostty-vt");
    Ok(Lib {
        api,
        cell,
        path,
        _lib: lib,
    })
}

/// 库在哪：`$LOLIDESK_GHOSTTY_VT`；可执行文件旁边；`<exe>/../Frameworks`、
/// `<exe>/../Resources`、`<exe>/../lib`。头一个存在的文件。
pub fn library_path() -> Result<PathBuf> {
    let mut tried: Vec<String> = Vec::new();
    match std::env::var_os(ENV) {
        Some(p) if !p.is_empty() => {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Ok(p);
            }
            tracing::warn!(path = %p.display(), "{ENV} 指的文件不存在，接着找别处");
            tried.push(format!("${ENV}（{}，不存在）", p.display()));
        }
        _ => tried.push(format!("${ENV}（没设）")),
    }
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.to_path_buf()))
    {
        candidates.push(dir.join(LIB_NAME));
        if let Some(up) = dir.parent() {
            for sub in ["Frameworks", "Resources", "lib"] {
                candidates.push(up.join(sub).join(LIB_NAME));
            }
        }
    }
    #[cfg(test)]
    candidates.extend(mygo_cache());
    for c in candidates {
        if c.is_file() {
            return Ok(c);
        }
        tried.push(c.display().to_string());
    }
    Err(anyhow!(
        "找不到 {LIB_NAME}，找过：{}。用 desk/scripts/fetch-ghostty.sh 下载，或者设 {ENV} 指向它",
        tried.join("、")
    ))
}

/// 测试时也找 mygo 下载的缓存（`<用户缓存>/mygo/natives/<sha256>/`）。
#[cfg(test)]
fn mygo_cache() -> Option<PathBuf> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-amd64",
        ("linux", "x86_64") => "linux-amd64",
        ("linux", "aarch64") => "linux-arm64",
        _ => return None,
    };
    let manifest: Value = serde_json::from_str(include_str!("../libghostty-vt.json")).ok()?;
    let sha = manifest["files"][platform]["sha256"].as_str()?;
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let cache = if cfg!(target_os = "macos") {
        home.join("Library/Caches")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
    };
    Some(cache.join("mygo/natives").join(sha).join(LIB_NAME))
}

// ---- 结果码 ----

pub(crate) const SUCCESS: i32 = 0;
pub(crate) const OUT_OF_SPACE: i32 = -3;

/// 把 GhosttyResult 变成 Result。
pub(crate) fn check(r: i32, what: &str) -> Result<()> {
    if r == SUCCESS {
        return Ok(());
    }
    let name = match r {
        -1 => "内存不够",
        -2 => "参数不对",
        -3 => "缓冲区不够",
        -4 => "没有值",
        -5 => "读写出错",
        -6 => "超出上限",
        -7 => "被拒绝",
        _ => "未知错误",
    };
    Err(anyhow!("libghostty-vt：{what}失败（{name}，{r}）"))
}

// ---- 枚举值 ----

macro_rules! consts {
    ($( $module:ident : $ty:literal { $($name:ident = $val:expr),* $(,)? } )*) => {
        $(
            #[allow(dead_code)]
            pub(crate) mod $module {
                $( pub const $name: i32 = $val; )*
            }
        )*
        /// 这里写死的枚举值，清单里的得一样：（类型，名字，值）。
        pub(crate) const ENUMS: &[(&str, &str, i64)] = &[
            $( $( ($ty, stringify!($name), $module::$name as i64), )* )*
        ];
    };
}

consts! {
    opt: "GhosttyTerminalOption" {
        USERDATA = 0, WRITE_PTY = 1, BELL = 2, TITLE_CHANGED = 5, SIZE = 6, COLOR_SCHEME = 7,
        DEVICE_ATTRIBUTES = 8, COLOR_FOREGROUND = 11, COLOR_BACKGROUND = 12, COLOR_CURSOR = 13,
        COLOR_PALETTE = 14, KITTY_IMAGE_STORAGE_LIMIT = 15, SELECTION = 21, DEFAULT_CURSOR_STYLE = 22,
        DEFAULT_CURSOR_BLINK = 23, PWD_CHANGED = 25, SCROLLBACK_MAX_BYTES = 27,
        DESKTOP_NOTIFICATION = 29, PROGRESS_REPORT = 30, MODE_DEFAULT = 33, MODE = 34,
        TERMINFO_NAME = 37, RENDER_HOLD = 41,
    }
    data: "GhosttyTerminalData" {
        COLS = 1, ROWS = 2, CURSOR_X = 3, CURSOR_Y = 4, ACTIVE_SCREEN = 6, KITTY_KEYBOARD_FLAGS = 8,
        SCROLLBAR = 9, MOUSE_TRACKING = 11, TITLE = 12, PWD = 13, SCROLLBACK_ROWS = 15,
        COLOR_FOREGROUND = 18, COLOR_BACKGROUND = 19, COLOR_CURSOR = 20, COLOR_PALETTE = 21,
        COLOR_FOREGROUND_DEFAULT = 22, COLOR_BACKGROUND_DEFAULT = 23, COLOR_CURSOR_DEFAULT = 24,
        COLOR_PALETTE_DEFAULT = 25, VIEWPORT_ACTIVE = 32, MODE = 37, MOUSE_SHAPE = 41,
    }
    screen: "GhosttyTerminalScreen" { PRIMARY = 0, ALTERNATE = 1 }
    cursor_style: "GhosttyTerminalCursorStyle" { BAR = 0, BLOCK = 1, UNDERLINE = 2, BLOCK_HOLLOW = 3 }
    visual_style: "GhosttyRenderStateCursorVisualStyle" { BAR = 0, BLOCK = 1, UNDERLINE = 2, BLOCK_HOLLOW = 3 }
    rs_data: "GhosttyRenderStateData" { COLS = 1, ROWS = 2, DIRTY = 3, ROW_ITERATOR = 4, CURSOR = 18, COLORS = 19 }
    dirty: "GhosttyRenderStateDirty" { FALSE = 0, PARTIAL = 1, FULL = 2 }
    rs_row: "GhosttyRenderStateRowData" { DIRTY = 1, RAW = 2, CELLS = 3, SELECTION = 4, CELLS_RAW = 5, ID = 7 }
    rs_cell: "GhosttyRenderStateRowCellsData" { STYLE = 2, GRAPHEMES_LEN = 3, GRAPHEMES_BUF = 4 }
    row: "GhosttyRowData" { WRAP = 1, SEMANTIC_PROMPT = 6 }
    row_prompt: "GhosttyRowSemanticPrompt" { NONE = 0, PROMPT = 1, PROMPT_CONTINUATION = 2 }
    content: "GhosttyCellContentTag" { CODEPOINT = 0, CODEPOINT_GRAPHEME = 1, BG_COLOR_PALETTE = 2, BG_COLOR_RGB = 3 }
    wide: "GhosttyCellWide" { NARROW = 0, WIDE = 1, SPACER_TAIL = 2, SPACER_HEAD = 3 }
    color_tag: "GhosttyStyleColorTag" { NONE = 0, PALETTE = 1, RGB = 2 }
    underline: "GhosttySgrUnderline" { NONE = 0, SINGLE = 1, DOUBLE = 2, CURLY = 3, DOTTED = 4, DASHED = 5 }
    scroll: "GhosttyTerminalScrollViewportTag" { TOP = 0, BOTTOM = 1, DELTA = 2, ROW = 3 }
    point: "GhosttyPointTag" { ACTIVE = 0, VIEWPORT = 1, SCREEN = 2, HISTORY = 3 }
    format: "GhosttyFormatterFormat" { PLAIN = 0, VT = 1, HTML = 2 }
    scheme: "GhosttyColorScheme" { LIGHT = 0, DARK = 1 }
    focus: "GhosttyFocusEvent" { GAINED = 0, LOST = 1 }
    key_action: "GhosttyKeyAction" { RELEASE = 0, PRESS = 1, REPEAT = 2 }
    key_opt: "GhosttyKeyEncoderOption" { KITTY_FLAGS = 5, MACOS_OPTION_AS_ALT = 6 }
    option_as_alt: "GhosttyOptionAsAlt" { FALSE = 0, TRUE = 1, LEFT = 2, RIGHT = 3 }
    mouse_action: "GhosttyMouseAction" { PRESS = 0, RELEASE = 1, MOTION = 2 }
    mouse_button: "GhosttyMouseButton" {
        UNKNOWN = 0, LEFT = 1, RIGHT = 2, MIDDLE = 3, FOUR = 4, FIVE = 5, SIX = 6, SEVEN = 7,
        EIGHT = 8, NINE = 9, TEN = 10, ELEVEN = 11,
    }
    mouse_opt: "GhosttyMouseEncoderOption" { SIZE = 2, ANY_BUTTON_PRESSED = 3, TRACK_LAST_CELL = 4 }
}

// ---- 结构体（照头文件，布局在载入时核对）----

/// 一个颜色（GhosttyColorRgb）。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// 视口在回滚里的位置（GhosttyTerminalScrollbar）：总行数（回滚加屏幕）、
/// 视口第一行离最上面多远、视口几行。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// 一行的身份（GhosttyRenderStateRowId）：行滚动时跟着行走，所以能认出哪些行滚上去了。
/// 它不反映内容：在原地改写一行，身份不变。要知道内容变没变，看行的脏标记或者比内容。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowId {
    bits: [u64; 2],
}

/// GhosttyString：库借出来的字节。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct GString {
    pub ptr: *const u8,
    pub len: usize,
}

impl GString {
    pub fn empty() -> GString {
        GString {
            ptr: std::ptr::null(),
            len: 0,
        }
    }

    /// 拷出来。
    ///
    /// SAFETY：调用方保证 ptr 指着 len 个还有效的字节（或为空）。
    pub unsafe fn to_string_lossy(self) -> String {
        if self.ptr.is_null() || self.len == 0 {
            return String::new();
        }
        // SAFETY：见上。
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.ptr, self.len) })
            .into_owned()
    }
}

/// GhosttyStyleColor：标签，再是调色板序号或 RGB 的联合（8 字节对齐）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct StyleColor {
    pub tag: i32,
    pub value: u64,
}

impl StyleColor {
    /// 联合里的字节：调色板序号在第 0 字节，RGB 在 0..3。
    pub fn bytes(self) -> [u8; 8] {
        self.value.to_ne_bytes()
    }
}

/// GhosttyStyle。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Style {
    pub size: usize,
    pub fg_color: StyleColor,
    pub bg_color: StyleColor,
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

impl Style {
    pub fn sized() -> Style {
        Style {
            size: size_of::<Style>(),
            ..Style::default()
        }
    }
}

/// GhosttyRenderStateCursor。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct RenderCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: i32,
}

/// GhosttyRenderStateColors。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RenderColors {
    pub size: usize,
    pub background: Rgb,
    pub foreground: Rgb,
    pub cursor: Rgb,
    pub cursor_has_value: bool,
    pub palette: [Rgb; 256],
}

/// GhosttyRenderStateRowSelection。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct RowSelection {
    pub size: usize,
    pub start_x: u16,
    pub end_x: u16,
}

/// GhosttyCellsView：一行格子的原始值，借来的。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct CellsView {
    pub ptr: *const u64,
    pub len: usize,
}

/// GhosttyTerminalScrollViewport：标签，再是 delta / row 的联合。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ScrollViewport {
    pub tag: i32,
    pub value: [u64; 2],
}

/// GhosttyTerminalModeConfig。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ModeConfig {
    pub mode: u16,
    pub value: bool,
}

/// GhosttyGridRef：一个格子，终端下次变之前有效。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct GridRef {
    pub size: usize,
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}

impl GridRef {
    pub fn sized() -> GridRef {
        GridRef {
            size: size_of::<GridRef>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        }
    }
}

/// GhosttyPointCoordinate。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct PointCoordinate {
    pub x: u16,
    pub y: u32,
}

/// GhosttyPointValue。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union PointValue {
    pub coordinate: PointCoordinate,
    pub _padding: [u64; 2],
}

/// GhosttyPoint。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Point {
    pub tag: i32,
    pub value: PointValue,
}

impl Point {
    pub fn new(tag: i32, x: u16, y: u32) -> Point {
        let mut value = PointValue { _padding: [0; 2] };
        value.coordinate = PointCoordinate { x, y };
        Point { tag, value }
    }
}

/// GhosttySelection：两个格子（含），终端下次变之前有效。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Selection {
    pub size: usize,
    pub start: GridRef,
    pub end: GridRef,
    pub rectangle: bool,
}

/// GhosttyTerminalSelectionFormatOptions。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SelectionFormatOptions {
    pub size: usize,
    pub emit: i32,
    pub unwrap: bool,
    pub trim: bool,
    pub selection: *const Selection,
}

/// GhosttyFormatterScreenExtra。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct FormatterScreenExtra {
    pub size: usize,
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

/// GhosttyFormatterTerminalExtra。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct FormatterTerminalExtra {
    pub size: usize,
    pub palette: bool,
    pub modes: bool,
    pub scrolling_region: bool,
    pub tabstops: bool,
    pub pwd: bool,
    pub keyboard: bool,
    pub screen: FormatterScreenExtra,
}

/// GhosttyFormatterTerminalOptions。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct FormatterTerminalOptions {
    pub size: usize,
    pub emit: i32,
    pub unwrap: bool,
    pub trim: bool,
    pub extra: FormatterTerminalExtra,
    pub selection: *const Selection,
}

/// GhosttyMouseEncoderSize。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct MouseEncoderSize {
    pub size: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

/// GhosttyMousePosition。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct MousePosition {
    pub x: f32,
    pub y: f32,
}

/// GhosttySizeReportSize。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

/// GhosttyDeviceAttributesPrimary。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DeviceAttributesPrimary {
    pub conformance_level: u16,
    pub features: [u16; 64],
    pub num_features: usize,
}

/// GhosttyDeviceAttributesSecondary。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DeviceAttributesSecondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}

/// GhosttyDeviceAttributesTertiary。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct DeviceAttributesTertiary {
    pub unit_id: u32,
}

/// GhosttyDeviceAttributes。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DeviceAttributes {
    pub primary: DeviceAttributesPrimary,
    pub secondary: DeviceAttributesSecondary,
    pub tertiary: DeviceAttributesTertiary,
}

/// GhosttyTerminalDesktopNotification。
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct DesktopNotification {
    pub size: usize,
    pub title: GString,
    pub body: GString,
}

/// GhosttyTerminalProgressReport。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ProgressReport {
    pub size: usize,
    pub state: i32,
    pub progress: i8,
}

/// 一个 C 结构体在这里的布局：名字、大小、各字段的偏移。
pub(crate) type Layout = (&'static str, usize, Vec<(&'static str, usize)>);

/// 这里传给库的每个结构体的布局，载入时（和测试里）跟清单比。
pub(crate) fn layouts() -> Vec<Layout> {
    macro_rules! layout {
        ($name:literal, $ty:ty { $($field:literal => $($path:ident).+),* $(,)? }) => {
            ($name, size_of::<$ty>(), vec![$( ($field, offset_of!($ty, $($path).+)) ),*])
        };
    }
    vec![
        layout!("GhosttyColorRgb", Rgb { "r" => r, "g" => g, "b" => b }),
        layout!("GhosttyString", GString { "ptr" => ptr, "len" => len }),
        layout!("GhosttyStyleColor", StyleColor { "tag" => tag, "value" => value }),
        layout!("GhosttyStyle", Style {
            "size" => size, "fg_color" => fg_color, "bg_color" => bg_color,
            "underline_color" => underline_color, "bold" => bold, "italic" => italic,
            "faint" => faint, "blink" => blink, "inverse" => inverse, "invisible" => invisible,
            "strikethrough" => strikethrough, "overline" => overline, "underline" => underline,
        }),
        layout!("GhosttyRenderStateCursor", RenderCursor {
            "size" => size, "viewport_has_value" => viewport_has_value, "viewport_x" => viewport_x,
            "viewport_y" => viewport_y, "wide_tail" => wide_tail, "visible" => visible,
            "blinking" => blinking, "password_input" => password_input, "visual_style" => visual_style,
        }),
        layout!("GhosttyRenderStateColors", RenderColors {
            "size" => size, "background" => background, "foreground" => foreground,
            "cursor" => cursor, "cursor_has_value" => cursor_has_value, "palette" => palette,
        }),
        layout!("GhosttyRenderStateRowSelection", RowSelection {
            "size" => size, "start_x" => start_x, "end_x" => end_x,
        }),
        layout!("GhosttyRenderStateRowId", RowId { "bits" => bits }),
        layout!("GhosttyCellsView", CellsView { "ptr" => ptr, "len" => len }),
        layout!("GhosttyTerminalScrollbar", Scrollbar { "total" => total, "offset" => offset, "len" => len }),
        layout!("GhosttyTerminalScrollViewport", ScrollViewport { "tag" => tag, "value" => value }),
        layout!("GhosttyTerminalModeConfig", ModeConfig { "mode" => mode, "value" => value }),
        layout!("GhosttyGridRef", GridRef { "size" => size, "node" => node, "x" => x, "y" => y }),
        layout!("GhosttyPointCoordinate", PointCoordinate { "x" => x, "y" => y }),
        layout!("GhosttyPointValue", PointValue { "coordinate" => coordinate, "_padding" => _padding }),
        layout!("GhosttyPoint", Point { "tag" => tag, "value" => value }),
        layout!("GhosttySelection", Selection {
            "size" => size, "start" => start, "end" => end, "rectangle" => rectangle,
        }),
        layout!("GhosttyTerminalSelectionFormatOptions", SelectionFormatOptions {
            "size" => size, "emit" => emit, "unwrap" => unwrap, "trim" => trim, "selection" => selection,
        }),
        layout!("GhosttyFormatterScreenExtra", FormatterScreenExtra {
            "size" => size, "cursor" => cursor, "style" => style, "hyperlink" => hyperlink,
            "protection" => protection, "kitty_keyboard" => kitty_keyboard, "charsets" => charsets,
        }),
        layout!("GhosttyFormatterTerminalExtra", FormatterTerminalExtra {
            "size" => size, "palette" => palette, "modes" => modes,
            "scrolling_region" => scrolling_region, "tabstops" => tabstops, "pwd" => pwd,
            "keyboard" => keyboard, "screen" => screen,
        }),
        layout!("GhosttyFormatterTerminalOptions", FormatterTerminalOptions {
            "size" => size, "emit" => emit, "unwrap" => unwrap, "trim" => trim,
            "extra" => extra, "selection" => selection,
        }),
        layout!("GhosttyMouseEncoderSize", MouseEncoderSize {
            "size" => size, "screen_width" => screen_width, "screen_height" => screen_height,
            "cell_width" => cell_width, "cell_height" => cell_height, "padding_top" => padding_top,
            "padding_bottom" => padding_bottom, "padding_right" => padding_right,
            "padding_left" => padding_left,
        }),
        layout!("GhosttyMousePosition", MousePosition { "x" => x, "y" => y }),
        layout!("GhosttySizeReportSize", SizeReportSize {
            "rows" => rows, "columns" => columns, "cell_width" => cell_width, "cell_height" => cell_height,
        }),
        layout!("GhosttyDeviceAttributesPrimary", DeviceAttributesPrimary {
            "conformance_level" => conformance_level, "features" => features,
            "num_features" => num_features,
        }),
        layout!("GhosttyDeviceAttributesSecondary", DeviceAttributesSecondary {
            "device_type" => device_type, "firmware_version" => firmware_version,
            "rom_cartridge" => rom_cartridge,
        }),
        layout!("GhosttyDeviceAttributesTertiary", DeviceAttributesTertiary { "unit_id" => unit_id }),
        layout!("GhosttyDeviceAttributes", DeviceAttributes {
            "primary" => primary, "secondary" => secondary, "tertiary" => tertiary,
        }),
        layout!("GhosttyTerminalDesktopNotification", DesktopNotification {
            "size" => size, "title" => title, "body" => body,
        }),
        layout!("GhosttyTerminalProgressReport", ProgressReport {
            "size" => size, "state" => state, "progress" => progress,
        }),
    ]
}

/// 格子（GhosttyCell，64 位）里一段位。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Bits {
    lsb: u32,
    width: u32,
}

impl Bits {
    pub fn of(self, v: u64) -> u64 {
        (v >> self.lsb) & ((1u64 << self.width) - 1)
    }
}

/// 格子各字段在哪几位，从清单读。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CellBits {
    pub tag: Bits,
    pub codepoint: Bits,
    pub style_id: Bits,
    pub wide: Bits,
    pub hyperlink: Bits,
    pub palette_index: Bits,
    pub r: Bits,
    pub g: Bits,
    pub b: Bits,
}

/// 拿清单核对这里的结构体和枚举值，读出格子的位。
pub(crate) fn check_manifest(json: &str) -> Result<CellBits, String> {
    let m: Value = serde_json::from_str(json).map_err(|e| format!("类型清单读不了：{e}"))?;
    let ptr = m["abi"]["pointer_size"].as_u64();
    if ptr != Some(size_of::<usize>() as u64) {
        return Err(format!("库的指针是 {ptr:?} 字节"));
    }
    let types = &m["types"];
    for (name, size, fields) in layouts() {
        let t = &types[name];
        if t.is_null() {
            return Err(format!("清单里没有 {name}"));
        }
        if t["size"].as_u64() != Some(size as u64) {
            return Err(format!("{name} 是 {} 字节，这里是 {size}", t["size"]));
        }
        for (field, off) in fields {
            let got = t["fields"][field]["offset"].as_u64();
            if got != Some(off as u64) {
                return Err(format!("{name}.{field} 在 {got:?}，这里是 {off}"));
            }
        }
    }
    for &(ty, name, value) in ENUMS {
        let got = types[ty]["values"][name].as_i64();
        if got != Some(value) {
            return Err(format!("{ty} {name} 是 {got:?}，这里是 {value}"));
        }
    }
    for &key in crate::input::Key::ALL {
        let name = key.manifest_name();
        let got = types["GhosttyKey"]["values"][name.as_str()].as_i64();
        if got != Some(key as i64) {
            return Err(format!(
                "GhosttyKey {name} 是 {got:?}，这里是 {}",
                key as i64
            ));
        }
    }

    let c = &types["GhosttyCell"];
    if c["kind"] != "packed" || c["size"].as_u64() != Some(8) {
        return Err("GhosttyCell 不是 64 位的压缩值".into());
    }
    let bits = &c["bits"];
    let field = |name: &str| -> Result<Bits, String> {
        let b = &bits[name];
        match (b["lsb"].as_u64(), b["width"].as_u64()) {
            (Some(lsb), Some(width)) if lsb + width <= 64 && width > 0 => Ok(Bits {
                lsb: lsb as u32,
                width: width as u32,
            }),
            _ => Err(format!("GhosttyCell 没有 {name}")),
        }
    };
    let content = field("content")?;
    let arm = |tag: &str, name: &str| -> Result<Bits, String> {
        let b = &bits["content"]["arms"][tag]["bits"][name];
        match (b["lsb"].as_u64(), b["width"].as_u64()) {
            (Some(lsb), Some(width)) if lsb + width <= content.width as u64 && width > 0 => {
                Ok(Bits {
                    lsb: content.lsb + lsb as u32,
                    width: width as u32,
                })
            }
            _ => Err(format!("GhosttyCell 的 content 没有 {tag}.{name}")),
        }
    };
    Ok(CellBits {
        tag: field("content_tag")?,
        style_id: field("style_id")?,
        wide: field("wide")?,
        hyperlink: field("hyperlink")?,
        codepoint: arm("CODEPOINT", "codepoint")?,
        palette_index: arm("BG_COLOR_PALETTE", "index")?,
        r: arm("BG_COLOR_RGB", "r")?,
        g: arm("BG_COLOR_RGB", "g")?,
        b: arm("BG_COLOR_RGB", "b")?,
    })
}
