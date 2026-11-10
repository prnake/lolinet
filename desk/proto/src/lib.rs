//! lolidesk 线协议。
//!
//! 一条 QUIC 连接。客户端开的每条双向流先发 1 字节 [`StreamKind`]，之后双方交替发
//! u32 LE 长度前缀的 postcard 消息（[`send`] / [`recv`]）。远程桌面画面走 QUIC datagram：
//! 每个 datagram 是 [`Frag`] 头 + 一段 [`VideoFrame`] 编码后的字节，客户端按 frame id 拼回来。
//!
//! 设计取舍见 ../README.md：比 Vibe42 少一层 libp2p / AP / 中继，
//! 因为设备已经在 lolinet 的 overlay 上，彼此可直达。

use bytes::Bytes;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

pub const VERSION: u16 = 1;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlResp {
    Welcome {
        host: String,
        os: String,
        version: String,
        desktop: bool,
    },
    Denied(String),
    Displays(Vec<DisplayInfo>),
    Sessions(Vec<SessionInfo>),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u32,
    pub title: String,
    pub command: String,
    pub cols: u16,
    pub rows: u16,
    pub viewers: u32,
}

// ---------------------------------------------------------------- 终端流

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TermReq {
    /// 新开一个会话（PTY + 主机侧终端模型）。
    Open {
        cols: u16,
        rows: u16,
        command: Option<String>,
        cwd: Option<String>,
    },
    /// 接上已有会话；主机先发 Snapshot 再续推输出。
    Attach {
        id: u32,
        cols: u16,
        rows: u16,
    },
    Input(Input),
    /// 主机改完 PTY 和模型后会给所有观看者发一份新 Snapshot。
    Resize {
        cols: u16,
        rows: u16,
    },
    Kill,
}

/// 客户端发的是「语义键」，由主机按它持有的终端模式（DECCKM、键盘应用模式、括号粘贴）编码成字节。
/// TUI 客户端直接透传本地终端的字节用 Raw。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Input {
    Key { key: Key, mods: Mods },
    Text(String),
    Paste(String),
    Raw(Vec<u8>),
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
    /// xterm 的修饰键参数：1 + shift(1) + alt(2) + ctrl(4)
    pub fn xterm_param(self) -> u8 {
        1 + (self.shift() as u8) + ((self.alt() as u8) << 1) + ((self.ctrl() as u8) << 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TermEvent {
    Opened {
        id: u32,
    },
    /// 整屏重建：data 是从主机模型序列化出的转义序列（光标、颜色、模式都在里面），客户端清屏后回放即可。
    Snapshot {
        cols: u16,
        rows: u16,
        data: Vec<u8>,
    },
    Output(Vec<u8>),
    Title(String),
    Bell,
    Exited(i32),
    Error(String),
}

/// 主机侧按终端模式把语义键编码成字节。
pub fn encode_key(key: Key, mods: Mods, app_cursor: bool, app_keypad: bool) -> Vec<u8> {
    let _ = app_keypad;
    let m = mods.xterm_param();
    let csi_mod = |fin: char, base: &str| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{fin}").into_bytes()
        } else {
            base.as_bytes().to_vec()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let cursor = |fin: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{fin}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{fin}").into_bytes()
        } else {
            format!("\x1b[{fin}").into_bytes()
        }
    };
    match key {
        Key::Char(c) => {
            let mut out = Vec::new();
            if mods.alt() {
                out.push(0x1b);
            }
            if mods.ctrl() {
                let u = c.to_ascii_uppercase();
                match u {
                    'A'..='Z' => out.push(u as u8 - b'@'),
                    '@' | ' ' | '2' => out.push(0),
                    '[' | '3' => out.push(0x1b),
                    '\\' | '4' => out.push(0x1c),
                    ']' | '5' => out.push(0x1d),
                    '^' | '6' => out.push(0x1e),
                    '_' | '-' | '7' => out.push(0x1f),
                    '?' | '8' => out.push(0x7f),
                    _ => out.extend(c.to_string().into_bytes()),
                }
            } else {
                out.extend(c.to_string().into_bytes());
            }
            out
        }
        Key::Enter => {
            if mods.alt() {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        Key::Tab => {
            if mods.shift() {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        Key::Backspace => {
            if mods.alt() {
                b"\x1b\x7f".to_vec()
            } else if mods.ctrl() {
                b"\x08".to_vec()
            } else {
                b"\x7f".to_vec()
            }
        }
        Key::Escape => b"\x1b".to_vec(),
        Key::Space => {
            if mods.ctrl() {
                vec![0]
            } else if mods.alt() {
                b"\x1b ".to_vec()
            } else {
                b" ".to_vec()
            }
        }
        Key::Up => cursor('A'),
        Key::Down => cursor('B'),
        Key::Right => cursor('C'),
        Key::Left => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F(n) => match n {
            1 => csi_mod('P', "\x1bOP"),
            2 => csi_mod('Q', "\x1bOQ"),
            3 => csi_mod('R', "\x1bOR"),
            4 => csi_mod('S', "\x1bOS"),
            5 => tilde(15),
            6 => tilde(17),
            7 => tilde(18),
            8 => tilde(19),
            9 => tilde(20),
            10 => tilde(21),
            11 => tilde(23),
            12 => tilde(24),
            _ => Vec::new(),
        },
    }
}

// ---------------------------------------------------------------- 远程桌面

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quality {
    /// 长边 1280，低码率
    Smooth,
    /// 长边 1920
    Balanced,
    /// 原生像素
    Sharp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeskReq {
    Start {
        display: u32,
        quality: Quality,
        max_fps: u8,
    },
    Stop,
    /// 丢帧后请求一个关键帧（主机下一帧强制 IDR）。
    Keyframe,
    Quality(Quality),
    Input(DeskInput),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeskEvent {
    /// 编码分辨率（像素）。
    Ready {
        width: u32,
        height: u32,
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
    /// dx / dy 单位是「行」
    Scroll {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    Key {
        key: Key,
        mods: Mods,
        down: bool,
    },
    Text(String),
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

    const BASE16: [[u8; 3]; 16] = [
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
    fn key_encoding() {
        let none = Mods::default();
        assert_eq!(encode_key(Key::Up, none, false, false), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, none, true, false), b"\x1bOA");
        assert_eq!(
            encode_key(Key::Up, Mods(Mods::SHIFT), true, false),
            b"\x1b[1;2A"
        );
        assert_eq!(
            encode_key(Key::Char('c'), Mods(Mods::CTRL), false, false),
            vec![3]
        );
        assert_eq!(
            encode_key(Key::Char('x'), Mods(Mods::ALT), false, false),
            b"\x1bx"
        );
        assert_eq!(
            encode_key(Key::Tab, Mods(Mods::SHIFT), false, false),
            b"\x1b[Z"
        );
        assert_eq!(encode_key(Key::F(5), none, false, false), b"\x1b[15~");
        assert_eq!(
            encode_key(Key::F(1), Mods(Mods::CTRL), false, false),
            b"\x1b[1;5P"
        );
        assert_eq!(
            encode_key(Key::Delete, Mods(Mods::CTRL | Mods::SHIFT), false, false),
            b"\x1b[3;6~"
        );
        assert_eq!(encode_key(Key::Backspace, none, false, false), b"\x7f");
    }

    #[test]
    fn message_roundtrip() {
        let msg = TermReq::Input(Input::Key {
            key: Key::Char('é'),
            mods: Mods(Mods::ALT),
        });
        let bytes = postcard::to_stdvec(&msg).unwrap();
        let back: TermReq = postcard::from_bytes(&bytes).unwrap();
        assert!(
            matches!(back, TermReq::Input(Input::Key { key: Key::Char('é'), mods }) if mods.alt())
        );
    }
}
