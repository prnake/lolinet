//! computer use 的客户端：
//! - `lolidesk mcp <主机>`：stdio 的 MCP 服务（JSON-RPC，一行一条），给 Claude Code、Codex 这些 agent 用：
//!   `claude mcp add desk-studio -- lolidesk mcp 10.188.16.10`。网里任何一台上的 agent 都能用任何一台的屏幕；
//! - `lolidesk cua <主机> <工具> [参数 JSON]`：在 shell 里调一次，结果打到 stdout（截图存成文件）。
//!
//! 工具在主机上做（desk/host/src/cua.rs），走 QUIC 上的 computer use 流，和远程桌面用同一份授权。
//! 坐标用 agent 看到的那张截图的像素：这里记着每块显示器上一张图多大，换成主机要的 0..1；还没截过图就按原生像素算。
//! 工具的名字和说明是英文：这是给模型看的接口，英文各家 agent 用起来最稳；主机回的出错原因照原样（中文）转给它。

use std::{collections::HashMap, io::Write as _, sync::Arc};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use lolidesk_core::net::Client;
use lolidesk_proto::{
    self as proto, ControlReq, ControlResp, CuaReq, CuaResp, DeskInput, DisplayInfo, Mods,
    MouseButton, StreamKind,
};
use quinn::{RecvStream, SendStream};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

/// 截图默认的长边：够看清文字，又不会让 agent 的上下文太贵
const DEFAULT_EDGE: u32 = 1440;
const DEFAULT_QUALITY: u8 = 70;
/// 一格滚轮是画面高度的多少
const NOTCH: f32 = 0.04;

pub enum Out {
    Text(String),
    Image { jpeg: Vec<u8>, text: String },
}

/// 连主机的办法：MCP 里第一次调工具才连，断了下次再连
pub type Connect = Box<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Arc<Client>>> + Send>>
        + Send
        + Sync,
>;

pub struct Cua {
    connect: Connect,
    client: Option<Arc<Client>>,
    stream: Option<(SendStream, RecvStream)>,
    displays: Vec<DisplayInfo>,
    /// 每块显示器上一张截图的像素
    shot: HashMap<u32, (u32, u32)>,
}

impl Cua {
    pub fn new(connect: Connect) -> Self {
        Self {
            connect,
            client: None,
            stream: None,
            displays: Vec::new(),
            shot: HashMap::new(),
        }
    }

    async fn client(&mut self) -> Result<Arc<Client>> {
        if let Some(c) = &self.client {
            if c.conn.close_reason().is_none() {
                return Ok(c.clone());
            }
        }
        self.stream = None;
        let c = (self.connect)().await?;
        if !c.welcome.allow.cua {
            // 主人那边关了整台的，还是只没给这台设备
            match c.welcome.role {
                lolidesk_proto::Role::Owner => bail!(
                    "{} 没开放 computer use（在它上面 lolidesk host allow cua on）",
                    c.welcome.host
                ),
                lolidesk_proto::Role::Device(_) => bail!(
                    "{} 没给这台设备开放 computer use（那边的主人能改：lolidesk host limit 或 allow）",
                    c.welcome.host
                ),
            }
        }
        self.client = Some(c.clone());
        Ok(c)
    }

    async fn call(&mut self, req: CuaReq) -> Result<CuaResp> {
        for attempt in 0..2 {
            let client = self.client().await?;
            if self.stream.is_none() {
                self.stream = Some(client.open(StreamKind::Cua).await?);
            }
            let (send, recv) = self.stream.as_mut().unwrap();
            let res: Result<CuaResp> = async {
                proto::send(send, &req).await?;
                proto::recv::<CuaResp>(recv)
                    .await?
                    .context("主机关了 computer use 流（主机版本太旧？）")
            }
            .await;
            match res {
                Ok(r) => return Ok(r),
                Err(e) if attempt == 0 => {
                    tracing::warn!("computer use 流断了，重连：{e:#}");
                    self.stream = None;
                    self.client = None;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    async fn displays(&mut self) -> Result<&[DisplayInfo]> {
        if self.displays.is_empty() {
            let client = self.client().await?;
            match client.control(ControlReq::ListDisplays).await? {
                ControlResp::Displays(d) => self.displays = d,
                ControlResp::Error(e) => bail!("{e}"),
                other => bail!("列显示器：{other:?}"),
            }
        }
        Ok(&self.displays)
    }

    /// 参数里的 display，不给就是主显示器
    async fn display(&mut self, args: &Value) -> Result<u32> {
        if let Some(d) = args.get("display").and_then(Value::as_u64) {
            return Ok(d as u32);
        }
        let list = self.displays().await?;
        list.iter()
            .find(|d| d.primary)
            .or(list.first())
            .map(|d| d.id)
            .context("主机上没有显示器")
    }

    /// agent 给的截图像素 -> 0..1
    async fn norm(&mut self, display: u32, x: f64, y: f64) -> Result<(f32, f32)> {
        let (w, h) = match self.shot.get(&display) {
            Some(s) => *s,
            None => {
                let list = self.displays().await?;
                let d = list
                    .iter()
                    .find(|d| d.id == display)
                    .context("没有这块显示器")?;
                (d.width, d.height)
            }
        };
        Ok(((x / w.max(1) as f64) as f32, (y / h.max(1) as f64) as f32))
    }

    async fn input(&mut self, display: u32, events: Vec<DeskInput>, gap_ms: u16) -> Result<Out> {
        match self.call(CuaReq::Input { display, events, gap_ms }).await? {
            CuaResp::Done => Ok(Out::Text("ok".into())),
            CuaResp::UserActive { idle_ms } => bail!(
                "USER_ACTIVE: someone used this computer {:.1}s ago (at the machine or in a remote desktop). \
                 Input was not delivered. Wait a few seconds and try again, or ask the person.",
                idle_ms as f64 / 1000.0
            ),
            other => Err(resp_err(other)),
        }
    }

    pub async fn run(&mut self, name: &str, args: &Value) -> Result<Out> {
        let num = |k: &str| {
            args.get(k)
                .and_then(Value::as_f64)
                .with_context(|| format!("missing number `{k}`"))
        };
        match name {
            "displays" => {
                let list = self.displays().await?.to_vec();
                let lines: Vec<String> = list
                    .iter()
                    .map(|d| {
                        format!(
                            "display {}: {}x{} px, {} Hz{}{}",
                            d.id,
                            d.width,
                            d.height,
                            d.refresh,
                            if d.primary { ", primary" } else { "" },
                            if d.name.is_empty() {
                                String::new()
                            } else {
                                format!(" ({})", d.name)
                            }
                        )
                    })
                    .collect();
                Ok(Out::Text(lines.join("\n")))
            }
            "screenshot" => {
                let display = self.display(args).await?;
                let max_edge = args
                    .get("max_edge")
                    .and_then(Value::as_u64)
                    .map(|v| v as u32)
                    .unwrap_or(DEFAULT_EDGE);
                let quality = args
                    .get("quality")
                    .and_then(Value::as_u64)
                    .map(|v| v.clamp(1, 100) as u8)
                    .unwrap_or(DEFAULT_QUALITY);
                match self
                    .call(CuaReq::Screenshot {
                        display,
                        max_edge,
                        quality,
                    })
                    .await?
                {
                    CuaResp::Screenshot {
                        jpeg,
                        width,
                        height,
                        native_width,
                        native_height,
                    } => {
                        self.shot.insert(display, (width, height));
                        Ok(Out::Image {
                            jpeg,
                            text: format!(
                                "Screenshot of display {display}: {width}x{height} px (native {native_width}x{native_height}). \
                                 Use this image's pixel coordinates in click, move, drag and scroll."
                            ),
                        })
                    }
                    other => Err(resp_err(other)),
                }
            }
            "click" => {
                let display = self.display(args).await?;
                let (x, y) = self.norm(display, num("x")?, num("y")?).await?;
                let button = match args.get("button").and_then(Value::as_str).unwrap_or("left") {
                    "left" => MouseButton::Left,
                    "right" => MouseButton::Right,
                    "middle" => MouseButton::Middle,
                    b => bail!("unknown button `{b}`: left, right or middle"),
                };
                let count = args
                    .get("count")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .clamp(1, 3);
                let (down, up, _) = modifiers(args.get("modifiers"))?;
                let mut ev = down;
                ev.push(DeskInput::Move { x, y });
                for _ in 0..count {
                    ev.push(DeskInput::Button {
                        x,
                        y,
                        button,
                        down: true,
                    });
                    ev.push(DeskInput::Button {
                        x,
                        y,
                        button,
                        down: false,
                    });
                }
                ev.extend(up);
                self.input(display, ev, 20).await
            }
            "move" => {
                let display = self.display(args).await?;
                let (x, y) = self.norm(display, num("x")?, num("y")?).await?;
                self.input(display, vec![DeskInput::Move { x, y }], 0).await
            }
            "drag" => {
                let display = self.display(args).await?;
                let (x0, y0) = self.norm(display, num("from_x")?, num("from_y")?).await?;
                let (x1, y1) = self.norm(display, num("to_x")?, num("to_y")?).await?;
                let mut ev = vec![
                    DeskInput::Move { x: x0, y: y0 },
                    DeskInput::Button {
                        x: x0,
                        y: y0,
                        button: MouseButton::Left,
                        down: true,
                    },
                ];
                // 分几步走过去：有的程序要看到中间的移动才认拖动
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    ev.push(DeskInput::Move {
                        x: x0 + (x1 - x0) * t,
                        y: y0 + (y1 - y0) * t,
                    });
                }
                ev.push(DeskInput::Button {
                    x: x1,
                    y: y1,
                    button: MouseButton::Left,
                    down: false,
                });
                self.input(display, ev, 16).await
            }
            "scroll" => {
                let display = self.display(args).await?;
                let (x, y) = self.norm(display, num("x")?, num("y")?).await?;
                let n = args
                    .get("amount")
                    .and_then(Value::as_f64)
                    .unwrap_or(3.0)
                    .clamp(0.0, 50.0) as f32;
                // 主机那边正值是内容往下走（往上滚）
                let (dx, dy) = match args
                    .get("direction")
                    .and_then(Value::as_str)
                    .unwrap_or("down")
                {
                    "down" => (0.0, -n * NOTCH),
                    "up" => (0.0, n * NOTCH),
                    "right" => (-n * NOTCH, 0.0),
                    "left" => (n * NOTCH, 0.0),
                    d => bail!("unknown direction `{d}`: up, down, left or right"),
                };
                self.input(
                    display,
                    vec![DeskInput::Move { x, y }, DeskInput::Scroll { x, y, dx, dy }],
                    10,
                )
                .await
            }
            "type_text" => {
                let display = self.display(args).await?;
                let text = args
                    .get("text")
                    .and_then(Value::as_str)
                    .context("missing string `text`")?;
                if text.is_empty() {
                    return Ok(Out::Text("ok".into()));
                }
                self.input(display, vec![DeskInput::Text(text.to_owned())], 0)
                    .await
            }
            "key" => {
                let display = self.display(args).await?;
                let keys = args
                    .get("keys")
                    .and_then(Value::as_str)
                    .context("missing string `keys`")?;
                self.input(display, key_events(keys)?, 0).await
            }
            "cursor" => match self.call(CuaReq::Cursor).await? {
                // 没截过图就只给比例：列显示器要屏幕录制权限，读鼠标位置不要，别为它去申请
                CuaResp::Cursor { display, x, y } => Ok(Out::Text(match self.shot.get(&display) {
                    Some(&(w, h)) => format!(
                        "display {display}: x={:.0}, y={:.0} (in the last screenshot's pixels, {w}x{h})",
                        x * w as f32,
                        y * h as f32
                    ),
                    None => format!(
                        "display {display}: {:.1}% from the left, {:.1}% from the top (take a screenshot to get pixel coordinates)",
                        x * 100.0,
                        y * 100.0
                    ),
                })),
                other => Err(resp_err(other)),
            },
            _ => bail!("unknown tool `{name}`"),
        }
    }
}

fn resp_err(r: CuaResp) -> anyhow::Error {
    match r {
        CuaResp::Denied(m) => anyhow::anyhow!("DENIED: {m}"),
        CuaResp::Error(m) => anyhow::anyhow!("{m}"),
        other => anyhow::anyhow!("unexpected reply: {other:?}"),
    }
}

/// 修饰键名 -> (W3C code, Mods 位)
fn modifier(name: &str) -> Option<(&'static str, u8)> {
    Some(match name.to_ascii_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" | "win" | "windows" => ("MetaLeft", Mods::SUPER),
        "ctrl" | "control" => ("ControlLeft", Mods::CTRL),
        "alt" | "option" | "opt" => ("AltLeft", Mods::ALT),
        "shift" => ("ShiftLeft", Mods::SHIFT),
        _ => return None,
    })
}

/// 按住修饰键的按下 / 松开事件，和按完时的 Mods
fn modifiers(v: Option<&Value>) -> Result<(Vec<DeskInput>, Vec<DeskInput>, Mods)> {
    let names: Vec<&str> = match v {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(s)) => s.split('+').collect(),
        Some(_) => bail!("`modifiers` is a list like [\"cmd\", \"shift\"]"),
    };
    let mut held = 0u8;
    let (mut down, mut up) = (Vec::new(), Vec::new());
    for n in names {
        let (code, bit) = modifier(n.trim())
            .with_context(|| format!("unknown modifier `{n}`: cmd, ctrl, alt, shift"))?;
        held |= bit;
        down.push(DeskInput::Key {
            code: code.into(),
            down: true,
            mods: Mods(held),
        });
    }
    let mut left = held;
    for d in down.iter().rev() {
        if let DeskInput::Key { code, .. } = d {
            left &= !modifier_bit(code);
            up.push(DeskInput::Key {
                code: code.clone(),
                down: false,
                mods: Mods(left),
            });
        }
    }
    Ok((down, up, Mods(held)))
}

fn modifier_bit(code: &str) -> u8 {
    match code {
        "MetaLeft" => Mods::SUPER,
        "ControlLeft" => Mods::CTRL,
        "AltLeft" => Mods::ALT,
        "ShiftLeft" => Mods::SHIFT,
        _ => 0,
    }
}

/// 一个键名 -> W3C code：enter、tab、a、1、f5、pageup……；已经是 code 的（KeyA、ArrowUp）原样
pub fn key_code(name: &str) -> Option<String> {
    let l = name.to_ascii_lowercase();
    let named = match l.as_str() {
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "esc" | "escape" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" | "forwarddelete" => "Delete",
        "space" | " " => "Space",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "-" | "minus" => "Minus",
        "=" | "equal" => "Equal",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "\\" => "Backslash",
        ";" => "Semicolon",
        "'" => "Quote",
        "," | "comma" => "Comma",
        "." | "period" => "Period",
        "/" | "slash" => "Slash",
        "`" => "Backquote",
        _ => "",
    };
    if !named.is_empty() {
        return Some(named.into());
    }
    let mut chars = l.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_lowercase() {
            return Some(format!("Key{}", c.to_ascii_uppercase()));
        }
        if c.is_ascii_digit() {
            return Some(format!("Digit{c}"));
        }
    }
    if let Some(n) = l
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .filter(|n| (1..=20).contains(n))
    {
        return Some(format!("F{n}"));
    }
    // 看着像 W3C code 的原样给主机（它有完整的表）
    (name.len() > 1
        && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric()))
    .then(|| name.to_owned())
}

/// "cmd+shift+t"、"enter" -> 按下修饰键、按下松开那个键、松开修饰键
pub fn key_events(keys: &str) -> Result<Vec<DeskInput>> {
    let parts: Vec<&str> = keys
        .split('+')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let (last, mods) = parts.split_last().context("`keys` is empty")?;
    let (mut ev, up, held) =
        modifiers(Some(&Value::Array(mods.iter().map(|m| json!(m)).collect())))?;
    let code = match modifier(last) {
        // 只按修饰键（比如单按 shift）
        Some((code, _)) => code.to_owned(),
        None => key_code(last).with_context(|| format!("unknown key `{last}`"))?,
    };
    ev.push(DeskInput::Key {
        code: code.clone(),
        down: true,
        mods: held,
    });
    ev.push(DeskInput::Key {
        code,
        down: false,
        mods: held,
    });
    ev.extend(up);
    Ok(ev)
}

pub fn tools() -> Value {
    let display = json!({"type": "integer", "description": "Display id from `displays`. Default: the primary display."});
    let xy = |what: &str| json!({"type": "number", "description": format!("{what}, in pixels of the last screenshot of this display")});
    json!([
        {
            "name": "screenshot",
            "description": "Take a screenshot of a display of the remote computer (JPEG). Coordinates for the other tools are in this image's pixels, so take a fresh one after anything changes.",
            "inputSchema": {"type": "object", "properties": {
                "display": display,
                "max_edge": {"type": "integer", "description": "Longest side in pixels (default 1440; 0 = native resolution)"},
                "quality": {"type": "integer", "description": "JPEG quality 1-100 (default 70)"}
            }}
        },
        {
            "name": "click",
            "description": "Click at a point. Double-click with count=2. Hold modifiers with e.g. [\"cmd\"].",
            "inputSchema": {"type": "object", "required": ["x", "y"], "properties": {
                "x": xy("X"), "y": xy("Y"), "display": display,
                "button": {"type": "string", "enum": ["left", "right", "middle"]},
                "count": {"type": "integer", "minimum": 1, "maximum": 3},
                "modifiers": {"type": "array", "items": {"type": "string", "enum": ["cmd", "ctrl", "alt", "shift"]}}
            }}
        },
        {
            "name": "move",
            "description": "Move the mouse pointer (hover) without clicking.",
            "inputSchema": {"type": "object", "required": ["x", "y"], "properties": {"x": xy("X"), "y": xy("Y"), "display": display}}
        },
        {
            "name": "drag",
            "description": "Press the left button at one point, move to another and release.",
            "inputSchema": {"type": "object", "required": ["from_x", "from_y", "to_x", "to_y"], "properties": {
                "from_x": xy("Start X"), "from_y": xy("Start Y"), "to_x": xy("End X"), "to_y": xy("End Y"), "display": display
            }}
        },
        {
            "name": "scroll",
            "description": "Scroll at a point. `down` reveals content further down.",
            "inputSchema": {"type": "object", "required": ["x", "y"], "properties": {
                "x": xy("X"), "y": xy("Y"), "display": display,
                "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                "amount": {"type": "number", "description": "Wheel notches (default 3)"}
            }}
        },
        {
            "name": "type_text",
            "description": "Type text into the focused field, as Unicode (independent of the keyboard layout and input method). Use `key` for Enter, Tab and shortcuts.",
            "inputSchema": {"type": "object", "required": ["text"], "properties": {"text": {"type": "string"}}}
        },
        {
            "name": "key",
            "description": "Press a key or a shortcut: \"enter\", \"tab\", \"escape\", \"cmd+c\", \"cmd+shift+t\", \"ctrl+a\", \"pagedown\", \"f5\", \"up\".",
            "inputSchema": {"type": "object", "required": ["keys"], "properties": {"keys": {"type": "string"}, "display": display}}
        },
        {
            "name": "cursor",
            "description": "Where the mouse pointer is, in the last screenshot's pixels.",
            "inputSchema": {"type": "object", "properties": {}}
        },
        {
            "name": "displays",
            "description": "List the remote computer's displays (id, native size, which is primary).",
            "inputSchema": {"type": "object", "properties": {}}
        }
    ])
}

/// `lolidesk mcp`：stdin 一行一条 JSON-RPC，stdout 回。日志只进 stderr
pub async fn mcp(mut cua: Cua, host: String) -> Result<()> {
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                write_msg(&mut out, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})).await?;
                continue;
            }
        };
        let Some(id) = msg.get("id").cloned() else {
            continue; // 通知（notifications/initialized 之类），不回
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "lolidesk", "version": proto::BUILD},
                "instructions": format!(
                    "These tools see and control the screen of the computer at {host} on the lolinet network. \
                     Start with `screenshot`, then act in its pixel coordinates, and take a new screenshot to check the result. \
                     If a tool reports USER_ACTIVE, a person is using that computer: wait or ask, don't retry in a loop."
                ),
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(match cua.run(name, &args).await {
                    Ok(Out::Text(t)) => {
                        json!({"content": [{"type": "text", "text": t}], "isError": false})
                    }
                    Ok(Out::Image { jpeg, text }) => json!({"content": [
                        {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&jpeg), "mimeType": "image/jpeg"},
                        {"type": "text", "text": text}
                    ], "isError": false}),
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true})
                    }
                })
            }
            m => Err(json!({"code": -32601, "message": format!("method not found: {m}")})),
        };
        let reply = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        write_msg(&mut out, &reply).await?;
    }
    Ok(())
}

async fn write_msg(out: &mut tokio::io::Stdout, v: &Value) -> Result<()> {
    let mut s = serde_json::to_vec(v)?;
    s.push(b'\n');
    out.write_all(&s).await?;
    out.flush().await?;
    Ok(())
}

/// `lolidesk cua <主机> <工具> [参数]`：文字打到 stdout；截图写到 --out（默认 ./screenshot-<显示器>.jpg）
pub async fn once(
    mut cua: Cua,
    tool: &str,
    args: &str,
    out: Option<std::path::PathBuf>,
) -> Result<()> {
    let v: Value = if args.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(args).context("参数要是 JSON 对象")?
    };
    match cua.run(tool, &v).await? {
        Out::Text(t) => println!("{t}"),
        Out::Image { jpeg, text } => {
            let path = out.unwrap_or_else(|| "screenshot.jpg".into());
            std::fs::File::create(&path)?.write_all(&jpeg)?;
            println!(
                "{text}\n已存到 {}（{} KB）",
                path.display(),
                jpeg.len() / 1024
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(ev: &[DeskInput]) -> Vec<String> {
        ev.iter()
            .map(|e| match e {
                DeskInput::Key { code, down, mods } => {
                    format!("{}{code}/{}", if *down { "+" } else { "-" }, mods.0)
                }
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn shortcuts() {
        assert_eq!(
            codes(&key_events("enter").unwrap()),
            ["+Enter/0", "-Enter/0"]
        );
        assert_eq!(
            codes(&key_events("cmd+shift+t").unwrap()),
            [
                "+MetaLeft/8",
                "+ShiftLeft/9",
                "+KeyT/9",
                "-KeyT/9",
                "-ShiftLeft/8",
                "-MetaLeft/0"
            ]
        );
        assert_eq!(
            codes(&key_events("Ctrl+A").unwrap()),
            ["+ControlLeft/2", "+KeyA/2", "-KeyA/2", "-ControlLeft/0"]
        );
        assert_eq!(codes(&key_events("f5").unwrap()), ["+F5/0", "-F5/0"]);
        assert_eq!(
            codes(&key_events("shift").unwrap()),
            ["+ShiftLeft/0", "-ShiftLeft/0"]
        );
        assert_eq!(key_code("ArrowUp").as_deref(), Some("ArrowUp"));
        assert_eq!(key_code("7").as_deref(), Some("Digit7"));
        assert!(key_events("cmd+nosuchkey").is_err());
        assert!(key_events("").is_err());
    }

    #[test]
    fn tool_schemas() {
        let t = tools();
        let names: Vec<&str> = t
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "screenshot",
                "click",
                "move",
                "drag",
                "scroll",
                "type_text",
                "key",
                "cursor",
                "displays"
            ]
        );
        for tool in t.as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        }
    }
}
