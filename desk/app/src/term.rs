//! 终端会话的流和它在界面这边的状态。
//!
//! 两层：
//! - [`TermStream`]：tokio 上的一条 Terminal 双向流。先发 `Open` 或 `Attach`，之后收 [`TermEvent`]、发输入。
//! - [`TermSession`]：GPUI 实体，一个会话一个，界面切走了也一直接着、一直收帧。
//!   帧（转义序列）和尺寸变化按到达顺序进 [`TermSession::drain`] 的队列，终端视图（`ui::grid`）照着喂给
//!   libghostty-vt：订阅 [`TermSessionEvent::Output`]，按顺序处理 [`TermChunk::Frame`] 和 [`TermChunk::Resized`]。
//!
//! 重连：流断了先看连接。连接也断了是 [`TermPhase::Reconnecting`]，等模型重连后 [`TermSession::reattach`]
//! （同一个会话编号、现在的尺寸，主机的第一帧让终端从头来）。连接还在而流断了（主机嫌这个客户端太慢，
//! 把它断开）就自己马上重接，最多连着三次。重接时主机说会话不在了是 [`TermPhase::Gone`]。

use std::{collections::VecDeque, rc::Rc, sync::Arc};

use anyhow::Result;
use gpui_kit::{Context, EventEmitter, Task};
use lolidesk_core::net::Client;
use lolidesk_proto::{self as proto, ClearKind, StreamKind, TermEvent, TermReq, TermSize};
use tokio::{sync::mpsc, task::AbortHandle};

use crate::runtime::{self, error_text};

/// 接上会话前还不知道视图多大：先按这个开，视图量好尺寸后发 Resize。
pub const DEFAULT_SIZE: TermSize = TermSize {
    cols: 120,
    rows: 36,
    cell_w: 0,
    cell_h: 0,
};

/// 连接还在、流被主机断了时自己重接几次。
const AUTO_REATTACH: u32 = 3;

/// 怎么接上会话。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermStart {
    /// 新开：command 空的是登录 shell
    Open {
        command: Option<String>,
        cwd: Option<String>,
    },
    /// 接上已有的会话
    Attach { id: u32 },
}

impl TermStart {
    fn request(&self, size: TermSize) -> TermReq {
        match self {
            TermStart::Open { command, cwd } => TermReq::Open {
                size,
                command: command.clone(),
                cwd: cwd.clone(),
            },
            TermStart::Attach { id } => TermReq::Attach { id: *id, size },
        }
    }
}

/// tokio 上的一条终端流。丢掉它就关流（会话留在主机上）。
pub struct TermStream {
    tx: mpsc::UnboundedSender<TermReq>,
    task: Option<AbortHandle>,
}

/// 流上来的事件原样转发；流结束时最后来一个 `None`。
pub type TermEvents = mpsc::UnboundedReceiver<Option<TermEvent>>;

impl TermStream {
    pub fn open(client: Arc<Client>, first: TermReq) -> (Self, TermEvents) {
        let (tx, req_rx) = mpsc::unbounded_channel::<TermReq>();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel::<Option<TermEvent>>();
        let task = runtime::spawn(async move {
            if let Err(e) = term_loop(&client, first, req_rx, &ev_tx).await {
                let _ = ev_tx.send(Some(TermEvent::Error(error_text(&e))));
            }
            let _ = ev_tx.send(None);
        });
        (
            Self {
                tx,
                task: Some(task.abort_handle()),
            },
            ev_rx,
        )
    }

    /// 测试用：不连主机，发出去的请求从返回的接收端读。
    pub fn detached() -> (Self, mpsc::UnboundedReceiver<TermReq>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx, task: None }, rx)
    }

    pub fn send(&self, req: TermReq) {
        let _ = self.tx.send(req);
    }

    pub fn close(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Drop for TermStream {
    fn drop(&mut self) {
        self.close();
    }
}

async fn term_loop(
    client: &Client,
    first: TermReq,
    mut req_rx: mpsc::UnboundedReceiver<TermReq>,
    ev_tx: &mpsc::UnboundedSender<Option<TermEvent>>,
) -> Result<()> {
    let (mut send, mut recv) = client.open(StreamKind::Terminal).await?;
    proto::send(&mut send, &first).await?;
    loop {
        tokio::select! {
            ev = proto::recv::<TermEvent>(&mut recv) => match ev? {
                None => break,
                Some(ev) => {
                    if ev_tx.send(Some(ev)).is_err() {
                        break;
                    }
                }
            },
            req = req_rx.recv() => {
                // Kill 之后主机挂断程序、关流；这边接着收完
                let Some(req) = req else { break };
                proto::send(&mut send, &req).await?;
            }
        }
    }
    let _ = send.finish();
    Ok(())
}

/// 开终端流的地方：真的连接，或者测试里的假连接。
pub trait TermLink {
    /// 开一条终端流，第一条请求是 `first`（Open / Attach）。
    fn open_stream(&self, first: TermReq) -> (TermStream, TermEvents);
    /// 连接还在不在。
    fn alive(&self) -> bool;
}

/// 一条 QUIC 连接上的终端流。
pub struct ClientLink(pub Arc<Client>);

impl TermLink for ClientLink {
    fn open_stream(&self, first: TermReq) -> (TermStream, TermEvents) {
        TermStream::open(self.0.clone(), first)
    }

    fn alive(&self) -> bool {
        self.0.conn.close_reason().is_none()
    }
}

// ---------------------------------------------------------------- 界面这边的会话

/// 终端视图按顺序处理的一段输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermChunk {
    /// 主机终端的一帧（转义序列），当程序输出喂给终端
    Frame(Vec<u8>),
    /// 会话换了尺寸
    Resized { cols: u16, rows: u16 },
}

/// 会话的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermPhase {
    /// 等主机回 Opened / 第一帧
    Opening,
    /// 接上了
    Live,
    /// 程序退出了（会话还在主机上，看输出）
    Exited(i32),
    /// 连接断了，等重连后重接
    Reconnecting,
    /// 流关了（自己断开、结束了会话，或者重接不成）
    Closed,
    /// 主机上没有这个会话了
    Gone,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermSessionEvent {
    /// 主机给了会话编号（新开的会话）
    Opened(u32),
    /// 有新的输出可以 [`TermSession::drain`]
    Output,
    Title(String),
    Bell,
    /// 程序发的桌面通知（OSC 9 / 777）
    Notify {
        title: String,
        body: String,
    },
    Exited(i32),
    Error(String),
    /// 连接断了（模型去看连接、排重连）
    Disconnected,
    /// 流结束了，不再重接
    Closed,
}

/// 流关了之后怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterClose {
    /// 停在这个状态
    Stay(ClosedAs),
    /// 连接还在：马上重接
    Reattach,
    /// 连接断了：等重连
    WaitReconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedAs {
    Closed,
    Gone,
    Exited,
}

/// 流关了：按原因决定下一步（纯函数，单测覆盖）。
pub fn after_close(
    user_closed: bool,
    gone: bool,
    exited: bool,
    has_id: bool,
    link_alive: bool,
    auto_tries: u32,
) -> AfterClose {
    if user_closed {
        AfterClose::Stay(ClosedAs::Closed)
    } else if gone {
        AfterClose::Stay(ClosedAs::Gone)
    } else if exited {
        AfterClose::Stay(ClosedAs::Exited)
    } else if !has_id {
        // 新开的会话还没拿到编号就断了：没法重接
        AfterClose::Stay(ClosedAs::Closed)
    } else if !link_alive {
        AfterClose::WaitReconnect
    } else if auto_tries < AUTO_REATTACH {
        AfterClose::Reattach
    } else {
        AfterClose::Stay(ClosedAs::Closed)
    }
}

/// 一个终端会话（界面这边）。
pub struct TermSession {
    pub host: std::net::SocketAddr,
    /// 会话编号：新开的要等主机回 Opened
    pub id: Option<u32>,
    pub start: TermStart,
    pub phase: TermPhase,
    pub title: String,
    pub error: Option<String>,
    /// 主机终端现在的尺寸（最后一个 Resized）
    pub host_size: Option<(u16, u16)>,
    /// 这边要的尺寸（最后一次 Resize；重接时带上）
    pub size: TermSize,
    /// 收到的帧数和字节数
    pub frames: u64,
    pub bytes: u64,
    pub bells: u64,
    /// 第几次接上（重接一次加一）
    pub attaches: u32,
    queue: VecDeque<TermChunk>,
    link: Option<Rc<dyn TermLink>>,
    stream: Option<TermStream>,
    /// 这条流上主机回过 Opened
    opened: bool,
    gone: bool,
    user_closed: bool,
    auto_tries: u32,
    pump: Option<Task<()>>,
}

impl EventEmitter<TermSessionEvent> for TermSession {}

impl TermSession {
    /// 开流并开始收事件。
    pub fn connect(
        link: Rc<dyn TermLink>,
        host: std::net::SocketAddr,
        start: TermStart,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut s = Self::offline(host, start.clone());
        s.link = Some(link);
        s.open(start.request(s.size), cx);
        s
    }

    /// 没有流的会话（测试、快照）。
    pub fn offline(host: std::net::SocketAddr, start: TermStart) -> Self {
        let id = match start {
            TermStart::Attach { id } => Some(id),
            TermStart::Open { .. } => None,
        };
        Self {
            host,
            id,
            start,
            phase: TermPhase::Opening,
            title: String::new(),
            error: None,
            host_size: None,
            size: DEFAULT_SIZE,
            frames: 0,
            bytes: 0,
            bells: 0,
            attaches: 0,
            queue: VecDeque::new(),
            link: None,
            stream: None,
            opened: false,
            gone: false,
            user_closed: false,
            auto_tries: 0,
            pump: None,
        }
    }

    fn open(&mut self, first: TermReq, cx: &mut Context<Self>) {
        let Some(link) = self.link.clone() else {
            return;
        };
        let (stream, mut events) = link.open_stream(first);
        self.stream = Some(stream);
        self.opened = false;
        self.gone = false;
        self.attaches += 1;
        self.phase = TermPhase::Opening;
        let attach = self.attaches;
        self.pump = Some(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let done = ev.is_none();
                let alive = this.update(cx, |s, cx| {
                    // 换过流了：旧流的事不算
                    if s.attaches != attach {
                        return;
                    }
                    match ev {
                        Some(ev) => s.handle(ev, cx),
                        None => s.on_closed(cx),
                    }
                });
                if alive.is_err() || done {
                    break;
                }
            }
        }));
    }

    /// 重接（连接恢复后、或者流被主机断了）：同一个会话编号、现在的尺寸。
    /// 不重接的（自己断开的、程序退出的、会话不在的）返回 false。
    pub fn reattach(&mut self, link: Rc<dyn TermLink>, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.id else {
            return false;
        };
        if self.user_closed || matches!(self.phase, TermPhase::Gone) {
            return false;
        }
        self.link = Some(link);
        self.error = None;
        self.open(
            TermReq::Attach {
                id,
                size: self.size,
            },
            cx,
        );
        cx.notify();
        true
    }

    /// 连接断了（模型的健康检查发现的）：关流，等重连。
    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, TermPhase::Opening | TermPhase::Live) && self.id.is_some() {
            self.stream = None;
            self.pump = None;
            self.phase = TermPhase::Reconnecting;
            cx.notify();
        }
    }

    /// 处理主机来的一个事件。
    pub fn handle(&mut self, ev: TermEvent, cx: &mut Context<Self>) {
        // 帧和尺寸只发 Output（终端自己重画）；状态变了才 notify（顶栏、侧栏跟着重画）
        let quiet = matches!(ev, TermEvent::Frame(_) | TermEvent::Resized { .. })
            && self.phase == TermPhase::Live;
        match ev {
            TermEvent::Opened { id } => {
                self.opened = true;
                self.id = Some(id);
                cx.emit(TermSessionEvent::Opened(id));
            }
            TermEvent::Frame(bytes) => {
                if self.phase == TermPhase::Opening {
                    self.phase = TermPhase::Live;
                }
                self.auto_tries = 0;
                self.frames += 1;
                self.bytes += bytes.len() as u64;
                self.queue.push_back(TermChunk::Frame(bytes));
                cx.emit(TermSessionEvent::Output);
            }
            TermEvent::Resized { cols, rows } => {
                self.host_size = Some((cols, rows));
                self.queue.push_back(TermChunk::Resized { cols, rows });
                cx.emit(TermSessionEvent::Output);
            }
            TermEvent::Title(t) => {
                self.title = t.clone();
                cx.emit(TermSessionEvent::Title(t));
            }
            TermEvent::Bell => {
                self.bells += 1;
                cx.emit(TermSessionEvent::Bell);
            }
            TermEvent::Notify { title, body } => cx.emit(TermSessionEvent::Notify { title, body }),
            TermEvent::Exited(code) => {
                self.phase = TermPhase::Exited(code);
                cx.emit(TermSessionEvent::Exited(code));
            }
            TermEvent::Error(e) => {
                // 接已有会话时主机还没回 Opened 就报错：会话不在了
                if !self.opened && matches!(self.phase, TermPhase::Opening) && self.id.is_some() {
                    self.gone = true;
                }
                self.error = Some(e.clone());
                if !self.gone {
                    cx.emit(TermSessionEvent::Error(e));
                }
            }
        }
        if !quiet {
            cx.notify();
        }
    }

    fn on_closed(&mut self, cx: &mut Context<Self>) {
        self.stream = None;
        let alive = self.link.as_ref().is_some_and(|l| l.alive());
        let next = after_close(
            self.user_closed,
            self.gone,
            matches!(self.phase, TermPhase::Exited(_)),
            self.id.is_some(),
            alive,
            self.auto_tries,
        );
        match next {
            AfterClose::Stay(ClosedAs::Exited) => cx.emit(TermSessionEvent::Closed),
            AfterClose::Stay(ClosedAs::Gone) => {
                self.phase = TermPhase::Gone;
                cx.emit(TermSessionEvent::Closed);
            }
            AfterClose::Stay(ClosedAs::Closed) => {
                self.phase = TermPhase::Closed;
                cx.emit(TermSessionEvent::Closed);
            }
            AfterClose::WaitReconnect => {
                self.phase = TermPhase::Reconnecting;
                cx.emit(TermSessionEvent::Disconnected);
            }
            AfterClose::Reattach => {
                self.auto_tries += 1;
                if let Some(link) = self.link.clone() {
                    self.reattach(link, cx);
                }
            }
        }
        cx.notify();
    }

    /// 取走排着的输出（按到达顺序）。
    pub fn drain(&mut self) -> Vec<TermChunk> {
        self.queue.drain(..).collect()
    }

    /// 排着还没取的输出段数。
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// 结束了（不会再有输出、也不会自己重接）。
    pub fn is_ended(&self) -> bool {
        matches!(
            self.phase,
            TermPhase::Exited(_) | TermPhase::Closed | TermPhase::Gone
        )
    }

    /// 能输入（流开着）。
    pub fn is_live(&self) -> bool {
        self.stream.is_some() && matches!(self.phase, TermPhase::Live | TermPhase::Exited(_))
    }

    fn send(&self, req: TermReq) {
        if let Some(s) = &self.stream {
            s.send(req);
        }
    }

    /// 键入、粘贴、鼠标、焦点的字节（视图按终端当前的模式编好码）。
    pub fn input(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.send(TermReq::Input(bytes));
        }
    }

    /// 视图放得下的尺寸（记下来，重接时带上）。
    pub fn resize(&mut self, size: TermSize) {
        self.size = size;
        self.send(TermReq::Resize(size));
    }

    /// 用户在这个窗口里用这个会话了：会话改用这边的尺寸。
    pub fn claim(&self) {
        self.send(TermReq::Claim);
    }

    pub fn clear(&self, kind: ClearKind) {
        self.send(TermReq::Clear(kind));
    }

    /// 结束会话：挂断程序，会话从主机的列表里去掉（之后不重接）。
    pub fn kill(&mut self) {
        self.user_closed = true;
        self.send(TermReq::Kill);
    }

    /// 断开：关流，会话留在主机上。
    pub fn close(&mut self) {
        self.user_closed = true;
        if let Some(mut s) = self.stream.take() {
            s.close();
        }
        self.pump = None;
        if !matches!(self.phase, TermPhase::Exited(_) | TermPhase::Gone) {
            self.phase = TermPhase::Closed;
        }
    }

    /// 自己断开过的会话重新接上（界面上的「重新接入」）。
    pub fn reopen(&mut self, link: Rc<dyn TermLink>, cx: &mut Context<Self>) -> bool {
        self.user_closed = false;
        self.auto_tries = 0;
        if matches!(self.phase, TermPhase::Exited(_)) {
            self.phase = TermPhase::Closed;
        }
        self.reattach(link, cx)
    }

    /// 测试用：换上一条不连主机的流，返回它发出去的请求。
    pub fn attach_detached(&mut self) -> mpsc::UnboundedReceiver<TermReq> {
        let (s, rx) = TermStream::detached();
        self.stream = Some(s);
        rx
    }
}

/// 测试用的假连接：记下开过的流，测试往里推事件、读请求。
#[derive(Default)]
pub struct FakeLink {
    pub alive: std::cell::Cell<bool>,
    #[allow(clippy::type_complexity)]
    pub streams: std::cell::RefCell<
        Vec<(
            TermReq,
            mpsc::UnboundedReceiver<TermReq>,
            mpsc::UnboundedSender<Option<TermEvent>>,
        )>,
    >,
}

impl FakeLink {
    pub fn new() -> Rc<Self> {
        let l = Rc::new(Self::default());
        l.alive.set(true);
        l
    }

    /// 第 i 条流的第一条请求。
    pub fn first(&self, i: usize) -> Option<TermReq> {
        self.streams.borrow().get(i).map(|s| s.0.clone())
    }

    /// 往第 i 条流推一个事件（None 是流关了）。
    pub fn push(&self, i: usize, ev: Option<TermEvent>) {
        if let Some(s) = self.streams.borrow().get(i) {
            let _ = s.2.send(ev);
        }
    }

    pub fn count(&self) -> usize {
        self.streams.borrow().len()
    }
}

impl TermLink for FakeLink {
    fn open_stream(&self, first: TermReq) -> (TermStream, TermEvents) {
        let (stream, req_rx) = TermStream::detached();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        self.streams.borrow_mut().push((first, req_rx, ev_tx));
        (stream, ev_rx)
    }

    fn alive(&self) -> bool {
        self.alive.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_requests() {
        let open = TermStart::Open {
            command: Some("claude".into()),
            cwd: None,
        };
        assert!(matches!(
            open.request(DEFAULT_SIZE),
            TermReq::Open { size, command: Some(c), cwd: None } if size.cols == 120 && c == "claude"
        ));
        assert!(matches!(
            TermStart::Attach { id: 7 }.request(TermSize::new(80, 24)),
            TermReq::Attach { id: 7, size } if size.rows == 24
        ));
    }

    #[test]
    fn stream_sends_in_order() {
        let (s, mut rx) = TermStream::detached();
        s.send(TermReq::Input(b"ls\r".to_vec()));
        s.send(TermReq::Resize(TermSize::new(100, 30)));
        s.send(TermReq::Claim);
        assert!(matches!(rx.try_recv().unwrap(), TermReq::Input(b) if b == b"ls\r"));
        assert!(matches!(rx.try_recv().unwrap(), TermReq::Resize(t) if t.cols == 100));
        assert!(matches!(rx.try_recv().unwrap(), TermReq::Claim));
    }

    #[test]
    fn close_decisions() {
        use AfterClose::*;
        // 自己断开、会话不在、程序退出：停
        assert_eq!(
            after_close(true, false, false, true, false, 0),
            Stay(ClosedAs::Closed)
        );
        assert_eq!(
            after_close(false, true, false, true, true, 0),
            Stay(ClosedAs::Gone)
        );
        assert_eq!(
            after_close(false, false, true, true, false, 0),
            Stay(ClosedAs::Exited)
        );
        // 没拿到编号的新会话：没法重接
        assert_eq!(
            after_close(false, false, false, false, false, 0),
            Stay(ClosedAs::Closed)
        );
        // 连接断了：等重连
        assert_eq!(
            after_close(false, false, false, true, false, 0),
            WaitReconnect
        );
        // 连接还在：重接，最多三次
        assert_eq!(after_close(false, false, false, true, true, 2), Reattach);
        assert_eq!(
            after_close(false, false, false, true, true, 3),
            Stay(ClosedAs::Closed)
        );
    }
}
