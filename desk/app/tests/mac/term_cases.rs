//! 终端的快照和测试：主机那边一个 libghostty-vt 终端 + Mirror 跑脚本化的程序输出，帧推进假会话，
//! 客户端的终端视图离屏画出来；再测按键编码、选区、输入法、重连后的重接、整屏一帧的耗时。

use std::{path::PathBuf, rc::Rc, time::Instant};

use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Entity, EntityInputHandler as _, HeadlessAppContext,
    InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, WindowBounds,
    WindowOptions, point, px, size,
    test::{TestEventExt, TestWindowExt},
};
use lolidesk_app::{
    model::Model,
    term::{DEFAULT_SIZE, FakeLink, TermLink, TermPhase, TermSession, TermStart},
    ui::{app::Workspace, grid::TermGrid},
};
use lolidesk_ghostty::{Mirror, Terminal};
use lolidesk_proto::{TermEvent, TermReq, TermSize};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{addr, context, fake_model, frame, shot, with_window};

/// 主机那边的终端 + 客户端的窗口。
struct Rig {
    cx: HeadlessAppContext,
    handle: AnyWindowHandle,
    model: Entity<Model>,
    session: Entity<TermSession>,
    grid: Entity<TermGrid>,
    rx: UnboundedReceiver<TermReq>,
    host: Terminal,
    mirror: Mirror,
    size: TermSize,
}

fn open_sized(
    cx: &mut HeadlessAppContext,
    model: Entity<Model>,
    w: f32,
    h: f32,
) -> AnyWindowHandle {
    let (handle, _) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(w), px(h)),
                    })),
                    focus: true,
                    show: false,
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| Workspace::new(model, window, cx)),
            )
        })
        .expect("开窗口");
    cx.update_window(handle, |_, window, _| window.activate_window())
        .unwrap();
    frame(cx, handle);
    handle
}

impl Rig {
    /// 开窗口、开一个假会话；客户端量好尺寸发 Resize 后，主机按这个尺寸建终端（像真主机一样回 Resized）。
    fn new(w: f32, h: f32) -> Rig {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let handle = open_sized(&mut cx, model.clone(), w, h);
        let session =
            cx.new(|_| TermSession::offline(addr("10.188.1.2:10190"), TermStart::Attach { id: 3 }));
        let mut rx = session.update(&mut cx, |s, _| s.attach_detached());
        let grid = with_window(&mut cx, handle, |_, cx| {
            model.update(cx, |m, cx| {
                let id = m.add_term(session.clone(), cx);
                m.terms
                    .iter()
                    .find(|t| t.entity_id() == id)
                    .map(|t| t.read(cx).grid.clone())
                    .unwrap()
            })
        });
        frame(&mut cx, handle);
        // 客户端量出的尺寸
        let mut size = DEFAULT_SIZE;
        while let Ok(r) = rx.try_recv() {
            if let TermReq::Resize(s) = r {
                size = s;
            }
        }
        assert!(size.cols > 20 && size.rows > 5, "客户端没发尺寸：{size:?}");
        let mut host = Terminal::new(size.cols, size.rows, Box::new(())).expect("主机终端");
        host.set_theme(&lolidesk_app::ui::grid::catseye_theme());
        session.update(&mut cx, |s, cx| {
            s.handle(TermEvent::Opened { id: 3 }, cx);
            s.handle(
                TermEvent::Resized {
                    cols: size.cols,
                    rows: size.rows,
                },
                cx,
            );
        });
        Rig {
            cx,
            handle,
            model,
            session,
            grid,
            rx,
            host,
            mirror: Mirror::new(),
            size,
        }
    }

    /// 主机的程序输出一段，帧推给客户端，画一帧。
    fn feed(&mut self, bytes: &[u8]) {
        self.host.write(bytes);
        self.push_frame();
        frame(&mut self.cx, self.handle);
    }

    fn push_frame(&mut self) {
        let (f, _) = self.mirror.frame(&mut self.host);
        if !f.is_empty() {
            self.session
                .update(&mut self.cx, |s, cx| s.handle(TermEvent::Frame(f), cx));
        }
    }

    /// 发给主机的输入（只要 Input）。
    fn inputs(&mut self) -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            if let TermReq::Input(b) = r {
                v.push(b);
            }
        }
        v
    }

    fn focus(&mut self) {
        let grid = self.grid.clone();
        with_window(&mut self.cx, self.handle, |window, cx| {
            grid.read(cx).focus.clone().focus(window, cx);
            window.render_frame(cx);
        });
        self.cx.run_until_parked();
        self.inputs();
    }

    fn key(&mut self, k: &str) -> Vec<u8> {
        self.inputs();
        with_window(&mut self.cx, self.handle, |window, cx| {
            window.key_down(k, false, cx);
            window.key_up(k, cx);
        });
        self.inputs().concat()
    }

    /// 格子 (x, y) 的中心（窗口坐标）。
    fn at(&mut self, x: u16, y: u16) -> gpui_kit::Point<gpui_kit::Pixels> {
        let (o, cw, ch, _, _) = self.grid.read_with(&self.cx, |g, _| g.cell_layout());
        o + point(px((x as f32 + 0.5) * cw), px((y as f32 + 0.5) * ch))
    }
}

/// 一屏像样的 shell：seq 1 500、带颜色的 ls、各种字体样式、调色板、真彩、中文和 emoji 对齐、框线、超链接。
fn demo_script(cols: u16) -> Vec<u8> {
    let prompt = "\x1b[38;5;114m➜\x1b[0m  \x1b[1;36mlolinet\x1b[0m \x1b[90mgit:(\x1b[31mmain\x1b[90m)\x1b[0m ";
    let mut s = String::new();
    s += prompt;
    s += "seq 1 500\r\n";
    for i in 1..=500 {
        s += &format!("{i}\r\n");
    }
    s += prompt;
    s += "ls -la desk/app/src\r\n";
    s += "total 312\r\n";
    for (mode, size, name, color) in [
        ("drwxr-xr-x", "  384", "ui", "1;34"),
        ("-rw-r--r--", " 9.1k", "backoff.rs", "0"),
        ("-rw-r--r--", "  14k", "desk.rs", "0"),
        ("-rw-r--r--", "  16k", "keys.rs", "0"),
        ("-rwxr-xr-x", " 1.2M", "lolidesk-app", "1;32"),
        (
            "lrwxr-xr-x",
            "   31",
            "libghostty-vt.dylib -> ../.ghostty/libghostty-vt.dylib",
            "1;36",
        ),
    ] {
        s += &format!("{mode}  1 me  staff  {size} 11月 10 04:20 \x1b[{color}m{name}\x1b[0m\r\n");
    }
    s += "\r\n";
    s += "\x1b[1m粗体 bold\x1b[0m  \x1b[3m斜体 italic\x1b[0m  \x1b[4m下划线\x1b[0m  \x1b[4:3;58;5;203m波浪线\x1b[0m  \x1b[9m删除线\x1b[0m  \x1b[2m淡 faint\x1b[0m  \x1b[7m 反色 \x1b[0m  \x1b[53m上划线\x1b[0m\r\n";
    // 16 色
    for i in 0..8 {
        s += &format!("\x1b[4{i}m  \x1b[0m");
    }
    s += " ";
    for i in 0..8 {
        s += &format!("\x1b[10{i}m  \x1b[0m");
    }
    s += "  ";
    for i in 0..8 {
        s += &format!("\x1b[3{i}m■\x1b[0m");
    }
    for i in 0..8 {
        s += &format!("\x1b[9{i}m■\x1b[0m");
    }
    s += "\r\n";
    // 256 色和真彩
    for n in 16..88u16 {
        s += &format!("\x1b[48;5;{n}m \x1b[0m");
    }
    s += "\r\n";
    let w = cols.saturating_sub(4).min(96);
    for i in 0..w {
        let t = i as f32 / w as f32;
        let (r, g, b) = (
            (206.0 * t + 20.0) as u8,
            (255.0 * (1.0 - (t - 0.5).abs() * 2.0)) as u8,
            (28.0 + 200.0 * (1.0 - t)) as u8,
        );
        s += &format!("\x1b[48;2;{r};{g};{b}m \x1b[0m");
    }
    s += "\r\n\r\n";
    // 宽字符对齐：每行的 | 应该上下对齐
    s += "中文对齐：你好，世界！|ok|  表情：🚀✨🐱 |end|\r\n";
    s += "abcdefghijklmnopqrstuv|ok|  ascii:aaaaaa |end|\r\n";
    s += "┌────────┬──────────┐  \x1b[38;5;173m╭──────────────────────╮\x1b[0m  ╔════╦════╗  ┏━━━━┓\r\n";
    s += "│ 名字   │ 大小     │  \x1b[38;5;173m│\x1b[0m \x1b[38;5;173m✻\x1b[0m Welcome to Claude! \x1b[38;5;173m│\x1b[0m  ║ ok ║ ok ║  ┃ ok ┃\r\n";
    s += "├────────┼──────────┤  \x1b[38;5;173m╰──────────────────────╯\x1b[0m  ╠════╬════╣  ┗━━━━┛\r\n";
    s += "│ grid.rs│ 31 KB    │   \x1b[38;5;173m▐▛███▜▌\x1b[0m  ░▒▓█  ▁▂▃▄▅▆▇█  ╚════╩════╝  ┄┄┈┈╌╌\r\n";
    s += "└────────┴──────────┘\r\n";
    s += "文档：\x1b]8;;https://example.com/lolidesk\x07\x1b[4mexample.com/lolidesk\x1b[24m\x1b]8;;\x07  https://github.com/prnake/lolinet\r\n";
    s += prompt;
    s += "\x1b[1mclaude\x1b[0m --resume";
    s.into_bytes()
}

/// 一个占满备用屏的程序（像 htop）：顶上的条、表格、选中行、底下的功能键。
fn alt_script(cols: u16, rows: u16) -> Vec<u8> {
    let mut s = String::from("\x1b[?1049h\x1b[?1h\x1b[H\x1b[2J");
    let bar = |label: &str, used: usize, total: usize| {
        let mut b = format!("  \x1b[36m{label:>3}\x1b[0m[");
        for i in 0..total {
            b += match (i < used, i * 3 < used * 2) {
                (true, true) => "\x1b[32m|",
                (true, false) => "\x1b[31m|",
                _ => " ",
            };
        }
        b + "\x1b[0m]"
    };
    for (i, (l, u)) in [("0", 30), ("1", 12), ("2", 44), ("3", 7)]
        .into_iter()
        .enumerate()
    {
        s += &format!("\x1b[{};1H{}", i + 2, bar(l, u, 46));
    }
    s += "\x1b[2;60H\x1b[1mTasks:\x1b[0m 412, 1180 thr; \x1b[32m2 running\x1b[0m";
    s += "\x1b[3;60H\x1b[1mLoad average:\x1b[0m 1.92 2.10 2.31";
    s += "\x1b[4;60H\x1b[1mUptime:\x1b[0m 3 days, 04:20:17";
    let header = format!(
        "{:<w$}",
        "    PID USER      PRI  NI  VIRT   RES  CPU% MEM%   TIME+  Command",
        w = cols as usize
    );
    s += &format!("\x1b[7;1H\x1b[30;42m{header}\x1b[0m");
    let procs = [
        ("41823", "me", "17", "lolidesk", "12.0"),
        ("38110", "me", "17", "claude", "6.3"),
        ("612", "root", "20", "WindowServer", "4.1"),
        ("90211", "me", "24", "zsh", "0.3"),
        ("1", "root", "20", "launchd", "0.0"),
    ];
    for (i, (pid, user, pri, cmd, cpu)) in procs.iter().enumerate() {
        let line =
            format!("{pid:>7} {user:<9} {pri:>3}   0  412G 98.2M {cpu:>5}  0.6  1:02.33  {cmd}");
        let line = format!("{line:<w$}", w = cols as usize);
        if i == 1 {
            s += &format!("\x1b[{};1H\x1b[30;46m{line}\x1b[0m", 8 + i);
        } else {
            s += &format!("\x1b[{};1H{line}", 8 + i);
        }
    }
    let keys = [
        "Help", "Setup", "Search", "Filter", "Tree", "SortBy", "Nice -", "Nice +", "Kill", "Quit",
    ];
    s += &format!("\x1b[{rows};1H");
    for (i, k) in keys.iter().enumerate() {
        s += &format!("\x1b[0mF{}\x1b[30;46m{k:<6}", i + 1);
    }
    s += "\x1b[0m\x1b[9;1H";
    s.into_bytes()
}

pub(super) fn terminal(out: &Option<PathBuf>) {
    let mut rig = Rig::new(1280., 820.);
    let cols = rig.size.cols;
    rig.feed(&demo_script(cols));
    rig.focus();
    shot(&mut rig.cx, rig.handle, out, "terminal");
    // 客户端终端和主机的一样
    let client = rig
        .grid
        .update(&mut rig.cx, |g, _| g.terminal_mut().map(|t| t.text()))
        .unwrap();
    assert_eq!(client, rig.host.text(), "客户端终端和主机的内容不一样");
    assert!(client.contains("你好，世界"));
    // 往回滚：显示滚动条
    let pos = rig.at(10, 10);
    with_window(&mut rig.cx, rig.handle, |window, cx| {
        window.dispatch_event(
            gpui_kit::ScrollEvent {
                position: pos,
                delta: gpui_kit::ScrollDelta::Lines(point(0., 30.)),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    shot(&mut rig.cx, rig.handle, out, "terminal-scrolled");
    let at_bottom = rig
        .grid
        .update(&mut rig.cx, |g, _| g.terminal_mut().map(|t| t.at_bottom()))
        .unwrap();
    assert!(!at_bottom, "滚轮没往回滚");
    // 按个键回到底
    rig.key("a->a");
    let at_bottom = rig
        .grid
        .update(&mut rig.cx, |g, _| g.terminal_mut().map(|t| t.at_bottom()))
        .unwrap();
    assert!(at_bottom, "按键没回到底");
}

pub(super) fn terminal_alt(out: &Option<PathBuf>) {
    let mut rig = Rig::new(1280., 820.);
    let (c, r) = (rig.size.cols, rig.size.rows);
    rig.feed(&alt_script(c, r));
    shot(&mut rig.cx, rig.handle, out, "terminal-alt");
    // 备用屏上滚轮发方向键（应用光标键模式：ESC O A）
    rig.focus();
    let pos = rig.at(10, 10);
    with_window(&mut rig.cx, rig.handle, |window, cx| {
        window.dispatch_event(
            gpui_kit::ScrollEvent {
                position: pos,
                delta: gpui_kit::ScrollDelta::Lines(point(0., 2.)),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
    });
    let sent = rig.inputs().concat();
    assert_eq!(sent, b"\x1bOA\x1bOA", "备用屏的滚轮");
}

pub(super) fn terminal_selection(out: &Option<PathBuf>) {
    let mut rig = Rig::new(1280., 820.);
    rig.feed(b"\x1b[H\x1b[2Jhello world selection-test lolinet/desk\r\nsecond line of text here\r\nthird line, the end\r\n$ ");
    rig.focus();
    // 双击选词
    let p = rig.at(15, 0);
    with_window(&mut rig.cx, rig.handle, |window, cx| {
        for n in 1..=2 {
            window.dispatch_event(
                MouseDownEvent {
                    button: MouseButton::Left,
                    position: p,
                    modifiers: Modifiers::default(),
                    click_count: n,
                    first_mouse: false,
                }
                .to_platform_input(),
                cx,
            );
            window.dispatch_event(
                MouseUpEvent {
                    button: MouseButton::Left,
                    position: p,
                    modifiers: Modifiers::default(),
                    click_count: n,
                }
                .to_platform_input(),
                cx,
            );
        }
    });
    let word = rig.grid.read_with(&rig.cx, |g, _| g.selected_text());
    assert_eq!(word.as_deref(), Some("selection-test"));
    // 拖着选两行半
    let (a, b) = (rig.at(6, 0), rig.at(9, 2));
    with_window(&mut rig.cx, rig.handle, |window, cx| {
        window.pointer_down(a, MouseButton::Left, cx);
        window.pointer_move(b, Some(MouseButton::Left), cx);
        window.pointer_up(b, MouseButton::Left, cx);
    });
    let text = rig.grid.read_with(&rig.cx, |g, _| g.selected_text());
    assert_eq!(
        text.as_deref(),
        Some("world selection-test lolinet/desk\nsecond line of text here\nthird line")
    );
    shot(&mut rig.cx, rig.handle, out, "terminal-selection");
    // ⌘C 复制选区，不发给程序
    let sent = rig.key("cmd-c");
    assert!(sent.is_empty(), "⌘C 发给了程序：{sent:?}");
    let clip = rig
        .cx
        .update(|cx| cx.read_from_clipboard().and_then(|c| c.text()));
    assert_eq!(clip.as_deref(), text.as_deref());
}

pub(super) fn terminal_ime(out: &Option<PathBuf>) {
    let mut rig = Rig::new(1280., 820.);
    rig.feed(b"\x1b[H\x1b[2J\x1b[38;5;114m\xe2\x9e\x9c\x1b[0m  echo ");
    rig.focus();
    let grid = rig.grid.clone();
    with_window(&mut rig.cx, rig.handle, |window, cx| {
        grid.update(cx, |g, cx| {
            g.replace_and_mark_text_in_range(None, "ni'hao", Some(6..6), window, cx)
        });
    });
    shot(&mut rig.cx, rig.handle, out, "terminal-ime");
    // 组字时按键归输入法，不发
    assert!(rig.inputs().is_empty());
    with_window(&mut rig.cx, rig.handle, |window, cx| {
        grid.update(cx, |g, cx| {
            g.replace_text_in_range(None, "你好", window, cx)
        });
    });
    assert_eq!(rig.inputs().concat(), "你好".as_bytes());
}

pub(super) fn terminal_keys(_: &Option<PathBuf>) {
    let mut rig = Rig::new(1280., 820.);
    rig.feed(b"$ ");
    rig.focus();
    assert_eq!(rig.key("tab"), b"\t");
    assert_eq!(rig.key("shift-tab"), b"\x1b[Z");
    assert_eq!(rig.key("up"), b"\x1b[A");
    assert_eq!(rig.key("ctrl-c"), b"\x03");
    assert_eq!(rig.key("alt-x"), b"\x1bx", "Option 当 Alt");
    assert_eq!(rig.key("a->a"), b"a", "普通字只发一次");
    assert_eq!(rig.key("enter"), b"\r");
    assert_eq!(rig.key("escape"), b"\x1b");
    assert_eq!(rig.key("f1"), b"\x1bOP");
    assert!(rig.key("cmd-c").is_empty(), "⌘C 没有选区时什么都不发");
    // 帧打开应用光标键模式之后，上是 ESC O A
    rig.feed(b"\x1b[?1h");
    assert_eq!(rig.key("up"), b"\x1bOA");
    // 焦点没被 Tab 挪走
    let focused = with_window(&mut rig.cx, rig.handle, |window, cx| {
        rig.grid.read(cx).focus.is_focused(window)
    });
    assert!(focused, "Tab 把焦点挪走了");
}

pub(super) fn terminal_reconnecting(out: &Option<PathBuf>) {
    let mut rig = Rig::new(1280., 820.);
    rig.feed(&demo_script(rig.size.cols));
    let session = rig.session.clone();
    rig.model.update(&mut rig.cx, |m, cx| {
        m.hosts[0].set_reconnecting_for_test("连接断了：timed out", 3);
        cx.notify();
    });
    session.update(&mut rig.cx, |s, cx| s.suspend(cx));
    assert_eq!(
        session.read_with(&rig.cx, |s, _| s.phase.clone()),
        TermPhase::Reconnecting
    );
    shot(&mut rig.cx, rig.handle, out, "terminal-reconnecting");
}

/// 断线重接：假连接上的会话。
pub(super) fn terminal_reattach(_: &Option<PathBuf>) {
    let mut cx = context();
    let model = fake_model(&mut cx);
    let host = addr("10.188.1.2:10190");
    let link = FakeLink::new();
    let l: Rc<dyn TermLink> = link.clone();
    let session = cx.new(|cx| TermSession::connect(l, host, TermStart::Attach { id: 7 }, cx));
    model.update(&mut cx, |m, cx| {
        m.add_term(session.clone(), cx);
    });
    assert!(matches!(link.first(0), Some(TermReq::Attach { id: 7, .. })));
    link.push(0, Some(TermEvent::Opened { id: 7 }));
    link.push(0, Some(TermEvent::Frame(b"hi".to_vec())));
    cx.run_until_parked();
    assert_eq!(
        session.read_with(&cx, |s, _| s.phase.clone()),
        TermPhase::Live
    );
    session.update(&mut cx, |s, _| s.resize(TermSize::new(90, 25)));

    // 连接断了：等重连，不算结束
    link.alive.set(false);
    link.push(0, None);
    cx.run_until_parked();
    assert_eq!(
        session.read_with(&cx, |s, _| s.phase.clone()),
        TermPhase::Reconnecting
    );

    // 重连上了：同一个编号、现在的尺寸重接
    let link2 = FakeLink::new();
    let l2: Rc<dyn TermLink> = link2.clone();
    let n = model.update(&mut cx, |m, cx| m.resume_terms(host, l2, cx));
    assert_eq!(n, 1);
    assert!(matches!(
        link2.first(0),
        Some(TermReq::Attach { id: 7, size }) if size.cols == 90 && size.rows == 25
    ));
    assert_eq!(
        session.read_with(&cx, |s, _| s.phase.clone()),
        TermPhase::Opening
    );

    // 主机说会话不在了：结束，不再重接
    link2.push(
        0,
        Some(TermEvent::Error("会话 7 不存在（可能已经结束了）".into())),
    );
    link2.push(0, None);
    cx.run_until_parked();
    assert_eq!(
        session.read_with(&cx, |s, _| s.phase.clone()),
        TermPhase::Gone
    );
    assert_eq!(link2.count(), 1);

    // 连接还在而流被主机断了：自己马上重接
    let link3 = FakeLink::new();
    let l3: Rc<dyn TermLink> = link3.clone();
    let s3 = cx.new(|cx| TermSession::connect(l3, host, TermStart::Attach { id: 9 }, cx));
    link3.push(0, Some(TermEvent::Opened { id: 9 }));
    link3.push(0, Some(TermEvent::Frame(b"x".to_vec())));
    link3.push(0, None);
    cx.run_until_parked();
    assert_eq!(link3.count(), 2, "没有自己重接");
    assert!(matches!(
        link3.first(1),
        Some(TermReq::Attach { id: 9, .. })
    ));

    // 自己断开的不重接
    s3.update(&mut cx, |s, _| s.close());
    link3.push(1, None);
    cx.run_until_parked();
    assert_eq!(link3.count(), 2);
}

/// 一帧的耗时：约 200×60 的终端。帧到了只通知终端（和真的一样），只重画变了的视图。
/// 两种：每帧整屏都是新的字（最坏），每帧滚 3 行（常见的 cat）。时间含客户端终端吃帧、读格子、排字、画。
pub(super) fn terminal_perf(_: &Option<PathBuf>) {
    let mut rig = Rig::new(1900., 1200.);
    let (cols, rows) = (rig.size.cols, rig.size.rows);
    let line = |i: usize, cols: u16| {
        let mut s = String::new();
        let mut n = 0;
        let mut k = i;
        while n + 9 < cols as usize {
            k = k
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let c = 31 + (k >> 33) % 7;
            s += &format!("\x1b[{c}mw{:07}\x1b[0m ", (k >> 20) % 10_000_000);
            n += 9;
        }
        s + "\r\n"
    };
    let measure = |rig: &mut Rig, lines_per_frame: usize, frames: usize, start: usize| {
        let mut times = Vec::new();
        let mut stats = Vec::new();
        for f in 0..frames {
            let mut chunk = String::new();
            for i in 0..lines_per_frame {
                chunk += &line(start + f * lines_per_frame + i, cols);
            }
            rig.host.write(chunk.as_bytes());
            // 主机那边出帧不算；从客户端收到帧开始算（测试模式下 GPUI 在 update 结束时就重画脏了的窗口）
            let (bytes, _) = rig.mirror.frame(&mut rig.host);
            let t = Instant::now();
            rig.session
                .update(&mut rig.cx, |s, cx| s.handle(TermEvent::Frame(bytes), cx));
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            stats.push(rig.grid.read_with(&rig.cx, |g, _| g.stats()));
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let s = stats[stats.len() / 2];
        (times[times.len() / 2], s)
    };
    // 先暖一下（字形进图集）
    measure(&mut rig, rows as usize, 5, 0);
    let (full, fs) = measure(&mut rig, rows as usize, 20, 1000);
    let (scroll, ss) = measure(&mut rig, 3, 40, 5000);
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "terminal_perf：{cols}×{rows}（{profile}）\n  整屏新字：一帧中位 {full:.2} ms（prepaint {:.2} ms，读 {} 行、新做 {} 行、新排字素 {}）\n  滚 3 行：一帧中位 {scroll:.2} ms（prepaint {:.2} ms，读 {} 行、新做 {} 行、新排字素 {}）",
        fs.prepare.as_secs_f64() * 1000.0,
        fs.rebuilt,
        fs.fresh,
        fs.shaped,
        ss.prepare.as_secs_f64() * 1000.0,
        ss.rebuilt,
        ss.fresh,
        ss.shaped,
    );
    assert!(ss.fresh <= 6, "滚动时行缓存没用上：新做了 {} 行", ss.fresh);
    assert_eq!(fs.shaped + ss.shaped, 0, "见过的字又排了一次");
}
