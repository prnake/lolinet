//! TUI：在当前终端里接主机上的一个会话。本地终端的字节原样发过去（本地终端已经按模式编好码），
//! 主机发来的 Mirror 帧原样写到 stdout（帧就是转义序列，第一帧从头画，含滚动历史）。
//! Ctrl-] 断开（会话留在主机上）。

use std::io::{Read, Write};

use anyhow::{bail, Result};
use lolidesk_proto::{self as proto, StreamKind, TermEvent, TermReq, TermSize};

use lolidesk_core::Client;

const DETACH: u8 = 0x1d;

/// 接哪个会话，或者新开时跑什么。
pub struct Open {
    pub session: Option<u32>,
    pub command: Option<String>,
    pub cwd: Option<String>,
}

pub async fn run(client: std::sync::Arc<Client>, open: Open) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let (mut send, mut recv) = client.open(StreamKind::Terminal).await?;
    let size = TermSize::new(cols, rows);
    let first = match open.session {
        Some(id) => TermReq::Attach { id, size },
        None => TermReq::Open {
            size,
            command: open.command,
            cwd: open.cwd,
        },
    };
    proto::send(&mut send, &first).await?;
    let id = match proto::recv::<TermEvent>(&mut recv).await? {
        Some(TermEvent::Opened { id }) => id,
        Some(TermEvent::Error(e)) => bail!("主机：{e}"),
        other => bail!("没拿到会话：{other:?}"),
    };
    eprintln!("会话 {id}，Ctrl-] 断开（会话留在 {}）", client.welcome.host);

    // stdin 不是终端（被脚本喂数据）时开不了 raw mode，就按行模式继续
    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    let _guard = RawGuard(raw);
    let mut stdout = std::io::stdout();

    // stdin 阻塞读放在普通线程
    let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if in_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut size = (cols, rows);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(300));
    loop {
        tokio::select! {
            ev = proto::recv::<TermEvent>(&mut recv) => {
                match ev? {
                    None => { eprintln!("\r\n主机关了连接"); break; }
                    Some(TermEvent::Frame(b)) => { stdout.write_all(&b)?; stdout.flush()?; }
                    // 本地终端的尺寸由用户定；会话听别的客户端的尺寸时，画面会按那个尺寸排
                    Some(TermEvent::Resized { .. }) => {}
                    Some(TermEvent::Title(t)) => { let _ = write!(stdout, "\x1b]0;{t}\x07"); let _ = stdout.flush(); }
                    Some(TermEvent::Bell) => { let _ = stdout.write_all(b"\x07"); let _ = stdout.flush(); }
                    Some(TermEvent::Notify { title, body }) => {
                        // 交给本地终端显示（OSC 777，Ghostty / kitty / iTerm2 都认）
                        let _ = write!(stdout, "\x1b]777;notify;{title};{body}\x07");
                        let _ = stdout.flush();
                    }
                    Some(TermEvent::Exited(code)) => { eprintln!("\r\n会话里的程序退出了（退出码 {code}）"); break; }
                    Some(TermEvent::Error(e)) => { eprintln!("\r\n主机：{e}"); }
                    Some(TermEvent::Opened { .. }) => {}
                }
            }
            bytes = in_rx.recv() => {
                let Some(mut bytes) = bytes else { break };
                if let Some(pos) = bytes.iter().position(|b| *b == DETACH) {
                    bytes.truncate(pos);
                    if !bytes.is_empty() { proto::send(&mut send, &TermReq::Input(bytes)).await?; }
                    eprintln!("\r\n已断开，会话 {id} 还在主机上：lolidesk <主机> -s {id}");
                    break;
                }
                proto::send(&mut send, &TermReq::Input(bytes)).await?;
            }
            _ = tick.tick() => {
                if let Ok(now) = crossterm::terminal::size() {
                    if now != size {
                        size = now;
                        proto::send(&mut send, &TermReq::Resize(TermSize::new(now.0, now.1))).await?;
                    }
                }
            }
        }
    }
    let _ = send.finish();
    client.conn.close(0u32.into(), b"bye");
    Ok(())
}

struct RawGuard(bool);
impl Drop for RawGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = crossterm::terminal::disable_raw_mode();
        }
        // 主机的帧可能开了这些模式（全屏、括号粘贴、鼠标、kitty 键盘协议……）：还给本地终端
        let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2026l\x1b[<u\x1b>\r\n");
        let _ = std::io::stdout().flush();
    }
}
