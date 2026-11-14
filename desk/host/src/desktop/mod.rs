//! 远程桌面采集 + 编码。macOS：ScreenCaptureKit → VideoToolbox 硬件 H.264。其他平台先不支持。
//!
//! 采的可以是一整块显示器，也可以是一个窗口（[`DeskTarget::Window`]，照 Vibe42）：窗口只推它自己的画面
//! （被挡住也照推），输入按窗口的框换算。推窗口时 `net.rs` 每秒看一次它（[`window_state`]）：
//! 挪了就换输入的换算，大小变了就重开采集，关了、最小化了、藏起来了就改推它所在的显示器（[`watch`]）。

use std::{
    collections::HashSet,
    sync::{atomic::AtomicBool, Arc},
};

use lolidesk_proto::{DeskTarget, VideoSettings, WindowInfo};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
mod mac_window;

#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct CaptureConfig {
    pub target: DeskTarget,
    pub video: VideoSettings,
    /// 置 true 时下一帧编成关键帧（客户端请求、发送端丢了帧都会置）。
    pub force_key: Arc<AtomicBool>,
}

/// 全局坐标（点，左上角原点，和 CGEvent 一样）里的一块：显示器的范围或者窗口的框。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Area {
    fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn contains(&self, (px, py): (f64, f64)) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    fn overlap(&self, o: &Area) -> f64 {
        let w = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let h = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        w.max(0.0) * h.max(0.0)
    }
}

/// 采集实际用上的参数（按显示器收紧过）。
#[derive(Debug, Clone, Copy)]
pub struct Actual {
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub bitrate: u32,
    /// 采的那块在哪（输入按它换算）
    pub area: Area,
    /// 来源的原生像素（推窗口时它变了要重开采集）
    pub source: (u32, u32),
    /// 来源在哪块显示器上（推窗口时：窗口没了就改推这块）
    pub display: u32,
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// 给 agent 的一张截图（JPEG）
pub struct Shot {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub native_width: u32,
    pub native_height: u32,
}

/// 推着的窗口此刻的样子
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WinState {
    pub area: Area,
    /// 原生像素
    pub px: (u32, u32),
    /// 在屏幕上（没最小化、程序没藏起来、在当前的桌面空间）
    pub on_screen: bool,
    pub display: u32,
}

#[cfg(target_os = "macos")]
pub use mac::{list_displays, screenshot, Capture};
#[cfg(target_os = "macos")]
pub use mac_window::{is_front, list_windows, raise_window, window_state};

#[cfg(not(target_os = "macos"))]
pub fn screenshot(_display: u32, _max_edge: u32, _quality: u8) -> anyhow::Result<Shot> {
    anyhow::bail!("这个平台还不支持截图")
}

#[cfg(not(target_os = "macos"))]
pub struct Capture;

#[cfg(not(target_os = "macos"))]
impl Capture {
    pub fn start(
        _cfg: CaptureConfig,
        _tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("这个平台还不支持远程桌面")
    }
    pub fn actual(&self) -> Actual {
        Actual {
            width: 0,
            height: 0,
            fps: 0,
            bitrate: 0,
            area: Area::default(),
            source: (0, 0),
            display: 0,
        }
    }
    pub fn stop(self) {}
}

#[cfg(not(target_os = "macos"))]
pub fn list_displays() -> anyhow::Result<Vec<lolidesk_proto::DisplayInfo>> {
    anyhow::bail!("这个平台还不支持远程桌面")
}

/// 别的平台还不能按窗口推（只能推整块显示器）
#[cfg(not(target_os = "macos"))]
pub fn list_windows() -> anyhow::Result<Vec<WindowInfo>> {
    anyhow::bail!("这台主机的系统还不能按窗口推，只能看整块显示器")
}

#[cfg(not(target_os = "macos"))]
pub fn window_state(_id: u32) -> Option<WinState> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn raise_window(_id: u32) -> anyhow::Result<()> {
    anyhow::bail!("这个平台还不支持远程桌面")
}

#[cfg(not(target_os = "macos"))]
pub fn is_front(_id: u32) -> bool {
    true
}

/// 主显示器（推的窗口打不开时改推它）
pub fn main_display() -> u32 {
    #[cfg(target_os = "macos")]
    return objc2_core_graphics::CGMainDisplayID();
    #[cfg(not(target_os = "macos"))]
    0
}

/// 帧率夹在 1 到显示器刷新率之间（刷新率不知道就按 120 算）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn clamp_fps(want: u8, refresh: u16) -> u8 {
    let top = if refresh == 0 { 120 } else { refresh.min(240) };
    want.clamp(1, top.min(u8::MAX as u16) as u8)
}

// ---------------------------------------------------------------- 窗口列表

/// 系统给的一个窗口（还没挑）
#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct RawWindow {
    pub id: u32,
    pub pid: i32,
    pub app: String,
    pub bundle: String,
    pub title: String,
    /// 窗口层级：普通窗口是 0（菜单栏、Dock、通知都在别的层）
    pub layer: i32,
    pub on_screen: bool,
    pub frame: Area,
}

/// 一块显示器：范围（点）和点到像素的倍数
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Screen {
    pub id: u32,
    pub bounds: Area,
    pub scale: f64,
}

/// 普通窗口层之外也会有的系统界面：不列
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SYSTEM_UI: &[&str] = &[
    "com.apple.dock",
    "com.apple.controlcenter",
    "com.apple.notificationcenterui",
    "com.apple.systemuiserver",
    "com.apple.loginwindow",
    "com.apple.Spotlight",
    "com.apple.WindowManager",
    "com.apple.screencaptureui",
    "com.apple.wallpaper.agent",
];

/// 太小的窗口（工具提示、拖拽时的影子）不列：点
const MIN_W: f64 = 64.0;
const MIN_H: f64 = 48.0;

/// 窗口在哪块显示器上：中心点所在的那块，不在任何一块上就按重叠最多的，都不沾就第一块。
pub fn screen_of<'a>(frame: &Area, screens: &'a [Screen]) -> Option<&'a Screen> {
    let c = frame.center();
    screens
        .iter()
        .find(|s| s.bounds.contains(c))
        .or_else(|| {
            screens
                .iter()
                .filter(|s| s.bounds.overlap(frame) > 0.0)
                .max_by(|a, b| a.bounds.overlap(frame).total_cmp(&b.bounds.overlap(frame)))
        })
        .or_else(|| screens.first())
}

/// 挑出能推的窗口：普通层、不是主机自己、不是系统界面、不太小，在屏幕上或者最小化了（`minimized`，
/// 辅助功能问到的；别的不在屏幕上的——藏起来的程序、别的桌面空间——推不出画面，不列）。
/// 按程序名、再按窗口编号排：列表不跟着窗口前后顺序跳。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn pick_windows(
    raw: Vec<RawWindow>,
    me: i32,
    minimized: &HashSet<u32>,
    screens: &[Screen],
) -> Vec<WindowInfo> {
    let mut out: Vec<WindowInfo> = raw
        .into_iter()
        .filter(|w| {
            w.layer == 0
                && w.pid != me
                && !SYSTEM_UI.contains(&w.bundle.as_str())
                && w.frame.w >= MIN_W
                && w.frame.h >= MIN_H
                && (w.on_screen || minimized.contains(&w.id))
        })
        .map(|w| {
            let screen = screen_of(&w.frame, screens);
            let scale = screen.map_or(1.0, |s| s.scale);
            WindowInfo {
                id: w.id,
                app: w.app.trim().to_string(),
                bundle: w.bundle,
                title: w.title.trim().to_string(),
                display: screen.map_or(0, |s| s.id),
                width: (w.frame.w * scale).round() as u32,
                height: (w.frame.h * scale).round() as u32,
                minimized: !w.on_screen,
            }
        })
        .collect();
    out.sort_by_key(|w| (w.app.to_lowercase(), w.id));
    out
}

// ---------------------------------------------------------------- 盯着推着的窗口

/// 推窗口时每秒看一次的结论
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Watch {
    /// 没变
    Keep,
    /// 挪了：输入换个算法
    Move(Area),
    /// 大小变了、稳住了：按新大小重开采集
    Restart,
    /// 关了、最小化了、程序藏起来了：改推它所在的显示器
    Lost,
}

/// 窗口大小在变：等它稳住（连着两次一样）再重开采集，拖着改大小时别每秒重开一次。
/// `pending` 是上一次看到的新大小。
pub fn watch(
    area: Area,
    source: (u32, u32),
    pending: &mut Option<(u32, u32)>,
    now: Option<WinState>,
) -> Watch {
    let Some(s) = now.filter(|s| s.on_screen) else {
        return Watch::Lost;
    };
    let resized = s.px.0.abs_diff(source.0) > 2 || s.px.1.abs_diff(source.1) > 2;
    if resized {
        if *pending == Some(s.px) {
            *pending = None;
            return Watch::Restart;
        }
        *pending = Some(s.px);
    } else {
        *pending = None;
    }
    if s.area != area {
        Watch::Move(s.area)
    } else {
        Watch::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fps_follows_refresh() {
        assert_eq!(clamp_fps(60, 0), 60);
        assert_eq!(clamp_fps(120, 60), 60);
        assert_eq!(clamp_fps(0, 60), 1);
        assert_eq!(clamp_fps(120, 120), 120);
    }

    fn area(x: f64, y: f64, w: f64, h: f64) -> Area {
        Area { x, y, w, h }
    }

    fn raw(id: u32, app: &str, title: &str, frame: Area) -> RawWindow {
        RawWindow {
            id,
            pid: 100 + id as i32,
            app: app.into(),
            bundle: format!("com.example.{app}"),
            title: title.into(),
            layer: 0,
            on_screen: true,
            frame,
        }
    }

    fn screens() -> Vec<Screen> {
        vec![
            Screen {
                id: 1,
                bounds: area(0., 0., 1512., 982.),
                scale: 2.0,
            },
            Screen {
                id: 4,
                bounds: area(1512., 0., 2560., 1440.),
                scale: 1.0,
            },
        ]
    }

    #[test]
    fn picks_ordinary_windows() {
        let me = 7;
        let mut menubar = raw(1, "Window Server", "Menubar", area(0., 0., 1512., 33.));
        menubar.layer = 24;
        let mut dock = raw(2, "Dock", "", area(0., 900., 1512., 82.));
        dock.bundle = "com.apple.dock".into();
        let tooltip = raw(3, "Safari", "", area(10., 10., 40., 20.));
        let mut mine = raw(4, "lolidesk", "", area(0., 0., 400., 300.));
        mine.pid = me;
        let mut other_space = raw(5, "Notes", "购物", area(100., 100., 600., 400.));
        other_space.on_screen = false;
        let mut minimized = raw(6, "Notes", "  待办 ", area(200., 100., 600., 400.));
        minimized.on_screen = false;
        let safari = raw(
            30,
            "Safari",
            "lolinet — GitHub",
            area(1600., 100., 1200., 800.),
        );
        let finder = raw(12, "访达", "下载", area(100., 100., 800., 500.));
        let safari2 = raw(9, "Safari", "", area(10., 10., 800., 600.));
        let got = pick_windows(
            vec![
                menubar,
                dock,
                tooltip,
                mine,
                other_space,
                minimized,
                safari,
                finder,
                safari2,
            ],
            me,
            &HashSet::from([6]),
            &screens(),
        );
        let ids: Vec<u32> = got.iter().map(|w| w.id).collect();
        // 程序名排，同一个程序按编号
        assert_eq!(ids, [6, 9, 30, 12]);
        let notes = &got[0];
        assert!(notes.minimized && notes.title == "待办");
        assert_eq!((notes.display, notes.width, notes.height), (1, 1200, 800));
        // 第二块屏是 1 倍
        let safari = &got[2];
        assert_eq!(
            (safari.display, safari.width, safari.height),
            (4, 1200, 800)
        );
        assert!(!safari.minimized);
    }

    #[test]
    fn screen_of_window() {
        let s = screens();
        // 中心在第二块上
        assert_eq!(screen_of(&area(1400., 0., 400., 300.), &s).unwrap().id, 4);
        // 中心在两块外面（屏幕下面）：按重叠
        assert_eq!(
            screen_of(&area(1500., 1300., 400., 400.), &s).unwrap().id,
            4
        );
        // 完全在外面：第一块
        assert_eq!(
            screen_of(&area(-5000., -5000., 10., 10.), &s).unwrap().id,
            1
        );
        assert!(screen_of(&area(0., 0., 1., 1.), &[]).is_none());
    }

    #[test]
    fn watching_a_window() {
        let a = area(100., 100., 800., 600.);
        let st = |area: Area, px: (u32, u32), on: bool| {
            Some(WinState {
                area,
                px,
                on_screen: on,
                display: 1,
            })
        };
        let mut pending = None;
        assert_eq!(
            watch(a, (1600, 1200), &mut pending, st(a, (1600, 1200), true)),
            Watch::Keep
        );
        // 挪了
        let b = area(150., 120., 800., 600.);
        assert_eq!(
            watch(a, (1600, 1200), &mut pending, st(b, (1600, 1200), true)),
            Watch::Move(b)
        );
        // 差一两个像素不算变大小
        assert_eq!(
            watch(a, (1600, 1200), &mut pending, st(a, (1601, 1200), true)),
            Watch::Keep
        );
        // 大小变了：先等一次，稳住了再重开
        let c = area(100., 100., 900., 600.);
        assert_eq!(
            watch(a, (1600, 1200), &mut pending, st(c, (1800, 1200), true)),
            Watch::Move(c)
        );
        assert_eq!(pending, Some((1800, 1200)));
        assert_eq!(
            watch(c, (1600, 1200), &mut pending, st(c, (1800, 1200), true)),
            Watch::Restart
        );
        assert_eq!(pending, None);
        // 还在拖：每次都不一样，不重开
        assert_eq!(
            watch(c, (1800, 1200), &mut pending, st(c, (1900, 1200), true)),
            Watch::Keep
        );
        assert_eq!(
            watch(c, (1800, 1200), &mut pending, st(c, (2000, 1200), true)),
            Watch::Keep
        );
        // 拖回原来的大小：不用重开
        assert_eq!(
            watch(c, (1800, 1200), &mut pending, st(c, (1800, 1200), true)),
            Watch::Keep
        );
        assert_eq!(pending, None);
        // 关了、最小化了
        assert_eq!(watch(a, (1600, 1200), &mut pending, None), Watch::Lost);
        assert_eq!(
            watch(a, (1600, 1200), &mut pending, st(a, (1600, 1200), false)),
            Watch::Lost
        );
    }
}
