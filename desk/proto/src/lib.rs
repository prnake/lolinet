//! lolidesk 线协议。
//!
//! 一条 QUIC 连接。客户端开的每条双向流先发 1 字节 [`StreamKind`]，之后双方交替发
//! u32 LE 长度前缀的 postcard 消息（[`send`] / [`recv`]）。远程桌面画面走 QUIC datagram：
//! 每个 datagram 是 [`Frag`] 头 + 一段 [`VideoFrame`] 编码后的字节，客户端按 frame id 拼回来。
//!
//! 设计取舍见仓库根目录的 docs/lolidesk.md：没有 libp2p、中继这一层，
//! 因为设备已经在 lolinet 的 overlay 上，彼此可直达。
//!
//! 版本 2：远程桌面可选分辨率、帧率和画质，键盘按物理键位（W3C `code`）收发。
//! computer use 流（[`StreamKind::Cua`]）：给 agent 用的截图和输入，和远程桌面共用主机的屏幕录制、辅助功能授权。
//! 配对（[`ControlReq::Pair`]）：还没凭据的设备经主人同意、或者用配对码，换一条只属于它的凭据（`g1.<id>.<密钥>`），
//! 之后 Hello 就用它；主人能看到每台设备、每个配对码，单独吊销。主机自己的 token 是「主人」的钥匙（本机 app 和命令行用）。
//! 控制流上能查这两个授权、在主机上打开设置页、重启主机服务（屏幕录制要重启才生效）。
//! 终端两端都是 libghostty-vt：主机的终端是会话的唯一真相，客户端收它的 Mirror 帧（转义序列）照着画，
//! 自己按当前模式给按键编码，把字节发回来。
//! ALPN 不变：版本不一致时由 Hello 回一个看得懂的 Denied，而不是 TLS 握手失败。
//! `Hello` 和 `Denied` 的位置、字段永远不改，旧客户端连新主机（或反过来）才拿得到这句话。

use bytes::Bytes;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const VERSION: u16 = 2;
/// 这一版的版本：CI 编译时设 LOLIDESK_VERSION=`git describe --always`（和包里 BUILD-INFO、lolidesk.json 的 version 一致），
/// `--version` 打印它，lolinet upgrade 拿它和清单比。本地不设就是 Cargo.toml 的版本号
pub const BUILD: &str = match option_env!("LOLIDESK_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
pub const ALPN: &[u8] = b"lolidesk/1";
pub const DEFAULT_PORT: u16 = 10190;
/// 证书是自签的，SNI 固定；客户端靠指纹钉扎（TOFU）认主机。
pub const SERVER_NAME: &str = "lolidesk";
pub const MAX_MSG: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Control,
    Terminal,
    Desktop,
    /// computer use：agent 一问一答（[`CuaReq`] / [`CuaResp`]）
    Cua,
}

impl StreamKind {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Control => 0,
            Self::Terminal => 1,
            Self::Desktop => 2,
            Self::Cua => 3,
        }
    }
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Control,
            1 => Self::Terminal,
            2 => Self::Desktop,
            3 => Self::Cua,
            _ => return None,
        })
    }
}

// ---------------------------------------------------------------- 控制流

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlReq {
    /// 每条连接先发这个；token 不对就断开。
    Hello {
        version: u16,
        token: String,
        client: String,
    },
    ListDisplays,
    ListSessions,
    /// 主机上装了哪些 agent（claude、codex……），给「新建」菜单用。
    ListLaunchers,
    /// 远程桌面要的两个系统权限开了没有（只查，不弹授权框）→ [`ControlResp::Permissions`]
    Permissions,
    /// 在主机上打开这一项的系统设置页（第一次还会弹系统的授权框）→ `Ok` / `Error`
    OpenPermission(PermissionKind),
    /// 重启主机服务（屏幕录制要重启才生效）：先回 `Ok` 再重启；不是 LaunchAgent 跑的回 `Error`
    RestartHost,
    /// 配对，不用先 Hello。带配对码就直接换凭据；不带就请主人同意（主机上弹框，最多等 [`PAIR_WAIT_SECS`] 秒）。
    /// → [`ControlResp::Paired`] 或 `Denied`。拿到凭据后在同一条控制流上 Hello
    Pair {
        device: DeviceInfo,
        code: Option<String>,
    },
    /// 谁能连这台电脑 → [`ControlResp::Access`]：主人看全部（含等着同意的）；配对过的设备只看自己和自己发的分享链接
    Access,
    /// 发一个配对码（主人；或者配对过的设备，给分享链接用，主机 `allow share` 开着时）→ [`ControlResp::Code`]
    CreateCode {
        label: String,
        /// 多少秒后不能再用（已经用它配对的设备不受影响）；None 是不过期
        expires_in: Option<u64>,
        /// 最多几台设备用它配对；None 是不限
        max_uses: Option<u32>,
        /// 用它配对的设备能做什么（和主机的开关取交集）；None 是和主机一样
        allow: Option<Allow>,
    },
    /// 吊销一台设备（`g-…`）或一个配对码（`c-…`，用它配对的设备一起）。主人什么都能吊销；
    /// 设备能吊销自己、自己发的配对码和用它们配对的设备 → `Ok`
    Revoke(String),
    /// 主人答复一个配对请求 → `Ok`
    Decide {
        id: u64,
        allow: bool,
    },
    /// 主人限制一台设备能做什么（None 是和主机一样）→ `Ok`
    Limit {
        grant: String,
        allow: Option<Allow>,
    },
}

/// 等主人同意最多多久
pub const PAIR_WAIT_SECS: u64 = 120;

/// 请求配对的设备自报的信息（主机记下来给主人看，不拿它做判断）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// 主机名
    pub name: String,
    /// macos / linux / …
    pub os: String,
    /// app、cli、mcp
    pub kind: String,
    pub version: String,
}

impl DeviceInfo {
    /// 这台机器（kind：app、cli、mcp）
    pub fn this(kind: &str) -> Self {
        Self {
            name: hostname(),
            os: std::env::consts::OS.into(),
            kind: kind.into(),
            version: BUILD.into(),
        }
    }
}

/// 本机名（去掉 .local）
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let raw = if n == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        String::new()
    };
    let name = raw.trim_end_matches(".local").to_string();
    if name.is_empty() {
        "unknown".into()
    } else {
        name
    }
}

unsafe extern "C" {
    fn gethostname(name: *mut std::ffi::c_char, len: usize) -> i32;
}

/// 角色：主人（主机 token，本机 app、命令行）或者一台配对过的设备
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Owner,
    Device(String),
}

/// 一台配对过的设备
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantInfo {
    /// g-xxxxxxxx
    pub id: String,
    pub device: DeviceInfo,
    /// 怎么配对的：主人同意的是 None，用配对码的是那个码的 id
    pub code: Option<String>,
    /// Unix 秒
    pub created: u64,
    pub created_addr: String,
    /// 最近一次连上（Unix 秒）和从哪个地址
    pub last_seen: u64,
    pub last_addr: String,
    /// 主人给它的限制（None 是和主机一样）
    pub allow: Option<Allow>,
    /// 现在连着几条
    pub online: u32,
}

/// 一个配对码（密钥只在发的时候给一次，主机只存哈希）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeInfo {
    /// c-xxxxxxxx
    pub id: String,
    pub label: String,
    pub created: u64,
    /// 谁发的：主人是 None，设备（分享链接）是它的 g-id
    pub created_by: Option<String>,
    pub expires: Option<u64>,
    pub max_uses: Option<u32>,
    pub uses: u32,
    pub allow: Option<Allow>,
}

/// 等主人同意的配对请求
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInfo {
    pub id: u64,
    pub device: DeviceInfo,
    pub addr: String,
    pub since: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessInfo {
    /// 问的这一方是不是主人
    pub owner: bool,
    /// 问的这一方的设备 id（主人是 None）
    pub me: Option<String>,
    pub grants: Vec<GrantInfo>,
    pub codes: Vec<CodeInfo>,
    pub pending: Vec<PendingInfo>,
}

// ---------------------------------------------------------------- 分享链接

/// `lolidesk://pair?h=<主机:端口>&f=<证书指纹>&c=<配对码>&n=<主机名>`。带着指纹，导入的一方不用再核对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairLink {
    pub addr: String,
    pub fingerprint: String,
    pub code: String,
    pub name: String,
}

impl PairLink {
    pub fn to_url(&self) -> String {
        format!(
            "lolidesk://pair?h={}&f={}&c={}&n={}",
            pct(&self.addr),
            self.fingerprint,
            pct(&self.code),
            pct(&self.name)
        )
    }

    /// 认 `lolidesk://pair?…`（前后的空白、引号去掉）；指纹要是 64 位十六进制
    pub fn parse(s: &str) -> Option<Self> {
        let s = s
            .trim()
            .trim_matches(|c| c == '\'' || c == '"' || c == '<' || c == '>');
        let q = s.strip_prefix("lolidesk://pair?")?;
        let (mut addr, mut fp, mut code, mut name) = (None, None, None, String::new());
        for kv in q.split('&') {
            let (k, v) = kv.split_once('=')?;
            let v = unpct(v)?;
            match k {
                "h" => addr = Some(v),
                "f" => fp = Some(v.to_ascii_lowercase()),
                "c" => code = Some(v),
                "n" => name = v,
                _ => {}
            }
        }
        let fp = fp.filter(|f| f.len() == 64 && f.bytes().all(|b| b.is_ascii_hexdigit()))?;
        let addr = addr.filter(|a| !a.is_empty() && a.len() <= 128)?;
        let code = code.filter(|c| !c.is_empty() && c.len() <= 64)?;
        Some(Self {
            addr,
            fingerprint: fp,
            code,
            name,
        })
    }
}

fn pct(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~:[]".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unpct(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(h, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// 远程桌面（和 computer use）要的系统权限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionKind {
    /// 屏幕录制：采屏、截图
    ScreenRecording,
    /// 辅助功能：注入键鼠
    Accessibility,
}

impl PermissionKind {
    /// macOS 系统设置里这一项的页面（「隐私与安全性」下）
    pub fn settings_url(self) -> &'static str {
        match self {
            Self::ScreenRecording => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
            }
            Self::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlResp {
    Welcome {
        host: String,
        os: String,
        version: String,
        desktop: bool,
        /// 主机存会话的历史和目录：重启后会话带着历史回来（程序重新起）。
        persistent: bool,
        /// 这台主机开放了什么（`lolidesk host allow`），已经和这台设备的限制取了交集
        allow: Allow,
        /// 连上来的是主人还是哪台配对过的设备
        role: Role,
    },
    Denied(String),
    Displays(Vec<DisplayInfo>),
    Sessions(Vec<SessionInfo>),
    Error(String),
    Launchers(Vec<Launcher>),
    /// 主机这个进程现在有没有这两个权限（没有远程桌面的平台回 Error）
    Permissions {
        screen_recording: bool,
        accessibility: bool,
    },
    /// 做了（OpenPermission、RestartHost、Revoke、Decide、Limit）
    Ok,
    /// 配对成功：以后 Hello 用这个凭据
    Paired {
        credential: String,
        host: String,
        grant: String,
    },
    Access(AccessInfo),
    /// 发好的配对码：`secret` 只给这一次。`addr` 是主机在网里的地址（拼分享链接用）
    Code {
        info: CodeInfo,
        secret: String,
        addr: String,
    },
}

/// 主机开放的能力。远程桌面和 computer use 用的是同一份屏幕录制、辅助功能授权，这里只是主机愿不愿意给。
/// 存成 JSON 时少写的键按开算（`serde(default)`）；线上（postcard）每个字段都在。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Allow {
    /// 终端会话
    pub terminal: bool,
    /// 看远程桌面
    pub desktop: bool,
    /// 远程桌面里操作键鼠（关了就是只看）
    pub control: bool,
    /// agent 的 computer use（截图、键鼠）
    pub cua: bool,
    /// 没配对过的设备能不能请求配对（主机上弹同意框）；只看主机的
    pub pair: bool,
    /// 配对过的设备能不能发分享链接（给别的机器用的配对码）
    pub share: bool,
}

impl Default for Allow {
    fn default() -> Self {
        Self {
            terminal: true,
            desktop: true,
            control: true,
            cua: true,
            pair: true,
            share: true,
        }
    }
}

impl Allow {
    /// 两边都开才开
    pub fn and(self, o: Allow) -> Allow {
        Allow {
            terminal: self.terminal && o.terminal,
            desktop: self.desktop && o.desktop,
            control: self.control && o.control,
            cua: self.cua && o.cua,
            pair: self.pair && o.pair,
            share: self.share && o.share,
        }
    }

    /// 按名字开关一项（terminal、desktop、control、cua、pair、share）
    pub fn set(&mut self, what: &str, on: bool) -> Result<(), String> {
        let f = match what {
            "terminal" => &mut self.terminal,
            "desktop" => &mut self.desktop,
            "control" => &mut self.control,
            "cua" => &mut self.cua,
            "pair" => &mut self.pair,
            "share" => &mut self.share,
            _ => {
                return Err(format!(
                    "没有 {what}：terminal、desktop、control、cua、pair、share"
                ))
            }
        };
        *f = on;
        Ok(())
    }

    /// 给人看的一行：开着的几项
    pub fn summary(&self) -> String {
        let names = [
            (self.terminal, "终端"),
            (self.desktop, "看桌面"),
            (self.control, "操作桌面"),
            (self.cua, "agent"),
            (self.share, "分享"),
        ];
        let on: Vec<&str> = names.iter().filter(|(b, _)| *b).map(|(_, n)| *n).collect();
        if on.is_empty() {
            "什么都不能做".into()
        } else {
            on.join("、")
        }
    }

    /// 什么都不能做（吊销了的设备）
    pub fn none() -> Allow {
        Allow {
            terminal: false,
            desktop: false,
            control: false,
            cua: false,
            pair: false,
            share: false,
        }
    }

    /// 只看不控：能看终端和桌面，不能操作、不能给 agent 用、不能再分享
    pub fn view_only() -> Allow {
        Allow {
            terminal: true,
            desktop: true,
            control: false,
            cua: false,
            pair: true,
            share: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u32,
    /// 原生像素
    pub width: u32,
    pub height: u32,
    /// 刷新率（Hz），不知道是 0
    pub refresh: u16,
    pub primary: bool,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u32,
    pub title: String,
    /// 开会话时给的命令；空的是登录 shell
    pub command: String,
    /// 前台程序的名字（终端前台进程组的组长），不知道是空
    pub foreground: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub viewers: u32,
    /// 开始时间（Unix 秒）
    pub started: u64,
    /// 主机重启后恢复的会话：历史是上一次的，程序是新起的
    pub restored: bool,
    /// 程序已经退出（会话留着给人看输出，关掉才没）
    pub exited: Option<i32>,
}

/// 「新建」菜单里的一项：id 用来认图标，command 空的是登录 shell。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Launcher {
    pub id: String,
    pub name: String,
    pub command: String,
}

// ---------------------------------------------------------------- 终端流

/// 终端的尺寸：格子数和一格的像素（不知道是 0）。程序问像素尺寸、按像素报鼠标（模式 1016）时要用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}

impl TermSize {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cell_w: 0,
            cell_h: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClearKind {
    /// 清掉滚动历史和光标所在行以上的屏幕，光标行移到顶上（⌘K）
    Screen,
    /// 只清滚动历史
    Scrollback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TermReq {
    /// 新开一个会话（PTY + 主机上的 libghostty-vt 终端）。command 在登录 shell 里跑，退出后留在 shell。
    Open {
        size: TermSize,
        command: Option<String>,
        cwd: Option<String>,
    },
    /// 接上已有会话：主机先回 Resized（会话现在的尺寸），再发第一帧（从头画）。
    Attach {
        id: u32,
        size: TermSize,
    },
    /// 键入、粘贴、鼠标、焦点的字节：客户端按终端当前的模式编好了码。
    Input(Vec<u8>),
    /// 客户端终端放得下的尺寸。会话只有一个尺寸：听用户最后在用的那个客户端（Claim）。
    Resize(TermSize),
    /// 用户在这个客户端里了（键入、点击、切过来）：会话改用它的尺寸。
    Claim,
    Clear(ClearKind),
    /// 结束会话：挂断程序，会话从列表里去掉。
    Kill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TermEvent {
    Opened {
        id: u32,
    },
    /// 主机终端的一帧（Mirror）：把客户端终端从上一帧带到主机现在的样子的转义序列，
    /// 客户端当程序输出喂给自己的终端。每条流的第一帧从头画（含滚动历史）。
    Frame(Vec<u8>),
    /// 会话换了尺寸：客户端终端在这里换（旧尺寸的帧之后、新尺寸的帧之前）。
    Resized {
        cols: u16,
        rows: u16,
    },
    Title(String),
    Bell,
    /// 程序发的桌面通知（OSC 9 / 777）
    Notify {
        title: String,
        body: String,
    },
    /// 程序退出了；会话留着（看输出），客户端关掉或 Kill 才没
    Exited(i32),
    Error(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mods(pub u8);

impl Mods {
    pub const SHIFT: u8 = 1;
    pub const CTRL: u8 = 2;
    pub const ALT: u8 = 4;
    pub const SUPER: u8 = 8;
    pub fn shift(self) -> bool {
        self.0 & Self::SHIFT != 0
    }
    pub fn ctrl(self) -> bool {
        self.0 & Self::CTRL != 0
    }
    pub fn alt(self) -> bool {
        self.0 & Self::ALT != 0
    }
    pub fn sup(self) -> bool {
        self.0 & Self::SUPER != 0
    }
}

// ---------------------------------------------------------------- 远程桌面

/// 画质：决定码率（每像素每帧多少 bit）。分辨率和帧率另外选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    /// 省流
    Low,
    /// 标准
    Medium,
    /// 清晰
    High,
    /// 极清：适合看小字
    Ultra,
}

impl Quality {
    /// H.264 屏幕内容的每像素每帧 bit 数。
    pub fn bits_per_pixel(self) -> f64 {
        match self {
            Self::Low => 0.05,
            Self::Medium => 0.1,
            Self::High => 0.18,
            Self::Ultra => 0.3,
        }
    }
}

/// 一次远程桌面的画面参数。主机按显示器实际能给的收紧（不放大、帧率不超过刷新率）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSettings {
    /// 编码画面的长边（像素）；0 = 原生
    pub long_edge: u32,
    /// 最高帧率；画面不动时不发帧
    pub fps: u8,
    pub quality: Quality,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            long_edge: 1920,
            fps: 60,
            quality: Quality::High,
        }
    }
}

/// 按长边算编码尺寸：偶数、保持比例、不放大。
pub fn target_size(long_edge: u32, px_w: u32, px_h: u32) -> (u32, u32) {
    let long = px_w.max(px_h).max(1);
    let cap = if long_edge == 0 { long } else { long_edge };
    let scale = (cap as f64 / long as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(px_w as f64 * scale), even(px_h as f64 * scale))
}

/// 码率（bit/s）。前 30 帧按满额算，30 帧以上的部分减半（帧率高时相邻帧更像），夹在 1–150 Mbps。
pub fn bitrate(quality: Quality, w: u32, h: u32, fps: u8) -> u32 {
    let fps = fps.max(1) as f64;
    let frames = fps.min(30.0) + (fps - 30.0).max(0.0) * 0.5;
    let bps = w as f64 * h as f64 * quality.bits_per_pixel() * frames;
    bps.clamp(1.0e6, 150.0e6) as u32
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeskReq {
    Start {
        display: u32,
        video: VideoSettings,
    },
    Stop,
    /// 丢帧后请求一个关键帧（主机下一帧强制 IDR）。
    Keyframe,
    /// 换分辨率 / 帧率 / 画质：主机重开采集和编码器，再发一次 Ready。
    Configure(VideoSettings),
    Input(DeskInput),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeskEvent {
    /// 实际生效的编码参数。
    Ready {
        width: u32,
        height: u32,
        fps: u8,
        /// bit/s
        bitrate: u32,
    },
    Stopped(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// 坐标都是 0..1 的归一化值（相对编码画面），主机换算成显示器的点坐标。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeskInput {
    Move {
        x: f32,
        y: f32,
    },
    Button {
        x: f32,
        y: f32,
        button: MouseButton,
        down: bool,
    },
    /// dx / dy 是画面高度的比例（归一化），主机乘上显示器高度换成像素滚动。
    /// 正值是内容往下 / 往右走（向上滚、向左滚），和 macOS 的滚轮事件同号。
    Scroll {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    /// 物理按键：`code` 是 W3C `KeyboardEvent.code` 的名字（KeyA、ShiftLeft、Tab……），按位置不按布局。
    /// 修饰键自己也按下 / 松开；`mods` 是这一刻按着的修饰键，主机拿它校正自己记的状态。
    Key {
        code: String,
        down: bool,
        mods: Mods,
    },
    /// 已经组好的文字（输入法、粘贴），按 Unicode 注入，不依赖远端键盘布局。
    Text(String),
    /// 客户端失焦：松开所有还按着的键和鼠标键。
    ReleaseAll,
}

/// datagram 头：magic(1) frame(4) index(2) count(2) flags(1)
#[derive(Debug, Clone, Copy)]
pub struct Frag {
    pub frame: u32,
    pub index: u16,
    pub count: u16,
    pub flags: u8,
}

pub const DGRAM_MAGIC: u8 = 0xD5;
pub const FRAG_HDR: usize = 10;
pub const FRAG_KEY: u8 = 1;

impl Frag {
    pub fn write(&self, out: &mut Vec<u8>) {
        out.push(DGRAM_MAGIC);
        out.extend_from_slice(&self.frame.to_le_bytes());
        out.extend_from_slice(&self.index.to_le_bytes());
        out.extend_from_slice(&self.count.to_le_bytes());
        out.push(self.flags);
    }
    pub fn parse(b: &[u8]) -> Option<(Frag, &[u8])> {
        if b.len() < FRAG_HDR || b[0] != DGRAM_MAGIC {
            return None;
        }
        let frame = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
        let index = u16::from_le_bytes([b[5], b[6]]);
        let count = u16::from_le_bytes([b[7], b[8]]);
        let flags = b[9];
        Some((
            Frag {
                frame,
                index,
                count,
                flags,
            },
            &b[FRAG_HDR..],
        ))
    }
}

/// 一帧 H.264：关键帧带 SPS / PPS；数据是 AVCC（4 字节大端长度前缀的 NALU）。
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub key: bool,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    pub avcc: Bytes,
}

impl VideoFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.avcc.len() + 64);
        out.push(self.key as u8);
        if self.key {
            let sps = self.sps.as_deref().unwrap_or(&[]);
            let pps = self.pps.as_deref().unwrap_or(&[]);
            out.extend_from_slice(&(sps.len() as u16).to_le_bytes());
            out.extend_from_slice(sps);
            out.extend_from_slice(&(pps.len() as u16).to_le_bytes());
            out.extend_from_slice(pps);
        }
        out.extend_from_slice(&self.avcc);
        out
    }
    pub fn parse(mut b: &[u8]) -> Option<Self> {
        let key = *b.first()? != 0;
        b = &b[1..];
        let (mut sps, mut pps) = (None, None);
        if key {
            let n = u16::from_le_bytes([*b.first()?, *b.get(1)?]) as usize;
            sps = Some(b.get(2..2 + n)?.to_vec());
            b = &b[2 + n..];
            let n = u16::from_le_bytes([*b.first()?, *b.get(1)?]) as usize;
            pps = Some(b.get(2..2 + n)?.to_vec());
            b = &b[2 + n..];
        }
        Some(Self {
            key,
            sps,
            pps,
            avcc: Bytes::copy_from_slice(b),
        })
    }
}

// ---------------------------------------------------------------- computer use

/// agent 的请求，一问一答。坐标和远程桌面一样是 0..1 的归一化值（相对这块显示器）；
/// 换成截图上的像素是客户端（`lolidesk mcp`）的事，它知道 agent 看的那张图多大。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CuaReq {
    /// 截一张图：JPEG，长边不超过 `max_edge`（0 是原生像素），`quality` 1..=100
    Screenshot {
        display: u32,
        max_edge: u32,
        quality: u8,
    },
    /// 按顺序做这些输入，每两个之间停 `gap_ms`。一次点击 = Move、Button 按下、Button 抬起。
    /// 做完会松开所有还按着的键和鼠标键。
    Input {
        display: u32,
        events: Vec<DeskInput>,
        gap_ms: u16,
    },
    /// 鼠标现在在哪
    Cursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CuaResp {
    Screenshot {
        jpeg: Vec<u8>,
        /// 这张图的像素
        width: u32,
        height: u32,
        /// 显示器的原生像素
        native_width: u32,
        native_height: u32,
    },
    Done,
    /// 归一化坐标，在哪块显示器上
    Cursor {
        display: u32,
        x: f32,
        y: f32,
    },
    /// 这台电脑前的人（或者远程桌面里的人）刚动过键鼠：agent 别抢，等 `idle_ms` 够了再试
    UserActive {
        idle_ms: u64,
    },
    /// 主机不给（`lolidesk host allow cua off`）
    Denied(String),
    Error(String),
}

// ---------------------------------------------------------------- 流上的消息收发

pub async fn send<T: Serialize>(s: &mut quinn::SendStream, msg: &T) -> anyhow::Result<()> {
    let body = postcard::to_stdvec(msg)?;
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(&body);
    s.write_all(&buf).await?;
    Ok(())
}

/// 对端正常关流返回 Ok(None)。
pub async fn recv<T: DeserializeOwned>(r: &mut quinn::RecvStream) -> anyhow::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(()) => {}
        Err(quinn::ReadExactError::FinishedEarly(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let n = u32::from_le_bytes(len) as usize;
    anyhow::ensure!(n <= MAX_MSG, "消息太大：{n} 字节");
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await?;
    Ok(Some(postcard::from_bytes(&body)?))
}

// ---------------------------------------------------------------- 调色板（猫眼：黑底、浅字、绿光标）

pub mod palette {
    pub const FG: [u8; 3] = [0xf2, 0xf2, 0xf3];
    pub const BG: [u8; 3] = [0x00, 0x00, 0x00];
    pub const CURSOR: [u8; 3] = [0xce, 0xff, 0x1c];

    pub const BASE16: [[u8; 3]; 16] = [
        [0x1c, 0x1c, 0x1c],
        [0xff, 0x5c, 0x49],
        [0x2f, 0xd0, 0x7f],
        [0xff, 0xc2, 0x1a],
        [0x3a, 0xa8, 0xff],
        [0xe0, 0x7b, 0xff],
        [0x5e, 0xe6, 0xd0],
        [0xd0, 0xd0, 0xd0],
        [0x5c, 0x5c, 0x5c],
        [0xff, 0x7a, 0x6a],
        [0x6b, 0xe8, 0xa6],
        [0xff, 0xd4, 0x5c],
        [0x7a, 0xc2, 0xff],
        [0xec, 0xa3, 0xff],
        [0x8e, 0xf0, 0xe0],
        [0xf5, 0xf5, 0xf5],
    ];

    pub fn indexed(i: u8) -> [u8; 3] {
        match i {
            0..=15 => BASE16[i as usize],
            16..=231 => {
                let c = i - 16;
                let f = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
                [f(c / 36), f((c / 6) % 6), f(c % 6)]
            }
            _ => {
                let v = 8 + 10 * (i - 232);
                [v, v, v]
            }
        }
    }
}

#[cfg(test)]
mod pair_link_tests {
    use super::*;

    #[test]
    fn round_trip() {
        let l = PairLink {
            addr: "10.188.16.10:10190".into(),
            fingerprint: "ab".repeat(32),
            code: "ABCD-EFGH-JKMN-PQRS".into(),
            name: "我的 Mac & co".into(),
        };
        let url = l.to_url();
        assert!(url.starts_with("lolidesk://pair?h=10.188.16.10:10190&f=abab"));
        // 空格和名字里的 & 都转义了：只剩三个分隔参数的 &
        assert!(!url.contains(' ') && url.matches('&').count() == 3);
        assert_eq!(PairLink::parse(&url), Some(l.clone()));
        assert_eq!(PairLink::parse(&format!("  '{url}' ")), Some(l.clone()));
        // IPv6 地址、大写指纹
        let v6 = PairLink {
            addr: "[fd00::1]:10190".into(),
            ..l.clone()
        };
        assert_eq!(
            PairLink::parse(&v6.to_url().replace(&"ab".repeat(32), &"AB".repeat(32))),
            Some(v6)
        );
        for bad in [
            "https://example.com/pair?h=1&f=2&c=3",
            "lolidesk://pair?h=1.2.3.4:1&c=X",
            "lolidesk://pair?h=1.2.3.4:1&f=abc&c=X",
            "lolidesk://pair?f=ababababababababababababababababababababababababababababababababab&c=X",
            "lolidesk://pair?h=1&f=ababababababababababababababababababababababababababababababababab&c=X&n=%ZZ",
        ] {
            assert_eq!(PairLink::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn allow_helpers() {
        let mut a = Allow::default();
        a.set("cua", false).unwrap();
        assert!(!a.cua && a.terminal);
        assert!(a.set("nope", true).is_err());
        assert_eq!(Allow::default().and(Allow::view_only()), Allow::view_only());
        assert_eq!(Allow::none().summary(), "什么都不能做");
        // allow.json 里少写的键按开算
        let j: Allow = serde_json::from_str(r#"{"cua": false}"#).unwrap();
        assert!(!j.cua && j.share && j.pair);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frag_roundtrip() {
        let f = Frag {
            frame: 0xdead_beef,
            index: 7,
            count: 9,
            flags: FRAG_KEY,
        };
        let mut buf = Vec::new();
        f.write(&mut buf);
        buf.extend_from_slice(b"payload");
        let (g, body) = Frag::parse(&buf).unwrap();
        assert_eq!(
            (g.frame, g.index, g.count, g.flags),
            (f.frame, f.index, f.count, f.flags)
        );
        assert_eq!(body, b"payload");
        assert!(Frag::parse(&buf[..FRAG_HDR - 1]).is_none());
        assert!(Frag::parse(b"\x00123456789").is_none());
    }

    #[test]
    fn video_frame_roundtrip() {
        let key = VideoFrame {
            key: true,
            sps: Some(vec![0x67, 1, 2]),
            pps: Some(vec![0x68, 3]),
            avcc: Bytes::from_static(&[0, 0, 0, 1, 0x65]),
        };
        let p = VideoFrame::parse(&key.encode()).unwrap();
        assert!(p.key);
        assert_eq!(p.sps.as_deref(), Some(&[0x67u8, 1, 2][..]));
        assert_eq!(p.pps.as_deref(), Some(&[0x68u8, 3][..]));
        assert_eq!(&p.avcc[..], &[0, 0, 0, 1, 0x65]);
        let delta = VideoFrame {
            key: false,
            sps: None,
            pps: None,
            avcc: Bytes::from_static(&[9, 9]),
        };
        let p = VideoFrame::parse(&delta.encode()).unwrap();
        assert!(!p.key && p.sps.is_none() && p.avcc[..] == [9, 9]);
        assert!(VideoFrame::parse(&[1, 5, 0]).is_none());
    }

    #[test]
    fn video_sizes() {
        assert_eq!(target_size(0, 3024, 1964), (3024, 1964));
        assert_eq!(target_size(1920, 3024, 1964), (1920, 1246));
        assert_eq!(target_size(1920, 1280, 800), (1280, 800));
        assert_eq!(target_size(1280, 1080, 1920), (720, 1280));
        let b = bitrate(Quality::Medium, 1920, 1080, 30);
        assert!((6_000_000..7_000_000).contains(&b), "{b}");
        assert!(bitrate(Quality::Medium, 1920, 1080, 60) > b);
        assert!(bitrate(Quality::Ultra, 1920, 1080, 60) > bitrate(Quality::High, 1920, 1080, 60));
        assert_eq!(bitrate(Quality::Low, 64, 64, 5), 1_000_000);
        assert_eq!(bitrate(Quality::Ultra, 7680, 4320, 120), 150_000_000);
    }

    /// Hello / Denied 的编码永远不能变：旧版本的对端靠它们读出「版本不匹配」。
    #[test]
    fn hello_and_denied_are_stable() {
        let hello = ControlReq::Hello {
            version: 1,
            token: "t".into(),
            client: "c".into(),
        };
        assert_eq!(
            postcard::to_stdvec(&hello).unwrap(),
            [0, 1, 1, b't', 1, b'c']
        );
        let denied = ControlResp::Denied("x".into());
        assert_eq!(postcard::to_stdvec(&denied).unwrap(), [1, 1, b'x']);
    }

    /// 新消息加在枚举末尾：前面的编号不动
    #[test]
    fn permission_messages() {
        assert_eq!(
            postcard::to_stdvec(&ControlReq::ListLaunchers).unwrap(),
            [3]
        );
        assert_eq!(postcard::to_stdvec(&ControlReq::Permissions).unwrap(), [4]);
        assert_eq!(
            postcard::to_stdvec(&ControlReq::OpenPermission(PermissionKind::Accessibility))
                .unwrap(),
            [5, 1]
        );
        assert_eq!(postcard::to_stdvec(&ControlReq::RestartHost).unwrap(), [6]);
        assert_eq!(postcard::to_stdvec(&ControlReq::Access).unwrap(), [8]);
        let resp = ControlResp::Permissions {
            screen_recording: true,
            accessibility: false,
        };
        let bytes = postcard::to_stdvec(&resp).unwrap();
        assert_eq!(bytes, [6, 1, 0]);
        assert!(matches!(
            postcard::from_bytes::<ControlResp>(&bytes).unwrap(),
            ControlResp::Permissions {
                screen_recording: true,
                accessibility: false
            }
        ));
        assert_eq!(postcard::to_stdvec(&ControlResp::Ok).unwrap(), [7]);
    }

    #[test]
    fn message_roundtrip() {
        let msg = TermReq::Open {
            size: TermSize {
                cols: 120,
                rows: 36,
                cell_w: 9,
                cell_h: 18,
            },
            command: Some("claude".into()),
            cwd: None,
        };
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let back: TermReq = postcard::from_bytes(&bytes).unwrap();
        assert!(
            matches!(back, TermReq::Open { size, command: Some(c), cwd: None } if size.cell_h == 18 && c == "claude")
        );
        let ev = TermEvent::Frame(b"\x1b[H".to_vec());
        let back: TermEvent = postcard::from_bytes(&postcard::to_stdvec(&ev).unwrap()).unwrap();
        assert!(matches!(back, TermEvent::Frame(b) if b == b"\x1b[H"));
    }
}
