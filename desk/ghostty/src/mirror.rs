//! Mirror：把主机终端显示的东西做成帧，客户端的终端吃下去就显示得一模一样；
//! 还有 History：存盘用的回滚和屏幕。
//!
//! 移植自 mygo plugins/terminal/mirror.go（MIT，Copyright (c) 2026 MyGo contributors）。
//! 没移植的：Kitty 图片（mirror_images.go；终端的图片上限是 0，程序会退回别的画法），
//! OSC 52 剪贴板的转发（程序写、读剪贴板都不传给客户端）。
//!
//! 帧把行画在原位，滚出屏幕的行按顺序从客户端屏幕顶上滚过去，所以客户端的回滚就是主机的：
//! 主机回滚到上限丢掉的行，客户端照样留着，归它自己的上限管。软换行还是软换行。
//! 第一帧、屏幕换了尺寸之后的那帧，让客户端从头来。程序在同步更新（模式 2026）时不出帧，
//! 最多扣一秒，`frame` 返回“稍后还有一帧”。
//!
//! 帧读终端时自己建渲染状态（每次整份拷）：留着一个的话只拷终端标了变的行，
//! 而标记归先读的人，可能是另一个 Mirror。一个终端可以有好几个 Mirror。

use std::{collections::HashMap, fmt::Write as _};

use crate::{
    ffi::RowId,
    input::{MouseAction, MouseButton, MouseEncoder, MouseEvent, MouseSize},
    render::{CellStyle, Cursor, CursorStyle, RenderState, StyleColor, Width},
    terminal::{Mode, Scroll, Terminal},
    Mods,
};

/// 一行属于 shell 提示符的哪部分（OSC 133）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
enum Prompt {
    #[default]
    None = 0,
    Start = 1,
    Continuation = 2,
}

impl Prompt {
    fn from_u8(v: u8) -> Prompt {
        match v {
            1 => Prompt::Start,
            2 => Prompt::Continuation,
            _ => Prompt::None,
        }
    }
}

// 客户端那边一行属于提示符的标记（OSC 133），帧画的每一行都带上，画完退出提示符（MARK_END）。
const MARK_PROMPT: &str = "\x1b]133;A\x07";
const MARK_CONTINUATION: &str = "\x1b]133;A;k=c\x07";
/// 在第一列清掉这一行的标记。
const MARK_OUTPUT: &str = "\x1b]133;C\x07";
/// 退出提示符，行的标记留着。
const MARK_END: &str = "\x1b]133;D\x07";

/// 从第一列之后标一行（MARK_PROMPT 会另起一行）。
fn mark_after(p: Prompt) -> &'static str {
    match p {
        Prompt::Start => "\x1b]133;P\x07",
        Prompt::Continuation => "\x1b]133;P;k=c\x07",
        Prompt::None => "",
    }
}

/// 客户端拿的模式：改变它发什么（键、鼠标）、怎么画的。关着的先发：鼠标的事件和格式各是一个设置，
/// 设其中一个模式就选它，重置任何一个就回到没有 / X10，同 Ghostty。
pub(crate) const MIRRORED_MODES: &[u16] = &[
    1, 66, // DECCKM、DECNKM（小键盘）
    67, 1035, 1036, 1039, // DECBKM（退格）、NumLock 小键盘、Alt 的 ESC
    1004, 1007, 2004, 2027, // 焦点、备用屏滚轮、括号粘贴、字素簇
    9, 1000, 1002, 1003, 1005, 1006, 1015, 1016, // 鼠标
    5,    // DECSCNM，整屏反色
];

const MOUSE_EVENTS: &[u16] = &[9, 1000, 1002, 1003];
const MOUSE_FORMATS: &[u16] = &[1005, 1006, 1015, 1016];

/// OSC 22 的指针形状，按 Ghostty 的顺序（GhosttyMouseShape）。
pub const MOUSE_SHAPES: &[&str] = &[
    "default",
    "context-menu",
    "help",
    "pointer",
    "progress",
    "wait",
    "cell",
    "crosshair",
    "text",
    "vertical-text",
    "alias",
    "copy",
    "move",
    "no-drop",
    "not-allowed",
    "grab",
    "grabbing",
    "all-scroll",
    "col-resize",
    "row-resize",
    "n-resize",
    "e-resize",
    "s-resize",
    "w-resize",
    "ne-resize",
    "nw-resize",
    "se-resize",
    "sw-resize",
    "ew-resize",
    "ns-resize",
    "nesw-resize",
    "nwse-resize",
    "zoom-in",
    "zoom-out",
];

/// 把一个终端显示的东西做成帧，给另一个终端显示。见模块说明。
#[derive(Default)]
pub struct Mirror {
    /// 出过帧；cols、rows 是画的尺寸，alt 是那时在备用屏。
    started: bool,
    cols: u16,
    rows: u16,
    alt: bool,
    /// 上一帧各行的身份（滚动时不变）；sent 是每行怎么画的（空的是没画），
    /// main 是备用屏显示时主屏的。
    ids: Vec<RowId>,
    sent: Vec<Vec<u8>>,
    main: Vec<Vec<u8>>,
    /// 上一帧显示主屏时主屏回滚有几行（-1：从备用屏开始的，主屏还没读）。
    scrollback: isize,
    /// 上一帧设的模式、颜色、光标、标题。
    modes: String,
    colors: String,
    cursor: String,
    title: String,
}

impl Mirror {
    pub fn new() -> Mirror {
        Mirror::default()
    }

    /// 下一帧让客户端从头来（比如换了一个客户端终端接着显示）。
    pub fn reset(&mut self) {
        self.started = false;
        self.colors.clear();
        self.title.clear();
    }

    /// 让客户端终端从上一帧显示的变成 `t` 现在显示的那段字节（没变时为空），
    /// 和稍后是不是还要一帧（同步更新扣着这一帧时）。
    pub fn frame(&mut self, t: &mut Terminal) -> (Vec<u8>, bool) {
        if t.render_held() {
            return (Vec::new(), true);
        }
        let mut out = String::new();
        self.frame_into(t, &mut out);
        (out.into_bytes(), false)
    }

    fn frame_into(&mut self, t: &mut Terminal, out: &mut String) {
        let (cols, rows) = t.size();
        let alt = t.alt_screen();
        let scrollback = t.scrollback_rows() as isize;
        if !t.at_bottom() {
            t.scroll(Scroll::Bottom);
        }
        let Ok(mut rs) = RenderState::new() else {
            return;
        };
        if rs.update(t).is_err() {
            return;
        }
        let cursor = rs.cursor();
        let mut ids = Vec::with_capacity(rows as usize);
        let mut screen = Vec::with_capacity(rows as usize);
        rs.begin_rows();
        let mut y = 0u16;
        while rs.next_raw_row() {
            ids.push(rs.row_id());
            screen.push(read_line(t, &mut rs, y));
            y += 1;
        }
        drop(rs);
        // 屏幕第一行从回滚最上面数是第几行。
        let top = if alt { 0 } else { scrollback };
        if !self.started || cols != self.cols || rows != self.rows {
            self.restart(t, out, screen, alt, top);
        } else if alt != self.alt && !alt && scrollback != self.scrollback {
            // 程序进备用屏之前有行进了主屏的回滚，被它挡着没画：从头来。
            self.restart(t, out, screen, alt, top);
        } else if alt != self.alt {
            if alt {
                out.push_str("\x1b[?1049h");
                self.main = std::mem::replace(&mut self.sent, vec![Vec::new(); rows as usize]);
            } else {
                out.push_str("\x1b[?1049l");
                self.sent = std::mem::take(&mut self.main);
            }
            self.redraw(cols, out, &screen);
        } else {
            let scrolled = if alt {
                0
            } else {
                self.scrolled(t, &ids, scrollback)
            };
            if scrolled < 0 {
                // 跟丢了：从头来。
                self.restart(t, out, screen, alt, top);
            } else if scrolled > 0 {
                // 有行进了回滚：它们按顺序从客户端屏幕顶上过去，屏幕的行跟在后面。回滚变少了、
                // 而它们之前的行一行不剩，是被清了；到上限的回滚是丢最老的行，客户端留着，
                // 归它自己的上限管。
                if scrollback < self.scrollback && scrollback <= scrolled {
                    out.push_str("\x1b[3J");
                }
                let mut lines = read_lines(t, top - scrolled, top);
                lines.extend(screen);
                self.write(cols, out, &lines);
            } else {
                // 主屏的回滚被清了。备用屏没有回滚（读出来是 0），self.scrollback 记的是主屏的，
                // 不比（mygo 这里会在备用屏的每帧都发一次 ED 3，Ghostty 在备用屏上不理它）。
                if !alt && scrollback < self.scrollback {
                    out.push_str("\x1b[3J");
                }
                self.redraw(cols, out, &screen);
            }
        }
        self.started = true;
        self.cols = cols;
        self.rows = rows;
        self.alt = alt;
        self.ids = ids;
        if !alt {
            self.scrollback = scrollback;
        }
        self.state(t, out, &cursor);
    }

    /// 上一帧以来滚出屏幕几行；ids 是现在各行的身份。看不出来是 -1。
    fn scrolled(&self, t: &Terminal, ids: &[RowId], scrollback: isize) -> isize {
        let (Some(first), Some(&last_first)) = (ids.first(), self.ids.first()) else {
            return -1;
        };
        let k = self.ids.iter().position(|id| id == first);
        let grew = scrollback - self.scrollback;
        match k {
            // 屏幕的行没动，回滚却在一屏行的下面长了：滚了不止一屏。
            Some(0) if grew > 0 => return grew,
            Some(k) => return k as isize,
            None => {}
        }
        // 滚了不止一屏：上一帧第一行以下、现在在回滚里的行数。回滚长了多少也能说明，
        // 但到上限的回滚边收边丢，说不准。
        let n = scrolled_past(t, last_first, scrollback);
        if n > 0 {
            return n;
        }
        if grew > 0 {
            return grew;
        }
        -1
    }

    /// 让客户端从头来：先清它的回滚和屏幕，再一行行画这边的。
    fn restart(
        &mut self,
        t: &Terminal,
        out: &mut String,
        screen: Vec<Line>,
        alt: bool,
        top: isize,
    ) {
        // 两个屏都清（图片没移植，不用删）：进出一次备用屏，回到主屏。
        out.push_str("\x1b[?1049h\x1b[?1049l");
        out.push_str("\x1b[0m\x1b[H\x1b[2J\x1b[3J");
        out.push_str(MARK_OUTPUT);
        self.modes.clear();
        self.cursor.clear();
        self.sent = vec![Vec::new(); screen.len()];
        self.main = Vec::new();
        let cols = t.size().0;
        if alt {
            // 全屏程序底下的主屏不读：客户端的主屏和回滚现在是空的，程序结束时再从头来。
            self.scrollback = -1;
            out.push_str("\x1b[?1049h");
            self.redraw(cols, out, &screen);
            return;
        }
        let mut lines = read_lines(t, 0, top);
        lines.extend(screen);
        self.write(cols, out, &lines);
    }

    /// 从客户端屏幕顶上一行接一行地画，前面多出来的（屏幕之前的）滚进它的回滚，
    /// 跟它们进这边回滚时一样。
    fn write(&mut self, cols: u16, out: &mut String, lines: &[Line]) {
        let rows = self.sent.len();
        let screen = lines.len() as isize - rows as isize;
        // 前几行盖在客户端屏幕原来的行上，别的画在滚进来的、没有标记的新行上。
        let was = |sent: &[Vec<u8>], i: usize| {
            if i < rows {
                key_prompt(&sent[i])
            } else {
                Prompt::None
            }
        };
        for i in 1..rows.min(lines.len()) {
            if lines[i - 1].wrap {
                premark(out, i, &lines[i], was(&self.sent, i));
            }
        }
        out.push_str("\x1b[H");
        for (i, l) in lines.iter().enumerate() {
            let last = i == lines.len() - 1;
            let next = (!last).then(|| &lines[i + 1]);
            let prompt = was(&self.sent, i);
            let k = row(cols, out, l, next, i > 0 && lines[i - 1].wrap, prompt);
            if i as isize >= screen {
                self.sent[(i as isize - screen) as usize] = k;
            }
            if !last && !l.wrap {
                out.push_str("\r\n");
            }
        }
    }

    /// 重画屏幕上变了的行，连同它们所在的软换行的整段，从那段的第一行画起。
    fn redraw(&mut self, cols: u16, out: &mut String, screen: &[Line]) {
        let rows = screen.len();
        if self.sent.len() != rows {
            self.sent = vec![Vec::new(); rows];
        }
        let mut y = 0;
        while y < rows {
            if !self.sent[y].is_empty() && screen[y].same_key(&self.sent[y]) {
                y += 1;
                continue;
            }
            // 从这段的第一行（前一行软换行接过来的往上找）。
            let mut s = y;
            while s > 0 && screen[s - 1].wrap {
                s -= 1;
            }
            let mut r = s + 1;
            while r < rows && screen[r - 1].wrap {
                premark(out, r, &screen[r], key_prompt(&self.sent[r]));
                r += 1;
            }
            let _ = write!(out, "\x1b[{}H", s + 1);
            let first = s;
            while s < rows {
                let next = (s + 1 < rows && screen[s].wrap).then(|| &screen[s + 1]);
                let was = key_prompt(&self.sent[s]);
                self.sent[s] = row(cols, out, &screen[s], next, s > first, was);
                if next.is_none() {
                    break;
                }
                s += 1;
            }
            y = s + 1;
        }
    }

    /// 设光标、模式、标题、颜色，变了才发。
    fn state(&mut self, t: &Terminal, out: &mut String, c: &Cursor) {
        let mut modes = String::new();
        let mut on = Vec::new();
        for &md in MIRRORED_MODES {
            if t.mode(Mode::dec(md)) {
                on.push(md);
            } else {
                let _ = write!(modes, "\x1b[?{md}l");
            }
        }
        let (on, off) = mouse_last(t, on);
        for md in on {
            let _ = write!(modes, "\x1b[?{md}h");
        }
        if off != 0 {
            let _ = write!(modes, "\x1b[?{off}l");
        }
        let mok = if t.modify_other_keys() { 2 } else { 1 };
        let _ = write!(modes, "\x1b[={};1u\x1b[>4;{mok}m", t.kitty_keyboard_flags());
        if let Some(name) = usize::try_from(t.mouse_shape())
            .ok()
            .and_then(|i| MOUSE_SHAPES.get(i))
        {
            let _ = write!(modes, "\x1b]22;{name}\x1b\\");
        }
        if modes != self.modes {
            out.push_str(&modes);
            self.modes = modes;
        }
        let colors = program_colors(t);
        if colors != self.colors {
            out.push_str(&colors);
            self.colors = colors;
        }
        let title = t.title();
        if title != self.title {
            let _ = write!(out, "\x1b]2;{title}\x1b\\");
            self.title = title;
        }
        let (x, y) = t.cursor();
        // 光标是这边终端默认的样子（程序没动或重置了）时，客户端也用它自己的默认（DECSCUSR 0）。
        let (style, blink) = t.default_cursor();
        let mut shape = 0;
        if c.style != style || c.blinking != blink {
            shape = match c.style {
                CursorStyle::Underline => 4,
                CursorStyle::Bar => 6,
                CursorStyle::Block | CursorStyle::BlockHollow => 2,
            };
            if c.blinking {
                shape -= 1;
            }
        }
        let visible = if c.visible { 'h' } else { 'l' };
        let cur = format!(
            "\x1b[0m\x1b[{};{}H\x1b[?25{visible}\x1b[{shape} q",
            y + 1,
            x + 1
        );
        // 画了行之后光标得放回去，哪怕它没动。
        if cur != self.cursor || !out.is_empty() {
            out.push_str(&cur);
            self.cursor = cur;
        }
    }
}

/// 回滚（有 scrollback 行）里在 id 这行以下的有几行，从下往上一屏一屏找；没有这行是 -1。
/// 视口放回最下面。
fn scrolled_past(t: &Terminal, id: RowId, scrollback: isize) -> isize {
    let rows = t.size().1 as isize;
    let found = (|| {
        let mut y = scrollback;
        while y > 0 {
            y = (y - rows).max(0);
            t.scroll(Scroll::Row(y as usize));
            let mut rs = RenderState::new().ok()?;
            rs.update(t).ok()?;
            let off = t.scrollbar().offset as isize;
            let mut found = None;
            rs.begin_rows();
            let mut i = 0;
            while rs.next_raw_row() {
                let r = off + i;
                if r < scrollback && found.is_none() && rs.row_id() == id {
                    found = Some(scrollback - r);
                }
                i += 1;
            }
            if found.is_some() {
                return found;
            }
            y = y.min(off);
        }
        None
    })();
    t.scroll(Scroll::Bottom);
    found.unwrap_or(-1)
}

/// 读从回滚最上面数第 from 到 to（不含）行，一屏一屏经视口读，读完视口放回最下面。
/// 每屏用自己的渲染状态（见模块说明）。
fn read_lines(t: &Terminal, from: isize, to: isize) -> Vec<Line> {
    let mut out = Vec::with_capacity((to - from).max(0) as usize);
    if to <= from {
        return out;
    }
    let rows = t.size().1 as isize;
    let mut y = from;
    while y < to {
        t.scroll(Scroll::Row(y as usize));
        let Ok(mut rs) = RenderState::new() else {
            break;
        };
        if rs.update(t).is_err() {
            break;
        }
        let off = t.scrollbar().offset as isize;
        rs.begin_rows();
        let mut i = 0isize;
        while rs.next_raw_row() {
            let r = off + i;
            if r == from + out.len() as isize && r < to {
                out.push(read_line(t, &mut rs, i as u16));
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
    while (out.len() as isize) < to - from {
        out.push(Line::default()); // 读不到的画成空行
    }
    out
}

/// 一行，照 Mirror 画它的样子。
#[derive(Clone, Debug, Default)]
struct Line {
    /// 从第一格到最后一个有东西的格子，转义序列，每格用自己的样式；空格子不带样式。
    vt: String,
    /// 几列有东西：字、宽字符后半、样式或背景。
    end: usize,
    /// 软换行到下一行；spacer_head：行尾放不下的宽字符挪到了下一行；starts_wide：以宽字符开头。
    wrap: bool,
    spacer_head: bool,
    starts_wide: bool,
    prompt: Prompt,
}

impl Line {
    /// 这行怎么显示：换不换行、提示符标记、再是格子。
    fn key(&self) -> Vec<u8> {
        let mut k = Vec::with_capacity(2 + self.vt.len());
        k.push(self.wrap as u8);
        k.push(self.prompt as u8);
        k.extend_from_slice(self.vt.as_bytes());
        k
    }

    fn same_key(&self, k: &[u8]) -> bool {
        k.len() == 2 + self.vt.len()
            && k[0] == self.wrap as u8
            && k[1] == self.prompt as u8
            && &k[2..] == self.vt.as_bytes()
    }
}

/// 按 key 画的那行的提示符标记；没画的是 None。
fn key_prompt(k: &[u8]) -> Prompt {
    if k.len() < 2 {
        Prompt::None
    } else {
        Prompt::from_u8(k[1])
    }
}

/// 标客户端光标所在第一列的那行：提示符开头、接续，或清掉它原来的标记 was。
/// 返回客户端这时是不是在提示符里。
fn mark(out: &mut String, p: Prompt, was: Prompt) -> bool {
    match p {
        Prompt::Start => out.push_str(MARK_PROMPT),
        Prompt::Continuation => out.push_str(MARK_CONTINUATION),
        Prompt::None if was != Prompt::None => out.push_str(MARK_OUTPUT),
        Prompt::None => {}
    }
    p != Prompt::None
}

/// 软换行带着宽字符换到第 y 行之前，先在第一列清掉它原来的标记 was（宽字符换过去会留着标记），
/// 只管没有标记的行（别的由 row 标）。
fn premark(out: &mut String, y: usize, l: &Line, was: Prompt) {
    if l.starts_wide && !l.vt.is_empty() && l.prompt == Prompt::None && was != Prompt::None {
        let _ = write!(out, "\x1b[{}H{MARK_OUTPUT}", y + 1);
    }
}

/// 从光标所在第 0 列画一行：不软换行的整行画完、清掉后面的；软换行到 next 的画到最后一列，
/// 让客户端也换过去。画之前先清这一行，但软换行接过来的行（cont）除外：光标还停在上一行
/// 行尾等着换行，清的话清的是上一行；它没东西、或者标记要变时，先写一个空格换过来，
/// 后面的字或清除会盖掉它。was 是客户端那行原来的提示符标记。返回这行怎么画的（key）。
fn row(
    cols: u16,
    out: &mut String,
    l: &Line,
    next: Option<&Line>,
    cont: bool,
    was: Prompt,
) -> Vec<u8> {
    let cols = cols as usize;
    out.push_str("\x1b[0m");
    let mut marked = false;
    let mut after = "";
    if !cont {
        // 整行清掉，原来的换行也没了。
        out.push_str("\x1b[2K");
        marked = mark(out, l.prompt, was);
    } else if l.vt.is_empty() || (l.prompt != was && !l.starts_wide) {
        // 空格把上一行换到这一行，再退回去；这一行从第一列标。
        out.push_str(" \r");
        marked = mark(out, l.prompt, was);
    } else if l.prompt != was && l.prompt != Prompt::None {
        // 上一行给宽字符留出了最后一列，换过来的是它不是空格：标记写在字后面
        // （光标那列不收标记，premark 清过了）。
        after = mark_after(l.prompt);
        marked = true;
    }
    out.push_str(&l.vt);
    out.push_str(after);
    out.push_str("\x1b[0m");
    match next {
        Some(n) if l.wrap => pad_wrap(out, l, cols, l.spacer_head && n.starts_wide),
        _ if l.end < cols => out.push_str("\x1b[K"),
        _ => {}
    }
    if marked {
        out.push_str(MARK_END);
    }
    l.key()
}

/// 软换行的一行画完字之后，把光标带到换行的地方：最后一列之后，宽字符放不下的话（spacer）
/// 停在最后一列，让下一个宽字符在客户端也放不下、换过去。
///
/// mygo 拿空格填到那里；这里中间的空格子用 ECH 擦成空的（续行没清过，可能有旧字），
/// 光标用 CHA 挪过去，客户端的格子和这边一样是空的，复制出来的文字也一样。不是宽字符
/// 换行时，最后一列还得写一个字光标才会等着换行，写个空格。
fn pad_wrap(out: &mut String, l: &Line, cols: usize, spacer: bool) {
    let to = if spacer { cols - 1 } else { cols };
    if l.end >= to {
        return;
    }
    // 擦到最后一列之前（最后一列宽字符会盖掉，或者下面写空格）。
    let gap = cols - 1 - l.end;
    if gap > 0 {
        let _ = write!(out, "\x1b[{gap}X\x1b[{cols}G");
    }
    if !spacer {
        out.push(' ');
    }
}

/// 读渲染状态的行迭代器停着的那行，视口第 y 行。
fn read_line(t: &Terminal, rs: &mut RenderState, y: u16) -> Line {
    let mut l = Line {
        wrap: rs.row_wrapped(),
        prompt: Prompt::from_u8(rs.row_prompt()),
        ..Line::default()
    };
    let n = rs.load_raw().len();
    {
        let cells = rs.raw();
        for (x, c) in cells.iter().enumerate() {
            if c.codepoint != 0
                || c.grapheme
                || c.bg.is_some()
                || c.style != 0
                || c.width() == Width::SpacerTail
            {
                l.end = x + 1;
            }
        }
        if n > 0 {
            l.spacer_head = cells[n - 1].width() == Width::SpacerHead;
            l.starts_wide = cells[0].width() == Width::Wide;
        }
    }
    const PLAIN: &str = "\x1b[0m";
    let mut b = String::new();
    let mut sgr = PLAIN.to_string();
    let mut link = String::new();
    let mut styles: HashMap<u16, String> = HashMap::new();
    for x in 0..l.end {
        let c = rs.raw()[x];
        if matches!(c.width(), Width::SpacerTail | Width::SpacerHead) {
            continue;
        }
        let want = match c.bg {
            // 没有字、程序填了背景的格子。
            Some((palette, bg)) if c.codepoint == 0 && !c.grapheme => CellStyle {
                bg: if palette {
                    StyleColor::Palette(bg.r)
                } else {
                    StyleColor::Rgb(bg)
                },
                ..CellStyle::default()
            }
            .sgr(),
            _ if c.style != 0 => styles
                .entry(c.style)
                .or_insert_with(|| rs.cell_style(x as u16).sgr())
                .clone(),
            _ => PLAIN.to_string(),
        };
        if want != sgr {
            b.push_str(&want);
            sgr = want;
        }
        let uri = if c.hyperlink {
            t.hyperlink_at(x as u16, y).unwrap_or_default()
        } else {
            String::new()
        };
        if uri != link {
            let _ = write!(b, "\x1b]8;;{uri}\x1b\\");
            link = uri;
        }
        let start = b.len();
        if c.grapheme {
            rs.cell_graphemes(x as u16, &mut b);
        }
        if b.len() == start {
            match char::from_u32(c.codepoint).filter(|&ch| ch != '\0') {
                Some(ch) => b.push(ch),
                None => b.push(' '),
            }
        }
    }
    if !link.is_empty() {
        b.push_str("\x1b]8;;\x1b\\");
    }
    if sgr != PLAIN {
        b.push_str(PLAIN);
    }
    l.vt = b;
    l
}

/// 把开着的模式排一下，让起作用的鼠标模式排最后：客户端按设的先后选事件和格式，
/// 一个设置里最后设的那个算数，别的留着设，所以 crossterm 先 1015 再 1006（herdr 就这么设）
/// 是 SGR 不是 URxvt。off：开着的模式里没有起作用的时候，之后要重置的一个（重置一个就回到
/// 没有或 X10，别的留着）。哪些起作用，看鼠标编码器发什么。
fn mouse_last(t: &Terminal, on: Vec<u16>) -> (Vec<u16>, u16) {
    let events = on.iter().filter(|m| MOUSE_EVENTS.contains(m)).count();
    let formats = on.iter().filter(|m| MOUSE_FORMATS.contains(m)).count();
    if events == 0 {
        return (on, 0); // 没有事件，格式就是那个格式
    }
    let Ok(mut e) = MouseEncoder::new() else {
        return (on, 0);
    };
    // 10×10 像素的格子，一行 300 个：X10 的坐标到 223，再往后 UTF-8 的要两个字节。
    let mut sends = |pressed: bool, action: MouseAction, button: Option<MouseButton>, x: f32| {
        e.sync(
            t,
            &MouseSize {
                screen_width: 3000,
                screen_height: 100,
                cell_width: 10,
                cell_height: 10,
                ..MouseSize::default()
            },
            pressed,
        );
        e.encode(&MouseEvent {
            action,
            button,
            mods: Mods::NONE,
            x,
            y: 5.0,
        })
        .to_vec()
    };
    let left = Some(MouseButton::Left);
    let event = if !sends(false, MouseAction::Motion, None, 5.0).is_empty() {
        1003
    } else if !sends(true, MouseAction::Motion, left, 15.0).is_empty() {
        1002
    } else if !sends(false, MouseAction::Release, left, 5.0).is_empty() {
        1000
    } else if !sends(false, MouseAction::Press, left, 5.0).is_empty() {
        9
    } else {
        0
    };
    let on = move_last(on, event);
    if event == 0 {
        let off = on
            .iter()
            .copied()
            .find(|m| MOUSE_EVENTS.contains(m))
            .unwrap_or(0);
        return (on, off);
    }
    if formats == 0 {
        return (on, 0);
    }
    let b = sends(false, MouseAction::Press, left, 1505.0);
    let format = if b.starts_with(b"\x1b[<") {
        if contains(&b, b";1505;") {
            1016
        } else {
            1006
        }
    } else if b.starts_with(b"\x1b[M") {
        if b.len() > 6 {
            1005
        } else {
            0
        }
    } else if !b.is_empty() {
        1015
    } else {
        0
    };
    let on = move_last(on, format);
    let mut off = 0;
    if format == 0 {
        if let Some(&m) = on.iter().rev().find(|m| MOUSE_FORMATS.contains(m)) {
            off = m;
        }
    }
    (on, off)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// 模式 md 开着的话挪到最后。
fn move_last(mut on: Vec<u16>, md: u16) -> Vec<u16> {
    if md == 0 || !on.contains(&md) {
        return on;
    }
    on.retain(|&m| m != md);
    on.push(md);
    on
}

/// 设程序设过的颜色（OSC 4、10、11、12），重置别的，那些用客户端自己的默认色。
fn program_colors(t: &Terminal) -> String {
    let set = t.colors(false);
    let def = t.colors(true);
    let mut b = String::new();
    let osc = |b: &mut String, code: &str, c: crate::Rgb| {
        let _ = write!(
            b,
            "\x1b]{code};rgb:{:02x}/{:02x}/{:02x}\x1b\\",
            c.r, c.g, c.b
        );
    };
    for (code, set, def) in [
        ("10", set.foreground, def.foreground),
        ("11", set.background, def.background),
        ("12", set.cursor, def.cursor),
    ] {
        match set {
            Some(c) if def != Some(c) => osc(&mut b, code, c),
            _ => {
                let _ = write!(b, "\x1b]1{code}\x1b\\");
            }
        }
    }
    b.push_str("\x1b]104\x1b\\");
    for i in 0..256 {
        if set.palette[i] != def.palette[i] {
            osc(&mut b, &format!("4;{i}"), set.palette[i]);
        }
    }
    b
}

impl Terminal {
    /// 存盘用的历史：回滚和屏幕到光标那行（光标上面没东西时到最后一行有东西的），
    /// 转义序列，不带光标和模式。喂给一个同宽的新终端，光标停在内容下面，下一个程序从
    /// 它下面开始而不是盖上去。全屏程序的屏幕没有历史：备用屏时返回空的。
    ///
    /// 会滚动视口读回滚，读完放回最下面。
    pub fn history(&self) -> Vec<u8> {
        if self.alt_screen() {
            return Vec::new();
        }
        let (cols, rows) = self.size();
        let cols = cols as usize;
        let sb = self.scrollback_rows() as isize;
        let lines = read_lines(self, 0, sb + rows as isize);
        let y = self.cursor().1 as isize;
        // 最后一行：到光标那行。
        let mut end = (sb + y).min(lines.len() as isize - 1);
        while end >= 0 && lines[end as usize].end == 0 {
            end -= 1;
        }
        let mut out = String::new();
        let Ok(end) = usize::try_from(end) else {
            return Vec::new();
        };
        for (i, l) in lines[..=end].iter().enumerate() {
            out.push_str(&l.vt);
            out.push_str("\x1b[0m");
            if l.wrap && i < end {
                // 软换行：带到最后一列，让下一行接着换过去。
                pad_wrap(&mut out, l, cols, l.spacer_head && lines[i + 1].starts_wide);
                continue;
            }
            out.push_str("\r\n");
        }
        out.into_bytes()
    }
}
