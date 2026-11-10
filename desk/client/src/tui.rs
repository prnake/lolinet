//! TUI：在当前终端里接主机上的一个会话。本地终端字节原样透传（Input::Raw），主机的输出原样写到 stdout。
//! Ctrl-] 断开（会话留在主机上）。

use std::io::{Read, Write};

use anyhow::{bail, Result};
use lolidesk_proto::{self as proto, Input, StreamKind, TermEvent, TermReq};

use crate::net::Client;

const DETACH: u8 = 0x1d;

pub async fn run(client: std::sync::Arc<Client>, session: Option<u32>) -> Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let (mut send, mut recv) = client.open(StreamKind::Terminal).await?;
    let first = match session {
        Some(id) => TermReq::Attach { id, cols, rows },
        None => TermReq::Open {
            cols,
            rows,
            command: None,
            cwd: None,
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
                    Some(TermEvent::Output(b)) => { stdout.write_all(&b)?; stdout.flush()?; }
                    Some(TermEvent::Snapshot { data, .. }) => { stdout.write_all(&data)?; stdout.flush()?; }
                    Some(TermEvent::Title(t)) => { let _ = write!(stdout, "\x1b]0;{t}\x07"); let _ = stdout.flush(); }
                    Some(TermEvent::Bell) => { let _ = stdout.write_all(b"\x07"); }
                    Some(TermEvent::Exited(code)) => { eprintln!("\r\n会话结束（退出码 {code}）"); break; }
                    Some(TermEvent::Error(e)) => { eprintln!("\r\n主机：{e}"); }
                    Some(TermEvent::Opened { .. }) => {}
                }
            }
            bytes = in_rx.recv() => {
                let Some(mut bytes) = bytes else { break };
                if let Some(pos) = bytes.iter().position(|b| *b == DETACH) {
                    bytes.truncate(pos);
                    if !bytes.is_empty() { proto::send(&mut send, &TermReq::Input(Input::Raw(bytes))).await?; }
                    eprintln!("\r\n已断开，会话 {id} 还在主机上：lolidesk --tui -s {id} …");
                    break;
                }
                proto::send(&mut send, &TermReq::Input(Input::Raw(bytes))).await?;
            }
            _ = tick.tick() => {
                if let Ok(now) = crossterm::terminal::size() {
                    if now != size {
                        size = now;
                        proto::send(&mut send, &TermReq::Resize { cols: now.0, rows: now.1 }).await?;
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
        let _ = std::io::stdout().write_all(b"\x1b[0m\x1b[?25h\x1b[?1049l\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b>");
        let _ = std::io::stdout().flush();
    }
}
