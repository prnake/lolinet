//! 物理键位：macOS 虚拟键码 ↔ W3C `KeyboardEvent.code` 的名字。
//!
//! GPUI 的 `Keystroke` 只有按当前键盘布局换算过的 `key`，远端要的是位置（物理键）。
//! GPUI 处理按键时是同步从 NSEvent 派发下来的，所以在回调里读
//! `NSApplication.currentEvent.keyCode` 就是这次按键的物理键码（[`current_key_code`]）。
//! 没有当前事件（测试、合成事件）时退回按 GPUI 的键名猜（[`key_name_to_code`]）。
//!
//! 表和主机的 `desk/host/src/input.rs::mac_keycode` 互为反表，改一边要改另一边。

use lolidesk_proto::Mods;

/// macOS 虚拟键码 → W3C code。别名（SuperLeft、Help……）只取 W3C 的正式名字。
pub fn vk_to_code(vk: u16) -> Option<&'static str> {
    Some(match vk {
        0 => "KeyA",
        1 => "KeyS",
        2 => "KeyD",
        3 => "KeyF",
        4 => "KeyH",
        5 => "KeyG",
        6 => "KeyZ",
        7 => "KeyX",
        8 => "KeyC",
        9 => "KeyV",
        10 => "IntlBackslash",
        11 => "KeyB",
        12 => "KeyQ",
        13 => "KeyW",
        14 => "KeyE",
        15 => "KeyR",
        16 => "KeyY",
        17 => "KeyT",
        18 => "Digit1",
        19 => "Digit2",
        20 => "Digit3",
        21 => "Digit4",
        22 => "Digit6",
        23 => "Digit5",
        24 => "Equal",
        25 => "Digit9",
        26 => "Digit7",
        27 => "Minus",
        28 => "Digit8",
        29 => "Digit0",
        30 => "BracketRight",
        31 => "KeyO",
        32 => "KeyU",
        33 => "BracketLeft",
        34 => "KeyI",
        35 => "KeyP",
        36 => "Enter",
        37 => "KeyL",
        38 => "KeyJ",
        39 => "Quote",
        40 => "KeyK",
        41 => "Semicolon",
        42 => "Backslash",
        43 => "Comma",
        44 => "Slash",
        45 => "KeyN",
        46 => "KeyM",
        47 => "Period",
        48 => "Tab",
        49 => "Space",
        50 => "Backquote",
        51 => "Backspace",
        53 => "Escape",
        54 => "MetaRight",
        55 => "MetaLeft",
        56 => "ShiftLeft",
        57 => "CapsLock",
        58 => "AltLeft",
        59 => "ControlLeft",
        60 => "ShiftRight",
        61 => "AltRight",
        62 => "ControlRight",
        63 => "Fn",
        64 => "F17",
        65 => "NumpadDecimal",
        67 => "NumpadMultiply",
        69 => "NumpadAdd",
        71 => "NumLock",
        72 => "AudioVolumeUp",
        73 => "AudioVolumeDown",
        74 => "AudioVolumeMute",
        75 => "NumpadDivide",
        76 => "NumpadEnter",
        78 => "NumpadSubtract",
        79 => "F18",
        80 => "F19",
        81 => "NumpadEqual",
        82 => "Numpad0",
        83 => "Numpad1",
        84 => "Numpad2",
        85 => "Numpad3",
        86 => "Numpad4",
        87 => "Numpad5",
        88 => "Numpad6",
        89 => "Numpad7",
        90 => "F20",
        91 => "Numpad8",
        92 => "Numpad9",
        93 => "IntlYen",
        94 => "IntlRo",
        95 => "NumpadComma",
        96 => "F5",
        97 => "F6",
        98 => "F7",
        99 => "F3",
        100 => "F8",
        101 => "F9",
        102 => "Lang2",
        103 => "F11",
        104 => "Lang1",
        105 => "F13",
        106 => "F16",
        107 => "F14",
        109 => "F10",
        110 => "ContextMenu",
        111 => "F12",
        113 => "F15",
        114 => "Insert",
        115 => "Home",
        116 => "PageUp",
        117 => "Delete",
        118 => "F4",
        119 => "End",
        120 => "F2",
        121 => "PageDown",
        122 => "F1",
        123 => "ArrowLeft",
        124 => "ArrowRight",
        125 => "ArrowDown",
        126 => "ArrowUp",
        _ => return None,
    })
}

/// 修饰键的键码属于哪一个修饰（按 [`Mods`] 的位）。Caps Lock、Fn 不算。
pub fn modifier_bit(vk: u16) -> Option<u8> {
    match vk {
        56 | 60 => Some(Mods::SHIFT),
        59 | 62 => Some(Mods::CTRL),
        58 | 61 => Some(Mods::ALT),
        55 | 54 => Some(Mods::SUPER),
        _ => None,
    }
}

/// 修饰位 → 左边那个键的 code（没有物理键码时用）。
pub fn modifier_code(bit: u8) -> Option<&'static str> {
    match bit {
        Mods::SHIFT => Some("ShiftLeft"),
        Mods::CTRL => Some("ControlLeft"),
        Mods::ALT => Some("AltLeft"),
        Mods::SUPER => Some("MetaLeft"),
        _ => None,
    }
}

/// GPUI 的键名（`Keystroke::key`，小写、按布局）→ W3C code。只在拿不到物理键码时用，按美式布局猜。
pub fn key_name_to_code(key: &str) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK",
        "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV",
        "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = [
        "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
        "Digit9",
    ];
    const FKEYS: [&str; 24] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
        "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    let lower = key.to_ascii_lowercase();
    let mut chars = lower.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        match c {
            'a'..='z' => return Some(LETTERS[(c as u8 - b'a') as usize]),
            '0'..='9' => return Some(DIGITS[(c as u8 - b'0') as usize]),
            _ => {}
        }
    }
    if let Some(n) = lower
        .strip_prefix('f')
        .and_then(|n| n.parse::<usize>().ok())
        && (1..=24).contains(&n)
    {
        return Some(FKEYS[n - 1]);
    }
    Some(match lower.as_str() {
        "tab" => "Tab",
        "enter" | "return" => "Enter",
        "escape" => "Escape",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "insert" => "Insert",
        "space" | " " => "Space",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "-" => "Minus",
        "=" => "Equal",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "\\" => "Backslash",
        ";" => "Semicolon",
        "'" => "Quote",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        "`" => "Backquote",
        "shift" => "ShiftLeft",
        "control" => "ControlLeft",
        "alt" => "AltLeft",
        "platform" | "cmd" => "MetaLeft",
        "capslock" => "CapsLock",
        _ => return None,
    })
}

/// 修饰键状态 → 协议的位。
pub fn mods(shift: bool, ctrl: bool, alt: bool, sup: bool) -> Mods {
    let mut m = 0;
    if shift {
        m |= Mods::SHIFT;
    }
    if ctrl {
        m |= Mods::CTRL;
    }
    if alt {
        m |= Mods::ALT;
    }
    if sup {
        m |= Mods::SUPER;
    }
    Mods(m)
}

/// 正在派发的 NSEvent 的物理键码：只有键盘事件（按下、松开、修饰键变化）有，别的事件读 keyCode 会抛异常。
/// 不在主线程或没有当前事件时返回 None。
#[cfg(target_os = "macos")]
pub fn current_key_code() -> Option<u16> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEventType};

    let mtm = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(mtm);
    let event = app.currentEvent()?;
    let kind = event.r#type();
    if kind == NSEventType::KeyDown
        || kind == NSEventType::KeyUp
        || kind == NSEventType::FlagsChanged
    {
        Some(event.keyCode())
    } else {
        None
    }
}

/// 修饰键单独按下 / 松开时判断方向的状态：记着哪些修饰键（键码）按着。
#[derive(Debug, Default, Clone)]
pub struct ModifierTracker {
    held: Vec<u16>,
    mods: u8,
}

impl ModifierTracker {
    /// 修饰键变化。`vk` 是这次变化的物理键码（拿不到就是 None），`now` 是变化后的修饰位。
    /// 返回要发的 (code, down) 序列。
    pub fn change(&mut self, vk: Option<u16>, now: u8) -> Vec<(&'static str, bool)> {
        let mut out = Vec::new();
        let before = self.mods;
        self.mods = now;
        if let Some(vk) = vk {
            if vk == 57 {
                // Caps Lock 每次变化是一次按下松开
                out.push(("CapsLock", true));
                out.push(("CapsLock", false));
                return out;
            }
            if let (Some(bit), Some(code)) = (modifier_bit(vk), vk_to_code(vk)) {
                let held = self.held.contains(&vk);
                if now & bit == 0 {
                    // 这个修饰整个松开了：两边按着的都松
                    let mut released: Vec<u16> = self
                        .held
                        .iter()
                        .copied()
                        .filter(|k| modifier_bit(*k) == Some(bit))
                        .collect();
                    if released.is_empty() {
                        released.push(vk);
                    }
                    self.held.retain(|k| modifier_bit(*k) != Some(bit));
                    for k in released {
                        if let Some(c) = vk_to_code(k) {
                            out.push((c, false));
                        }
                    }
                } else if held {
                    // 另一边还按着：这一边松开
                    self.held.retain(|k| *k != vk);
                    out.push((code, false));
                } else {
                    self.held.push(vk);
                    out.push((code, true));
                }
                return out;
            }
        }
        // 没有物理键码：按位的差猜左边的键
        for bit in [Mods::SHIFT, Mods::CTRL, Mods::ALT, Mods::SUPER] {
            let (was, is) = (before & bit != 0, now & bit != 0);
            if was != is
                && let Some(code) = modifier_code(bit)
            {
                out.push((code, is));
            }
        }
        out
    }

    /// 全部松开（失焦后远端已经 ReleaseAll）。
    pub fn reset(&mut self) {
        self.held.clear();
        self.mods = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_checks() {
        assert_eq!(vk_to_code(48), Some("Tab"));
        assert_eq!(vk_to_code(56), Some("ShiftLeft"));
        assert_eq!(vk_to_code(55), Some("MetaLeft"));
        assert_eq!(vk_to_code(126), Some("ArrowUp"));
        assert_eq!(vk_to_code(0), Some("KeyA"));
        assert_eq!(vk_to_code(122), Some("F1"));
        assert_eq!(vk_to_code(52), None);
        assert_eq!(vk_to_code(200), None);
    }

    /// 和主机的 mac_keycode 互为反表：每个键码只出现一次，名字不重复。
    #[test]
    fn table_is_a_bijection() {
        let mut seen = std::collections::HashSet::new();
        for vk in 0..=200u16 {
            if let Some(code) = vk_to_code(vk) {
                assert!(seen.insert(code), "{code} 出现了两次");
            }
        }
        assert_eq!(seen.len(), 120, "和主机的 mac_keycode 一样多");
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name_to_code("a"), Some("KeyA"));
        assert_eq!(key_name_to_code("Z"), Some("KeyZ"));
        assert_eq!(key_name_to_code("7"), Some("Digit7"));
        assert_eq!(key_name_to_code("tab"), Some("Tab"));
        assert_eq!(key_name_to_code("f12"), Some("F12"));
        assert_eq!(key_name_to_code("up"), Some("ArrowUp"));
        assert_eq!(key_name_to_code("["), Some("BracketLeft"));
        assert_eq!(key_name_to_code("f0"), None);
        assert_eq!(key_name_to_code("é"), None);
    }

    #[test]
    fn modifier_tracking() {
        let mut t = ModifierTracker::default();
        // 左 Shift 按下、右 Shift 按下、右松、左松
        assert_eq!(t.change(Some(56), Mods::SHIFT), [("ShiftLeft", true)]);
        assert_eq!(t.change(Some(60), Mods::SHIFT), [("ShiftRight", true)]);
        assert_eq!(t.change(Some(60), Mods::SHIFT), [("ShiftRight", false)]);
        assert_eq!(t.change(Some(56), 0), [("ShiftLeft", false)]);
        // Cmd 按下，然后整个松开时两边都松
        assert_eq!(t.change(Some(55), Mods::SUPER), [("MetaLeft", true)]);
        assert_eq!(
            t.change(Some(54), Mods::SUPER),
            [("MetaRight", true)],
            "另一边也按下"
        );
        let mut up = t.change(Some(55), 0);
        up.sort();
        assert_eq!(up, [("MetaLeft", false), ("MetaRight", false)]);
        // 没有键码：按位猜
        assert_eq!(
            t.change(None, Mods::CTRL | Mods::ALT),
            [("ControlLeft", true), ("AltLeft", true)]
        );
        assert_eq!(t.change(None, Mods::ALT), [("ControlLeft", false)]);
        // Caps Lock
        assert_eq!(
            t.change(Some(57), Mods::ALT),
            [("CapsLock", true), ("CapsLock", false)]
        );
    }

    #[test]
    fn mods_bits() {
        assert_eq!(mods(true, false, false, true).0, Mods::SHIFT | Mods::SUPER);
        assert_eq!(mods(false, true, true, false).0, Mods::CTRL | Mods::ALT);
    }
}
