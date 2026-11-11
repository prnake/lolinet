//! 输入编码：键、鼠标、粘贴、焦点，按终端的模式编成程序读的字节。
//!
//! 照 mygo plugins/terminal/internal/vt/encode.go 移植（MIT，Copyright (c) 2026 MyGo contributors）。

use std::{
    ffi::c_char,
    ops::{BitAnd, BitOr, BitOrAssign},
    ptr,
};

use anyhow::Result;

use crate::{
    ffi::{self, api, Handle},
    terminal::{Mode, Terminal},
};

macro_rules! keys {
    ($($name:ident),* $(,)?) => {
        /// 一个键（GhosttyKey），按它在键盘上的位置，名字同 W3C 的 `KeyboardEvent.code`。
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        #[repr(i32)]
        #[allow(clippy::enum_variant_names)]
        pub enum Key {
            #[default]
            $($name),*
        }

        impl Key {
            /// 全部的键，按 GhosttyKey 的顺序。
            pub const ALL: &'static [Key] = &[$(Key::$name),*];

            /// 从 W3C 的 `KeyboardEvent.code`（"KeyA"、"Tab"、"ShiftLeft"、"ArrowUp"、
            /// "Numpad1"……）找键；不认识的是 Unidentified。
            pub fn from_code(code: &str) -> Key {
                match code {
                    $(stringify!($name) => Key::$name,)*
                    // 老浏览器的叫法。
                    "OSLeft" => Key::MetaLeft,
                    "OSRight" => Key::MetaRight,
                    _ => Key::Unidentified,
                }
            }

            /// W3C 的名字。
            pub fn code(self) -> &'static str {
                match self {
                    $(Key::$name => stringify!($name),)*
                }
            }
        }
    };
}

keys! {
    Unidentified,
    // 书写键（W3C § 3.1.1）
    Backquote, Backslash, BracketLeft, BracketRight, Comma,
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    Equal, IntlBackslash, IntlRo, IntlYen,
    KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
    KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    Minus, Period, Quote, Semicolon, Slash,
    // 功能键（§ 3.1.2）
    AltLeft, AltRight, Backspace, CapsLock, ContextMenu, ControlLeft, ControlRight, Enter,
    MetaLeft, MetaRight, ShiftLeft, ShiftRight, Space, Tab, Convert, KanaMode, NonConvert,
    // 控制区（§ 3.2）
    Delete, End, Help, Home, Insert, PageDown, PageUp,
    // 方向键（§ 3.3）
    ArrowDown, ArrowLeft, ArrowRight, ArrowUp,
    // 小键盘（§ 3.4）
    NumLock, Numpad0, Numpad1, Numpad2, Numpad3, Numpad4, Numpad5, Numpad6, Numpad7, Numpad8,
    Numpad9, NumpadAdd, NumpadBackspace, NumpadClear, NumpadClearEntry, NumpadComma,
    NumpadDecimal, NumpadDivide, NumpadEnter, NumpadEqual, NumpadMemoryAdd, NumpadMemoryClear,
    NumpadMemoryRecall, NumpadMemoryStore, NumpadMemorySubtract, NumpadMultiply,
    NumpadParenLeft, NumpadParenRight, NumpadSubtract, NumpadSeparator, NumpadUp, NumpadDown,
    NumpadRight, NumpadLeft, NumpadBegin, NumpadHome, NumpadEnd, NumpadInsert, NumpadDelete,
    NumpadPageUp, NumpadPageDown,
    // 功能区（§ 3.5）
    Escape, F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12, F13, F14, F15, F16, F17, F18,
    F19, F20, F21, F22, F23, F24, F25, Fn, FnLock, PrintScreen, ScrollLock, Pause,
    // 媒体键（§ 3.6）
    BrowserBack, BrowserFavorites, BrowserForward, BrowserHome, BrowserRefresh, BrowserSearch,
    BrowserStop, Eject, LaunchApp1, LaunchApp2, LaunchMail, MediaPlayPause, MediaSelect,
    MediaStop, MediaTrackNext, MediaTrackPrevious, Power, Sleep, AudioVolumeDown,
    AudioVolumeMute, AudioVolumeUp, WakeUp,
    // 旧的、特殊的（§ 3.7）
    Copy, Cut, Paste,
}

impl Key {
    /// 在类型清单里的名字：KeyA → A，Digit0 → DIGIT_0，LaunchApp1 → LAUNCH_APP_1。
    pub(crate) fn manifest_name(self) -> String {
        let code = self.code();
        let code = match code.strip_prefix("Key") {
            Some(rest) if rest.len() == 1 => rest,
            _ => code,
        };
        let mut out = String::new();
        let mut prev: Option<char> = None;
        for c in code.chars() {
            if let Some(p) = prev {
                if (c.is_ascii_uppercase() && p.is_ascii_lowercase())
                    || (c.is_ascii_digit() && p.is_ascii_lowercase())
                {
                    out.push('_');
                }
            }
            out.push(c.to_ascii_uppercase());
            prev = Some(c);
        }
        out
    }
}

/// 修饰键（GhosttyMods）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods(pub u16);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const SHIFT: Mods = Mods(1 << 0);
    pub const CTRL: Mods = Mods(1 << 1);
    pub const ALT: Mods = Mods(1 << 2);
    pub const SUPER: Mods = Mods(1 << 3);
    pub const CAPS_LOCK: Mods = Mods(1 << 4);
    pub const NUM_LOCK: Mods = Mods(1 << 5);
    /// 按的是右边的 Shift（有 SHIFT 时才有意义）；下面几个同理。
    pub const SHIFT_RIGHT: Mods = Mods(1 << 6);
    pub const CTRL_RIGHT: Mods = Mods(1 << 7);
    pub const ALT_RIGHT: Mods = Mods(1 << 8);
    pub const SUPER_RIGHT: Mods = Mods(1 << 9);

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Mods {
    type Output = Mods;
    fn bitor(self, rhs: Mods) -> Mods {
        Mods(self.0 | rhs.0)
    }
}

impl BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Mods) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Mods {
    type Output = Mods;
    fn bitand(self, rhs: Mods) -> Mods {
        Mods(self.0 & rhs.0)
    }
}

/// 键怎么了。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyAction {
    Release,
    #[default]
    Press,
    Repeat,
}

impl KeyAction {
    fn to_ffi(self) -> i32 {
        match self {
            KeyAction::Release => ffi::key_action::RELEASE,
            KeyAction::Press => ffi::key_action::PRESS,
            KeyAction::Repeat => ffi::key_action::REPEAT,
        }
    }
}

/// 一次按键。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyEvent<'a> {
    pub action: KeyAction,
    pub key: Key,
    pub mods: Mods,
    /// 打出 text 用掉的修饰键，比如打 "A" 的 Shift。
    pub consumed_mods: Mods,
    /// 这个键打出的字（不算 Ctrl、Alt 的变化）；不打字的键是空的。
    pub text: &'a str,
    /// 不按修饰键时这个键打出的字符。
    pub unshifted: Option<char>,
    /// 输入法正在组字。
    pub composing: bool,
}

/// macOS 上 Option 当不当 Alt。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptionAsAlt {
    #[default]
    No,
    Yes,
    Left,
    Right,
}

/// 键盘编码器：按终端的模式编成传统序列、xterm 的 modifyOtherKeys 或 Kitty 键盘协议。
pub struct KeyEncoder {
    h: Handle,
    ev: Handle,
    buf: Vec<u8>,
    option_as_alt: OptionAsAlt,
}

// SAFETY：句柄只经 &mut self 用，库不把编码器绑在线程上。
unsafe impl Send for KeyEncoder {}

impl KeyEncoder {
    /// 新建（会先载入库）。
    pub fn new() -> Result<KeyEncoder> {
        let api = &ffi::lib_checked()?.api;
        let mut e = KeyEncoder {
            h: ptr::null_mut(),
            ev: ptr::null_mut(),
            buf: vec![0; 128],
            option_as_alt: OptionAsAlt::No,
        };
        // SAFETY：出参；失败时 Drop 只释放建好了的。
        unsafe {
            ffi::check(
                (api.key_encoder_new)(ptr::null(), &mut e.h),
                "新建键盘编码器",
            )?;
            ffi::check((api.key_event_new)(ptr::null(), &mut e.ev), "新建按键事件")?;
        }
        Ok(e)
    }

    /// 照终端现在的模式设好（光标键、小键盘、Alt、modifyOtherKeys、Kitty 标志……）。
    /// 每次编码前调一次，或者在每帧之后。
    pub fn sync(&mut self, t: &Terminal) {
        // SAFETY：两个句柄都有效。
        unsafe { (api().key_encoder_setopt_from_terminal)(self.h, t.handle()) };
        self.apply_option_as_alt();
    }

    /// macOS 上 Option 当不当 Alt。
    pub fn set_option_as_alt(&mut self, v: OptionAsAlt) {
        self.option_as_alt = v;
        self.apply_option_as_alt();
    }

    fn apply_option_as_alt(&mut self) {
        let v = match self.option_as_alt {
            OptionAsAlt::No => ffi::option_as_alt::FALSE,
            OptionAsAlt::Yes => ffi::option_as_alt::TRUE,
            OptionAsAlt::Left => ffi::option_as_alt::LEFT,
            OptionAsAlt::Right => ffi::option_as_alt::RIGHT,
        };
        self.setopt(ffi::key_opt::MACOS_OPTION_AS_ALT, &v);
    }

    fn setopt<T>(&mut self, opt: i32, v: &T) {
        // SAFETY：h 有效；T 是这个选项要的类型（调用处保证）。
        unsafe { (api().key_encoder_setopt)(self.h, opt, v as *const T as *const _) };
    }

    /// 编一个键；不发东西的键是空的。结果在下次调用前有效。
    pub fn encode(&mut self, ev: &KeyEvent<'_>) -> &[u8] {
        let api = api();
        // SAFETY：ev 句柄有效；utf8 指着 ev.text，编完马上清掉，不会留下悬空指针。
        unsafe {
            (api.key_event_set_action)(self.ev, ev.action.to_ffi());
            (api.key_event_set_key)(self.ev, ev.key as i32);
            (api.key_event_set_mods)(self.ev, ev.mods.0);
            (api.key_event_set_consumed_mods)(self.ev, ev.consumed_mods.0);
            (api.key_event_set_composing)(self.ev, ev.composing);
            (api.key_event_set_unshifted_codepoint)(self.ev, ev.unshifted.map_or(0, |c| c as u32));
            if ev.text.is_empty() {
                (api.key_event_set_utf8)(self.ev, ptr::null(), 0);
            } else {
                (api.key_event_set_utf8)(self.ev, ev.text.as_ptr() as *const c_char, ev.text.len());
            }
        }
        let mut n = 0usize;
        let mut r = self.encode_into(&mut n);
        if r == ffi::OUT_OF_SPACE && n > self.buf.len() {
            self.buf.resize(n, 0);
            r = self.encode_into(&mut n);
        }
        // SAFETY：同上，清掉借来的文字。
        unsafe { (api.key_event_set_utf8)(self.ev, ptr::null(), 0) };
        if r != 0 {
            return &[];
        }
        &self.buf[..n.min(self.buf.len())]
    }

    fn encode_into(&mut self, n: &mut usize) -> i32 {
        // SAFETY：两个句柄有效，buf 有 len 字节。
        unsafe {
            (api().key_encoder_encode)(
                self.h,
                self.ev,
                self.buf.as_mut_ptr() as *mut c_char,
                self.buf.len(),
                n,
            )
        }
    }
}

impl Drop for KeyEncoder {
    fn drop(&mut self) {
        let api = api();
        // SAFETY：只释放建好了的。
        unsafe {
            if !self.ev.is_null() {
                (api.key_event_free)(self.ev);
            }
            if !self.h.is_null() {
                (api.key_encoder_free)(self.h);
            }
        }
    }
}

/// modifyOtherKeys 2 开着没有：没有 Kitty 协议时，它开着 Ctrl+P 是 `CSI 27;5;112~`，不然是 ^P。
pub(crate) fn modify_other_keys(t: &Terminal) -> bool {
    let Ok(mut e) = KeyEncoder::new() else {
        return false;
    };
    // SAFETY：两个句柄都有效。
    unsafe { (api().key_encoder_setopt_from_terminal)(e.h, t.handle()) };
    e.setopt(ffi::key_opt::KITTY_FLAGS, &0u8);
    e.encode(&KeyEvent {
        action: KeyAction::Press,
        key: Key::KeyP,
        mods: Mods::CTRL,
        text: "p",
        unshifted: Some('p'),
        ..KeyEvent::default()
    })
    .starts_with(b"\x1b[27;")
}

/// 鼠标怎么了。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum MouseAction {
    #[default]
    Press,
    Release,
    Motion,
}

/// 鼠标键：4、5 是滚轮上下，6、7 是滚轮左右。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    Back,
    Forward,
    Ten,
    Eleven,
}

impl MouseButton {
    fn to_ffi(self) -> i32 {
        use ffi::mouse_button::*;
        match self {
            MouseButton::Left => LEFT,
            MouseButton::Right => RIGHT,
            MouseButton::Middle => MIDDLE,
            MouseButton::WheelUp => FOUR,
            MouseButton::WheelDown => FIVE,
            MouseButton::WheelLeft => SIX,
            MouseButton::WheelRight => SEVEN,
            MouseButton::Back => EIGHT,
            MouseButton::Forward => NINE,
            MouseButton::Ten => TEN,
            MouseButton::Eleven => ELEVEN,
        }
    }
}

/// 一次鼠标事件：位置是相对终端区域左上角的像素。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MouseEvent {
    pub action: MouseAction,
    /// 哪个键；单纯移动是 None。
    pub button: Option<MouseButton>,
    pub mods: Mods,
    pub x: f32,
    pub y: f32,
}

/// 终端区域的像素尺寸：整个区域、一格、四边留白。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseSize {
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

/// 鼠标编码器：按程序要的事件和格式编；程序没要的是空的。
pub struct MouseEncoder {
    h: Handle,
    ev: Handle,
    buf: [u8; 64],
}

// SAFETY：句柄只经 &mut self 用，库不把编码器绑在线程上。
unsafe impl Send for MouseEncoder {}

impl MouseEncoder {
    /// 新建（会先载入库）。移动只在换了格子时报，同 xterm。
    pub fn new() -> Result<MouseEncoder> {
        let api = &ffi::lib_checked()?.api;
        let mut m = MouseEncoder {
            h: ptr::null_mut(),
            ev: ptr::null_mut(),
            buf: [0; 64],
        };
        // SAFETY：出参；失败时 Drop 只释放建好了的。
        unsafe {
            ffi::check(
                (api.mouse_encoder_new)(ptr::null(), &mut m.h),
                "新建鼠标编码器",
            )?;
            ffi::check(
                (api.mouse_event_new)(ptr::null(), &mut m.ev),
                "新建鼠标事件",
            )?;
        }
        m.setopt(ffi::mouse_opt::TRACK_LAST_CELL, &true);
        Ok(m)
    }

    fn setopt<T>(&mut self, opt: i32, v: &T) {
        // SAFETY：h 有效；T 是这个选项要的类型（调用处保证）。
        unsafe { (api().mouse_encoder_setopt)(self.h, opt, v as *const T as *const _) };
    }

    /// 照终端现在的鼠标模式设好，再给区域尺寸和有没有键按着。每次编码前调。
    pub fn sync(&mut self, t: &Terminal, size: &MouseSize, any_button_pressed: bool) {
        // SAFETY：两个句柄都有效。
        unsafe { (api().mouse_encoder_setopt_from_terminal)(self.h, t.handle()) };
        let s = ffi::MouseEncoderSize {
            size: std::mem::size_of::<ffi::MouseEncoderSize>(),
            screen_width: size.screen_width.max(1),
            screen_height: size.screen_height.max(1),
            cell_width: size.cell_width.max(1),
            cell_height: size.cell_height.max(1),
            padding_top: size.padding_top,
            padding_bottom: size.padding_bottom,
            padding_right: size.padding_right,
            padding_left: size.padding_left,
        };
        self.setopt(ffi::mouse_opt::SIZE, &s);
        self.setopt(ffi::mouse_opt::ANY_BUTTON_PRESSED, &any_button_pressed);
    }

    /// 忘掉上次报的格子（换了终端、重新进入时）。
    pub fn reset(&mut self) {
        // SAFETY：h 有效。
        unsafe { (api().mouse_encoder_reset)(self.h) };
    }

    /// 编一次事件；程序没要这种事件时是空的。结果在下次调用前有效。
    pub fn encode(&mut self, ev: &MouseEvent) -> &[u8] {
        let api = api();
        let action = match ev.action {
            MouseAction::Press => ffi::mouse_action::PRESS,
            MouseAction::Release => ffi::mouse_action::RELEASE,
            MouseAction::Motion => ffi::mouse_action::MOTION,
        };
        let mut n = 0usize;
        // SAFETY：句柄有效，buf 有 len 字节。
        let r = unsafe {
            (api.mouse_event_set_action)(self.ev, action);
            match ev.button {
                Some(b) => (api.mouse_event_set_button)(self.ev, b.to_ffi()),
                None => (api.mouse_event_clear_button)(self.ev),
            }
            (api.mouse_event_set_mods)(self.ev, ev.mods.0);
            (api.mouse_event_set_position)(self.ev, ffi::MousePosition { x: ev.x, y: ev.y });
            (api.mouse_encoder_encode)(
                self.h,
                self.ev,
                self.buf.as_mut_ptr() as *mut c_char,
                self.buf.len(),
                &mut n,
            )
        };
        if r != 0 {
            return &[];
        }
        &self.buf[..n.min(self.buf.len())]
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        let api = api();
        // SAFETY：只释放建好了的。
        unsafe {
            if !self.ev.is_null() {
                (api.mouse_event_free)(self.ev);
            }
            if !self.h.is_null() {
                (api.mouse_encoder_free)(self.h);
            }
        }
    }
}

/// 粘贴：开了括号粘贴（模式 2004）就包起来，没开就把换行变回车；能自己跑命令的控制字符
/// 换成空格。
pub fn paste(t: &Terminal, text: &str) -> Vec<u8> {
    encode_paste(text, t.mode(Mode::BRACKETED_PASTE))
}

/// 粘贴，括号粘贴开没开自己给。会先载入库；载入不了是空的。
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if text.is_empty() {
        return Vec::new();
    }
    let Ok(lib) = ffi::lib_checked() else {
        return Vec::new();
    };
    let api = &lib.api;
    let mut out = vec![0u8; text.len() + 16];
    let mut n = 0usize;
    for _ in 0..2 {
        // 编码会改输入，每次用一份新的。
        let mut data = text.as_bytes().to_vec();
        // SAFETY：data、out 各有 len 字节，n 是出参。
        let r = unsafe {
            (api.paste_encode)(
                data.as_mut_ptr() as *mut c_char,
                data.len(),
                bracketed,
                out.as_mut_ptr() as *mut c_char,
                out.len(),
                &mut n,
            )
        };
        if r == ffi::OUT_OF_SPACE && n > out.len() {
            out.resize(n, 0);
            continue;
        }
        if r != 0 {
            return Vec::new();
        }
        out.truncate(n);
        return out;
    }
    Vec::new()
}

/// 粘贴这段文字不会自己跑起命令来（没有换行、没有括号粘贴的结束序列）。
/// 会先载入库；载入不了时自己按这两条查。
pub fn paste_is_safe(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let Ok(lib) = ffi::lib_checked() else {
        return !text.contains(['\n', '\r']) && !text.contains("\x1b[201~");
    };
    // SAFETY：text 有 len 字节。
    unsafe { (lib.api.paste_is_safe)(text.as_ptr() as *const c_char, text.len()) }
}

/// 终端得到或失去焦点时报给程序的字节；程序没开焦点报告（模式 1004）就是空的。
pub fn focus(t: &Terminal, gained: bool) -> Vec<u8> {
    if !t.mode(Mode::FOCUS_EVENT) {
        return Vec::new();
    }
    let event = if gained {
        ffi::focus::GAINED
    } else {
        ffi::focus::LOST
    };
    let mut buf = [0u8; 8];
    let mut n = 0usize;
    // SAFETY：buf 有 len 字节，n 是出参。
    let r =
        unsafe { (api().focus_encode)(event, buf.as_mut_ptr() as *mut c_char, buf.len(), &mut n) };
    if r != 0 {
        return Vec::new();
    }
    buf[..n.min(buf.len())].to_vec()
}
