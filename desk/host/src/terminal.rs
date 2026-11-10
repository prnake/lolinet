//! 终端会话：PTY（portable-pty）+ 主机侧终端模型（alacritty_terminal）。
//!
//! 和 Vibe42 一样，状态在主机：客户端接上来先拿一份整屏快照（从模型序列化成转义序列），
//! 之后收原始 PTY 字节。多个客户端可以同看一个会话；断线重连只是再 Attach 一次。

use std::{
    collections::HashMap,
    io::{Read, Write},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::Dimensions,
    term::{cell::Flags, Config, Term, TermMode},
    vte::ansi::{Color, NamedColor, Processor, Rgb},
};
use anyhow::{Context, Result};
use lolidesk_proto::{encode_key, palette, Input, SessionInfo, TermEvent};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tokio::sync::broadcast;

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

#[derive(Clone)]
struct Listener(std::sync::mpsc::Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

struct State {
    term: Term<Listener>,
    parser: Processor,
    size: Size,
}

pub struct Session {
    pub id: u32,
    pub command: String,
    state: Mutex<State>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    out: broadcast::Sender<TermEvent>,
    title: Mutex<String>,
    pub viewers: AtomicU32,
}

impl Session {
    fn spawn(
        id: u32,
        cols: u16,
        rows: u16,
        command: Option<String>,
        cwd: Option<String>,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<Arc<Self>> {
        let size = Size {
            cols: cols.max(2),
            rows: rows.max(1),
        };
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;
        let mut cmd = match &command {
            Some(c) => {
                let mut b = CommandBuilder::new("/bin/sh");
                b.arg("-lc");
                b.arg(c);
                b
            }
            None => CommandBuilder::new_default_prog(),
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("LOLIDESK", "1");
        if let Some(d) = &cwd {
            cmd.cwd(d);
        } else if let Some(h) = dirs::home_dir() {
            cmd.cwd(h);
        }
        let mut child = pty.slave.spawn_command(cmd).context("spawn shell")?;
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader()?;
        let writer = pty.master.take_writer()?;
        let killer = child.clone_killer();

        let (etx, erx) = std::sync::mpsc::channel::<Event>();
        let term = Term::new(
            Config {
                scrolling_history: 5000,
                ..Config::default()
            },
            &size,
            Listener(etx),
        );
        let (out, _) = broadcast::channel(2048);
        let session = Arc::new(Session {
            id,
            command: command.clone().unwrap_or_else(|| "shell".into()),
            state: Mutex::new(State {
                term,
                parser: Processor::new(),
                size,
            }),
            master: Mutex::new(pty.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            out,
            title: Mutex::new(String::new()),
            viewers: AtomicU32::new(0),
        });

        // PTY 读线程：喂模型，同时广播原始字节。广播必须在持锁时做，Attach 才能「锁内订阅 + 快照」而不重复。
        let s = session.clone();
        std::thread::Builder::new()
            .name(format!("pty-{id}"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut st = s.state.lock().unwrap();
                            let State { term, parser, .. } = &mut *st;
                            parser.advance(term, &buf[..n]);
                            let _ = s.out.send(TermEvent::Output(buf[..n].to_vec()));
                        }
                    }
                }
                let code = child.wait().map(|st| st.exit_code() as i32).unwrap_or(-1);
                let _ = s.out.send(TermEvent::Exited(code));
                on_exit();
            })?;

        // 模型事件线程：终端程序的查询（DA、光标位置、颜色、尺寸）要回写 PTY；标题、响铃广播给客户端。
        // 只持弱引用：会话没人用了，Term 里的事件发送端跟着销毁，这个线程就结束。
        let weak = Arc::downgrade(&session);
        std::thread::Builder::new()
            .name(format!("term-ev-{id}"))
            .spawn(move || {
                while let Ok(ev) = erx.recv() {
                    let Some(s) = weak.upgrade() else { break };
                    match ev {
                        Event::PtyWrite(text) => s.write(text.as_bytes()),
                        Event::Title(t) => {
                            *s.title.lock().unwrap() = t.clone();
                            let _ = s.out.send(TermEvent::Title(t));
                        }
                        Event::ResetTitle => {
                            s.title.lock().unwrap().clear();
                            let _ = s.out.send(TermEvent::Title(String::new()));
                        }
                        Event::Bell => {
                            let _ = s.out.send(TermEvent::Bell);
                        }
                        Event::ColorRequest(idx, fmt) => {
                            let rgb = {
                                let st = s.state.lock().unwrap();
                                st.term.colors()[idx].unwrap_or_else(|| default_color(idx))
                            };
                            s.write(fmt(rgb).as_bytes());
                        }
                        Event::TextAreaSizeRequest(fmt) => {
                            let size = s.state.lock().unwrap().size;
                            s.write(
                                fmt(WindowSize {
                                    num_lines: size.rows,
                                    num_cols: size.cols,
                                    cell_width: 8,
                                    cell_height: 16,
                                })
                                .as_bytes(),
                            );
                        }
                        Event::Exit | Event::ChildExit(_) => break,
                        _ => {}
                    }
                }
            })?;
        Ok(session)
    }

    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut w = self.writer.lock().unwrap();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    pub fn encode_input(&self, input: &Input) -> Vec<u8> {
        match input {
            Input::Raw(b) => b.clone(),
            Input::Text(s) => s.as_bytes().to_vec(),
            Input::Key { key, mods } => {
                let mode = *self.state.lock().unwrap().term.mode();
                encode_key(
                    *key,
                    *mods,
                    mode.contains(TermMode::APP_CURSOR),
                    mode.contains(TermMode::APP_KEYPAD),
                )
            }
            Input::Paste(s) => {
                let mode = *self.state.lock().unwrap().term.mode();
                let body = s.replace("\r\n", "\r").replace('\n', "\r");
                if mode.contains(TermMode::BRACKETED_PASTE) {
                    format!("\x1b[200~{body}\x1b[201~").into_bytes()
                } else {
                    body.into_bytes()
                }
            }
        }
    }

    /// 改 PTY 和模型尺寸，然后给所有观看者一份新快照（两端模型各自重排会分叉，统一以主机为准）。
    pub fn resize(&self, cols: u16, rows: u16) {
        let size = Size {
            cols: cols.max(2),
            rows: rows.max(1),
        };
        {
            let mut st = self.state.lock().unwrap();
            if st.size.cols == size.cols && st.size.rows == size.rows {
                return;
            }
            st.size = size;
            st.term.resize(size);
            let _ = self.master.lock().unwrap().resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            let data = snapshot(&st.term);
            let _ = self.out.send(TermEvent::Snapshot {
                cols: size.cols,
                rows: size.rows,
                data,
            });
        }
    }

    /// 锁内订阅再做快照：之后广播出去的输出一定是快照之后的。
    pub fn attach(&self) -> (broadcast::Receiver<TermEvent>, TermEvent) {
        let st = self.state.lock().unwrap();
        let rx = self.out.subscribe();
        let data = snapshot(&st.term);
        (
            rx,
            TermEvent::Snapshot {
                cols: st.size.cols,
                rows: st.size.rows,
                data,
            },
        )
    }

    pub fn kill(&self) {
        let _ = self.killer.lock().unwrap().kill();
    }

    pub fn info(&self) -> SessionInfo {
        let size = self.state.lock().unwrap().size;
        SessionInfo {
            id: self.id,
            title: self.title.lock().unwrap().clone(),
            command: self.command.clone(),
            cols: size.cols,
            rows: size.rows,
            viewers: self.viewers.load(Ordering::Relaxed),
        }
    }
}

fn default_color(idx: usize) -> Rgb {
    let [r, g, b] = match idx {
        0..=255 => palette::indexed(idx as u8),
        256 => palette::FG,
        257 => palette::BG,
        258 => palette::CURSOR,
        _ => palette::FG,
    };
    Rgb { r, g, b }
}

/// 把模型可见区序列化成转义序列：清屏、逐行逐格带 SGR、光标、模式。客户端回放就能重建同样的屏幕。
pub fn snapshot<T: EventListener>(term: &Term<T>) -> Vec<u8> {
    use std::fmt::Write as _;
    let content = term.renderable_content();
    let mode = content.mode;
    let colors = content.colors;
    let mut out = String::with_capacity(16 * 1024);
    if mode.contains(TermMode::ALT_SCREEN) {
        out.push_str("\x1b[?1049h");
    } else {
        out.push_str("\x1b[?1049l");
    }
    out.push_str("\x1b[0m\x1b[?25l\x1b[2J\x1b[H");
    let mut cur_line = i32::MIN;
    let mut cur = (
        Color::Named(NamedColor::Foreground),
        Color::Named(NamedColor::Background),
        Flags::empty(),
    );
    for cell in content.display_iter {
        let line = cell.point.line.0;
        let c = cell.cell;
        if c.flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        if line != cur_line {
            let _ = write!(out, "\x1b[{};{}H", line + 1, cell.point.column.0 + 1);
            cur_line = line;
        }
        let style_flags = c.flags
            & (Flags::BOLD
                | Flags::DIM
                | Flags::ITALIC
                | Flags::UNDERLINE
                | Flags::INVERSE
                | Flags::HIDDEN
                | Flags::STRIKEOUT);
        let want = (c.fg, c.bg, style_flags);
        if want != cur {
            out.push_str("\x1b[0");
            if style_flags.contains(Flags::BOLD) {
                out.push_str(";1");
            }
            if style_flags.contains(Flags::DIM) {
                out.push_str(";2");
            }
            if style_flags.contains(Flags::ITALIC) {
                out.push_str(";3");
            }
            if style_flags.contains(Flags::UNDERLINE) {
                out.push_str(";4");
            }
            if style_flags.contains(Flags::INVERSE) {
                out.push_str(";7");
            }
            if style_flags.contains(Flags::HIDDEN) {
                out.push_str(";8");
            }
            if style_flags.contains(Flags::STRIKEOUT) {
                out.push_str(";9");
            }
            sgr_color(&mut out, c.fg, true, colors);
            sgr_color(&mut out, c.bg, false, colors);
            out.push('m');
            cur = want;
        }
        let ch = if c.c == '\0' { ' ' } else { c.c };
        out.push(ch);
        if let Some(zw) = c.zerowidth() {
            out.extend(zw.iter());
        }
    }
    out.push_str("\x1b[0m");
    let cur_pt = content.cursor.point;
    let _ = write!(out, "\x1b[{};{}H", cur_pt.line.0 + 1, cur_pt.column.0 + 1);
    let flag = |s: &mut String, on: bool, code: &str| {
        s.push_str("\x1b[?");
        s.push_str(code);
        s.push(if on { 'h' } else { 'l' });
    };
    flag(&mut out, mode.contains(TermMode::SHOW_CURSOR), "25");
    flag(&mut out, mode.contains(TermMode::APP_CURSOR), "1");
    flag(&mut out, mode.contains(TermMode::BRACKETED_PASTE), "2004");
    flag(
        &mut out,
        mode.contains(TermMode::MOUSE_REPORT_CLICK),
        "1000",
    );
    flag(&mut out, mode.contains(TermMode::MOUSE_DRAG), "1002");
    flag(&mut out, mode.contains(TermMode::MOUSE_MOTION), "1003");
    flag(&mut out, mode.contains(TermMode::SGR_MOUSE), "1006");
    flag(&mut out, mode.contains(TermMode::FOCUS_IN_OUT), "1004");
    out.push_str(if mode.contains(TermMode::APP_KEYPAD) {
        "\x1b="
    } else {
        "\x1b>"
    });
    out.into_bytes()
}

fn sgr_color(
    out: &mut String,
    color: Color,
    fg: bool,
    colors: &alacritty_terminal::term::color::Colors,
) {
    use std::fmt::Write as _;
    let _ = colors;
    let base = if fg { 30 } else { 40 };
    match color {
        Color::Named(NamedColor::Foreground) if fg => out.push_str(";39"),
        Color::Named(NamedColor::Background) if !fg => out.push_str(";49"),
        Color::Named(n) => {
            let idx = n as usize;
            match idx {
                0..=7 => {
                    let _ = write!(out, ";{}", base + idx);
                }
                8..=15 => {
                    let _ = write!(out, ";{}", base + 60 + idx - 8);
                }
                _ => {
                    // Foreground/Background 用在相反位置、Cursor、Dim*：退回默认
                    let _ = write!(out, ";{}", if fg { 39 } else { 49 });
                }
            }
        }
        Color::Indexed(i) => {
            let _ = write!(out, ";{};5;{}", base + 8, i);
        }
        Color::Spec(rgb) => {
            let _ = write!(out, ";{};2;{};{};{}", base + 8, rgb.r, rgb.g, rgb.b);
        }
    }
}

#[derive(Default)]
pub struct Sessions {
    next: AtomicU32,
    map: Arc<Mutex<HashMap<u32, Arc<Session>>>>,
}

impl Sessions {
    pub fn open(
        &self,
        cols: u16,
        rows: u16,
        command: Option<String>,
        cwd: Option<String>,
    ) -> Result<Arc<Session>> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        // 持着表锁开会话：进程秒退时，读线程里的摘除一定排在下面的 insert 之后
        let mut map = self.map.lock().unwrap();
        let table = self.map.clone();
        let s = Session::spawn(id, cols, rows, command, cwd, move || {
            table.lock().unwrap().remove(&id);
        })?;
        map.insert(id, s.clone());
        Ok(s)
    }

    pub fn get(&self, id: u32) -> Option<Arc<Session>> {
        self.map.lock().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let mut v: Vec<_> = self
            .map
            .lock()
            .unwrap()
            .values()
            .map(|s| s.info())
            .collect();
        v.sort_by_key(|s| s.id);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::event::VoidListener;

    fn term(cols: u16, rows: u16) -> (Term<VoidListener>, Processor) {
        let size = Size { cols, rows };
        (
            Term::new(Config::default(), &size, VoidListener),
            Processor::new(),
        )
    }

    fn cells(t: &Term<VoidListener>) -> Vec<(i32, usize, char, Color, Color, Flags)> {
        t.renderable_content()
            .display_iter
            .filter(|c| {
                c.cell.c != ' ' || !matches!(c.cell.bg, Color::Named(NamedColor::Background))
            })
            .map(|c| {
                (
                    c.point.line.0,
                    c.point.column.0,
                    c.cell.c,
                    c.cell.fg,
                    c.cell.bg,
                    c.cell.flags
                        & (Flags::BOLD | Flags::ITALIC | Flags::UNDERLINE | Flags::INVERSE),
                )
            })
            .collect()
    }

    /// 主机模型 → 快照 → 客户端模型：可见格子、颜色、属性、光标、模式都要一致。
    #[test]
    fn snapshot_replays_identically() {
        let (mut host, mut p) = term(40, 6);
        let input = "plain \x1b[1;31mbold red\x1b[0m \x1b[38;5;208mindexed\x1b[0m \x1b[48;2;10;20;30m\x1b[3;4mrgb bg\x1b[0m\r\n\
                     \x1b[7minverse\x1b[0m 中文宽字\r\n\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[2;5H";
        p.advance(&mut host, input.as_bytes());
        let data = snapshot(&host);
        let (mut client, mut cp) = term(40, 6);
        cp.advance(&mut client, &data);
        assert_eq!(cells(&host), cells(&client));
        assert_eq!(
            host.renderable_content().cursor.point,
            client.renderable_content().cursor.point
        );
        for m in [
            TermMode::APP_CURSOR,
            TermMode::BRACKETED_PASTE,
            TermMode::MOUSE_REPORT_CLICK,
            TermMode::SHOW_CURSOR,
            TermMode::ALT_SCREEN,
        ] {
            assert_eq!(host.mode().contains(m), client.mode().contains(m), "{m:?}");
        }
    }

    #[test]
    fn snapshot_restores_alt_screen() {
        let (mut host, mut p) = term(20, 4);
        p.advance(&mut host, b"main\x1b[?1049h\x1b[Halt screen\x1b[?25l");
        let data = snapshot(&host);
        let (mut client, mut cp) = term(20, 4);
        cp.advance(&mut client, &data);
        assert!(client.mode().contains(TermMode::ALT_SCREEN));
        assert!(!client.mode().contains(TermMode::SHOW_CURSOR));
        assert_eq!(cells(&host), cells(&client));
    }
}
