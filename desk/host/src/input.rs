//! 远程输入注入。macOS 用 CGEvent：鼠标按坐标、滚轮按行、键盘按 keycode + 修饰键，
//! 文本用 Unicode 字符串注入（不依赖键盘布局）。需要「辅助功能」权限，没有就静默失败。

#[cfg(not(target_os = "macos"))]
use lolidesk_proto::DeskInput;

#[cfg(target_os = "macos")]
pub use mac::Injector;

#[cfg(not(target_os = "macos"))]
pub struct Injector;

#[cfg(not(target_os = "macos"))]
impl Injector {
    pub fn new(_display: u32) -> Self {
        Self
    }
    pub fn apply(&mut self, _i: &DeskInput) {}
}

#[cfg(target_os = "macos")]
mod mac {
    #![allow(unused_unsafe, non_upper_case_globals)]

    use std::time::Instant;

    use lolidesk_proto::{DeskInput, Key, Mods, MouseButton};
    use objc2_core_foundation::{CFRetained, CGPoint, CGRect};
    use objc2_core_graphics::{
        CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
        CGEventTapLocation, CGEventType, CGMouseButton, CGScrollEventUnit,
    };

    /// CGEventSource 是线程安全的 CoreFoundation 对象；objc2 没标 Send，这里包一层让 Injector 能放进 tokio 任务。
    struct Source(CFRetained<CGEventSource>);
    unsafe impl Send for Source {}

    pub struct Injector {
        bounds: CGRect,
        src: Option<Source>,
        down: [bool; 3],
        pos: CGPoint,
        last_click: Option<(Instant, CGPoint, MouseButton)>,
        clicks: i64,
    }

    impl Injector {
        pub fn new(display: u32) -> Self {
            let bounds = unsafe { CGDisplayBounds(display) };
            let src = CGEventSource::new(CGEventSourceStateID::HIDSystemState).map(Source);
            Self {
                bounds,
                src,
                down: [false; 3],
                pos: CGPoint {
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                },
                last_click: None,
                clicks: 0,
            }
        }

        fn point(&self, x: f32, y: f32) -> CGPoint {
            CGPoint {
                x: self.bounds.origin.x + (x.clamp(0.0, 1.0) as f64) * self.bounds.size.width,
                y: self.bounds.origin.y + (y.clamp(0.0, 1.0) as f64) * self.bounds.size.height,
            }
        }

        fn post(&self, ev: Option<CFRetained<CGEvent>>) {
            if let Some(ev) = ev {
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
            }
        }

        fn flags(mods: Mods) -> CGEventFlags {
            let mut f = CGEventFlags::empty();
            if mods.shift() {
                f |= CGEventFlags::MaskShift;
            }
            if mods.ctrl() {
                f |= CGEventFlags::MaskControl;
            }
            if mods.alt() {
                f |= CGEventFlags::MaskAlternate;
            }
            if mods.sup() {
                f |= CGEventFlags::MaskCommand;
            }
            f
        }

        fn mouse_move(&mut self, p: CGPoint) {
            self.pos = p;
            let ty = if self.down[0] {
                CGEventType::LeftMouseDragged
            } else if self.down[1] {
                CGEventType::RightMouseDragged
            } else if self.down[2] {
                CGEventType::OtherMouseDragged
            } else {
                CGEventType::MouseMoved
            };
            let btn = if self.down[2] {
                CGMouseButton::Center
            } else if self.down[1] {
                CGMouseButton::Right
            } else {
                CGMouseButton::Left
            };
            self.post(CGEvent::new_mouse_event(
                self.src.as_ref().map(|s| &*s.0),
                ty,
                p,
                btn,
            ));
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
            if down {
                let now = Instant::now();
                self.clicks = match self.last_click {
                    Some((t, lp, lb))
                        if lb == button
                            && now.duration_since(t).as_millis() < 400
                            && (lp.x - p.x).abs() < 5.0
                            && (lp.y - p.y).abs() < 5.0 =>
                    {
                        self.clicks + 1
                    }
                    _ => 1,
                };
                self.last_click = Some((now, p, button));
            }
            self.down[idx] = down;
            self.pos = p;
            let ev = CGEvent::new_mouse_event(self.src.as_ref().map(|s| &*s.0), ty, p, cgb);
            if let Some(ev) = &ev {
                CGEvent::set_integer_value_field(
                    Some(ev),
                    CGEventField::MouseEventClickState,
                    self.clicks.max(1),
                );
            }
            self.post(ev);
        }

        fn scroll(&mut self, p: CGPoint, dx: f32, dy: f32) {
            if p.x != self.pos.x || p.y != self.pos.y {
                self.mouse_move(p);
            }
            let (ly, lx) = (dy.round() as i32, dx.round() as i32);
            if ly == 0 && lx == 0 {
                return;
            }
            self.post(CGEvent::new_scroll_wheel_event2(
                self.src.as_ref().map(|s| &*s.0),
                CGScrollEventUnit::Line,
                2,
                ly,
                lx,
                0,
            ));
        }

        fn key(&mut self, key: Key, mods: Mods, down: bool) {
            let (code, shifted, text) = keycode(key);
            let mut flags = Self::flags(mods);
            if shifted {
                flags |= CGEventFlags::MaskShift;
            }
            let ev = CGEvent::new_keyboard_event(self.src.as_ref().map(|s| &*s.0), code, down);
            if let Some(ev) = &ev {
                CGEvent::set_flags(Some(ev), flags);
                // 普通字符：带上 Unicode 文本，不依赖远端键盘布局；有 Cmd / Ctrl 时靠 keycode
                if let Some(c) = text {
                    if !mods.ctrl() && !mods.sup() && !mods.alt() {
                        let mut buf = [0u16; 2];
                        let u = c.encode_utf16(&mut buf);
                        unsafe {
                            CGEvent::keyboard_set_unicode_string(Some(ev), u.len() as _, u.as_ptr())
                        };
                    }
                }
            }
            self.post(ev);
        }

        fn text(&mut self, s: &str) {
            let units: Vec<u16> = s.encode_utf16().collect();
            for chunk in units.chunks(20) {
                for down in [true, false] {
                    let ev = CGEvent::new_keyboard_event(self.src.as_ref().map(|s| &*s.0), 0, down);
                    if let Some(ev) = &ev {
                        CGEvent::set_flags(Some(ev), CGEventFlags::empty());
                        unsafe {
                            CGEvent::keyboard_set_unicode_string(
                                Some(ev),
                                chunk.len() as _,
                                chunk.as_ptr(),
                            )
                        };
                    }
                    self.post(ev);
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
                DeskInput::Key { key, mods, down } => self.key(*key, *mods, *down),
                DeskInput::Text(s) => self.text(s),
            }
        }
    }

    /// 美式键盘的虚拟键码。返回 (keycode, 需要 shift, 作为文本注入的字符)。
    fn keycode(key: Key) -> (u16, bool, Option<char>) {
        match key {
            Key::Enter => (36, false, None),
            Key::Tab => (48, false, None),
            Key::Space => (49, false, Some(' ')),
            Key::Backspace => (51, false, None),
            Key::Escape => (53, false, None),
            Key::Left => (123, false, None),
            Key::Right => (124, false, None),
            Key::Down => (125, false, None),
            Key::Up => (126, false, None),
            Key::Home => (115, false, None),
            Key::End => (119, false, None),
            Key::PageUp => (116, false, None),
            Key::PageDown => (121, false, None),
            Key::Delete => (117, false, None),
            Key::Insert => (114, false, None),
            Key::F(n) => (
                match n {
                    1 => 122,
                    2 => 120,
                    3 => 99,
                    4 => 118,
                    5 => 96,
                    6 => 97,
                    7 => 98,
                    8 => 100,
                    9 => 101,
                    10 => 109,
                    11 => 103,
                    12 => 111,
                    _ => 0,
                },
                false,
                None,
            ),
            Key::Char(c) => {
                let lower = c.to_ascii_lowercase();
                let (code, shifted) = match lower {
                    'a' => (0, false),
                    's' => (1, false),
                    'd' => (2, false),
                    'f' => (3, false),
                    'h' => (4, false),
                    'g' => (5, false),
                    'z' => (6, false),
                    'x' => (7, false),
                    'c' => (8, false),
                    'v' => (9, false),
                    'b' => (11, false),
                    'q' => (12, false),
                    'w' => (13, false),
                    'e' => (14, false),
                    'r' => (15, false),
                    'y' => (16, false),
                    't' => (17, false),
                    '1' => (18, false),
                    '!' => (18, true),
                    '2' => (19, false),
                    '@' => (19, true),
                    '3' => (20, false),
                    '#' => (20, true),
                    '4' => (21, false),
                    '$' => (21, true),
                    '6' => (22, false),
                    '^' => (22, true),
                    '5' => (23, false),
                    '%' => (23, true),
                    '=' => (24, false),
                    '+' => (24, true),
                    '9' => (25, false),
                    '(' => (25, true),
                    '7' => (26, false),
                    '&' => (26, true),
                    '-' => (27, false),
                    '_' => (27, true),
                    '8' => (28, false),
                    '*' => (28, true),
                    '0' => (29, false),
                    ')' => (29, true),
                    ']' => (30, false),
                    '}' => (30, true),
                    'o' => (31, false),
                    'u' => (32, false),
                    '[' => (33, false),
                    '{' => (33, true),
                    'i' => (34, false),
                    'p' => (35, false),
                    'l' => (37, false),
                    'j' => (38, false),
                    '\'' => (39, false),
                    '"' => (39, true),
                    'k' => (40, false),
                    ';' => (41, false),
                    ':' => (41, true),
                    '\\' => (42, false),
                    '|' => (42, true),
                    ',' => (43, false),
                    '<' => (43, true),
                    '/' => (44, false),
                    '?' => (44, true),
                    'n' => (45, false),
                    'm' => (46, false),
                    '.' => (47, false),
                    '>' => (47, true),
                    '`' => (50, false),
                    '~' => (50, true),
                    _ => (0, false),
                };
                (code, shifted || c.is_ascii_uppercase(), Some(c))
            }
        }
    }
}
