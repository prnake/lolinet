//! 键、鼠标、粘贴、焦点的编码跟着终端的模式走。

use super::lib;
use crate::{
    encode_paste, focus, paste, paste_is_safe, Key, KeyAction, KeyEncoder, KeyEvent, Mods,
    MouseAction, MouseButton, MouseEncoder, MouseEvent, MouseSize, OptionAsAlt, Terminal,
};

fn key(e: &mut KeyEncoder, t: &Terminal, key: Key, mods: Mods, text: &str) -> Vec<u8> {
    e.sync(t);
    e.encode(&KeyEvent {
        action: KeyAction::Press,
        key,
        mods,
        text,
        unshifted: text.chars().next(),
        ..KeyEvent::default()
    })
    .to_vec()
}

#[test]
fn keys() {
    if !lib() {
        return;
    }
    let mut t = Terminal::new(80, 24, Box::new(())).unwrap();
    let mut e = KeyEncoder::new().unwrap();
    e.set_option_as_alt(OptionAsAlt::Yes);
    assert_eq!(key(&mut e, &t, Key::Tab, Mods::NONE, ""), b"\t");
    assert_eq!(key(&mut e, &t, Key::Tab, Mods::SHIFT, ""), b"\x1b[Z");
    assert_eq!(key(&mut e, &t, Key::KeyA, Mods::NONE, "a"), b"a");
    assert_eq!(key(&mut e, &t, Key::Enter, Mods::NONE, ""), b"\r");
    assert_eq!(key(&mut e, &t, Key::Backspace, Mods::NONE, ""), b"\x7f");
    assert_eq!(key(&mut e, &t, Key::ArrowUp, Mods::NONE, ""), b"\x1b[A");
    assert_eq!(key(&mut e, &t, Key::KeyC, Mods::CTRL, "c"), b"\x03");
    assert_eq!(key(&mut e, &t, Key::KeyX, Mods::ALT, "x"), b"\x1bx");
    assert_eq!(key(&mut e, &t, Key::F1, Mods::NONE, ""), b"\x1bOP");
    // 应用光标键。
    t.write(b"\x1b[?1h");
    assert_eq!(key(&mut e, &t, Key::ArrowUp, Mods::NONE, ""), b"\x1bOA");
    t.write(b"\x1b[?1l");
    assert_eq!(key(&mut e, &t, Key::ArrowUp, Mods::NONE, ""), b"\x1b[A");
    // Kitty 键盘协议：Ctrl+C 是 CSI u。
    t.write(b"\x1b[>1u");
    assert_eq!(t.kitty_keyboard_flags(), 1);
    assert_eq!(key(&mut e, &t, Key::KeyC, Mods::CTRL, "c"), b"\x1b[99;5u");
    t.write(b"\x1b[<u");
    assert_eq!(key(&mut e, &t, Key::KeyC, Mods::CTRL, "c"), b"\x03");
    // modifyOtherKeys 2。
    assert!(!t.modify_other_keys());
    t.write(b"\x1b[>4;2m");
    assert!(t.modify_other_keys());
    assert_eq!(
        key(&mut e, &t, Key::KeyP, Mods::CTRL, "p"),
        b"\x1b[27;5;112~"
    );
    // 松开键在传统模式下什么都不发。
    e.sync(&t);
    let release = e.encode(&KeyEvent {
        action: KeyAction::Release,
        key: Key::KeyA,
        text: "a",
        unshifted: Some('a'),
        ..KeyEvent::default()
    });
    assert!(release.is_empty());
}

#[test]
fn key_codes() {
    assert_eq!(Key::from_code("KeyA"), Key::KeyA);
    assert_eq!(Key::from_code("Tab"), Key::Tab);
    assert_eq!(Key::from_code("ShiftLeft"), Key::ShiftLeft);
    assert_eq!(Key::from_code("ArrowUp"), Key::ArrowUp);
    assert_eq!(Key::from_code("Numpad1"), Key::Numpad1);
    assert_eq!(Key::from_code("OSLeft"), Key::MetaLeft);
    assert_eq!(Key::from_code("Nope"), Key::Unidentified);
    assert_eq!(Key::Digit0.manifest_name(), "DIGIT_0");
    assert_eq!(Key::KeyZ.manifest_name(), "Z");
    assert_eq!(Key::LaunchApp1.manifest_name(), "LAUNCH_APP_1");
    assert_eq!(Key::F12.manifest_name(), "F12");
    assert_eq!(Key::NumpadClearEntry.manifest_name(), "NUMPAD_CLEAR_ENTRY");
}

#[test]
fn paste_and_focus() {
    if !lib() {
        return;
    }
    let mut t = Terminal::new(80, 24, Box::new(())).unwrap();
    assert_eq!(paste(&t, "hi"), b"hi");
    assert_eq!(paste(&t, "a\nb"), b"a\rb");
    t.write(b"\x1b[?2004h");
    assert_eq!(paste(&t, "hi"), b"\x1b[200~hi\x1b[201~");
    assert_eq!(
        encode_paste("x\x1b[201~y", true),
        b"\x1b[200~x [201~y\x1b[201~"
    );
    let long = "x".repeat(5000);
    assert_eq!(encode_paste(&long, false), long.as_bytes());
    assert!(paste_is_safe("ls"));
    assert!(!paste_is_safe("rm -rf /\n"));
    assert!(focus(&t, true).is_empty());
    t.write(b"\x1b[?1004h");
    assert_eq!(focus(&t, true), b"\x1b[I");
    assert_eq!(focus(&t, false), b"\x1b[O");
}

#[test]
fn mouse() {
    if !lib() {
        return;
    }
    let mut t = Terminal::new(80, 24, Box::new(())).unwrap();
    let mut m = MouseEncoder::new().unwrap();
    let size = MouseSize {
        screen_width: 800,
        screen_height: 240,
        cell_width: 10,
        cell_height: 10,
        ..MouseSize::default()
    };
    // 第 3 列第 4 行（从 1 数）。
    let press = MouseEvent {
        action: MouseAction::Press,
        button: Some(MouseButton::Left),
        mods: Mods::NONE,
        x: 25.0,
        y: 35.0,
    };
    m.sync(&t, &size, false);
    assert!(m.encode(&press).is_empty(), "程序没要鼠标");
    t.write(b"\x1b[?1000h\x1b[?1006h");
    assert!(t.mouse_tracking());
    m.sync(&t, &size, false);
    assert_eq!(m.encode(&press), b"\x1b[<0;3;4M");
    m.sync(&t, &size, true);
    let release = MouseEvent {
        action: MouseAction::Release,
        ..press
    };
    assert_eq!(m.encode(&release), b"\x1b[<0;3;4m");
    // 1000 不报拖动，1002 报。
    let drag = MouseEvent {
        action: MouseAction::Motion,
        x: 55.0,
        ..press
    };
    m.sync(&t, &size, true);
    assert!(m.encode(&drag).is_empty());
    t.write(b"\x1b[?1002h");
    m.sync(&t, &size, true);
    assert_eq!(m.encode(&drag), b"\x1b[<32;6;4M");
    // 滚轮。
    let wheel = MouseEvent {
        button: Some(MouseButton::WheelUp),
        ..press
    };
    m.sync(&t, &size, false);
    assert_eq!(m.encode(&wheel), b"\x1b[<64;3;4M");
}
