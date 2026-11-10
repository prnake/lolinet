//! QUIC 服务：每条连接先在控制流上 Hello 过 token，之后终端流 / 桌面流各开各的。

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

use anyhow::{bail, Context, Result};
use lolidesk_proto::{
    self as proto, ControlReq, ControlResp, DeskEvent, DeskInput, DeskReq, Frag, StreamKind,
    TermEvent, TermReq, VideoSettings, FRAG_KEY,
};
use quinn::{Connection, RecvStream, SendStream};

use crate::{config, desktop, input, launchers, terminal::Sessions};

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
            sessions: Sessions::new(config::config_dir().join("sessions")),
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
                        version: env!("CARGO_PKG_VERSION").into(),
                        desktop: desktop::supported(),
                        persistent: shared.sessions.persistent(),
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
                let sessions = shared.sessions.clone();
                let list = tokio::task::spawn_blocking(move || sessions.list()).await?;
                proto::send(send, &ControlResp::Sessions(list)).await?;
            }
            ControlReq::ListLaunchers => {
                let list = tokio::task::spawn_blocking(launchers::list).await?;
                proto::send(send, &ControlResp::Launchers(list)).await?;
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
    let opened = match first {
        TermReq::Open { size, command, cwd } => {
            let sessions = shared.sessions.clone();
            tokio::task::spawn_blocking(move || sessions.open(size, command, cwd))
                .await?
                .map(|s| (s, size))
        }
        TermReq::Attach { id, size } => shared
            .sessions
            .get(id)
            .map(|s| (s, size))
            .with_context(|| format!("会话 {id} 不存在（可能已经结束了）")),
        other => Err(anyhow::anyhow!(
            "终端流第一条必须是 Open/Attach，收到 {other:?}"
        )),
    };
    let (session, size) = match opened {
        Ok(v) => v,
        Err(e) => {
            // 说清楚为什么开不了，再关流
            let _ = proto::send(&mut send, &TermEvent::Error(format!("{e:#}"))).await;
            let _ = send.finish();
            return Err(e);
        }
    };
    proto::send(&mut send, &TermEvent::Opened { id: session.id }).await?;

    // 会话往这个队列里放帧和事件，这里按顺序发出去；队列满了会话会断开这个客户端（它重接从第一帧开始）
    let (tx, mut rx) = tokio::sync::mpsc::channel::<TermEvent>(crate::terminal::VIEWER_QUEUE);
    let viewer = session.attach(tx, size);
    let fwd = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if proto::send(&mut send, &ev).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });

    let result: Result<()> = async {
        while let Some(req) = proto::recv::<TermReq>(&mut recv).await? {
            match req {
                TermReq::Input(bytes) => session.input(viewer, &bytes),
                TermReq::Resize(size) => session.ask(viewer, size),
                TermReq::Claim => session.claim(viewer),
                TermReq::Clear(what) => session.clear(what),
                TermReq::Kill => session.kill(),
                TermReq::Open { .. } | TermReq::Attach { .. } => bail!("已经接上了"),
            }
        }
        Ok(())
    }
    .await;
    session.detach(viewer);
    // 等已经排着的帧发完（会话那头丢了发送端，队列就会结束）
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), fwd).await;
    result
}

async fn desktop_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    conn: Connection,
) -> Result<()> {
    let mut capture: Option<desktop::Capture> = None;
    let mut display = 0u32;
    let mut injector: Option<input::Injector> = None;
    // 发送任务只有一个，跨重开采集复用；采集只管往 tx 里塞编好的帧
    let (tx, rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let dropped = Arc::new(AtomicU64::new(0));
    let force_key = Arc::new(AtomicBool::new(true));
    let sender = tokio::spawn(send_frames(
        conn.clone(),
        rx,
        dropped.clone(),
        force_key.clone(),
    ));
    let result: Result<()> = async {
        while let Some(req) = proto::recv::<DeskReq>(&mut recv).await? {
            match req {
                DeskReq::Start { display: id, video } => {
                    if capture.is_some() {
                        proto::send(&mut send, &DeskEvent::Error("已经在推了".into())).await?;
                        continue;
                    }
                    display = id;
                    match start_capture(display, video, &force_key, tx.clone(), &mut send).await? {
                        Some(c) => {
                            injector = Some(input::Injector::new(display));
                            capture = Some(c);
                        }
                        None => continue,
                    }
                }
                DeskReq::Configure(video) => {
                    let Some(old) = capture.take() else {
                        continue;
                    };
                    old.stop();
                    capture =
                        start_capture(display, video, &force_key, tx.clone(), &mut send).await?;
                }
                DeskReq::Stop => break,
                DeskReq::Keyframe => force_key.store(true, Ordering::Relaxed),
                DeskReq::Input(i) => {
                    if let Some(inj) = &injector {
                        inj.apply(i);
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
    if let Some(inj) = injector.take() {
        inj.apply(DeskInput::ReleaseAll);
    }
    sender.abort();
    let _ = proto::send(&mut send, &DeskEvent::Stopped("结束".into())).await;
    tracing::info!(dropped = dropped.load(Ordering::Relaxed), "远程桌面结束");
    result
}

/// 开采集，成功就回 Ready（实际参数），失败回 Error。
async fn start_capture(
    display: u32,
    video: VideoSettings,
    force_key: &Arc<AtomicBool>,
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    send: &mut SendStream,
) -> Result<Option<desktop::Capture>> {
    let cfg = desktop::CaptureConfig {
        display,
        video,
        force_key: force_key.clone(),
    };
    let started = tokio::task::spawn_blocking(move || desktop::Capture::start(cfg, tx)).await?;
    match started {
        Ok(c) => {
            let a = c.actual();
            let display_id = display;
            tracing::info!(
                display_id,
                ?video,
                w = a.width,
                h = a.height,
                fps = a.fps,
                bitrate = a.bitrate,
                "远程桌面开始"
            );
            proto::send(
                send,
                &DeskEvent::Ready {
                    width: a.width,
                    height: a.height,
                    fps: a.fps,
                    bitrate: a.bitrate,
                },
            )
            .await?;
            Ok(Some(c))
        }
        Err(e) => {
            tracing::warn!("采屏失败：{e:#}");
            proto::send(send, &DeskEvent::Error(format!("{e:#}"))).await?;
            Ok(None)
        }
    }
}

/// 一帧切成若干 datagram。发送缓冲放不下整帧就整帧丢（发一半等于白发），
/// 然后跳过后面的差分帧、让编码器出关键帧：差分帧缺了参考，发过去也解不出来。
async fn send_frames(
    conn: Connection,
    mut rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    dropped: Arc<AtomicU64>,
    force_key: Arc<AtomicBool>,
) {
    let mut frame_id: u32 = 0;
    let mut skipping = false;
    while let Some(frame) = rx.recv().await {
        let key = frame.first().is_some_and(|b| *b != 0);
        if skipping && !key {
            continue;
        }
        skipping = false;
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
        if conn.datagram_send_buffer_space() < frame.len() + count * (proto::FRAG_HDR + 8) {
            dropped.fetch_add(1, Ordering::Relaxed);
            skipping = true;
            force_key.store(true, Ordering::Relaxed);
            continue;
        }
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
