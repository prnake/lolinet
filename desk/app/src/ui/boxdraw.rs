//! 框线和方块字符（U+2500–U+259F）自己画成矩形，不用字体的字形：
//! 字体的框线高度对不上行高，上下两行之间会断开，交叉的地方也对不齐。
//! 和 Ghostty、gpui-ghostty 一样按格子画：每格从中心往上下左右四个方向伸线，
//! 线有细、粗、双三种；圆角画弧；方块按格子的几分之几填满；明暗块是半透明的整格。

use gpui_kit::{Bounds, Hsla, PathBuilder, Pixels, Window, fill, point, px, size};

/// 一个方向上的线：0 没有、1 细、2 粗、3 双。
pub type Arms = [u8; 4];
const UP: usize = 0;
const RIGHT: usize = 1;
const DOWN: usize = 2;
const LEFT: usize = 3;

/// U+2500–U+257F 每个字符四个方向（上右下左）的线；空的是虚线、斜线（另外处理）。
const LINES: [&str; 128] = [
    // 2500
    "0101", "0202", "1010", "2020", "", "", "", "", "", "", "", "", "0110", "0210", "0120", "0220",
    // 2510
    "0011", "0012", "0021", "0022", "1100", "1200", "2100", "2200", "1001", "1002", "2001", "2002",
    "1110", "1210", "2110", "1120", // 2520
    "2120", "2210", "1220", "2220", "1011", "1012", "2011", "1021", "2021", "2012", "1022", "2022",
    "0111", "0112", "0211", "0212", // 2530
    "0121", "0122", "0221", "0222", "1101", "1102", "1201", "1202", "2101", "2102", "2201", "2202",
    "1111", "1112", "1211", "1212", // 2540
    "2111", "1121", "2121", "2112", "2211", "1122", "1221", "2212", "1222", "2122", "2221", "2222",
    "", "", "", "", // 2550
    "0303", "3030", "0310", "0130", "0330", "0013", "0031", "0033", "1300", "3100", "3300", "1003",
    "3001", "3003", "1310", "3130", // 2560
    "3330", "1013", "3031", "3033", "0313", "0131", "0333", "1303", "3101", "3303", "1313", "3131",
    "3333", "0110", "0011", "1001", // 2570
    "1100", "", "", "", "0001", "1000", "0100", "0010", "0002", "2000", "0200", "0020", "0201",
    "1020", "0102", "2010",
];

/// 四个方向的线；不是能画的框线是 None。
pub fn arms(c: char) -> Option<Arms> {
    let i = (c as u32).checked_sub(0x2500)? as usize;
    let s = LINES.get(i)?.as_bytes();
    if s.len() != 4 {
        return None;
    }
    Some([s[0] - b'0', s[1] - b'0', s[2] - b'0', s[3] - b'0'])
}

/// 圆角 ╭╮╯╰。
fn rounded(c: char) -> bool {
    matches!(c, '╭' | '╮' | '╯' | '╰')
}

/// 虚线：（横的？，粗的？，几段）。
fn dashed(c: char) -> Option<(bool, bool, u8)> {
    Some(match c {
        '┄' => (true, false, 3),
        '┅' => (true, true, 3),
        '┆' => (false, false, 3),
        '┇' => (false, true, 3),
        '┈' => (true, false, 4),
        '┉' => (true, true, 4),
        '┊' => (false, false, 4),
        '┋' => (false, true, 4),
        '╌' => (true, false, 2),
        '╍' => (true, true, 2),
        '╎' => (false, false, 2),
        '╏' => (false, true, 2),
        _ => return None,
    })
}

/// 方块：格子里填的部分（左、上、右、下，0..1）和不透明度；象限块是几块。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Block {
    Rect(f32, f32, f32, f32, f32),
    /// 象限：1 左上、2 右上、4 左下、8 右下
    Quads(u8),
}

pub fn block(c: char) -> Option<Block> {
    let e = |n: f32| n / 8.0;
    Some(match c {
        '▀' => Block::Rect(0., 0., 1., 0.5, 1.),
        '▁'..='▇' => {
            let n = (c as u32 - '▀' as u32) as f32;
            Block::Rect(0., 1. - e(n), 1., 1., 1.)
        }
        '█' => Block::Rect(0., 0., 1., 1., 1.),
        '▉'..='▏' => {
            let n = 8.0 - (c as u32 - '█' as u32) as f32;
            Block::Rect(0., 0., e(n), 1., 1.)
        }
        '▐' => Block::Rect(0.5, 0., 1., 1., 1.),
        '░' => Block::Rect(0., 0., 1., 1., 0.25),
        '▒' => Block::Rect(0., 0., 1., 1., 0.5),
        '▓' => Block::Rect(0., 0., 1., 1., 0.75),
        '▔' => Block::Rect(0., 0., 1., e(1.), 1.),
        '▕' => Block::Rect(e(7.), 0., 1., 1., 1.),
        '▖' => Block::Quads(4),
        '▗' => Block::Quads(8),
        '▘' => Block::Quads(1),
        '▙' => Block::Quads(1 | 4 | 8),
        '▚' => Block::Quads(1 | 8),
        '▛' => Block::Quads(1 | 2 | 4),
        '▜' => Block::Quads(1 | 2 | 8),
        '▝' => Block::Quads(2),
        '▞' => Block::Quads(2 | 4),
        '▟' => Block::Quads(2 | 4 | 8),
        _ => return None,
    })
}

/// 这个字符自己画（不交给字体）。
pub fn is_drawn(c: char) -> bool {
    arms(c).is_some() || dashed(c).is_some() || block(c).is_some()
}

/// 对齐到设备像素。
pub fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// 画一格的框线或方块。`cell` 是格子（已经对齐到设备像素），`light` 是细线的粗细。
pub fn paint(
    c: char,
    cell: Bounds<Pixels>,
    color: Hsla,
    light: f32,
    scale: f32,
    window: &mut Window,
) {
    let (x0, y0) = (f32::from(cell.origin.x), f32::from(cell.origin.y));
    let (x1, y1) = (
        x0 + f32::from(cell.size.width),
        y0 + f32::from(cell.size.height),
    );
    let rect = |l: f32, t: f32, r: f32, b: f32| {
        Bounds::new(
            point(px(snap(l, scale)), px(snap(t, scale))),
            size(
                px((snap(r, scale) - snap(l, scale)).max(1.0 / scale)),
                px((snap(b, scale) - snap(t, scale)).max(1.0 / scale)),
            ),
        )
    };
    if let Some(b) = block(c) {
        let (w, h) = (x1 - x0, y1 - y0);
        match b {
            Block::Rect(l, t, r, bt, a) => {
                let mut col = color;
                col.a *= a;
                window.paint_quad(fill(
                    rect(x0 + l * w, y0 + t * h, x0 + r * w, y0 + bt * h),
                    col,
                ));
            }
            Block::Quads(q) => {
                let (mx, my) = (x0 + w / 2.0, y0 + h / 2.0);
                for (bit, l, t, r, bt) in [
                    (1, x0, y0, mx, my),
                    (2, mx, y0, x1, my),
                    (4, x0, my, mx, y1),
                    (8, mx, my, x1, y1),
                ] {
                    if q & bit != 0 {
                        window.paint_quad(fill(rect(l, t, r, bt), color));
                    }
                }
            }
        }
        return;
    }
    let t = light.max(1.0 / scale);
    let cx = snap((x0 + x1) / 2.0 - t / 2.0, scale) + t / 2.0;
    let cy = snap((y0 + y1) / 2.0 - t / 2.0, scale) + t / 2.0;
    if let Some((horizontal, heavy, n)) = dashed(c) {
        let th = if heavy { t * 2.0 } else { t };
        let (len, from) = if horizontal {
            (x1 - x0, x0)
        } else {
            (y1 - y0, y0)
        };
        let seg = len / n as f32;
        for i in 0..n {
            let a = from + seg * i as f32 + seg * 0.2;
            let b = from + seg * (i + 1) as f32 - seg * 0.2;
            let r = if horizontal {
                rect(a, cy - th / 2.0, b, cy + th / 2.0)
            } else {
                rect(cx - th / 2.0, a, cx + th / 2.0, b)
            };
            window.paint_quad(fill(r, color));
        }
        return;
    }
    let Some(a) = arms(c) else { return };
    if rounded(c) {
        // 圆角：从一边的中点直线到弧，弧（二次曲线，控制点在格子中心）到另一边的中点
        let r = ((x1 - x0).min(y1 - y0) / 2.0).max(1.0);
        let (hx, vy) = (
            if a[RIGHT] > 0 { x1 } else { x0 },
            if a[DOWN] > 0 { y1 } else { y0 },
        );
        let sx = if a[RIGHT] > 0 { 1.0 } else { -1.0 };
        let sy = if a[DOWN] > 0 { 1.0 } else { -1.0 };
        let mut p = PathBuilder::stroke(px(t));
        p.move_to(point(px(hx), px(cy)));
        p.line_to(point(px(cx + sx * r), px(cy)));
        p.curve_to(point(px(cx), px(cy + sy * r)), point(px(cx), px(cy)));
        p.line_to(point(px(cx), px(vy)));
        if let Ok(path) = p.build() {
            window.paint_path(path, color);
        }
        return;
    }
    let half = |w: u8| match w {
        2 => t,
        3 => t * 1.5,
        _ => t / 2.0,
    };
    // 双线的两条线离中心多远（两条线中间空一条细线宽）
    let g = t;
    // 单线、粗线伸过中心多少：垂直方向上最宽的那条线的一半，交叉处才不缺角
    let ext_h = half(a[UP].max(a[DOWN]));
    let ext_v = half(a[LEFT].max(a[RIGHT]));
    // 双线在中心附近停在哪：照着垂直方向有没有线，留出交叉的口子
    let off = |w: u8| if w == 3 { g } else { t / 2.0 };
    let mut quad =
        |l: f32, tp: f32, r: f32, b: f32| window.paint_quad(fill(rect(l, tp, r, b), color));
    let has = |d: usize| a[d] > 0;
    // 往右：一条（或双线的两条）从中心到右边
    if a[RIGHT] > 0 {
        if a[RIGHT] == 3 {
            let up_start = if has(UP) {
                cx + off(a[UP])
            } else if has(LEFT) {
                cx
            } else {
                cx - off(a[DOWN])
            };
            let down_start = if has(DOWN) {
                cx + off(a[DOWN])
            } else if has(LEFT) {
                cx
            } else {
                cx - off(a[UP])
            };
            quad(up_start - t / 2.0, cy - g - t / 2.0, x1, cy - g + t / 2.0);
            quad(down_start - t / 2.0, cy + g - t / 2.0, x1, cy + g + t / 2.0);
        } else {
            let h = half(a[RIGHT]);
            quad(cx - ext_h, cy - h, x1, cy + h);
        }
    }
    if a[LEFT] > 0 {
        if a[LEFT] == 3 {
            let up_end = if has(UP) {
                cx - off(a[UP])
            } else if has(RIGHT) {
                cx
            } else {
                cx + off(a[DOWN])
            };
            let down_end = if has(DOWN) {
                cx - off(a[DOWN])
            } else if has(RIGHT) {
                cx
            } else {
                cx + off(a[UP])
            };
            quad(x0, cy - g - t / 2.0, up_end + t / 2.0, cy - g + t / 2.0);
            quad(x0, cy + g - t / 2.0, down_end + t / 2.0, cy + g + t / 2.0);
        } else {
            let h = half(a[LEFT]);
            quad(x0, cy - h, cx + ext_h, cy + h);
        }
    }
    if a[DOWN] > 0 {
        if a[DOWN] == 3 {
            let left_start = if has(LEFT) {
                cy + off(a[LEFT])
            } else if has(UP) {
                cy
            } else {
                cy - off(a[RIGHT])
            };
            let right_start = if has(RIGHT) {
                cy + off(a[RIGHT])
            } else if has(UP) {
                cy
            } else {
                cy - off(a[LEFT])
            };
            quad(cx - g - t / 2.0, left_start - t / 2.0, cx - g + t / 2.0, y1);
            quad(
                cx + g - t / 2.0,
                right_start - t / 2.0,
                cx + g + t / 2.0,
                y1,
            );
        } else {
            let h = half(a[DOWN]);
            quad(cx - h, cy - ext_v, cx + h, y1);
        }
    }
    if a[UP] > 0 {
        if a[UP] == 3 {
            let left_end = if has(LEFT) {
                cy - off(a[LEFT])
            } else if has(DOWN) {
                cy
            } else {
                cy + off(a[RIGHT])
            };
            let right_end = if has(RIGHT) {
                cy - off(a[RIGHT])
            } else if has(DOWN) {
                cy
            } else {
                cy + off(a[LEFT])
            };
            quad(cx - g - t / 2.0, y0, cx - g + t / 2.0, left_end + t / 2.0);
            quad(cx + g - t / 2.0, y0, cx + g + t / 2.0, right_end + t / 2.0);
        } else {
            let h = half(a[UP]);
            quad(cx - h, y0, cx + h, cy + ext_v);
        }
    }
}

/// 波浪下划线：一串二次曲线。
pub fn curly(x0: f32, x1: f32, y: f32, amp: f32, thick: f32, color: Hsla, window: &mut Window) {
    let period = (amp * 4.0).max(3.0);
    let mut p = PathBuilder::stroke(px(thick));
    p.move_to(point(px(x0), px(y)));
    let mut x = x0;
    let mut up = true;
    while x < x1 {
        let nx = (x + period / 2.0).min(x1);
        let cy = if up { y - amp * 2.0 } else { y + amp * 2.0 };
        p.curve_to(point(px(nx), px(y)), point(px((x + nx) / 2.0), px(cy)));
        x = nx;
        up = !up;
    }
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table() {
        assert_eq!(arms('─'), Some([0, 1, 0, 1]));
        assert_eq!(arms('│'), Some([1, 0, 1, 0]));
        assert_eq!(arms('┌'), Some([0, 1, 1, 0]));
        assert_eq!(arms('┼'), Some([1, 1, 1, 1]));
        assert_eq!(arms('╋'), Some([2, 2, 2, 2]));
        assert_eq!(arms('┡'), Some([2, 2, 1, 0]));
        assert_eq!(arms('╔'), Some([0, 3, 3, 0]));
        assert_eq!(arms('╬'), Some([3, 3, 3, 3]));
        assert_eq!(arms('╒'), Some([0, 3, 1, 0]));
        assert_eq!(arms('╭'), Some([0, 1, 1, 0]));
        assert_eq!(arms('╯'), Some([1, 0, 0, 1]));
        assert_eq!(arms('╴'), Some([0, 0, 0, 1]));
        assert_eq!(arms('╿'), Some([2, 0, 1, 0]));
        assert_eq!(arms('╱'), None, "斜线交给字体");
        assert_eq!(arms('a'), None);
        // 每一项要么是四个数字，要么是空的（虚线、斜线）
        for (i, s) in LINES.iter().enumerate() {
            assert!(
                s.is_empty() || (s.len() == 4 && s.bytes().all(|b| (b'0'..=b'3').contains(&b))),
                "U+{:04X}",
                0x2500 + i
            );
            let c = char::from_u32(0x2500 + i as u32).unwrap();
            assert!(
                is_drawn(c) || matches!(c, '╱' | '╲' | '╳'),
                "U+{:04X} 没人画",
                0x2500 + i
            );
        }
    }

    #[test]
    fn blocks() {
        assert_eq!(block('█'), Some(Block::Rect(0., 0., 1., 1., 1.)));
        assert_eq!(block('▄'), Some(Block::Rect(0., 0.5, 1., 1., 1.)));
        assert_eq!(block('▁'), Some(Block::Rect(0., 0.875, 1., 1., 1.)));
        assert_eq!(block('▌'), Some(Block::Rect(0., 0., 0.5, 1., 1.)));
        assert_eq!(block('▏'), Some(Block::Rect(0., 0., 0.125, 1., 1.)));
        assert_eq!(block('▉'), Some(Block::Rect(0., 0., 0.875, 1., 1.)));
        assert_eq!(block('▛'), Some(Block::Quads(7)));
        assert_eq!(block('░'), Some(Block::Rect(0., 0., 1., 1., 0.25)));
        for c in '\u{2580}'..='\u{259f}' {
            assert!(block(c).is_some(), "U+{:04X}", c as u32);
        }
        assert!(!is_drawn('■'));
        assert_eq!(snap(7.8, 2.0), 8.0);
        assert_eq!(snap(7.7, 2.0), 7.5);
    }
}
