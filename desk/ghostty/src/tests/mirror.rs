//! Mirror 的来回：主机终端一步步吃脚本化的输出，每步取一帧喂给客户端终端，
//! 两边一格一格地比。场景照 mygo 的 mirror_test.go。

use std::fmt::Write as _;

use super::{lib, lines};
use crate::{
    mirror::MIRRORED_MODES,
    render::{CellStyle, RenderState, StyleColor, Width},
    terminal::{Mode, Scroll, Terminal},
    Mirror,
};

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(cols, rows, Box::new(())).unwrap()
}

/// 一行，照格子显示的样子写出来：字、样式、宽度、链接；换不换行；提示符标记。
/// 跟 Mirror 怎么编码无关：空格子和空格一样，行尾宽字符的占位和空白一样。
#[derive(Debug, PartialEq, Eq)]
struct CellRow {
    cells: String,
    wrap: bool,
    prompt: u8,
}

/// 读回滚和屏幕的每一行（同 mygo 的 cellRows）。
fn cell_rows(t: &Terminal) -> Vec<CellRow> {
    let rows = t.size().1 as usize;
    let mut total = rows;
    if !t.alt_screen() {
        total += t.scrollback_rows();
    }
    let mut out: Vec<CellRow> = Vec::new();
    let mut y = 0;
    while y < total {
        t.scroll(Scroll::Row(y));
        let mut rs = RenderState::new().unwrap();
        rs.update(t).unwrap();
        let off = t.scrollbar().offset as usize;
        rs.begin_rows();
        let mut i = 0usize;
        while rs.next_raw_row() {
            if off + i == out.len() && out.len() < total {
                let wrap = rs.row_wrapped();
                let prompt = rs.row_prompt();
                let n = rs.load_raw().len();
                let mut s = String::new();
                for x in 0..n {
                    let c = rs.raw()[x];
                    let mut text = String::new();
                    if c.grapheme {
                        rs.cell_graphemes(x as u16, &mut text);
                    } else if let Some(ch) = char::from_u32(c.codepoint) {
                        text.push(ch);
                    }
                    if c.codepoint == 0 || c.width() == Width::SpacerHead {
                        text = " ".into();
                    }
                    if c.width() == Width::SpacerTail {
                        text.clear();
                    }
                    let mut st = if c.style != 0 {
                        rs.cell_style(x as u16).sgr()
                    } else {
                        String::new()
                    };
                    if let (Some((palette, bg)), 0) = (c.bg, c.codepoint) {
                        // 程序填了背景、没有字的格子，显示成那个背景的空格。
                        let bg = if palette {
                            StyleColor::Palette(bg.r)
                        } else {
                            StyleColor::Rgb(bg)
                        };
                        st = CellStyle {
                            bg,
                            ..CellStyle::default()
                        }
                        .sgr();
                    }
                    let link = if c.hyperlink {
                        t.hyperlink_at(x as u16, i as u16).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let wide = if c.width() == Width::SpacerHead {
                        0
                    } else {
                        c.wide
                    };
                    if text == " " && st == "\x1b[0m" {
                        st.clear();
                    }
                    let _ = write!(s, "[{text}|{st:?}|{wide}|{link}]");
                }
                while let Some(rest) = s.strip_suffix("[ |\"\"|0|]") {
                    s.truncate(rest.len());
                }
                out.push(CellRow {
                    cells: s,
                    wrap,
                    prompt,
                });
            }
            i += 1;
        }
        let next = off + rows;
        if next <= y {
            break;
        }
        y = next;
    }
    t.scroll(Scroll::Bottom);
    out
}

/// 客户端显示的跟主机一样：每一格（字、样式、颜色、宽度、链接）、每行的换行、光标、模式、标题。
/// pruned：主机的回滚到了上限丢了行，客户端留着，比较时只看客户端最后那些行。
/// text：也比 text()（随机改动时不比，见 random_edits）。
fn same(a: &Terminal, b: &Terminal, label: &str, pruned: bool, text: bool) {
    assert_eq!(a.alt_screen(), b.alt_screen(), "{label}：备用屏");
    assert_eq!(a.size(), b.size(), "{label}：尺寸");
    if !a.alt_screen() {
        let (sa, sb) = (a.scrollback_rows(), b.scrollback_rows());
        assert!(
            sa == sb || (pruned && sb > sa),
            "{label}：回滚 {sa} 行，客户端 {sb} 行"
        );
    }
    let ra = cell_rows(a);
    let mut rb = cell_rows(b);
    if pruned && rb.len() > ra.len() {
        rb.drain(..rb.len() - ra.len());
    }
    assert_eq!(ra.len(), rb.len(), "{label}：行数");
    let screen_top = ra.len() - a.size().1 as usize;
    for (y, (x, z)) in ra.iter().zip(&rb).enumerate() {
        assert_eq!(
            x.cells,
            z.cells,
            "{label}：第 {y} 行（屏幕第 {} 行）",
            y as isize - screen_top as isize
        );
        // 屏幕最后一行的软换行客户端没法画（要滚屏），后面也没东西接，下次滚屏时会重画。
        if y != ra.len() - 1 {
            assert_eq!(x.wrap, z.wrap, "{label}：第 {y} 行的换行");
        }
        assert_eq!(x.prompt, z.prompt, "{label}：第 {y} 行的提示符标记");
    }
    // 文字按软换行把行接起来；最后一行软换行时（比如滚动区域外的底行写满了绕回自己）
    // 客户端那行没有换行标记，文字会不一样，格子是一样的（上面比过了）。
    if text && !pruned && !ra.last().is_some_and(|r| r.wrap) {
        assert_eq!(a.text(), b.text(), "{label}：文字");
    }
    assert_eq!(a.cursor(), b.cursor(), "{label}：光标");
    for &m in MIRRORED_MODES.iter().chain(&[25]) {
        assert_eq!(
            a.mode(Mode::dec(m)),
            b.mode(Mode::dec(m)),
            "{label}：模式 {m}"
        );
    }
    assert_eq!(
        a.kitty_keyboard_flags(),
        b.kitty_keyboard_flags(),
        "{label}：Kitty 键盘标志"
    );
    assert_eq!(
        a.modify_other_keys(),
        b.modify_other_keys(),
        "{label}：modifyOtherKeys"
    );
    assert_eq!(a.mouse_shape(), b.mouse_shape(), "{label}：指针形状");
    assert_eq!(a.title(), b.title(), "{label}：标题");
}

/// 一个主机终端和一个吃它 Mirror 帧的客户端终端。
struct Mirrored {
    src: Terminal,
    dst: Terminal,
    m: Mirror,
    frames: usize,
    pruned: bool,
    text: bool,
}

impl Mirrored {
    fn new(cols: u16, rows: u16) -> Mirrored {
        let mut x = Mirrored {
            src: term(cols, rows),
            dst: term(cols, rows),
            m: Mirror::new(),
            frames: 0,
            pruned: false,
            text: true,
        };
        x.frame("开始");
        x
    }

    fn feed(&mut self, label: &str, out: &str) {
        self.src.write(out.as_bytes());
        self.frame(label);
    }

    /// 改尺寸：主机先出一帧（旧尺寸下的），两边都改，再出一帧（同 kero 的 resize）。
    fn resize(&mut self, cols: u16, rows: u16) {
        self.frame("改尺寸之前");
        self.src.resize(cols, rows, 8, 16);
        self.dst.resize(cols, rows, 8, 16);
        self.frame(&format!("改成 {cols}x{rows}"));
    }

    fn frame(&mut self, label: &str) {
        let (f, due) = self.m.frame(&mut self.src);
        if due {
            return; // 同步更新扣着，下一帧再显示
        }
        self.dst.write(&f);
        self.frames += 1;
        same(
            &self.src,
            &self.dst,
            &format!("{label}（第 {} 帧）", self.frames),
            self.pruned,
            self.text,
        );
    }
}

#[test]
fn round_trip() {
    if !lib() {
        return;
    }
    let mut x = Mirrored::new(20, 6);
    x.feed("几行", &lines(0, 3, "line "));
    x.feed("超过一屏", &lines(3, 9, "line "));
    x.feed("滚过好多屏", &lines(9, 300, "row "));
    x.feed("再一行", "last\r\n");
    x.feed(
        "16 色",
        "\x1b[31mred\x1b[0m \x1b[1;32mgreen\x1b[0m \x1b[44mbg\x1b[0m\r\n",
    );
    x.feed(
        "256 色",
        "\x1b[38;5;208morange\x1b[48;5;22m on\x1b[0m \x1b[38;5;250mgray\x1b[0m\r\n",
    );
    x.feed(
        "真彩色",
        "\x1b[38;2;10;20;30mrgb\x1b[48;2;200;100;50m bg\x1b[0m\r\n",
    );
    x.feed(
        "粗斜下划线",
        "\x1b[1mbold\x1b[22m \x1b[3mital\x1b[23m \x1b[4mund\x1b[24m \x1b[4:3;58;5;1mcurl\x1b[0m \x1b[9;53mx\x1b[0m \x1b[2;7mrev\x1b[0m\r\n",
    );
    x.feed(
        "链接",
        "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\ after\r\n",
    );
    x.feed("中文和 emoji", "你好世界 😄 👍🏽 ok\r\n");
    x.feed("组合字符", "e\u{301} a\u{308}\r\n");
    x.feed("软换行", &format!("{}\r\n", "abcdefghij".repeat(5)));
    x.feed("软换行滚出去", &lines(0, 8, "after "));
    x.feed("行尾放不下宽字符", &format!("{}世\r\n", "x".repeat(19)));
    x.feed("光标定位", "\x1b[2;5Hhere\x1b[4;10Hthere\x1b[6;1H");
    x.feed("清行", "\x1b[3;3H\x1b[K\x1b[5;4H\x1b[1K\x1b[2;1H\x1b[2K");
    x.feed("插行删行", "\x1b[3;1H\x1b[2L\x1b[2;1H\x1b[1M");
    x.feed(
        "滚动区域",
        &format!(
            "\x1b[2;4r\x1b[4;1H{}\x1b[r\x1b[6;1H",
            lines(0, 5, "in region ")
        ),
    );
    x.feed("背景填行", "\x1b[41m\x1b[K\x1b[0m\x1b[10Gafter\r\n");
    x.feed("清屏", "\x1b[2J\x1b[H");
    x.feed("清屏之后", &lines(0, 3, "new "));
    x.feed("清回滚", "\x1b[3J");
    x.feed("清回滚之后", &lines(0, 9, "more "));
    x.feed("标题", "\x1b]2;hello title\x07");
    x.feed(
        "模式",
        "\x1b[?1h\x1b[?2004h\x1b[>1u\x1b[?1000h\x1b[?1006h\x1b[?1004h\x1b[>4;2m\x1b]22;pointer\x1b\\",
    );
    x.feed(
        "进备用屏",
        "\x1b[?1049h\x1b[H\x1b[2Jfull screen\x1b[3;3Hthere",
    );
    x.feed("备用屏上画", &format!("\x1b[5;1H{}", "-".repeat(20)));
    x.resize(30, 8);
    x.feed(
        "备用屏改尺寸后重画",
        "\x1b[H\x1b[2Jredrawn at 30x8\x1b[8;1Hstatus",
    );
    x.feed("出备用屏", "\x1b[?1049l");
    x.feed("之后", "$ ls\r\n");
    x.feed("长行", &format!("{}\r\n$ ", "0123456789".repeat(4)));
    x.resize(12, 5);
    x.feed("缩小之后", &lines(0, 3, "s"));
    x.resize(40, 10);
    x.feed("放大之后", "more\r\n");
    x.feed(
        "模式关",
        "\x1b[?1l\x1b[?2004l\x1b[<u\x1b[?1000l\x1b[?1006l\x1b[?1004l\x1b[>4;1m\x1b[?25l",
    );
    x.feed("光标样式", "\x1b[?25h\x1b[5 q");
    x.feed("提示符", "\x1b]133;A\x07$ \x1b]133;B\x07echo hi");
    x.feed("命令输出", "\r\n\x1b]133;C\x07hi\r\n\x1b]133;D;0\x07");
    x.feed("没换行的提示符", "$ ");
}

#[test]
fn cursor_style_follows_the_client_default() {
    if !lib() {
        return;
    }
    let mut src = term(20, 4);
    let mut dst = term(20, 4);
    dst.set_default_cursor(crate::CursorStyle::Bar, false);
    let mut m = Mirror::new();
    let mut step = |out: &str, style, blink| {
        src.write(out.as_bytes());
        let (f, _) = m.frame(&mut src);
        dst.write(&f);
        let mut rs = RenderState::new().unwrap();
        rs.update(&dst).unwrap();
        let c = rs.cursor();
        assert_eq!((c.style, c.blinking), (style, blink), "{out:?}");
    };
    use crate::CursorStyle::*;
    step("$ ", Bar, false);
    step("\x1b[3 q", Underline, true);
    step("\x1b[2 q", Block, false);
    step("\x1b[0 q", Bar, false);
}

#[test]
fn two_mirrors_and_a_late_one() {
    if !lib() {
        return;
    }
    let mut src = term(30, 6);
    let mut clients = vec![(Mirror::new(), term(30, 6))];
    clients.push((Mirror::new(), term(30, 6)));
    let steps = [
        lines(0, 4, "a"),
        "\x1b[2;1Hedit\x1b[K".to_string(),
        lines(0, 40, "b"),
        "\x1b[44mbg\x1b[0m\x1b[20Gx\r\n".to_string(),
        "\x1b[?1049h\x1b[Hfull".to_string(),
        "\x1b[?1049l".to_string(),
        "你好 😄\r\n".to_string(),
    ];
    let frame_all = |src: &mut Terminal, clients: &mut Vec<(Mirror, Terminal)>, label: &str| {
        for (i, (m, dst)) in clients.iter_mut().enumerate() {
            let (f, due) = m.frame(src);
            assert!(!due);
            dst.write(&f);
            same(src, dst, &format!("{label}，第 {i} 个客户端"), false, true);
        }
    };
    frame_all(&mut src, &mut clients, "开始");
    for (i, out) in steps.iter().enumerate() {
        src.write(out.as_bytes());
        frame_all(&mut src, &mut clients, &format!("第 {i} 步"));
        // 第一个客户端多取一帧，第二个的下一帧一次补上好几屏。
        if i == 3 {
            src.write(lines(0, 20, "skipped ").as_bytes());
            let (f, _) = clients[0].0.frame(&mut src);
            clients[0].1.write(&f);
        }
    }
    // 后来的 Mirror：第一帧从头来，带上回滚。
    clients.push((Mirror::new(), term(30, 6)));
    frame_all(&mut src, &mut clients, "后来的");
    for (i, out) in steps.iter().enumerate() {
        src.write(out.as_bytes());
        frame_all(&mut src, &mut clients, &format!("又第 {i} 步"));
    }
    // reset 之后换一个新的客户端终端接着显示。
    clients[0].0.reset();
    clients[0].1 = term(30, 6);
    frame_all(&mut src, &mut clients, "reset 之后");
}

#[test]
fn reset_while_a_full_screen_program_shows() {
    if !lib() {
        return;
    }
    let mut x = Mirrored::new(20, 5);
    x.feed("历史", &lines(0, 50, "x"));
    x.feed("全屏程序", "\x1b[?1049hfull screen");
    x.m.reset();
    x.dst = term(20, 5);
    x.frame("另一个客户端");
    x.feed("程序退出", "\x1b[?1049l");
}

#[test]
fn clear_screen_and_scrollback() {
    if !lib() {
        return;
    }
    let mut x = Mirrored::new(20, 6);
    x.feed("历史", &format!("{}~/dev\r\n$ ", lines(0, 12, "line ")));
    assert!(x.src.clear_screen());
    x.frame("清屏");
    assert_eq!(x.dst.text().trim(), "$");
    x.feed("打字", "ls");
    x.feed("重画这行", "\r\x1b[K$ ls -la");
    x.resize(30, 8);
    x.resize(12, 5);
    assert_eq!(x.dst.text().trim(), "$ ls -la");
    x.feed("之后的输出", &format!("\r\n{}", lines(0, 9, "out ")));
    x.src.clear_scrollback();
    x.frame("清了回滚");
    assert_eq!(x.src.scrollback_rows(), 0);
    x.feed("全屏程序里不清", "\x1b[?1049h");
    assert!(!x.src.clear_screen());
}

#[test]
fn synchronized_output_holds_frames() {
    if !lib() {
        return;
    }
    let mut x = Mirrored::new(20, 5);
    x.src.write(b"\x1b[?2026hhalf");
    let (f, due) = x.m.frame(&mut x.src);
    assert!(f.is_empty() && due, "同步更新时不该出帧");
    x.feed("同步更新结束", "-drawn\x1b[?2026l");
    assert!(x.dst.text().contains("half-drawn"));
}

#[test]
fn scrollback_limit() {
    if !lib() {
        return;
    }
    let mut x = Mirrored::new(20, 5);
    x.src.set_scrollback_limit(1);
    for i in 0..40 {
        x.feed(&format!("第 {i} 批"), &lines(i * 100, i * 100 + 60, "x"));
    }
}

/// 到上限的回滚边收边丢最老的行：客户端留着，不从头来，不管一帧滚一行还是一屏多。
#[test]
fn scrollback_pruned() {
    if !lib() {
        return;
    }
    let mut x = Mirrored::new(40, 5);
    x.src.set_scrollback_limit(64 << 10);
    x.pruned = true;
    let (mut drops, mut n, mut prev) = (0, 0, 0);
    for i in 0..400 {
        let count = if i % 2 == 1 { 23 } else { 1 };
        x.src
            .write(lines(n, n + count, "line abcdefghijklmnopqrstuvwxyz ").as_bytes());
        n += count;
        let (f, due) = x.m.frame(&mut x.src);
        assert!(!due);
        assert!(
            !String::from_utf8_lossy(&f).contains("\x1b[3J"),
            "第 {i} 帧清了客户端的回滚"
        );
        x.dst.write(&f);
        x.frames += 1;
        let sb = x.src.scrollback_rows();
        if sb < prev {
            drops += 1;
        }
        if sb < prev || i % 25 == 0 {
            same(&x.src, &x.dst, &format!("第 {i} 帧"), true, false);
        }
        prev = sb;
    }
    assert!(drops >= 2, "回滚只丢了 {drops} 次行");
    // 真清回滚的话客户端也清。
    x.pruned = false;
    x.feed("清回滚", "\x1b[3J");
    x.feed("清了之后", &lines(0, 12, "after "));
}

/// 简单的伪随机数（xorshift），不引 rand。
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// 上千次随机改动，什么都有，每帧都比。
#[test]
fn random_edits() {
    if !lib() {
        return;
    }
    for seed in 1..=3u64 {
        let mut r = Rng(seed.wrapping_mul(0x9e3779b97f4a7c15) | 1);
        let mut x = Mirrored::new(16, 6);
        // 插行、删行、滚动区域会让主机留下“这行软换行、下一行却不是接续”的状态，
        // 帧只能真的换行画出来，客户端下一行就成了接续：格子和换行标记一样（每帧都比），
        // 按软换行接起来的 text() 会不一样，这里不比（mygo 的随机测试也不比文字）。
        x.text = false;
        let words = [
            "a".to_string(),
            "hello".into(),
            "世界".into(),
            "😄".into(),
            "\x1b[1;32mgreen\x1b[0m".into(),
            "\x1b[48;5;3m  \x1b[0m".into(),
            "\x1b]8;;https://x.y\x1b\\ln\x1b]8;;\x1b\\".into(),
            "w".repeat(21),
        ];
        for i in 0..1000 {
            let mut b = String::new();
            for _ in 0..1 + r.below(4) {
                let op = r.below(13);
                let s = match op {
                    0 => words[r.below(words.len())].clone(),
                    1 => "\r\n".into(),
                    2 => lines(0, 1 + r.below(20), "r"),
                    3 => format!("\x1b[{};{}H", 1 + r.below(6), 1 + r.below(16)),
                    4 => ["\x1b[K", "\x1b[1K", "\x1b[2K", "\x1b[J", "\x1b[1J"][r.below(5)].into(),
                    5 => format!("\x1b[{}L", 1 + r.below(3)),
                    6 => format!("\x1b[{}M", 1 + r.below(3)),
                    7 => format!("\x1b[{}S", 1 + r.below(3)),
                    8 => format!("\x1b[{}T", 1 + r.below(3)),
                    9 => ["\x1b[2;5r", "\x1b[r"][r.below(2)].into(),
                    10 => ["\x1b[?1049h", "\x1b[?1049l"][r.below(2)].into(),
                    11 => [
                        "\x1b[?25l",
                        "\x1b[?25h",
                        "\x1b[?2004h",
                        "\x1b[?1000h\x1b[?1006h",
                        "\x1b[?1000l",
                    ][r.below(5)]
                    .into(),
                    _ => "\x1bM".into(), // 反向换行
                };
                b.push_str(&s);
            }
            x.feed(&format!("种子 {seed} 第 {i} 步 {b:?}"), &b);
        }
    }
}

/// History 是光标以上显示的东西，光标停在它下面：光标下面程序画的（比如状态栏）不要。
#[test]
fn history() {
    if !lib() {
        return;
    }
    let mut src = term(20, 6);
    src.write(
        format!(
            "{}{}\r\n\x1b[1;31mred\x1b[0m\r\n> input\r\nstatus bar\x1b[1A\x1b[3G",
            lines(0, 10, "line "),
            "abcdefghij".repeat(3)
        )
        .as_bytes(),
    );
    let h = src.history();
    let mut dst = term(20, 6);
    dst.write(&h);
    let text = dst.text();
    for want in [
        "line 0",
        "line 9",
        &"abcdefghij".repeat(3),
        "red",
        "> input",
    ] {
        assert!(text.contains(want), "历史丢了 {want:?}：\n{text}");
    }
    assert!(!text.contains("status bar"), "历史留了光标下面的：\n{text}");
    // 软换行还是一行。
    assert!(text.contains(&format!("{}\n", "abcdefghij".repeat(3))));
    // 光标在新的一行的开头。
    let (cx, cy) = dst.cursor();
    assert_eq!(cx, 0);
    let mut rs = RenderState::new().unwrap();
    rs.update(&dst).unwrap();
    let mut rows = rs.rows();
    let mut y = 0;
    while let Some(mut row) = rows.next_row() {
        if y == cy {
            let cells = row.cells();
            assert!(cells.iter().all(|(_, t)| t.trim().is_empty()));
        }
        y += 1;
    }
    // 回滚以上的文字跟主机一样（光标以上的部分）。
    let src_text = src.text();
    let above = src_text.split("> input").next().unwrap();
    assert!(text.starts_with(above), "{text:?} 不以 {above:?} 开头");
    // 全屏程序的屏幕没有历史。
    let mut alt = term(20, 6);
    alt.write(b"\x1b[?1049hfull screen");
    assert!(alt.history().is_empty());
}
