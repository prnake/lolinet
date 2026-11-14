//! 一次远程桌面的流：控制走 Desktop 双向流，画面走连接上的 datagram。
//!
//! 三条线：
//! - 流任务（tokio）：发 Start / Configure / Keyframe / Input，收 Ready / Stopped / Error，转成 [`DeskStreamEvent`]。
//! - datagram 任务（tokio）：按 frame id 拼帧（[`Reassembler`]），拼齐的交给解码线程；解码跟不上就丢、等关键帧。
//! - 解码线程：VideoToolbox 解成 IOSurface 做底的 NV12，放进槽里（只留最新一帧），`Notify` 一下界面。
//!   丢了帧（帧号断了、解码出错、没拼齐）就等关键帧，最多每 250 ms 要一次。
//!
//! 一条连接同一时间只有一个远程桌面（datagram 是连接级的）。

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc as smpsc,
    },
    time::Instant,
};

use anyhow::Result;
use lolidesk_core::{
    decode::{Decoder, Frame},
    net::Client,
};
use lolidesk_proto::{
    self as proto, DeskEvent, DeskInput, DeskReq, DeskTarget, Frag, StreamKind, VideoFrame,
    VideoSettings,
};
use tokio::{
    sync::{Notify, mpsc},
    task::AbortHandle,
};

use crate::{
    runtime::{self, error_text},
    video::{Counters, KeyframeGate, RateMeter, Reassembler, StatsSnapshot},
};

/// 流上来的事。
#[derive(Debug, Clone)]
pub enum DeskStreamEvent {
    /// 实际生效的编码参数
    Ready {
        width: u32,
        height: u32,
        fps: u8,
        bitrate: u32,
    },
    Stopped(String),
    Error(String),
    /// 主机现在推的是这个（Open / Select 之后；窗口没了改推显示器时 note 说为什么）
    Showing {
        target: DeskTarget,
        note: String,
    },
    /// 流结束了（之后不会再有事件）
    Closed,
}

/// 解码线程和界面共享的：最新一帧、计数、关没关。
pub struct DeskShared {
    frame: Mutex<Option<Frame>>,
    /// 新的一帧到了（多次通知会合并成一次）
    pub frame_ready: Notify,
    counters: Mutex<Counters>,
    closed: AtomicBool,
}

impl DeskShared {
    fn new() -> Self {
        Self {
            frame: Mutex::new(None),
            frame_ready: Notify::new(),
            counters: Mutex::new(Counters::default()),
            closed: AtomicBool::new(false),
        }
    }

    /// 拿走最新一帧（没有新的就是 None）。
    pub fn take_frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap().take()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    pub fn counters(&self) -> Counters {
        self.counters.lock().unwrap().clone()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.frame_ready.notify_one();
    }

    fn count(&self, f: impl FnOnce(&mut Counters)) {
        f(&mut self.counters.lock().unwrap());
    }
}

/// 发给主机的请求（画面上的输入用，能复制）。
#[derive(Clone)]
pub struct DeskSender(mpsc::UnboundedSender<DeskReq>);

impl DeskSender {
    pub fn input(&self, input: DeskInput) {
        let _ = self.0.send(DeskReq::Input(input));
    }

    pub fn send(&self, req: DeskReq) {
        let _ = self.0.send(req);
    }

    /// 测试用：不连主机，发出去的请求从返回的接收端读。
    pub fn detached() -> (Self, mpsc::UnboundedReceiver<DeskReq>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }
}

pub struct DeskStream {
    tx: DeskSender,
    shared: Arc<DeskShared>,
    client: Arc<Client>,
    tasks: Vec<AbortHandle>,
    meter: RateMeter,
    stopped: bool,
}

impl DeskStream {
    /// 开始一次远程桌面：一块显示器（Start，哪个版本的主机都认），或者一个窗口（Open，
    /// 只对列得出窗口的主机）。事件从返回的接收端读（最后一个是 Closed）。
    pub fn start(
        client: Arc<Client>,
        target: DeskTarget,
        video: VideoSettings,
    ) -> (Self, mpsc::UnboundedReceiver<DeskStreamEvent>) {
        let (tx, req_rx) = mpsc::unbounded_channel::<DeskReq>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel::<DeskStreamEvent>();
        let shared = Arc::new(DeskShared::new());

        // 解码线程：拼好的帧进来，解成画面放进槽里
        let (frame_tx, frame_rx) = smpsc::sync_channel::<(Vec<u8>, bool)>(8);
        {
            let shared = shared.clone();
            let keyframe = DeskSender(tx.clone());
            if let Err(e) = std::thread::Builder::new()
                .name("lolidesk-decode".into())
                .spawn(move || decode_loop(shared, frame_rx, keyframe))
            {
                let _ = ev_tx.send(DeskStreamEvent::Error(format!("开不了解码线程：{e}")));
            }
        }

        let stream_task = {
            let shared = shared.clone();
            let client = client.clone();
            runtime::spawn(async move {
                let r = stream_loop(&client, target, video, req_rx, &ev_tx).await;
                if let Err(e) = r {
                    let _ = ev_tx.send(DeskStreamEvent::Error(error_text(&e)));
                }
                let _ = ev_tx.send(DeskStreamEvent::Closed);
                shared.close();
            })
        };
        let dgram_task = {
            let shared = shared.clone();
            let conn = client.conn.clone();
            runtime::spawn(async move { datagram_loop(conn, shared, frame_tx).await })
        };
        (
            Self {
                tx: DeskSender(tx),
                shared,
                client,
                tasks: vec![stream_task.abort_handle(), dgram_task.abort_handle()],
                meter: RateMeter::default(),
                stopped: false,
            },
            ev_rx,
        )
    }

    pub fn shared(&self) -> Arc<DeskShared> {
        self.shared.clone()
    }

    pub fn sender(&self) -> DeskSender {
        self.tx.clone()
    }

    pub fn configure(&self, video: VideoSettings) {
        self.tx.send(DeskReq::Configure(video));
    }

    /// 在这条流上换推的东西（主机回 Showing + Ready，换不了回 Error）
    pub fn select(&self, target: DeskTarget) {
        self.tx.send(DeskReq::Select(target));
    }

    pub fn keyframe(&self) {
        self.tx.send(DeskReq::Keyframe);
    }

    /// 取一次统计（速率按上次取以来算）。
    pub fn stats(&mut self) -> StatsSnapshot {
        let rtt = self.client.rtt().as_secs_f64() * 1000.0;
        self.meter
            .sample(Instant::now(), &self.shared.counters(), rtt)
    }

    /// 停：告诉主机、停收画面。流任务发完 Stop 自己结束。
    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.tx.send(DeskReq::Stop);
        // 画面不收了（解码线程跟着 frame_tx 被丢掉而结束）
        if let Some(dgram) = self.tasks.get(1) {
            dgram.abort();
        }
        self.shared.close();
    }
}

impl Drop for DeskStream {
    fn drop(&mut self) {
        self.stop();
        // 流任务在发完 Stop 后自己退出；万一卡住，连接关掉时也会退出
    }
}

async fn stream_loop(
    client: &Client,
    target: DeskTarget,
    video: VideoSettings,
    mut req_rx: mpsc::UnboundedReceiver<DeskReq>,
    ev_tx: &mpsc::UnboundedSender<DeskStreamEvent>,
) -> Result<()> {
    let (mut send, mut recv) = client.open(StreamKind::Desktop).await?;
    let first = match target {
        DeskTarget::Display(display) => DeskReq::Start { display, video },
        DeskTarget::Window(_) => DeskReq::Open { target, video },
    };
    proto::send(&mut send, &first).await?;
    loop {
        tokio::select! {
            ev = proto::recv::<DeskEvent>(&mut recv) => match ev? {
                None => break,
                Some(DeskEvent::Ready { width, height, fps, bitrate }) => {
                    let _ = ev_tx.send(DeskStreamEvent::Ready { width, height, fps, bitrate });
                }
                Some(DeskEvent::Stopped(why)) => {
                    let _ = ev_tx.send(DeskStreamEvent::Stopped(why));
                    break;
                }
                Some(DeskEvent::Error(e)) => {
                    let _ = ev_tx.send(DeskStreamEvent::Error(e));
                }
                Some(DeskEvent::Showing { target, note }) => {
                    let _ = ev_tx.send(DeskStreamEvent::Showing { target, note });
                }
            },
            req = req_rx.recv() => {
                let Some(req) = req else { break };
                let stop = matches!(req, DeskReq::Stop);
                proto::send(&mut send, &req).await?;
                if stop {
                    let _ = ev_tx.send(DeskStreamEvent::Stopped(String::new()));
                    break;
                }
            }
        }
    }
    let _ = send.finish();
    Ok(())
}

/// 收 datagram、拼帧，拼齐的交给解码线程。
async fn datagram_loop(
    conn: quinn::Connection,
    shared: Arc<DeskShared>,
    frames: smpsc::SyncSender<(Vec<u8>, bool)>,
) {
    let mut r = Reassembler::new();
    while let Ok(dg) = conn.read_datagram().await {
        if shared.is_closed() {
            break;
        }
        let Some((frag, body)) = Frag::parse(&dg) else {
            continue;
        };
        let before = r.incomplete;
        let done = r.push(frag, body);
        let lost = r.incomplete - before;
        let Some(done) = done else {
            if lost > 0 {
                shared.count(|c| c.incomplete += lost);
            }
            continue;
        };
        shared.count(|c| {
            c.frames += 1;
            c.bytes += done.data.len() as u64;
            c.incomplete += lost;
        });
        let gap = done.gap && !done.key;
        if frames.try_send((done.data, gap)).is_err() {
            // 解码跟不上：这一帧丢了，后面也得等关键帧
            shared.count(|c| c.overrun += 1);
            let _ = frames.try_send((Vec::new(), true));
        }
    }
    shared.close();
}

fn decode_loop(
    shared: Arc<DeskShared>,
    rx: smpsc::Receiver<(Vec<u8>, bool)>,
    keyframe: DeskSender,
) {
    let mut decoder = Decoder::new();
    let mut need_key = true;
    let mut gate = KeyframeGate::default();
    while let Ok((data, gap)) = rx.recv() {
        if shared.is_closed() {
            break;
        }
        if gap {
            need_key = true;
        }
        let frame = if data.is_empty() {
            None
        } else {
            VideoFrame::parse(&data)
        };
        let usable = match &frame {
            Some(f) if f.key => {
                need_key = false;
                true
            }
            Some(_) => !need_key,
            None => false,
        };
        if !usable {
            if (need_key || frame.is_none()) && gate.should_request(Instant::now()) {
                keyframe.send(DeskReq::Keyframe);
                shared.count(|c| c.keyframe_requests += 1);
            }
            continue;
        }
        let Some(f) = frame else { continue };
        let t = Instant::now();
        match decoder.decode(&f) {
            Ok(Some(picture)) => {
                let us = t.elapsed().as_micros() as u64;
                shared.count(|c| {
                    c.decoded += 1;
                    c.decode_us += us;
                });
                *shared.frame.lock().unwrap() = Some(picture);
                shared.frame_ready.notify_one();
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("解码：{e:#}");
                need_key = true;
            }
        }
    }
}
