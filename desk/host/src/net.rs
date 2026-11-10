//! QUIC 服务：每条连接先在控制流上 Hello 过 token，之后终端流 / 桌面流各开各的。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use anyhow::{bail, Context, Result};
use lolidesk_proto::{
    self as proto, ControlReq, ControlResp, DeskEvent, DeskReq, Frag, StreamKind, TermEvent,
    TermReq, FRAG_KEY,
};
use quinn::{Connection, RecvStream, SendStream};

use crate::{config, desktop, input, terminal::Sessions};

pub struct Shared {
    token: String,
    host: String,
    pub sessions: Sessions,
}

impl Shared {
    pub fn new(token: String) -> Arc<Self> {
        Arc::new(Self {
            token,
            host: config::hostname(),
            sessions: Sessions::default(),
        })
    }
}

pub async fn serve(endpoint: quinn::Endpoint, shared: Arc<Shared>) -> Result<()> {
    while let Some(incoming) = endpoint.accept().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            let remote = incoming.remote_address();
            match incoming.await {
                Ok(conn) => {
                    tracing::info!(%remote, "连接建立");
                    if let Err(e) = handle_conn(conn, shared).await {
                        tracing::info!(%remote, "连接结束：{e:#}");
                    }
                }
                Err(e) => tracing::warn!(%remote, "握手失败：{e}"),
            }
        });
    }
    Ok(())
}

async fn handle_conn(conn: Connection, shared: Arc<Shared>) -> Result<()> {
    let authed = Arc::new(AtomicBool::new(false));
    loop {
        let (send, recv) = conn.accept_bi().await?;
        let (shared, authed, conn) = (shared.clone(), authed.clone(), conn.clone());
        tokio::spawn(async move {
            if let Err(e) = handle_stream(send, recv, conn, shared, authed).await {
                tracing::debug!("流结束：{e:#}");
            }
        });
    }
}

async fn handle_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    conn: Connection,
    shared: Arc<Shared>,
    authed: Arc<AtomicBool>,
) -> Result<()> {
    let mut kind = [0u8; 1];
    recv.read_exact(&mut kind).await?;
    let kind = StreamKind::from_u8(kind[0]).context("未知流类型")?;
    if kind != StreamKind::Control && !authed.load(Ordering::Relaxed) {
        bail!("还没 Hello 就开 {kind:?} 流");
    }
    match kind {
        StreamKind::Control => control(&mut send, &mut recv, &conn, &shared, &authed).await,
        StreamKind::Terminal => terminal(send, recv, &shared).await,
        StreamKind::Desktop => desktop_stream(send, recv, conn).await,
    }
}

async fn control(
    send: &mut SendStream,
    recv: &mut RecvStream,
    conn: &Connection,
    shared: &Shared,
    authed: &AtomicBool,
) -> Result<()> {
    while let Some(req) = proto::recv::<ControlReq>(recv).await? {
        match req {
            ControlReq::Hello {
                version,
                token,
                client,
            } => {
                if version != proto::VERSION {
                    proto::send(
                        send,
                        &ControlResp::Denied(format!(
                            "协议版本不匹配：主机 {}，客户端 {version}",
                            proto::VERSION
                        )),
                    )
                    .await?;
                    conn.close(1u32.into(), b"version");
                    bail!("版本不匹配");
                }
                if !constant_eq(token.as_bytes(), shared.token.as_bytes()) {
                    proto::send(send, &ControlResp::Denied("token 不对".into())).await?;
                    conn.close(2u32.into(), b"token");
                    bail!("token 不对（{client}）");
                }
                authed.store(true, Ordering::Relaxed);
                tracing::info!(client, "已鉴权");
                proto::send(
                    send,
                    &ControlResp::Welcome {
                        host: shared.host.clone(),
                        os: std::env::consts::OS.into(),
                        version: proto::BUILD.into(),
                        desktop: desktop::supported(),
                    },
                )
                .await?;
            }
            _ if !authed.load(Ordering::Relaxed) => {
                proto::send(send, &ControlResp::Denied("先 Hello".into())).await?;
            }
            ControlReq::ListDisplays => {
                let resp = match desktop::list_displays() {
                    Ok(d) => ControlResp::Displays(d),
                    Err(e) => ControlResp::Error(format!("{e:#}")),
                };
                proto::send(send, &resp).await?;
            }
            ControlReq::ListSessions => {
                proto::send(send, &ControlResp::Sessions(shared.sessions.list())).await?;
            }
        }
    }
    Ok(())
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn terminal(mut send: SendStream, mut recv: RecvStream, shared: &Shared) -> Result<()> {
    let first = proto::recv::<TermReq>(&mut recv)
        .await?
        .context("终端流没发 Open/Attach 就关了")?;
    let session = match first {
        TermReq::Open {
            cols,
            rows,
            command,
            cwd,
        } => shared.sessions.open(cols, rows, command, cwd)?,
        TermReq::Attach { id, cols, rows } => {
            let s = shared
                .sessions
                .get(id)
                .with_context(|| format!("会话 {id} 不存在"))?;
            s.resize(cols, rows);
            s
        }
        other => bail!("终端流第一条必须是 Open/Attach，收到 {other:?}"),
    };
    proto::send(&mut send, &TermEvent::Opened { id: session.id }).await?;
    let (mut rx, snap) = session.attach();
    proto::send(&mut send, &snap).await?;
    session.viewers.fetch_add(1, Ordering::Relaxed);

    let fwd = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let exit = matches!(ev, TermEvent::Exited(_));
                    if proto::send(&mut send, &ev).await.is_err() || exit {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "观看者落后，丢输出");
                    let _ = proto::send(
                        &mut send,
                        &TermEvent::Error(format!("丢了 {n} 段输出，请重新接入")),
                    )
                    .await;
                }
                Err(_) => break,
            }
        }
        let _ = send.finish();
    });

    let result: Result<()> = async {
        while let Some(req) = proto::recv::<TermReq>(&mut recv).await? {
            match req {
                TermReq::Input(i) => session.write(&session.encode_input(&i)),
                TermReq::Resize { cols, rows } => session.resize(cols, rows),
                TermReq::Kill => session.kill(),
                TermReq::Open { .. } | TermReq::Attach { .. } => bail!("已经接上了"),
            }
        }
        Ok(())
    }
    .await;
    session.viewers.fetch_sub(1, Ordering::Relaxed);
    fwd.abort();
    result
}

async fn desktop_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    conn: Connection,
) -> Result<()> {
    let mut capture: Option<desktop::Capture> = None;
    let mut sender: Option<tokio::task::JoinHandle<()>> = None;
    let mut injector: Option<input::Injector> = None;
    let result: Result<()> = async {
        while let Some(req) = proto::recv::<DeskReq>(&mut recv).await? {
            match req {
                DeskReq::Start {
                    display: display_id,
                    quality,
                    max_fps,
                } => {
                    let display = display_id;
                    if capture.is_some() {
                        proto::send(&mut send, &DeskEvent::Error("已经在推了".into())).await?;
                        continue;
                    }
                    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
                    match desktop::Capture::start(
                        desktop::CaptureConfig {
                            display,
                            quality,
                            fps: max_fps.clamp(5, 60),
                        },
                        tx,
                    ) {
                        Ok(c) => {
                            let (w, h) = c.size();
                            proto::send(
                                &mut send,
                                &DeskEvent::Ready {
                                    width: w,
                                    height: h,
                                },
                            )
                            .await?;
                            sender = Some(tokio::spawn(send_frames(conn.clone(), rx)));
                            injector = Some(input::Injector::new(display));
                            capture = Some(c);
                            tracing::info!(display_id, ?quality, max_fps, w, h, "远程桌面开始");
                        }
                        Err(e) => {
                            tracing::warn!("采屏失败：{e:#}");
                            proto::send(&mut send, &DeskEvent::Error(format!("{e:#}"))).await?;
                        }
                    }
                }
                DeskReq::Stop => break,
                DeskReq::Keyframe => {
                    if let Some(c) = &capture {
                        c.request_keyframe();
                    }
                }
                DeskReq::Quality(q) => {
                    if let Some(c) = &capture {
                        c.set_quality(q);
                    }
                }
                DeskReq::Input(i) => {
                    if let Some(inj) = &mut injector {
                        inj.apply(&i);
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    if let Some(c) = capture.take() {
        c.stop();
    }
    if let Some(s) = sender.take() {
        s.abort();
    }
    let _ = proto::send(&mut send, &DeskEvent::Stopped("结束".into())).await;
    tracing::info!("远程桌面结束");
    result
}

/// 一帧切成若干 datagram。来不及发就整帧丢（datagram 本来就不保证到达，客户端丢了会要关键帧）。
async fn send_frames(conn: Connection, mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>) {
    let mut frame_id: u32 = 0;
    while let Some(frame) = rx.recv().await {
        frame_id = frame_id.wrapping_add(1);
        let Some(max) = conn.max_datagram_size() else {
            continue;
        };
        let payload = max.saturating_sub(proto::FRAG_HDR).max(64);
        let count = frame.len().div_ceil(payload);
        if count > u16::MAX as usize {
            tracing::warn!(len = frame.len(), "帧太大，丢弃");
            continue;
        }
        let key = frame.first().is_some_and(|b| *b != 0);
        for (i, chunk) in frame.chunks(payload).enumerate() {
            let mut d = Vec::with_capacity(proto::FRAG_HDR + chunk.len());
            Frag {
                frame: frame_id,
                index: i as u16,
                count: count as u16,
                flags: if key { FRAG_KEY } else { 0 },
            }
            .write(&mut d);
            d.extend_from_slice(chunk);
            if conn.send_datagram(d.into()).is_err() {
                break;
            }
        }
    }
}
