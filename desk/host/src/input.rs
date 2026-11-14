//! 远程输入注入。macOS 用 CGEvent：鼠标按坐标、滚动按像素、键盘按物理键位（W3C code → 虚拟键码），
//! 文本用 Unicode 字符串注入（不依赖键盘布局）。需要「辅助功能」权限，没有就静默失败。
//!
//! 修饰键是这里最容易出错的地方：
//! - 修饰键本身要按下、松开（发它自己的键码），光在字母键上设 flags 不够：很多程序看的是修饰键状态；
//! - 每个键盘、鼠标事件的 flags 都按「此刻按着哪些修饰键」重算（Shift+Tab、Cmd+点击才对）；
//! - 事件源用私有状态（Private），不和真键盘的状态表混，也不会越按越多；
//! - 修饰键变化后停 8 ms，让 WindowServer 先看到它，再发后面的键。
//!
//! 注入跑在自己的线程上：上面那 8 ms 不能卡住网络任务。
//!
//! 人优先（照 Cindy 的 USER_INPUT_ACTIVE）：agent 的输入在「有人刚用过」时不做。人是这台电脑前的人
//! （系统的 HID 空闲时间，减掉我们自己注入的那些），或者远程桌面里正在操作的人。

use std::sync::atomic::{AtomicU64, Ordering};

use lolidesk_proto::DeskInput;

use crate::desktop::Area;

/// 推着一个窗口时，远程点它之前它被别的窗口盖住了（这台电脑前的人点了别的）：先提上来，
/// 不然点击落在盖着它的那个窗口上。提完等 WindowServer 换好层级
fn bring_front(id: u32) {
    if crate::desktop::is_front(id) {
        return;
    }
    match crate::desktop::raise_window(id) {
        Ok(()) => std::thread::sleep(std::time::Duration::from_millis(120)),
        Err(e) => tracing::debug!("提不了窗口 {id}：{e:#}"),
    }
}

/// 最近一次远程桌面里的人注入输入的时刻（Unix 毫秒）
pub static VIEWER_LAST: AtomicU64 = AtomicU64::new(0);
/// 最近一次 agent（computer use）注入输入的时刻
pub static AGENT_LAST: AtomicU64 = AtomicU64::new(0);
/// 人动过之后这么久之内，agent 的输入先不做
pub const HOLD_MS: u64 = 3000;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 有人在用这台电脑：返回人上一次输入距今的毫秒；没人在用是 None
pub fn person_active() -> Option<u64> {
    decide(
        now_ms(),
        VIEWER_LAST.load(Ordering::Relaxed),
        AGENT_LAST.load(Ordering::Relaxed),
        imp::last_hid_ms(),
    )
}

/// `hid_last`：系统最后一次键鼠事件的时刻（含我们注入的）。它晚于我们自己最后一次注入 500 ms 以上，才算人
fn decide(now: u64, viewer: u64, agent: u64, hid_last: Option<u64>) -> Option<u64> {
    if viewer > 0 && now.saturating_sub(viewer) < HOLD_MS {
        return Some(now.saturating_sub(viewer));
    }
    let hid = hid_last?;
    let ours = viewer.max(agent);
    (now.saturating_sub(hid) < HOLD_MS && hid > ours + 500).then(|| now.saturating_sub(hid))
}

/// 鼠标在哪：(显示器, x, y)，x / y 是 0..1
pub fn cursor() -> Option<(u32, f32, f32)> {
    imp::cursor()
}

enum Msg {
    Input(DeskInput),
    /// 前面的都做完了就回一声
    Flush(tokio::sync::oneshot::Sender<()>),
    /// 坐标换算改按这一块（推窗口时是窗口的框，窗口挪了跟着换）
    Area(Area),
    /// 推的是这个窗口：按下鼠标前它不在最前面就先提上来（None 是推整块显示器）
    Window(Option<u32>),
}

pub struct Injector {
    tx: std::sync::mpsc::Sender<Msg>,
}

impl Injector {
    pub fn new(display: u32) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let _ = std::thread::Builder::new()
            .name("input".into())
            .spawn(move || {
                let mut inj = imp::Injector::new(display);
                let mut window: Option<u32> = None;
                while let Ok(m) = rx.recv() {
                    match m {
                        Msg::Input(i) => {
                            if let (Some(id), DeskInput::Button { down: true, .. }) = (window, &i) {
                                bring_front(id);
                            }
                            inj.apply(&i)
                        }
                        Msg::Flush(done) => {
                            let _ = done.send(());
                        }
                        Msg::Area(a) => inj.set_area(a),
                        Msg::Window(w) => window = w,
                    }
                }
                inj.apply(&DeskInput::ReleaseAll);
            });
        Self { tx }
    }

    pub fn apply(&self, i: DeskInput) {
        let _ = self.tx.send(Msg::Input(i));
    }

    /// 坐标按这一块换算（显示器的范围、窗口的框）
    pub fn set_area(&self, a: Area) {
        let _ = self.tx.send(Msg::Area(a));
    }

    /// 推的是这个窗口（None：整块显示器）
    pub fn set_window(&self, w: Option<u32>) {
        let _ = self.tx.send(Msg::Window(w));
    }

    /// 等前面排着的输入都做完（agent 做完马上截图，要看到结果）
    pub async fn flush(&self) {
        let (done, wait) = tokio::sync::oneshot::channel();
        if self.tx.send(Msg::Flush(done)).is_ok() {
            let _ = wait.await;
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use lolidesk_proto::DeskInput;

    pub fn last_hid_ms() -> Option<u64> {
        None
    }

    pub fn cursor() -> Option<(u32, f32, f32)> {
        None
    }

    pub struct Injector;

    impl Injector {
        pub fn new(_display: u32) -> Self {
            Self
        }
        pub fn apply(&mut self, _i: &DeskInput) {}
        pub fn set_area(&mut self, _a: crate::desktop::Area) {}
    }
}

/// W3C `KeyboardEvent.code` → macOS 虚拟键码（kVK_*，按 ANSI 布局的位置）。
/// 修饰键左右分开；Super / OS 是 Meta 的别名（不同客户端叫法不同）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn mac_keycode(code: &str) -> Option<u16> {
    Some(match code {
        "KeyA" => 0,
        "KeyS" => 1,
        "KeyD" => 2,
        "KeyF" => 3,
        "KeyH" => 4,
        "KeyG" => 5,
        "KeyZ" => 6,
        "KeyX" => 7,
        "KeyC" => 8,
        "KeyV" => 9,
        "IntlBackslash" => 10,
        "KeyB" => 11,
        "KeyQ" => 12,
        "KeyW" => 13,
        "KeyE" => 14,
        "KeyR" => 15,
        "KeyY" => 16,
        "KeyT" => 17,
        "Digit1" => 18,
        "Digit2" => 19,
        "Digit3" => 20,
        "Digit4" => 21,
        "Digit6" => 22,
        "Digit5" => 23,
        "Equal" => 24,
        "Digit9" => 25,
        "Digit7" => 26,
        "Minus" => 27,
        "Digit8" => 28,
        "Digit0" => 29,
        "BracketRight" => 30,
        "KeyO" => 31,
        "KeyU" => 32,
        "BracketLeft" => 33,
        "KeyI" => 34,
        "KeyP" => 35,
        "Enter" => 36,
        "KeyL" => 37,
        "KeyJ" => 38,
        "Quote" => 39,
        "KeyK" => 40,
        "Semicolon" => 41,
        "Backslash" => 42,
        "Comma" => 43,
        "Slash" => 44,
        "KeyN" => 45,
        "KeyM" => 46,
        "Period" => 47,
        "Tab" => 48,
        "Space" => 49,
        "Backquote" => 50,
        "Backspace" => 51,
        "Escape" => 53,
        "MetaRight" | "SuperRight" | "OSRight" => 54,
        "MetaLeft" | "SuperLeft" | "OSLeft" => 55,
        "ShiftLeft" => 56,
        "CapsLock" => 57,
        "AltLeft" => 58,
        "ControlLeft" => 59,
        "ShiftRight" => 60,
        "AltRight" => 61,
        "ControlRight" => 62,
        "Fn" => 63,
        "F17" => 64,
        "NumpadDecimal" => 65,
        "NumpadMultiply" => 67,
        "NumpadAdd" => 69,
        "NumLock" | "NumpadClear" => 71,
        "AudioVolumeUp" => 72,
        "AudioVolumeDown" => 73,
        "AudioVolumeMute" => 74,
        "NumpadDivide" => 75,
        "NumpadEnter" => 76,
        "NumpadSubtract" => 78,
        "F18" => 79,
        "F19" => 80,
        "NumpadEqual" => 81,
        "Numpad0" => 82,
        "Numpad1" => 83,
        "Numpad2" => 84,
        "Numpad3" => 85,
        "Numpad4" => 86,
        "Numpad5" => 87,
        "Numpad6" => 88,
        "Numpad7" => 89,
        "F20" => 90,
        "Numpad8" => 91,
        "Numpad9" => 92,
        "IntlYen" => 93,
        "IntlRo" => 94,
        "NumpadComma" => 95,
        "F5" => 96,
        "F6" => 97,
        "F7" => 98,
        "F3" => 99,
        "F8" => 100,
        "F9" => 101,
        "Lang2" => 102,
        "F11" => 103,
        "Lang1" => 104,
        "F13" => 105,
        "F16" => 106,
        "F14" => 107,
        "F10" => 109,
        "ContextMenu" => 110,
        "F12" => 111,
        "F15" => 113,
        "Insert" | "Help" => 114,
        "Home" => 115,
        "PageUp" => 116,
        "Delete" => 117,
        "F4" => 118,
        "End" => 119,
        "F2" => 120,
        "PageDown" => 121,
        "F1" => 122,
        "ArrowLeft" => 123,
        "ArrowRight" => 124,
        "ArrowDown" => 125,
        "ArrowUp" => 126,
        _ => return None,
    })
}

/// 修饰键的键码属于哪一个修饰（按 Mods 的位）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn modifier_bit(keycode: u16) -> Option<u8> {
    use lolidesk_proto::Mods;
    match keycode {
        56 | 60 => Some(Mods::SHIFT),
        59 | 62 => Some(Mods::CTRL),
        58 | 61 => Some(Mods::ALT),
        55 | 54 => Some(Mods::SUPER),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
mod imp {
    #![allow(unused_unsafe, non_upper_case_globals)]

    use std::time::{Duration, Instant};

    use lolidesk_proto::{DeskInput, Mods, MouseButton};
    use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
    use objc2_core_graphics::{
        CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
        CGEventTapLocation, CGEventType, CGMouseButton, CGScrollEventUnit,
    };

    use super::{mac_keycode, modifier_bit};

    /// 系统最后一次键鼠事件（HID 层，含注入的）的时刻
    pub fn last_hid_ms() -> Option<u64> {
        // kCGAnyInputEventType
        let any = CGEventType(u32::MAX);
        let idle =
            CGEventSource::seconds_since_last_event_type(CGEventSourceStateID::HIDSystemState, any);
        (idle.is_finite() && idle >= 0.0)
            .then(|| super::now_ms().saturating_sub((idle * 1000.0) as u64))
    }

    pub fn cursor() -> Option<(u32, f32, f32)> {
        let ev = CGEvent::new(None)?;
        let p = CGEvent::location(Some(&ev));
        let mut ids = [0u32; 4];
        let mut n = 0u32;
        let err = unsafe {
            objc2_core_graphics::CGGetDisplaysWithPoint(
                p,
                ids.len() as u32,
                ids.as_mut_ptr(),
                &mut n,
            )
        };
        if err != objc2_core_graphics::CGError::Success || n == 0 {
            return None;
        }
        let id = ids[0];
        let b = unsafe { CGDisplayBounds(id) };
        let x = ((p.x - b.origin.x) / b.size.width.max(1.0)) as f32;
        let y = ((p.y - b.origin.y) / b.size.height.max(1.0)) as f32;
        Some((id, x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
    }

    /// 修饰键变化后等 WindowServer 看到它。
    const SETTLE: Duration = Duration::from_millis(8);

    /// 每个修饰位按下时用哪个键码（左边那个）。
    const MOD_KEYS: [(u8, u16); 4] = [
        (Mods::SHIFT, 56),
        (Mods::CTRL, 59),
        (Mods::ALT, 58),
        (Mods::SUPER, 55),
    ];

    pub struct Injector {
        bounds: CGRect,
        src: Option<CFRetained<CGEventSource>>,
        /// 按着的修饰键（键码）
        mods_down: Vec<u16>,
        /// 按着的普通键（键码）
        keys_down: Vec<u16>,
        buttons: [bool; 3],
        pos: CGPoint,
        last_click: Option<(Instant, CGPoint, MouseButton)>,
        clicks: i64,
        /// 像素滚动的小数部分，攒到整数再发
        scroll_rest: (f64, f64),
    }

    impl Injector {
        pub fn new(display: u32) -> Self {
            let bounds = unsafe { CGDisplayBounds(display) };
            Self {
                bounds,
                src: CGEventSource::new(CGEventSourceStateID::Private),
                mods_down: Vec::new(),
                keys_down: Vec::new(),
                buttons: [false; 3],
                pos: CGPoint {
                    x: bounds.origin.x + bounds.size.width / 2.0,
                    y: bounds.origin.y + bounds.size.height / 2.0,
                },
                last_click: None,
                clicks: 0,
                scroll_rest: (0.0, 0.0),
            }
        }

        fn src(&self) -> Option<&CGEventSource> {
            self.src.as_deref()
        }

        /// 坐标改按这一块换算（推窗口：窗口的框）
        pub fn set_area(&mut self, a: crate::desktop::Area) {
            if a.w > 0.0 && a.h > 0.0 {
                self.bounds = CGRect {
                    origin: CGPoint { x: a.x, y: a.y },
                    size: objc2_core_foundation::CGSize {
                        width: a.w,
                        height: a.h,
                    },
                };
            }
        }

        fn point(&self, x: f32, y: f32) -> CGPoint {
            let b = self.bounds;
            CGPoint {
                x: b.origin.x + (x.clamp(0.0, 1.0) as f64) * (b.size.width - 1.0).max(0.0),
                y: b.origin.y + (y.clamp(0.0, 1.0) as f64) * (b.size.height - 1.0).max(0.0),
            }
        }

        /// 此刻按着的修饰键对应的 flags。
        fn held_flags(&self) -> CGEventFlags {
            let mut f = CGEventFlags::empty();
            for k in &self.mods_down {
                f |= match modifier_bit(*k) {
                    Some(Mods::SHIFT) => CGEventFlags::MaskShift,
                    Some(Mods::CTRL) => CGEventFlags::MaskControl,
                    Some(Mods::ALT) => CGEventFlags::MaskAlternate,
                    Some(Mods::SUPER) => CGEventFlags::MaskCommand,
                    _ => CGEventFlags::empty(),
                };
            }
            f
        }

        fn held_mods(&self) -> u8 {
            self.mods_down
                .iter()
                .filter_map(|k| modifier_bit(*k))
                .fold(0, |a, b| a | b)
        }

        /// 留下事件自带的标志（方向键的 NumericPad / SecondaryFn 这些），去掉修饰位，再按住的修饰键补上。
        fn set_flags(&self, ev: &CGEvent) {
            let strip = CGEventFlags::MaskShift
                | CGEventFlags::MaskControl
                | CGEventFlags::MaskAlternate
                | CGEventFlags::MaskCommand
                | CGEventFlags::MaskAlphaShift;
            let own = CGEvent::flags(Some(ev)) & !strip;
            CGEvent::set_flags(Some(ev), own | self.held_flags());
        }

        fn post(ev: &CGEvent) {
            CGEvent::post(CGEventTapLocation::HIDEventTap, Some(ev));
        }

        fn post_key(&self, keycode: u16, down: bool, repeat: bool) {
            if let Some(ev) = CGEvent::new_keyboard_event(self.src(), keycode, down) {
                self.set_flags(&ev);
                if repeat {
                    CGEvent::set_integer_value_field(
                        Some(&ev),
                        CGEventField::KeyboardEventAutorepeat,
                        1,
                    );
                }
                Self::post(&ev);
            }
        }

        fn modifier(&mut self, keycode: u16, down: bool) {
            let held = self.mods_down.contains(&keycode);
            if down == held {
                return;
            }
            if down {
                self.mods_down.push(keycode);
            } else {
                self.mods_down.retain(|k| *k != keycode);
            }
            // flags 是变化之后的状态：按下 Shift 的事件本身就带 Shift
            self.post_key(keycode, down, false);
            std::thread::sleep(SETTLE);
            // macOS 上 Cmd 按着时按下的键，客户端常常收不到它们的松开；Cmd 一松就把它们都放掉
            if !down && modifier_bit(keycode) == Some(Mods::SUPER) {
                for k in std::mem::take(&mut self.keys_down) {
                    self.post_key(k, false, false);
                }
            }
        }

        /// 让主机记的修饰键和客户端说的一致（客户端可能在失焦时漏发了松开）。
        fn sync_mods(&mut self, want: Mods) {
            let have = self.held_mods();
            for (bit, key) in MOD_KEYS {
                let w = want.0 & bit != 0;
                let h = have & bit != 0;
                if w && !h {
                    self.modifier(key, true);
                } else if !w && h {
                    let held: Vec<u16> = self
                        .mods_down
                        .iter()
                        .copied()
                        .filter(|k| modifier_bit(*k) == Some(bit))
                        .collect();
                    for k in held {
                        self.modifier(k, false);
                    }
                }
            }
        }

        fn key(&mut self, code: &str, down: bool, mods: Mods) {
            let Some(keycode) = mac_keycode(code) else {
                tracing::debug!(code, "不认识的键");
                return;
            };
            if modifier_bit(keycode).is_some() {
                self.modifier(keycode, down);
                return;
            }
            if keycode == 57 {
                // Caps Lock：macOS 把它当成状态切换，注入一次按下就够
                if down {
                    self.post_key(keycode, true, false);
                    self.post_key(keycode, false, false);
                }
                return;
            }
            if down {
                self.sync_mods(mods);
            }
            let repeat = down && self.keys_down.contains(&keycode);
            if down && !repeat {
                self.keys_down.push(keycode);
            }
            if !down {
                if !self.keys_down.contains(&keycode) {
                    return;
                }
                self.keys_down.retain(|k| *k != keycode);
            }
            self.post_key(keycode, down, repeat);
        }

        fn text(&mut self, s: &str) {
            let units: Vec<u16> = s.encode_utf16().collect();
            for chunk in units.chunks(20) {
                for down in [true, false] {
                    if let Some(ev) = CGEvent::new_keyboard_event(self.src(), 0, down) {
                        CGEvent::set_flags(Some(&ev), CGEventFlags::empty());
                        unsafe {
                            CGEvent::keyboard_set_unicode_string(
                                Some(&ev),
                                chunk.len() as _,
                                chunk.as_ptr(),
                            )
                        };
                        Self::post(&ev);
                    }
                }
            }
        }

        fn mouse_event(&self, ty: CGEventType, p: CGPoint, btn: CGMouseButton, clicks: i64) {
            if let Some(ev) = CGEvent::new_mouse_event(self.src(), ty, p, btn) {
                self.set_flags(&ev);
                if clicks > 0 {
                    CGEvent::set_integer_value_field(
                        Some(&ev),
                        CGEventField::MouseEventClickState,
                        clicks,
                    );
                }
                Self::post(&ev);
            }
        }

        fn mouse_move(&mut self, p: CGPoint) {
            self.pos = p;
            let (ty, btn) = if self.buttons[0] {
                (CGEventType::LeftMouseDragged, CGMouseButton::Left)
            } else if self.buttons[1] {
                (CGEventType::RightMouseDragged, CGMouseButton::Right)
            } else if self.buttons[2] {
                (CGEventType::OtherMouseDragged, CGMouseButton::Center)
            } else {
                (CGEventType::MouseMoved, CGMouseButton::Left)
            };
            self.mouse_event(ty, p, btn, 0);
        }

        fn button(&mut self, p: CGPoint, button: MouseButton, down: bool) {
            let (idx, cgb, ty) = match (button, down) {
                (MouseButton::Left, true) => (0, CGMouseButton::Left, CGEventType::LeftMouseDown),
                (MouseButton::Left, false) => (0, CGMouseButton::Left, CGEventType::LeftMouseUp),
                (MouseButton::Right, true) => {
                    (1, CGMouseButton::Right, CGEventType::RightMouseDown)
                }
                (MouseButton::Right, false) => (1, CGMouseButton::Right, CGEventType::RightMouseUp),
                (MouseButton::Middle, true) => {
                    (2, CGMouseButton::Center, CGEventType::OtherMouseDown)
                }
                (MouseButton::Middle, false) => {
                    (2, CGMouseButton::Center, CGEventType::OtherMouseUp)
                }
            };
            if !down && !self.buttons[idx] {
                return;
            }
            if down {
                let now = Instant::now();
                self.clicks = match self.last_click {
                    Some((t, lp, lb))
                        if lb == button
                            && now.duration_since(t) < Duration::from_millis(500)
                            && (lp.x - p.x).abs() < 5.0
                            && (lp.y - p.y).abs() < 5.0 =>
                    {
                        (self.clicks + 1).min(3)
                    }
                    _ => 1,
                };
                self.last_click = Some((now, p, button));
            }
            self.buttons[idx] = down;
            self.pos = p;
            self.mouse_event(ty, p, cgb, self.clicks.max(1));
        }

        fn scroll(&mut self, p: CGPoint, dx: f32, dy: f32) {
            if p.x != self.pos.x || p.y != self.pos.y {
                self.mouse_move(p);
            }
            // 归一化 → 像素（点）。按画面高度换算，横竖同一个比例
            let h = self.bounds.size.height;
            let fx = self.scroll_rest.0 + dx as f64 * h;
            let fy = self.scroll_rest.1 + dy as f64 * h;
            let (ix, iy) = (fx.trunc() as i32, fy.trunc() as i32);
            self.scroll_rest = (fx - ix as f64, fy - iy as f64);
            if ix == 0 && iy == 0 {
                return;
            }
            if let Some(ev) =
                CGEvent::new_scroll_wheel_event2(self.src(), CGScrollEventUnit::Pixel, 2, iy, ix, 0)
            {
                self.set_flags(&ev);
                CGEvent::set_integer_value_field(
                    Some(&ev),
                    CGEventField::ScrollWheelEventIsContinuous,
                    1,
                );
                Self::post(&ev);
            }
        }

        fn release_all(&mut self) {
            for k in std::mem::take(&mut self.keys_down) {
                self.post_key(k, false, false);
            }
            for k in self.mods_down.clone() {
                self.modifier(k, false);
            }
            let p = self.pos;
            for (i, b) in [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
                .into_iter()
                .enumerate()
            {
                if self.buttons[i] {
                    self.button(p, b, false);
                }
            }
        }

        pub fn apply(&mut self, i: &DeskInput) {
            match i {
                DeskInput::Move { x, y } => {
                    let p = self.point(*x, *y);
                    self.mouse_move(p);
                }
                DeskInput::Button { x, y, button, down } => {
                    let p = self.point(*x, *y);
                    self.button(p, *button, *down);
                }
                DeskInput::Scroll { x, y, dx, dy } => {
                    let p = self.point(*x, *y);
                    self.scroll(p, *dx, *dy);
                }
                DeskInput::Key { code, down, mods } => self.key(code, *down, *mods),
                DeskInput::Text(s) => self.text(s),
                DeskInput::ReleaseAll => self.release_all(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn person_first() {
        let now = 1_000_000;
        // 没人动过
        assert_eq!(decide(now, 0, 0, Some(now - 60_000)), None);
        assert_eq!(decide(now, 0, 0, None), None);
        // 这台电脑前的人 1 秒前动过
        assert_eq!(decide(now, 0, 0, Some(now - 1000)), Some(1000));
        // 那是 agent 自己刚注入的（HID 时刻和注入时刻挨着）
        assert_eq!(decide(now, 0, now - 1000, Some(now - 990)), None);
        // agent 注入之后人又动了
        assert_eq!(decide(now, 0, now - 2000, Some(now - 500)), Some(500));
        // 远程桌面里的人 2 秒前在操作
        assert_eq!(decide(now, now - 2000, 0, Some(now - 2000)), Some(2000));
        // 远程桌面的人 4 秒前操作过，之后没人
        assert_eq!(decide(now, now - 4000, 0, Some(now - 4000)), None);
    }

    #[test]
    fn keycodes() {
        assert_eq!(mac_keycode("Tab"), Some(48));
        assert_eq!(mac_keycode("ShiftLeft"), Some(56));
        assert_eq!(mac_keycode("SuperLeft"), mac_keycode("MetaLeft"));
        assert_eq!(mac_keycode("KeyA"), Some(0));
        assert_eq!(mac_keycode("ArrowUp"), Some(126));
        assert_eq!(mac_keycode("Nope"), None);
        assert_eq!(modifier_bit(60), Some(lolidesk_proto::Mods::SHIFT));
        assert_eq!(modifier_bit(48), None);
    }
}
