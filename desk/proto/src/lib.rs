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
//! 终端两端都是 libghostty-vt：主机的终端是会话的唯一真相，客户端收它的 Mirror 帧（转义序列）照着画，
//! 自己按当前模式给按键编码，把字节发回来。
//! ALPN 不变：版本不一致时由 Hello 回一个看得懂的 Denied，而不是 TLS 握手失败。
//! `Hello` 和 `Denied` 的位置、字段永远不改，旧客户端连新主机（或反过来）才拿得到这句话。

use bytes::Bytes;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const VERSION: u16 = 2;
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
}

impl StreamKind {
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Control => 0,
            Self::Terminal => 1,
            Self::Desktop => 2,
        }
    }
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Self::Control,
            1 => Self::Terminal,
            2 => Self::Desktop,
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
    },
    Denied(String),
    Displays(Vec<DisplayInfo>),
    Sessions(Vec<SessionInfo>),
    Error(String),
    Launchers(Vec<Launcher>),
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
