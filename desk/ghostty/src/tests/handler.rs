//! 回调：查询的回答、标题、目录、响铃、通知、进度、配色。

use std::sync::{Arc, Mutex};

use super::lib;
use crate::{Handler, Mode, Rgb, Terminal, Theme};

#[derive(Default)]
struct Seen {
    pty: Vec<u8>,
    titles: Vec<String>,
    pwds: Vec<String>,
    bells: usize,
    notes: Vec<(String, String)>,
    progress: Vec<(u8, Option<u8>)>,
}

struct Recorder(Arc<Mutex<Seen>>);

impl Handler for Recorder {
    fn write_pty(&mut self, bytes: &[u8]) {
        self.0.lock().unwrap().pty.extend_from_slice(bytes);
    }
    fn title(&mut self, title: &str) {
        self.0.lock().unwrap().titles.push(title.into());
    }
    fn pwd(&mut self, pwd: &str) {
        self.0.lock().unwrap().pwds.push(pwd.into());
    }
    fn bell(&mut self) {
        self.0.lock().unwrap().bells += 1;
    }
    fn notify(&mut self, title: &str, body: &str) {
        self.0
            .lock()
            .unwrap()
            .notes
            .push((title.into(), body.into()));
    }
    fn progress(&mut self, state: u8, percent: Option<u8>) {
        self.0.lock().unwrap().progress.push((state, percent));
    }
}

fn recorded(cols: u16, rows: u16) -> (Terminal, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let t = Terminal::new(cols, rows, Box::new(Recorder(seen.clone()))).unwrap();
    (t, seen)
}

fn take_pty(seen: &Arc<Mutex<Seen>>) -> String {
    String::from_utf8(std::mem::take(&mut seen.lock().unwrap().pty)).unwrap()
}

#[test]
fn queries_are_answered() {
    if !lib() {
        return;
    }
    let (mut t, seen) = recorded(80, 24);
    t.write(b"\x1b[c");
    assert_eq!(take_pty(&seen), "\x1b[?62;22c");
    t.write(b"\x1b[3;5H\x1b[6n");
    assert_eq!(take_pty(&seen), "\x1b[3;5R");
    t.write(b"\x1b[18t");
    assert_eq!(take_pty(&seen), "\x1b[8;24;80t");
    t.resize(100, 30, 9, 18);
    t.write(b"\x1b[14t");
    assert_eq!(take_pty(&seen), "\x1b[4;540;900t");
    t.write(b"\x1b[?2004$p");
    assert_eq!(take_pty(&seen), "\x1b[?2004;2$y");
    // 配色：程序问默认背景。
    t.set_theme(&Theme::dark());
    t.write(b"\x1b]11;?\x1b\\");
    assert!(
        take_pty(&seen).contains("rgb:1e1e/1e1e/1e1e"),
        "OSC 11 的回答不对"
    );
    t.write(b"\x1b[?996n");
    assert_eq!(take_pty(&seen), "\x1b[?997;1n");
    // 开了 2031 的程序在深浅变了时收到通知。
    t.write(b"\x1b[?2031h");
    assert!(t.mode(Mode::COLOR_SCHEME));
    t.set_theme(&Theme::light());
    assert_eq!(take_pty(&seen), "\x1b[?997;2n");
}

#[test]
fn title_pwd_bell_notify_progress() {
    if !lib() {
        return;
    }
    let (mut t, seen) = recorded(80, 24);
    t.write(b"\x1b]2;hello\x07");
    assert_eq!(t.title(), "hello");
    t.write(b"\x1b]0;again\x1b\\");
    assert_eq!(t.title(), "again");
    t.write(b"\x1b]7;file://host/tmp/x\x07");
    assert_eq!(t.pwd(), "file://host/tmp/x");
    t.write(b"\x07\x07");
    t.write(b"\x1b]777;notify;Title;Body\x07");
    t.write(b"\x1b]9;4;1;42\x07");
    let s = seen.lock().unwrap();
    assert_eq!(s.titles, ["hello", "again"]);
    assert_eq!(s.pwds, ["file://host/tmp/x"]);
    assert_eq!(s.bells, 2);
    assert_eq!(s.notes, [("Title".to_string(), "Body".to_string())]);
    assert_eq!(s.progress, [(1, Some(42))]);
}

#[test]
fn theme_colors() {
    if !lib() {
        return;
    }
    let mut th = Theme::dark();
    assert!(th.is_dark() && !Theme::light().is_dark());
    let p = th.palette256();
    assert_eq!(p[1], Rgb::hex(0xcd3131));
    assert_eq!(p[16], Rgb::new(0, 0, 0));
    assert_eq!(p[231], Rgb::new(255, 255, 255));
    assert_eq!(p[232], Rgb::new(8, 8, 8));
    th.cursor = Some(Rgb::hex(0x00ff00));
    let (mut t, seen) = recorded(10, 2);
    t.set_theme(&th);
    t.write(b"\x1b]12;?\x07\x1b]4;1;?\x07");
    let reply = take_pty(&seen);
    assert!(reply.contains("12;rgb:0000/ffff/0000"), "{reply:?}");
    assert!(reply.contains("4;1;rgb:cdcd/3131/3131"), "{reply:?}");
}
