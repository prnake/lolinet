//! 给 GUI 画的那一层：颜色解析、宽字符、反色、光标、视口滚动、选区。

use super::{lib, lines};
use crate::{
    CursorStyle, Dirty, Point, RenderState, Rgb, Scroll, Selection, Terminal, Theme, Underline,
    Width,
};

#[test]
fn cells_and_colors() {
    if !lib() {
        return;
    }
    let mut t = Terminal::new(12, 3, Box::new(())).unwrap();
    let th = Theme::dark();
    t.set_theme(&th);
    t.write(
        "\x1b[31mA\x1b[0m世\x1b[7mB\x1b[0m\x1b[4:3;58;2;1;2;3;1mC\x1b[0me\u{301}\x1b[44m \x1b[0m"
            .as_bytes(),
    );
    t.write(b"\r\n\x1b[2 q");
    let mut rs = RenderState::new().unwrap();
    rs.update(&t).unwrap();
    assert_eq!(rs.size(), (12, 3));
    assert_eq!(rs.dirty(), Dirty::Full);
    assert_eq!(rs.colors().foreground, th.foreground);
    assert_eq!(rs.colors().background, th.background);
    let c = rs.cursor();
    assert_eq!(c.viewport, Some((0, 1)));
    assert!(c.visible && !c.blinking);
    assert_eq!(c.style, CursorStyle::Block);
    let mut rows = rs.rows();
    let mut row = rows.next_row().unwrap();
    assert_eq!(row.y(), 0);
    let cells = row.cells();
    assert_eq!(cells.len(), 12);
    let all: Vec<_> = cells.iter().collect();
    let (a, text) = all[0];
    assert_eq!((text, a.fg, a.bg), ("A", th.palette[1], None));
    let (wide, text) = all[1];
    assert_eq!((text, wide.width), ("世", Width::Wide));
    let (tail, text) = all[2];
    assert_eq!((text, tail.width), ("", Width::SpacerTail));
    let (b, text) = all[3];
    assert_eq!(text, "B");
    assert!(b.inverse);
    assert_eq!((b.fg, b.bg), (th.background, Some(th.foreground)));
    let (c, text) = all[4];
    assert_eq!(text, "C");
    assert!(c.bold);
    assert_eq!(c.underline, Underline::Curly);
    assert_eq!(c.underline_color, Some(Rgb::new(1, 2, 3)));
    let (_, text) = all[5];
    assert_eq!(text, "e\u{301}");
    let (bg, text) = all[6];
    assert_eq!((text, bg.bg), (" ", Some(th.palette[4])));
    let (empty, text) = all[7];
    assert_eq!((text, empty.bg), ("", None));
    rs.clean();
    rs.update(&t).unwrap();
    assert_eq!(rs.dirty(), Dirty::Clean);
    t.write(b"x");
    rs.update(&t).unwrap();
    assert_eq!(rs.dirty(), Dirty::Partial);
    let mut rows = rs.rows();
    let mut dirty = Vec::new();
    while let Some(row) = rows.next_row() {
        dirty.push(row.dirty());
    }
    assert_eq!(dirty, [false, true, false]);
}

#[test]
fn viewport_and_selection() {
    if !lib() {
        return;
    }
    let mut t = Terminal::new(20, 4, Box::new(())).unwrap();
    t.write(lines(0, 30, "line ").as_bytes());
    assert_eq!(t.scrollback_rows(), 27);
    let sb = t.scrollbar();
    assert_eq!((sb.total, sb.offset, sb.len), (31, 27, 4));
    t.scroll_viewport(Scroll::Top);
    assert!(!t.at_bottom());
    let mut rs = RenderState::new().unwrap();
    rs.update(&t).unwrap();
    assert_eq!(rs.scrollbar().offset, 0);
    assert_eq!(rs.cursor().viewport, None);
    let mut rows = rs.rows();
    let mut row = rows.next_row().unwrap();
    let text: String = row.cells().iter().map(|(_, s)| s).collect();
    assert_eq!(text, "line 0");
    t.scroll_viewport(Scroll::Delta(2));
    assert_eq!(t.scrollbar().offset, 2);
    t.scroll_viewport(Scroll::Row(10));
    assert_eq!(t.viewport_to_screen(0, 1), Point { x: 0, y: 11 });
    t.scroll_viewport(Scroll::Bottom);
    assert!(t.at_bottom());
    // 跨回滚选：第 1 行第 2 列到第 3 行第 5 列（屏幕坐标）。
    let sel = Selection {
        start: Point { x: 2, y: 1 },
        end: Point { x: 5, y: 3 },
        rectangle: false,
    };
    assert_eq!(
        t.selection_text(&sel).as_deref(),
        Some("ne 1\nline 2\nline 3")
    );
    let rect = Selection {
        rectangle: true,
        ..sel
    };
    assert_eq!(t.selection_text(&rect).as_deref(), Some("ne 1\nne 2\nne 3"));
    assert!(t
        .selection_text(&Selection {
            start: Point { x: 0, y: 1000 },
            end: Point { x: 0, y: 1001 },
            rectangle: false
        })
        .is_none());
    // 设到终端上，画的时候每行报选中的列。
    let on_screen = Selection {
        start: Point { x: 1, y: 28 },
        end: Point { x: 3, y: 29 },
        rectangle: false,
    };
    assert!(t.set_selection(Some(on_screen)));
    rs.update(&t).unwrap();
    let mut rows = rs.rows();
    let mut sels = Vec::new();
    while let Some(row) = rows.next_row() {
        sels.push(row.selection());
    }
    assert_eq!(sels, [None, Some((1, 19)), Some((0, 3)), None]);
    assert!(t.set_selection(None));
    // 链接。
    t.write(b"\x1b]8;;https://example.com/a\x1b\\link\x1b]8;;\x1b\\");
    let (_, y) = t.cursor();
    assert_eq!(
        t.hyperlink_at(1, y).as_deref(),
        Some("https://example.com/a")
    );
    assert_eq!(t.hyperlink_at(10, y), None);
}
