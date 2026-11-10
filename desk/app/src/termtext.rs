//! 终端格子上的纯计算：放得下几列几行、双击选词的边界、⌘ 点的网址、滚轮换成几行。
//! 一行按列给成字符：宽字符的后半格是 `'\0'`（算前一格的），空格子是 `' '`。

/// 终端四边留白（逻辑像素）。
pub const PAD_X: f32 = 14.0;
pub const PAD_Y: f32 = 10.0;

/// w×h 的区域里、去掉留白，放得下几列几行（至少 2×1）。
pub fn fit_grid(w: f32, h: f32, cell_w: f32, cell_h: f32) -> (u16, u16) {
    if cell_w <= 0.0 || cell_h <= 0.0 {
        return (2, 1);
    }
    let cols = ((w - 2.0 * PAD_X) / cell_w).floor().clamp(2.0, 1000.0) as u16;
    let rows = ((h - 2.0 * PAD_Y) / cell_h).floor().clamp(1.0, 500.0) as u16;
    (cols, rows)
}

/// 选词时算单词的字符：不是空白、不是这些标点。
fn is_word(c: char) -> bool {
    !c.is_whitespace() && !"\"'`()[]{}<>|,;│".contains(c)
}

/// 这一列往前退到宽字符的前半格。
fn base_col(chars: &[char], mut col: usize) -> usize {
    while col > 0 && chars.get(col) == Some(&'\0') {
        col -= 1;
    }
    col
}

/// 双击 `col` 选中的范围（含两头）。点在空白或标点上只选这一格。
pub fn word_range(chars: &[char], col: usize) -> (usize, usize) {
    if chars.is_empty() {
        return (0, 0);
    }
    let col = base_col(chars, col.min(chars.len() - 1));
    let at = |i: usize| match chars[i] {
        '\0' => true,
        c => is_word(c),
    };
    if !is_word(chars[col]) {
        let mut end = col;
        while end + 1 < chars.len() && chars[end + 1] == '\0' {
            end += 1;
        }
        return (col, end);
    }
    let mut start = col;
    while start > 0 && at(start - 1) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && at(end + 1) {
        end += 1;
    }
    (start, end)
}

const SCHEMES: [&str; 6] = [
    "https://", "http://", "file://", "ssh://", "ftp://", "mailto:",
];

/// `col` 上的网址（http、https、file……）；不是网址是 None。
pub fn url_at(chars: &[char], col: usize) -> Option<String> {
    if col >= chars.len() {
        return None;
    }
    let col = base_col(chars, col);
    let token = |c: char| c != '\0' && !c.is_whitespace() && !"\"'`<>|│".contains(c);
    if !token(chars[col]) {
        return None;
    }
    let mut start = col;
    while start > 0 && (token(chars[start - 1]) || chars[start - 1] == '\0') {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && (token(chars[end + 1]) || chars[end + 1] == '\0') {
        end += 1;
    }
    let text: String = chars[start..=end].iter().filter(|c| **c != '\0').collect();
    // 网址可能从 token 中间开始（比如 "(https://…)"、"见:https://…"）
    let (at, scheme) = SCHEMES
        .iter()
        .filter_map(|s| text.find(s).map(|i| (i, *s)))
        .min_by_key(|(i, _)| *i)?;
    let mut url = &text[at..];
    // 网址后面的标点不算
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
        // 没配对的右括号不算（Rust_(language) 里配对的留着）
        let unpaired = (trimmed.ends_with(')')
            && trimmed.matches('(').count() < trimmed.matches(')').count())
            || (trimmed.ends_with(']') && !trimmed.contains('['));
        let trimmed = if unpaired {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if trimmed.len() == url.len() {
            break;
        }
        url = trimmed;
    }
    (url.len() > scheme.len()).then(|| url.to_string())
}

/// 滚轮：像素累加，满一行算一步。返回这次走几步（正的是内容往下走，同 macOS 滚轮）。
pub fn wheel_steps(accum: &mut f32, delta: f32, step: f32) -> i32 {
    if step <= 0.0 {
        return 0;
    }
    *accum += delta / step;
    let n = accum.trunc();
    *accum -= n;
    n as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(s: &str) -> Vec<char> {
        // 测试里 '·' 当宽字符的后半格
        s.chars().map(|c| if c == '·' { '\0' } else { c }).collect()
    }

    #[test]
    fn grid_fits() {
        assert_eq!(fit_grid(1000.0, 600.0, 8.0, 18.0), (121, 32));
        assert_eq!(fit_grid(10.0, 10.0, 8.0, 18.0), (2, 1));
        assert_eq!(fit_grid(1000.0, 600.0, 0.0, 18.0), (2, 1));
    }

    #[test]
    fn words() {
        let line = cols("ls -la /usr/local/bin  foo(bar)");
        assert_eq!(word_range(&line, 0), (0, 1));
        assert_eq!(word_range(&line, 4), (3, 5));
        assert_eq!(word_range(&line, 10), (7, 20));
        assert_eq!(word_range(&line, 21), (21, 21), "空白只选一格");
        assert_eq!(word_range(&line, 24), (23, 25));
        assert_eq!(word_range(&line, 26), (26, 26), "括号只选一格");
        assert_eq!(word_range(&line, 28), (27, 29));
        // 宽字符：点后半格也选整个词
        let cjk = cols("中·文· ok");
        assert_eq!(word_range(&cjk, 1), (0, 3));
        assert_eq!(word_range(&cjk, 6), (5, 6));
        assert_eq!(word_range(&[], 3), (0, 0));
    }

    #[test]
    fn urls() {
        let line = cols("see https://example.com/a?b=1. and (http://x.io/p) ok");
        assert_eq!(
            url_at(&line, 10).as_deref(),
            Some("https://example.com/a?b=1")
        );
        assert_eq!(url_at(&line, 40).as_deref(), Some("http://x.io/p"));
        assert_eq!(url_at(&line, 1), None);
        assert_eq!(url_at(&line, 3), None, "空白");
        assert_eq!(url_at(&cols("https://"), 2), None, "只有协议头");
        let wiki = cols("https://en.wikipedia.org/wiki/Rust_(language)");
        assert_eq!(
            url_at(&wiki, 5).as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        assert_eq!(
            url_at(&cols("文·:https://a.b"), 5).as_deref(),
            Some("https://a.b")
        );
    }

    #[test]
    fn wheel() {
        let mut acc = 0.0;
        assert_eq!(wheel_steps(&mut acc, 10.0, 18.0), 0);
        assert_eq!(wheel_steps(&mut acc, 10.0, 18.0), 1);
        assert!((acc - 2.0 / 18.0).abs() < 1e-5);
        assert_eq!(wheel_steps(&mut acc, -40.0, 18.0), -2);
        assert_eq!(wheel_steps(&mut acc, 5.0, 0.0), 0);
    }
}
