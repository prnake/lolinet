//! 界面上的文字和几何：不碰 GPUI 的纯函数，单测覆盖。

use std::net::SocketAddr;

use lolidesk_proto::{Launcher, Quality, SessionInfo, target_size};

/// 证书指纹（hex）四个一组、八组一行，好和主机上打出来的对。
pub fn group_fingerprint(hex: &str) -> String {
    let mut out = String::with_capacity(hex.len() + hex.len() / 4);
    for (i, chunk) in hex.as_bytes().chunks(4).enumerate() {
        if i > 0 {
            out.push(if i % 8 == 0 { '\n' } else { ' ' });
        }
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
    }
    out
}

/// 主机的显示名：记住的名字，没有就是地址（默认端口省掉）。
pub fn host_title(name: &str, addr: &SocketAddr) -> String {
    if !name.is_empty() {
        return name.to_string();
    }
    let s = addr.to_string();
    match s.strip_suffix(&format!(":{}", lolidesk_proto::DEFAULT_PORT)) {
        Some(bare) => bare
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
        None => s,
    }
}

/// 往返延迟：整数毫秒。
pub fn rtt_label(ms: f64) -> String {
    format!("{:.0} ms", ms.max(0.0))
}

// ---------------------------------------------------------------- 远程桌面的画面参数

/// 分辨率选项：编码画面的长边，0 = 原生。
pub const LONG_EDGES: [u32; 7] = [0, 3840, 2560, 1920, 1600, 1280, 960];
/// 帧率选项。
pub const FRAME_RATES: [u8; 4] = [15, 30, 60, 120];

/// 显示器在长边 `edge` 时的编码尺寸（不放大，和主机算法一致）。
pub fn scaled(w: u32, h: u32, edge: u32) -> (u32, u32) {
    target_size(edge, w, h)
}

/// 「原生 3024×1964」或「1920×1246」。
pub fn resolution_label(w: u32, h: u32, edge: u32) -> String {
    let (sw, sh) = scaled(w, h, edge);
    if edge == 0 || edge >= w.max(h) {
        format!("原生 {sw}×{sh}")
    } else {
        format!("{sw}×{sh}")
    }
}

/// 这块显示器能选的分辨率：不超过原生，去掉重复的。
pub fn resolution_options(w: u32, h: u32) -> Vec<(u32, String)> {
    let long = w.max(h);
    let mut out: Vec<(u32, String)> = Vec::new();
    for edge in LONG_EDGES {
        if edge != 0 && edge >= long {
            continue;
        }
        let label = resolution_label(w, h, edge);
        if !out.iter().any(|(_, l)| *l == label) {
            out.push((edge, label));
        }
    }
    out
}

/// 帧率选项：不超过显示器刷新率（不知道刷新率时全给）。
pub fn fps_options(refresh: u16) -> Vec<u8> {
    let v: Vec<u8> = FRAME_RATES
        .into_iter()
        .filter(|f| refresh == 0 || u16::from(*f) <= refresh)
        .collect();
    if v.is_empty() {
        vec![FRAME_RATES[0]]
    } else {
        v
    }
}

/// 帧率夹到刷新率以内。
pub fn cap_fps(fps: u8, refresh: u16) -> u8 {
    let opts = fps_options(refresh);
    if opts.contains(&fps) {
        fps
    } else {
        opts.into_iter()
            .filter(|f| *f <= fps)
            .max()
            .unwrap_or(FRAME_RATES[0])
    }
}

pub const QUALITIES: [(Quality, &str, &str); 4] = [
    (Quality::Low, "省流", "码率最低，适合慢网"),
    (Quality::Medium, "标准", "日常够用"),
    (Quality::High, "清晰", "小字也清楚"),
    (Quality::Ultra, "极清", "码率最高，局域网用"),
];

pub fn quality_name(q: Quality) -> &'static str {
    QUALITIES
        .iter()
        .find(|(x, _, _)| *x == q)
        .map(|(_, n, _)| *n)
        .unwrap_or("?")
}

/// 画质在偏好文件里存成 0..3（和旧版的 app.json 兼容）。
pub fn quality_index(q: Quality) -> u8 {
    match q {
        Quality::Low => 0,
        Quality::Medium => 1,
        Quality::High => 2,
        Quality::Ultra => 3,
    }
}

pub fn quality_from_index(i: u8) -> Quality {
    match i {
        0 => Quality::Low,
        1 => Quality::Medium,
        2 => Quality::High,
        _ => Quality::Ultra,
    }
}

/// 显示器的名字：没名字的叫「显示器 N」。
pub fn display_name(name: &str, id: u32) -> String {
    if name.trim().is_empty() {
        format!("显示器 {id}")
    } else {
        name.to_string()
    }
}

/// 窗口的名字：标题，没有标题就用程序名。
pub fn window_label(w: &lolidesk_proto::WindowInfo) -> String {
    if w.title.trim().is_empty() {
        w.app.clone()
    } else {
        w.title.clone()
    }
}

/// 最多 `max` 个字，多的换成「…」（菜单、工具条上放不下长标题）。
pub fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------- 画面几何

/// 窗口坐标里的矩形（逻辑像素）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        self.w > 0.0
            && px >= self.x
            && px <= self.x + self.w
            && py >= self.y
            && py <= self.y + self.h
    }
}

/// 把 w×h 的画面等比放进 `r`，居中（和 GPUI 的 `ObjectFit::Contain` 一样，可以放大）。
pub fn fit(r: Rect, w: u32, h: u32) -> Rect {
    if w == 0 || h == 0 || r.w <= 0.0 || r.h <= 0.0 {
        return r;
    }
    let ar = w as f32 / h as f32;
    let (mut pw, mut ph) = (r.w, r.w / ar);
    if ph > r.h {
        ph = r.h;
        pw = r.h * ar;
    }
    Rect::new(r.x + (r.w - pw) / 2.0, r.y + (r.h - ph) / 2.0, pw, ph)
}

/// 指针在画面里的归一化坐标（0..1）；在画面外是 None。
pub fn normalise(pic: Rect, px: f32, py: f32) -> Option<(f32, f32)> {
    if pic.w <= 0.0 || pic.h <= 0.0 {
        return None;
    }
    let x = (px - pic.x) / pic.w;
    let y = (py - pic.y) / pic.h;
    ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some((x, y))
}

/// 和 [`normalise`] 一样，但夹在 0..1 里（松开鼠标时指针可能已经在画面外）。
pub fn normalise_clamped(pic: Rect, px: f32, py: f32) -> (f32, f32) {
    if pic.w <= 0.0 || pic.h <= 0.0 {
        return (0.0, 0.0);
    }
    (
        ((px - pic.x) / pic.w).clamp(0.0, 1.0),
        ((py - pic.y) / pic.h).clamp(0.0, 1.0),
    )
}

/// 滚动：窗口像素 → 画面高度的比例。协议和 macOS 滚轮同号（内容往下 / 往右走为正），GPUI 也没翻转。
pub fn scroll_fraction(pic: Rect, dx_px: f32, dy_px: f32) -> Option<(f32, f32)> {
    if pic.h <= 0.0 {
        return None;
    }
    Some((dx_px / pic.h, dy_px / pic.h))
}

// ---------------------------------------------------------------- 会话

/// 路径的最后一段（`~/code/lolinet` → `lolinet`）。
pub fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.starts_with('/') { "/" } else { "" };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// 主机上常见的 agent：命令 → 名字。
const KNOWN_AGENTS: [(&str, &str); 12] = [
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("opencode", "OpenCode"),
    ("cursor-agent", "Cursor Agent"),
    ("amp", "Amp"),
    ("qwen", "Qwen Code"),
    ("kimi", "Kimi CLI"),
    ("droid", "Droid"),
    ("crush", "Crush"),
    ("pi", "Pi"),
    ("herdr", "herdr"),
];

/// 会话的显示名：登录 shell 叫「终端」，命令认得出是 agent 就用它的名字，否则就是命令。
pub fn session_name(launchers: &[Launcher], command: &str) -> String {
    let command = command.trim();
    if command.is_empty() {
        return "终端".into();
    }
    if let Some(l) = launchers.iter().find(|l| l.command == command) {
        return l.name.clone();
    }
    let first = command.split_whitespace().next().unwrap_or(command);
    let first = basename(first);
    for (cmd, name) in KNOWN_AGENTS {
        if first == cmd {
            return name.to_string();
        }
    }
    command.to_string()
}

/// 会话是不是 agent（图标用星星而不是终端）。
pub fn is_agent(command: &str) -> bool {
    !command.trim().is_empty()
}

/// 会话行的第二行：标题（程序设的，比前台程序名有信息）或前台程序，加目录名。
pub fn session_subtitle(s: &SessionInfo, host_name: &str) -> String {
    let mut sub = s.title.trim().to_string();
    if sub.is_empty() || sub == host_name || sub == s.foreground {
        sub = s.foreground.clone();
    }
    let dir = basename(&s.cwd);
    if !dir.is_empty() {
        if !sub.is_empty() {
            sub.push_str(" · ");
        }
        sub.push_str(dir);
    }
    sub
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_names() {
        let mut w = lolidesk_proto::WindowInfo {
            id: 1,
            app: "Ghostty".into(),
            bundle: String::new(),
            title: "  ".into(),
            display: 1,
            width: 10,
            height: 10,
            minimized: false,
        };
        assert_eq!(window_label(&w), "Ghostty");
        w.title = "~/code".into();
        assert_eq!(window_label(&w), "~/code");
        assert_eq!(clip("远程桌面的窗口列表", 5), "远程桌面…");
        assert_eq!(clip("abc", 3), "abc");
        assert_eq!(clip("", 3), "");
    }

    #[test]
    fn fingerprint_groups() {
        let hex = "3f9a1c0be2d47a8890f1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d";
        assert_eq!(
            group_fingerprint(hex),
            "3f9a 1c0b e2d4 7a88 90f1 c2d3 e4f5 a6b7\nc8d9 e0f1 a2b3 c4d5 e6f7 0819 2a3b 4c5d"
        );
        assert_eq!(group_fingerprint("abcdef"), "abcd ef");
        assert_eq!(group_fingerprint(""), "");
    }

    #[test]
    fn titles() {
        let a: SocketAddr = "10.188.1.2:10190".parse().unwrap();
        assert_eq!(host_title("", &a), "10.188.1.2");
        assert_eq!(host_title("desktop", &a), "desktop");
        let b: SocketAddr = "10.188.1.2:9".parse().unwrap();
        assert_eq!(host_title("", &b), "10.188.1.2:9");
        let c: SocketAddr = "[::1]:10190".parse().unwrap();
        assert_eq!(host_title("", &c), "::1");
        assert_eq!(rtt_label(2.6), "3 ms");
    }

    #[test]
    fn resolution_labels() {
        assert_eq!(resolution_label(3024, 1964, 0), "原生 3024×1964");
        assert_eq!(resolution_label(3024, 1964, 1920), "1920×1246");
        assert_eq!(resolution_label(1920, 1080, 3840), "原生 1920×1080");
        let opts = resolution_options(2560, 1440);
        let labels: Vec<&str> = opts.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "原生 2560×1440",
                "1920×1080",
                "1600×900",
                "1280×720",
                "960×540"
            ]
        );
        assert_eq!(opts[1].0, 1920);
        // 竖屏：长边是高
        assert_eq!(resolution_label(1080, 1920, 1280), "720×1280");
    }

    #[test]
    fn frame_rates() {
        assert_eq!(fps_options(60), [15, 30, 60]);
        assert_eq!(fps_options(120), [15, 30, 60, 120]);
        assert_eq!(fps_options(0), [15, 30, 60, 120]);
        assert_eq!(fps_options(10), [15]);
        assert_eq!(cap_fps(120, 60), 60);
        assert_eq!(cap_fps(60, 0), 60);
        assert_eq!(cap_fps(45, 120), 30);
    }

    #[test]
    fn quality_roundtrip() {
        for (q, _, _) in QUALITIES {
            assert_eq!(quality_from_index(quality_index(q)), q);
        }
        assert_eq!(quality_name(Quality::High), "清晰");
    }

    #[test]
    fn fit_and_normalise() {
        // 16:10 的画面放进 1000×500：按高放，左右留边
        let r = fit(Rect::new(0.0, 52.0, 1000.0, 500.0), 1600, 1000);
        assert_eq!(r, Rect::new(100.0, 52.0, 800.0, 500.0));
        // 宽的画面放进高的区域：按宽放，上下留边
        let r = fit(Rect::new(10.0, 0.0, 400.0, 800.0), 1920, 1080);
        assert_eq!(r.w, 400.0);
        assert!((r.h - 225.0).abs() < 0.01);
        assert!((r.y - 287.5).abs() < 0.01);
        // 小画面会放大（和 ObjectFit::Contain 一样）
        let r = fit(Rect::new(0.0, 0.0, 200.0, 200.0), 10, 10);
        assert_eq!(r, Rect::new(0.0, 0.0, 200.0, 200.0));
        // 没有尺寸：原样
        let base = Rect::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(fit(base, 0, 10), base);

        let pic = Rect::new(100.0, 52.0, 800.0, 500.0);
        assert_eq!(normalise(pic, 100.0, 52.0), Some((0.0, 0.0)));
        assert_eq!(normalise(pic, 500.0, 302.0), Some((0.5, 0.5)));
        assert_eq!(normalise(pic, 900.0, 552.0), Some((1.0, 1.0)));
        assert_eq!(normalise(pic, 99.0, 300.0), None);
        assert_eq!(normalise(pic, 300.0, 600.0), None);
        assert_eq!(normalise_clamped(pic, 0.0, 1000.0), (0.0, 1.0));
        assert_eq!(scroll_fraction(pic, -40.0, 100.0), Some((-0.08, 0.2)));
        assert!(pic.contains(500.0, 300.0) && !pic.contains(50.0, 300.0));
    }

    #[test]
    fn sessions() {
        assert_eq!(basename("/Users/me/code/lolinet"), "lolinet");
        assert_eq!(basename("~/code/lolinet/"), "lolinet");
        assert_eq!(basename("/"), "/");
        assert_eq!(basename(""), "");
        let launchers = vec![Launcher {
            id: "codex".into(),
            name: "Codex CLI".into(),
            command: "codex".into(),
        }];
        assert_eq!(session_name(&launchers, ""), "终端");
        assert_eq!(session_name(&launchers, "codex"), "Codex CLI");
        assert_eq!(session_name(&[], "claude --resume"), "Claude Code");
        assert_eq!(session_name(&[], "/opt/bin/claude"), "Claude Code");
        assert_eq!(session_name(&[], "htop"), "htop");

        let mut s = SessionInfo {
            id: 3,
            title: "✳ 修远程桌面的快捷键".into(),
            command: "claude".into(),
            foreground: "claude".into(),
            cwd: "/Users/me/code/lolinet".into(),
            cols: 120,
            rows: 36,
            viewers: 1,
            started: 0,
            restored: false,
            exited: None,
        };
        assert_eq!(
            session_subtitle(&s, "desktop"),
            "✳ 修远程桌面的快捷键 · lolinet"
        );
        s.title = "desktop".into();
        s.foreground = "zsh".into();
        assert_eq!(session_subtitle(&s, "desktop"), "zsh · lolinet");
        s.cwd.clear();
        assert_eq!(session_subtitle(&s, "desktop"), "zsh");
        assert_eq!(display_name("", 4), "显示器 4");
    }
}
