//! 窗口的状态：记住的主机、连接、会话列表、选中项、开着的终端和远程桌面。
//!
//! 一个 GPUI 实体，侧栏、首页、各个面板都看它（`cx.observe`）。改状态的方法都在这里；
//! 要等网络的在 tokio 上跑（[`runtime::run`]），结果回到这里，带着「第几次连接」（epoch）——
//! 断开、重连之后旧连接的结果对不上就丢掉。
//!
//! 定时：每 2 秒看一次连接还在不在（`close_reason`）、更新延迟，每 4 秒刷一次会话列表。
//! 权限引导卡片显示着时每 2 秒问一次主机的权限；网络页开着时每 3 秒读一次本机 lolinet（在后台线程上），
//! 没开着时 30 秒一次。
//!
//! 重连：连不上、连接断了都按 1、2、4……60 秒退避重试（连上了从头算；窗口回到前台马上试）。
//! 断的时候开着的终端不关：变暗、等着，重连后用同一个会话编号重新 Attach；远程桌面重新 Start。
//! 用户自己点「断开」的不重连；主机拒绝、证书指纹变了也不重连（重试也没用）。

use std::{
    net::{IpAddr, SocketAddr},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};
use gpui_kit::{AppContext, Context, Entity, EntityId, EventEmitter, Task, Window};
use lolidesk_core::{
    hosts::Hosts,
    net::{self, Client, Welcome},
};
use lolidesk_proto::{
    ControlReq, ControlResp, DisplayInfo, Launcher, PermissionKind, SessionInfo, VideoSettings,
};

use crate::{
    backoff::Retry,
    labels::{self, cap_fps, host_title},
    lolinet::{self, JoinTarget, Lolinet, Status as NetStatus},
    prefs::Prefs,
    runtime::{self, error_text},
    term::{ClientLink, TermLink, TermPhase, TermSession, TermSessionEvent, TermStart, TermStream},
    ui::{desktop::DeskPane, terminal::TerminalView},
};

/// 主机连接的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Offline,
    Connecting,
    Online,
    /// 连不上，等下一次重试（界面上也叫「连接中」）
    Reconnecting,
    /// 连不上、不再重试（主机拒绝、指纹变了）
    Failed,
}

/// 侧栏里的一台主机。
pub struct HostState {
    pub addr: SocketAddr,
    pub name: String,
    pub fingerprint: String,
    token: String,
    pub link: Link,
    pub error: String,
    pub client: Option<Arc<Client>>,
    pub welcome: Option<Welcome>,
    /// 每连一次加一：旧连接的结果对不上就丢掉
    epoch: u64,
    pub sessions: Vec<SessionInfo>,
    pub displays: Vec<DisplayInfo>,
    /// 列显示器失败（比如没有屏幕录制权限）
    pub displays_error: Option<String>,
    pub launchers: Vec<Launcher>,
    pub rtt_ms: f64,
    pub expanded: bool,
    /// 要不要保持连着（用户点了「断开」就不要）
    pub wanted: bool,
    pub retry: Retry,
    /// 远程桌面要的两个权限（主机说的）；还没问到、或者主机没有远程桌面是 None
    pub perms: Option<Perms>,
    /// 权限引导卡片的进度（这次连接里）
    pub guide: Guide,
}

/// 主机的屏幕录制、辅助功能权限。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Perms {
    pub screen: bool,
    pub access: bool,
}

/// 权限引导卡片：哪些行出现过（开了也留着，变绿）、要不要重启主机服务。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Guide {
    pub screen: bool,
    pub access: bool,
    /// 屏幕录制这次连接里从没开变成开了：要重启主机服务才生效
    pub restart: bool,
    /// 点过屏幕录制的「在主机上打开设置」（系统可能要重启服务后才认）
    pub opened_screen: bool,
    /// 点了「重启主机服务」，等它断开重连
    pub restarting: bool,
}

impl HostState {
    pub fn new(addr: SocketAddr, name: &str, fingerprint: &str, token: &str) -> Self {
        Self {
            addr,
            name: name.to_string(),
            fingerprint: fingerprint.to_string(),
            token: token.to_string(),
            link: Link::Offline,
            error: String::new(),
            client: None,
            welcome: None,
            epoch: 0,
            sessions: Vec::new(),
            displays: Vec::new(),
            displays_error: None,
            launchers: Vec::new(),
            rtt_ms: 0.0,
            expanded: true,
            wanted: true,
            retry: Retry::default(),
            perms: None,
            guide: Guide::default(),
        }
    }

    /// 主机给远程桌面或 computer use（这两样要权限）
    pub fn wants_desktop(&self) -> bool {
        self.welcome
            .as_ref()
            .is_some_and(|w| w.desktop && (w.allow.desktop || w.allow.cua))
    }

    /// 辅助功能有用：主机允许远程操作或 computer use
    pub fn needs_access(&self) -> bool {
        self.welcome
            .as_ref()
            .is_some_and(|w| w.allow.control || w.allow.cua)
    }

    /// 权限引导还差几步（0 就不显示卡片）：没开的权限，加上要不要重启服务。
    pub fn guide_steps(&self) -> usize {
        let Some(p) = self.perms.filter(|_| self.online() && self.wants_desktop()) else {
            return 0;
        };
        usize::from(!p.screen)
            + usize::from(self.needs_access() && !p.access)
            + usize::from(self.guide.restart)
    }

    /// 卡片上要不要「重启主机服务」：屏幕录制刚开了，或者点过打开设置（系统可能要重启才认）
    pub fn guide_offers_restart(&self) -> bool {
        self.guide.restart || (self.guide.opened_screen && self.perms.is_some_and(|p| !p.screen))
    }

    /// 主机答了权限：记下哪些没开（卡片上留一行），屏幕录制从没开变成开了就要重启服务。
    /// 返回屏幕录制是不是刚开。
    pub fn set_perms(&mut self, p: Perms) -> bool {
        let was = self.perms;
        if !p.screen {
            self.guide.screen = true;
        }
        if !p.access && self.needs_access() {
            self.guide.access = true;
        }
        let turned_on = was.is_some_and(|w| !w.screen) && p.screen;
        if turned_on {
            self.guide.restart = true;
        }
        self.perms = Some(p);
        turned_on
    }

    /// 新连接：权限重新问
    fn reset_perms(&mut self) {
        self.perms = None;
        self.guide = Guide::default();
    }

    pub fn title(&self) -> String {
        host_title(&self.name, &self.addr)
    }

    pub fn online(&self) -> bool {
        self.link == Link::Online
    }

    /// 有远程桌面（主机说有，且列到了显示器）。
    pub fn has_desktop(&self) -> bool {
        self.welcome.as_ref().is_some_and(|w| w.desktop)
    }

    /// 首页「远程桌面」按钮开的那块：主显示器，没有就第一块。
    pub fn primary_display(&self) -> Option<&DisplayInfo> {
        self.displays
            .iter()
            .find(|d| d.primary)
            .or_else(|| self.displays.first())
    }

    /// 快照、测试用：直接设成在线的样子（没有连接）。
    pub fn set_online_for_test(
        &mut self,
        welcome: Welcome,
        rtt_ms: f64,
        sessions: Vec<SessionInfo>,
        displays: Vec<DisplayInfo>,
        launchers: Vec<Launcher>,
    ) {
        self.link = Link::Online;
        self.welcome = Some(welcome);
        self.rtt_ms = rtt_ms;
        self.sessions = sessions;
        self.displays = displays;
        self.launchers = launchers;
    }

    pub fn set_failed_for_test(&mut self, error: &str) {
        self.link = Link::Failed;
        self.error = error.to_string();
    }

    /// 快照用：连不上、第 attempt 次之后等着重试。
    pub fn set_reconnecting_for_test(&mut self, error: &str, attempt: u32) {
        self.link = Link::Reconnecting;
        self.error = error.to_string();
        self.retry = Retry {
            attempt,
            due: Some(Instant::now() + crate::backoff::delay(attempt.saturating_sub(1))),
        };
    }

    /// 连接中或等着重连。
    pub fn reconnecting(&self) -> bool {
        matches!(self.link, Link::Connecting | Link::Reconnecting) && self.retry.attempt > 0
    }
}

/// 这个错误重试有没有用：主机拒绝（token 不对、版本不对）、证书指纹变了，重试也一样。
pub fn retryable(error: &str) -> bool {
    !(error.contains("主机拒绝")
        || error.contains("证书指纹变了")
        || error.contains("Hello 回应不对"))
}

/// 右边显示什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Home,
    /// 一个终端（TerminalView 的实体编号）
    Term(EntityId),
    Desk,
    /// 网络页：本机入网、节点、邀请、出口节点、命令行
    Network,
}

/// 网络页的状态（本机 lolinet）。
#[derive(Default)]
pub struct NetState {
    /// 没有就什么都不跑（测试、快照没设假数据的时候）
    backend: Option<Arc<dyn Lolinet>>,
    /// 最近一次读到的；还没读过是 None
    pub status: Option<NetStatus>,
    loading: bool,
    /// 正在做的事（要管理员密码的那些）
    pub job: Option<NetJob>,
    /// 上一件事的结果：成功没有、输出
    pub output: Option<(NetJob, bool, String)>,
}

/// 网络页上要管理员权限的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetJob {
    Join(JoinTarget),
    Exit(bool),
    InstallCli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

/// 发给窗口的事：弹一条通知。
#[derive(Debug, Clone)]
pub enum ModelEvent {
    Toast(ToastKind, String),
    /// 切到了这个终端：把键盘给它
    FocusTerm(EntityId),
}

pub struct Model {
    pub hosts: Vec<HostState>,
    pub selection: Selection,
    /// 这个窗口里开着的终端（切走了也接着）
    pub terms: Vec<Entity<TerminalView>>,
    pub desk: Option<Entity<DeskPane>>,
    pub prefs: Prefs,
    pub net: NetState,
    /// 记住的主机文件；测试里没有
    store: Option<Hosts>,
    ticker: Option<Task<()>>,
    /// 后台任务（结束会话时临时开的流）
    background: Vec<Task<()>>,
}

impl EventEmitter<ModelEvent> for Model {}

pub fn client_name() -> String {
    format!("lolidesk-app {}", crate::VERSION)
}

impl Model {
    /// 读记住的主机和偏好。
    pub fn load() -> Self {
        let store = Hosts::load().ok();
        let mut m = Self::empty(Prefs::load());
        if let Some(store) = &store {
            for h in store.list() {
                m.hosts
                    .push(HostState::new(h.addr, &h.name, &h.fingerprint, &h.token));
            }
        }
        m.store = store;
        m.net.backend = Some(Arc::new(lolinet::System));
        m.sort();
        m
    }

    /// 什么都没有（测试、快照）。
    pub fn empty(prefs: Prefs) -> Self {
        Self {
            hosts: Vec::new(),
            selection: Selection::Home,
            terms: Vec::new(),
            desk: None,
            prefs,
            net: NetState::default(),
            store: None,
            ticker: None,
            background: Vec::new(),
        }
    }

    fn sort(&mut self) {
        self.hosts.sort_by_key(|h| h.title().to_lowercase());
    }

    /// 开机：连上记住的主机，开始定时检查。
    pub fn start(&mut self, cx: &mut Context<Self>) {
        let addrs: Vec<SocketAddr> = self.hosts.iter().map(|h| h.addr).collect();
        for a in addrs {
            self.connect(a, cx);
        }
        self.refresh_net(cx);
        self.ticker = Some(cx.spawn(async move |this, cx| {
            let mut n: u64 = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                n += 1;
                if this.update(cx, |m, cx| m.tick(n, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn host(&self, addr: SocketAddr) -> Option<&HostState> {
        self.hosts.iter().find(|h| h.addr == addr)
    }

    pub fn host_mut(&mut self, addr: SocketAddr) -> Option<&mut HostState> {
        self.hosts.iter_mut().find(|h| h.addr == addr)
    }

    fn host_gen_mut(&mut self, addr: SocketAddr, epoch: u64) -> Option<&mut HostState> {
        self.host_mut(addr).filter(|h| h.epoch == epoch)
    }

    pub fn online_count(&self) -> usize {
        self.hosts.iter().filter(|h| h.online()).count()
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>, cx: &mut Context<Self>) {
        cx.emit(ModelEvent::Toast(kind, text.into()));
    }

    // ------------------------------------------------------------ 连接

    /// 用户点了「连接」：要保持连着，退避从头算，马上连。
    pub fn connect_now(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        if let Some(h) = self.host_mut(addr) {
            h.wanted = true;
            h.retry.reset();
        }
        self.connect(addr, cx);
    }

    /// 在后台连一台记住的主机（带着记住的指纹和 token）。
    pub fn connect(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let Some(h) = self.host_mut(addr) else {
            return;
        };
        if matches!(h.link, Link::Connecting | Link::Online) {
            return;
        }
        h.retry.due = None;
        h.epoch += 1;
        h.error.clear();
        let epoch = h.epoch;
        let Some(fp) = hex::decode(&h.fingerprint)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
        else {
            h.link = Link::Failed;
            h.error = "记住的证书指纹坏了：忘掉这台主机再重新添加".into();
            cx.notify();
            return;
        };
        h.link = Link::Connecting;
        let token = h.token.clone();
        let name = client_name();
        cx.spawn(async move |this, cx| {
            let r = runtime::run(async move { net::connect(addr, &token, Some(fp), &name).await })
                .await;
            let _ = this.update(cx, |m, cx| m.on_connected(addr, epoch, r, cx));
        })
        .detach();
        cx.notify();
    }

    fn on_connected(
        &mut self,
        addr: SocketAddr,
        epoch: u64,
        r: anyhow::Result<(Arc<Client>, [u8; 32])>,
        cx: &mut Context<Self>,
    ) {
        let Some(h) = self.host_gen_mut(addr, epoch) else {
            if let Ok((client, _)) = r {
                client.conn.close(0u32.into(), b"stale");
            }
            return;
        };
        match r {
            Ok((client, _fp)) => {
                h.link = Link::Online;
                h.retry.reset();
                h.error.clear();
                h.reset_perms();
                h.rtt_ms = client.rtt().as_secs_f64() * 1000.0;
                h.welcome = Some(client.welcome.clone());
                h.expanded = true;
                let new_name = client.welcome.host.clone();
                h.client = Some(client.clone());
                if !new_name.is_empty() && new_name != h.name {
                    h.name = new_name;
                    self.persist(addr);
                }
                self.refresh(addr, true, cx);
                self.resume_host(addr, client, cx);
            }
            Err(e) => {
                h.error = error_text(&e);
                if h.wanted && retryable(&h.error) {
                    h.link = Link::Reconnecting;
                    h.retry.schedule(Instant::now());
                } else {
                    h.link = Link::Failed;
                    h.retry.reset();
                }
            }
        }
        cx.notify();
    }

    /// 连接断了（健康检查、或者流上发现的）：关掉连接，终端和桌面挂起，排重连。
    pub fn connection_lost(&mut self, addr: SocketAddr, why: &str, cx: &mut Context<Self>) {
        let Some(h) = self.host_mut(addr) else { return };
        h.epoch += 1;
        if let Some(c) = h.client.take() {
            c.conn.close(0u32.into(), b"lost");
        }
        h.error = format!("连接断了：{why}");
        h.reset_perms();
        if h.wanted {
            h.link = Link::Reconnecting;
            h.retry.schedule(Instant::now());
        } else {
            h.link = Link::Offline;
        }
        let sessions: Vec<Entity<TermSession>> = self
            .terms
            .iter()
            .map(|t| t.read(cx).session.clone())
            .filter(|s| s.read(cx).host == addr)
            .collect();
        for s in sessions {
            s.update(cx, |s, cx| s.suspend(cx));
        }
        if let Some(d) = self.desk.clone()
            && d.read(cx).host == addr
        {
            d.update(cx, |d, cx| d.suspend(cx));
        }
        cx.notify();
    }

    /// 看一台主机的连接还在不在；断了就按断了处理。
    pub fn check_host(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let why = self
            .host(addr)
            .and_then(|h| h.client.as_ref())
            .and_then(|c| c.conn.close_reason())
            .map(|r| r.to_string());
        if let Some(why) = why {
            self.connection_lost(addr, &why, cx);
        }
    }

    /// 重连上了：挂起的终端用同一个会话编号重接，挂起的远程桌面重新开始。
    fn resume_host(&mut self, addr: SocketAddr, client: Arc<Client>, cx: &mut Context<Self>) {
        let link: Rc<dyn TermLink> = Rc::new(ClientLink(client.clone()));
        self.resume_terms(addr, link, cx);
        if let Some(d) = self.desk.clone()
            && d.read(cx).host == addr
            && d.read(cx).suspended
        {
            let info = d.read(cx).display.clone();
            let video = self.video_for(&info);
            d.update(cx, |d, cx| d.resume(client, video, cx));
        }
    }

    /// 挂起（等重连）的终端重新接上；返回重接了几个。
    pub fn resume_terms(
        &mut self,
        addr: SocketAddr,
        link: Rc<dyn TermLink>,
        cx: &mut Context<Self>,
    ) -> usize {
        let sessions: Vec<Entity<TermSession>> = self
            .terms
            .iter()
            .map(|t| t.read(cx).session.clone())
            .filter(|s| {
                let s = s.read(cx);
                s.host == addr && s.phase == TermPhase::Reconnecting
            })
            .collect();
        let mut n = 0;
        for s in sessions {
            if s.update(cx, |s, cx| s.reattach(link.clone(), cx)) {
                n += 1;
            }
        }
        n
    }

    /// 窗口回到前台：等着重连的主机马上试。
    pub fn retry_now(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let due: Vec<SocketAddr> = self
            .hosts
            .iter_mut()
            .filter(|h| h.link == Link::Reconnecting && h.wanted)
            .map(|h| {
                h.retry.hurry(now);
                h.addr
            })
            .collect();
        for a in due {
            self.connect(a, cx);
        }
    }

    /// 添加主机成功：记住它（地址、指纹、token、主机名），状态设成在线。
    pub fn adopt(
        &mut self,
        addr: SocketAddr,
        client: Arc<Client>,
        fingerprint: [u8; 32],
        token: &str,
        cx: &mut Context<Self>,
    ) {
        let fp = hex::encode(fingerprint);
        let welcome = client.welcome.clone();
        if self.host(addr).is_none() {
            self.hosts
                .push(HostState::new(addr, &welcome.host, &fp, token));
        }
        if let Some(h) = self.host_mut(addr) {
            h.epoch += 1;
            if let Some(old) = h.client.take() {
                old.conn.close(0u32.into(), b"replaced");
            }
            h.name = welcome.host.clone();
            h.fingerprint = fp;
            h.token = token.to_string();
            h.link = Link::Online;
            h.wanted = true;
            h.retry.reset();
            h.error.clear();
            h.reset_perms();
            h.rtt_ms = client.rtt().as_secs_f64() * 1000.0;
            h.welcome = Some(welcome.clone());
            h.client = Some(client);
            h.expanded = true;
        }
        self.persist(addr);
        self.sort();
        self.refresh(addr, true, cx);
        self.toast(
            ToastKind::Success,
            format!("已连上 {}", host_title(&welcome.host, &addr)),
            cx,
        );
        cx.notify();
    }

    /// 把一台主机写进记住的主机文件。
    fn persist(&mut self, addr: SocketAddr) {
        let Some(h) = self.hosts.iter().find(|h| h.addr == addr) else {
            return;
        };
        let (fp, token, name) = (h.fingerprint.clone(), h.token.clone(), h.name.clone());
        if let Some(store) = &mut self.store {
            store.remember(addr, &fp, &token, &name);
            if let Err(e) = store.save() {
                tracing::warn!("存主机列表：{e:#}");
            }
        }
    }

    /// 断开一台主机：关掉它的终端和桌面。
    pub fn disconnect(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let mine: Vec<Entity<TerminalView>> = self
            .terms
            .iter()
            .filter(|t| t.read(cx).host(cx) == addr)
            .cloned()
            .collect();
        for t in mine {
            self.close_term(t.entity_id(), cx);
        }
        if self.desk.as_ref().is_some_and(|d| d.read(cx).host == addr) {
            self.stop_desk(cx);
        }
        if let Some(h) = self.host_mut(addr) {
            h.epoch += 1;
            if let Some(c) = h.client.take() {
                c.conn.close(0u32.into(), b"bye");
            }
            h.wanted = false;
            h.retry.reset();
            h.link = Link::Offline;
            h.sessions.clear();
            h.displays.clear();
            h.displays_error = None;
            h.launchers.clear();
            h.reset_perms();
        }
        cx.notify();
    }

    /// 忘掉一台主机（指纹和 token）。
    pub fn forget(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        self.disconnect(addr, cx);
        self.hosts.retain(|h| h.addr != addr);
        if let Some(store) = &mut self.store {
            store.forget(&addr);
            if let Err(e) = store.save() {
                tracing::warn!("存主机列表：{e:#}");
            }
        }
        cx.notify();
    }

    pub fn toggle_expanded(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        if let Some(h) = self.host_mut(addr) {
            h.expanded = !h.expanded;
            cx.notify();
        }
    }

    /// 在后台拉会话列表；`all` 时连显示器和 agent 一起拉。
    pub fn refresh(&mut self, addr: SocketAddr, all: bool, cx: &mut Context<Self>) {
        let Some(h) = self.host(addr) else {
            return;
        };
        let (Some(client), true) = (h.client.clone(), h.online()) else {
            return;
        };
        let epoch = h.epoch;
        let desktop = h.has_desktop();
        if all && desktop {
            self.refresh_perms(addr, cx);
        }
        cx.spawn(async move |this, cx| {
            let c = client.clone();
            let sessions = runtime::run(async move {
                match c.control(ControlReq::ListSessions).await? {
                    ControlResp::Sessions(s) => Ok(s),
                    ControlResp::Error(e) => bail!(e),
                    other => bail!("回应不对：{other:?}"),
                }
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                if let (Some(h), Ok(mut s)) = (m.host_gen_mut(addr, epoch), sessions) {
                    s.sort_by_key(|s| s.id);
                    if h.sessions.len() != s.len()
                        || h.sessions.iter().zip(&s).any(|(a, b)| !same_session(a, b))
                    {
                        h.sessions = s;
                        cx.notify();
                    }
                }
            });
            if !all {
                return;
            }
            if desktop {
                let c = client.clone();
                let displays = runtime::run(async move {
                    match c.control(ControlReq::ListDisplays).await? {
                        ControlResp::Displays(d) => Ok(d),
                        ControlResp::Error(e) => bail!(e),
                        other => bail!("回应不对：{other:?}"),
                    }
                })
                .await;
                let _ = this.update(cx, |m, cx| {
                    if let Some(h) = m.host_gen_mut(addr, epoch) {
                        match displays {
                            Ok(d) => {
                                h.displays = d;
                                h.displays_error = None;
                            }
                            Err(e) => {
                                h.displays.clear();
                                h.displays_error = Some(error_text(&e));
                            }
                        }
                        cx.notify();
                    }
                });
            }
            let c = client.clone();
            let launchers = runtime::run(async move {
                match c.control(ControlReq::ListLaunchers).await? {
                    ControlResp::Launchers(l) => Ok(l),
                    ControlResp::Error(e) => bail!(e),
                    other => bail!("回应不对：{other:?}"),
                }
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                if let (Some(h), Ok(l)) = (m.host_gen_mut(addr, epoch), launchers) {
                    h.launchers = l;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 每秒一次：到点的重连；每 2 秒看连接、延迟，每 4 秒刷会话。
    fn tick(&mut self, n: u64, cx: &mut Context<Self>) {
        let now = Instant::now();
        let due: Vec<SocketAddr> = self
            .hosts
            .iter()
            .filter(|h| h.link == Link::Reconnecting && h.wanted && h.retry.is_due(now))
            .map(|h| h.addr)
            .collect();
        for a in due {
            self.connect(a, cx);
        }
        if self.hosts.iter().any(|h| h.link == Link::Reconnecting) {
            // 侧栏上的「N 秒后」跟着走
            cx.notify();
        }
        // 网络页开着时每 3 秒读一次本机 lolinet；没开着时 30 秒一次（侧栏上的在线数）
        let net_every = if self.selection == Selection::Network {
            3
        } else {
            30
        };
        if n.is_multiple_of(net_every) {
            self.refresh_net(cx);
        }
        if !n.is_multiple_of(2) {
            return;
        }
        // 权限引导卡片显示着：每 2 秒问一次，开了就变绿
        let guiding: Vec<SocketAddr> = self
            .hosts
            .iter()
            .filter(|h| h.guide_steps() > 0 && !h.guide.restarting)
            .map(|h| h.addr)
            .collect();
        for a in guiding {
            self.refresh_perms(a, cx);
        }
        let mut dead = Vec::new();
        let mut changed = false;
        for h in &mut self.hosts {
            let Some(c) = &h.client else { continue };
            if let Some(why) = c.conn.close_reason() {
                dead.push((h.addr, why.to_string()));
                continue;
            }
            let rtt = c.rtt().as_secs_f64() * 1000.0;
            if (rtt - h.rtt_ms).abs() >= 0.5 {
                h.rtt_ms = rtt;
                changed = true;
            }
        }
        for (addr, why) in dead {
            self.connection_lost(addr, &why, cx);
            changed = true;
        }
        if n.is_multiple_of(4) {
            let online: Vec<SocketAddr> = self
                .hosts
                .iter()
                .filter(|h| h.online())
                .map(|h| h.addr)
                .collect();
            for a in online {
                self.refresh(a, false, cx);
            }
        }
        if changed {
            cx.notify();
        }
    }

    // ------------------------------------------------------------ 选中

    pub fn select(&mut self, sel: Selection, cx: &mut Context<Self>) {
        if self.selection != sel {
            self.selection = sel;
            cx.notify();
        }
        match sel {
            Selection::Term(id) => cx.emit(ModelEvent::FocusTerm(id)),
            Selection::Network => self.refresh_net(cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------ 权限

    /// 问主机两个权限开了没有（主机没有远程桌面会回 Error：不再问，卡片不出现）。
    pub fn refresh_perms(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let Some(h) = self.host(addr) else { return };
        let (Some(client), true) = (h.client.clone(), h.online()) else {
            return;
        };
        let epoch = h.epoch;
        cx.spawn(async move |this, cx| {
            let r = runtime::run(async move {
                match client.control(ControlReq::Permissions).await? {
                    ControlResp::Permissions {
                        screen_recording,
                        accessibility,
                    } => Ok(Perms {
                        screen: screen_recording,
                        access: accessibility,
                    }),
                    ControlResp::Error(e) => bail!(e),
                    other => bail!("回应不对：{other:?}"),
                }
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                let Some(h) = m.host_gen_mut(addr, epoch) else {
                    return;
                };
                let Ok(p) = r else { return };
                if h.perms == Some(p) {
                    return;
                }
                let turned_on = h.set_perms(p);
                cx.notify();
                // 屏幕录制刚开：显示器可能列得出来了
                if turned_on {
                    m.refresh(addr, false, cx);
                    m.refresh_displays(addr, cx);
                }
            });
        })
        .detach();
    }

    /// 只重新列显示器（权限刚开时）。
    fn refresh_displays(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let Some(h) = self.host(addr) else { return };
        let Some(client) = h.client.clone() else {
            return;
        };
        let epoch = h.epoch;
        cx.spawn(async move |this, cx| {
            let r = runtime::run(async move {
                match client.control(ControlReq::ListDisplays).await? {
                    ControlResp::Displays(d) => Ok(d),
                    ControlResp::Error(e) => bail!(e),
                    other => bail!("回应不对：{other:?}"),
                }
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                if let (Some(h), Ok(d)) = (m.host_gen_mut(addr, epoch), r) {
                    h.displays = d;
                    h.displays_error = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 「在主机上打开设置」：主机弹授权框、打开设置页；主机就是这台 Mac 的话，这边也打开。
    pub fn open_permission(
        &mut self,
        addr: SocketAddr,
        kind: PermissionKind,
        cx: &mut Context<Self>,
    ) {
        let Some(h) = self.host_mut(addr) else { return };
        let Some(client) = h.client.clone() else {
            return;
        };
        if kind == PermissionKind::ScreenRecording {
            h.guide.opened_screen = true;
        }
        let title = h.title();
        if is_this_mac(client.addr.ip()) {
            let _ = std::process::Command::new("/usr/bin/open")
                .arg(kind.settings_url())
                .spawn();
        }
        cx.spawn(async move |this, cx| {
            let r = runtime::run(async move {
                match client.control(ControlReq::OpenPermission(kind)).await? {
                    ControlResp::Ok => Ok(()),
                    ControlResp::Error(e) => bail!(e),
                    other => bail!("回应不对：{other:?}"),
                }
            })
            .await;
            let _ = this.update(cx, |m, cx| match r {
                Ok(()) => m.toast(
                    ToastKind::Info,
                    format!("已在 {title} 上打开系统设置：给 lolidesk 打开"),
                    cx,
                ),
                Err(e) => m.toast(ToastKind::Error, error_text(&e), cx),
            });
        })
        .detach();
        cx.notify();
    }

    /// 「重启主机服务」：主机回了话就自己重启，连接会断，之后自动重连。
    pub fn restart_host(&mut self, addr: SocketAddr, cx: &mut Context<Self>) {
        let Some(h) = self.host_mut(addr) else { return };
        let Some(client) = h.client.clone() else {
            return;
        };
        h.guide.restarting = true;
        let epoch = h.epoch;
        cx.spawn(async move |this, cx| {
            let r = runtime::run(async move {
                match client.control(ControlReq::RestartHost).await? {
                    ControlResp::Ok => Ok(()),
                    ControlResp::Error(e) => bail!(e),
                    other => bail!("回应不对：{other:?}"),
                }
            })
            .await;
            let failed = r.is_err();
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = r {
                    if let Some(h) = m.host_gen_mut(addr, epoch) {
                        h.guide.restarting = false;
                    }
                    m.toast(ToastKind::Error, error_text(&e), cx);
                }
                cx.notify();
            });
            if failed {
                return;
            }
            // 主机说好了，但 20 秒后还是这条连接：没重启成，按钮还给用户
            cx.background_executor()
                .timer(Duration::from_secs(20))
                .await;
            let _ = this.update(cx, |m, cx| {
                if let Some(h) = m.host_gen_mut(addr, epoch)
                    && h.guide.restarting
                {
                    h.guide.restarting = false;
                    m.toast(
                        ToastKind::Error,
                        "主机服务没有重启：在主机上跑 lolidesk host install",
                        cx,
                    );
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ 网络（本机 lolinet）

    /// 换掉读本机 lolinet 的那一层（测试用假的）。
    pub fn set_lolinet(&mut self, backend: Arc<dyn Lolinet>) {
        self.net.backend = Some(backend);
    }

    pub fn lolinet(&self) -> Option<Arc<dyn Lolinet>> {
        self.net.backend.clone()
    }

    /// 在后台读一次本机的入网情况、节点、出口节点、命令行。
    pub fn refresh_net(&mut self, cx: &mut Context<Self>) {
        let Some(backend) = self.net.backend.clone() else {
            return;
        };
        if self.net.loading {
            return;
        }
        self.net.loading = true;
        let task = cx.background_spawn(async move { backend.status() });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |m, cx| {
                m.net.loading = false;
                if m.net.status.as_ref() != Some(&status) {
                    m.net.status = Some(status);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 要管理员密码的事（入网、出口节点、装命令行）：在后台跑，跑完记下输出、再读一次状态。
    pub fn run_net_job(&mut self, job: NetJob, cx: &mut Context<Self>) {
        let Some(backend) = self.net.backend.clone() else {
            return;
        };
        if self.net.job.is_some() {
            return;
        }
        self.net.job = Some(job.clone());
        self.net.output = None;
        let j = job.clone();
        let task = cx.background_spawn(async move {
            match &j {
                NetJob::Join(t) => backend.join(t),
                NetJob::Exit(on) => backend.set_exit(*on),
                NetJob::InstallCli => backend.install_cli(),
            }
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |m, cx| {
                m.net.job = None;
                let (ok, text) = match r {
                    Ok(out) => (true, out),
                    Err(e) => (false, error_text(&e)),
                };
                let note = match (&job, ok) {
                    (NetJob::Join(_), true) => Some((ToastKind::Success, "已加入网络".to_string())),
                    (NetJob::Exit(true), true) => Some((ToastKind::Success, "出口节点开了".into())),
                    (NetJob::Exit(false), true) => Some((ToastKind::Info, "出口节点关了".into())),
                    (NetJob::InstallCli, true) => {
                        Some((ToastKind::Success, "lolidesk 装到命令行了".into()))
                    }
                    (_, false) => Some((ToastKind::Error, first_line(&text))),
                };
                m.net.output = Some((job, ok, text));
                if let Some((k, t)) = note {
                    m.toast(k, t, cx);
                }
                cx.notify();
                m.refresh_net(cx);
            });
        })
        .detach();
        cx.notify();
    }

    // ------------------------------------------------------------ 终端

    /// 开（或切到）一个终端：Attach 已经开着的就直接切过去。
    pub fn open_term(&mut self, addr: SocketAddr, start: TermStart, cx: &mut Context<Self>) {
        if let TermStart::Attach { id } = start
            && let Some(t) = self
                .terms
                .iter()
                .find(|t| {
                    let v = t.read(cx);
                    v.host(cx) == addr && v.session_id(cx) == Some(id) && !v.is_ended(cx)
                })
                .cloned()
        {
            self.select(Selection::Term(t.entity_id()), cx);
            return;
        }
        let Some(h) = self.host(addr) else { return };
        let Some(client) = h.client.clone() else {
            self.toast(ToastKind::Error, format!("{} 没连上", h.title()), cx);
            return;
        };
        // 同一会话以前开过、断了的视图：换成新的
        if let TermStart::Attach { id } = start {
            let stale: Vec<EntityId> = self
                .terms
                .iter()
                .filter(|t| {
                    let v = t.read(cx);
                    v.host(cx) == addr && v.session_id(cx) == Some(id)
                })
                .map(|t| t.entity_id())
                .collect();
            self.terms.retain(|t| !stale.contains(&t.entity_id()));
        }
        let link: Rc<dyn TermLink> = Rc::new(ClientLink(client));
        let session = cx.new(|cx| TermSession::connect(link, addr, start, cx));
        self.add_term(session, cx);
    }

    /// 把一个会话放进窗口、切过去（测试里用假连接的会话也走这里）。
    pub fn add_term(&mut self, session: Entity<TermSession>, cx: &mut Context<Self>) -> EntityId {
        self.watch_session(&session, cx);
        let model = cx.weak_entity();
        let view = cx.new(|cx| TerminalView::new(session, model, cx));
        let id = view.entity_id();
        self.terms.push(view);
        self.selection = Selection::Term(id);
        cx.emit(ModelEvent::FocusTerm(id));
        cx.notify();
        id
    }

    /// 横幅上的「重新接入」：断开了、程序退出了的会话重新接上。
    pub fn reopen_term(&mut self, view: EntityId, cx: &mut Context<Self>) {
        let Some(t) = self.terms.iter().find(|t| t.entity_id() == view).cloned() else {
            return;
        };
        let session = t.read(cx).session.clone();
        let addr = session.read(cx).host;
        let Some(client) = self.host(addr).and_then(|h| h.client.clone()) else {
            self.toast(ToastKind::Error, "主机还没连上", cx);
            return;
        };
        let link: Rc<dyn TermLink> = Rc::new(ClientLink(client));
        session.update(cx, |s, cx| s.reopen(link, cx));
        cx.emit(ModelEvent::FocusTerm(view));
    }

    /// 会话的事：新编号、退出、通知，刷新侧栏或弹通知。
    pub fn watch_session(&mut self, session: &Entity<TermSession>, cx: &mut Context<Self>) {
        cx.subscribe(session, |m, session, ev, cx| match ev {
            TermSessionEvent::Opened(_) => {
                let addr = session.read(cx).host;
                m.refresh(addr, false, cx);
                cx.notify();
            }
            TermSessionEvent::Exited(_) | TermSessionEvent::Closed | TermSessionEvent::Title(_) => {
                cx.notify()
            }
            TermSessionEvent::Disconnected => {
                let addr = session.read(cx).host;
                m.check_host(addr, cx);
                cx.notify();
            }
            TermSessionEvent::Notify { title, body } => {
                let text = match (title.trim(), body.trim()) {
                    ("", b) => b.to_string(),
                    (t, "") => t.to_string(),
                    (t, b) => format!("{t}：{b}"),
                };
                if !text.is_empty() {
                    m.toast(ToastKind::Info, text, cx);
                }
            }
            TermSessionEvent::Error(e) => {
                let e = e.clone();
                m.toast(ToastKind::Error, e, cx);
            }
            TermSessionEvent::Output | TermSessionEvent::Bell => {}
        })
        .detach();
    }

    /// 断开一个终端（会话留在主机上）。
    pub fn close_term(&mut self, view: EntityId, cx: &mut Context<Self>) {
        if let Some(t) = self.terms.iter().find(|t| t.entity_id() == view).cloned() {
            let session = t.read(cx).session.clone();
            session.update(cx, |s, _| s.close());
        }
        self.terms.retain(|t| t.entity_id() != view);
        if self.selection == Selection::Term(view) {
            self.selection = Selection::Home;
        }
        cx.notify();
    }

    /// 结束会话：开着的直接 Kill；没开的临时接上再 Kill。
    pub fn kill_session(&mut self, addr: SocketAddr, id: u32, cx: &mut Context<Self>) {
        if let Some(t) = self
            .terms
            .iter()
            .find(|t| {
                let v = t.read(cx);
                v.host(cx) == addr && v.session_id(cx) == Some(id)
            })
            .cloned()
        {
            let vid = t.entity_id();
            let session = t.read(cx).session.clone();
            session.update(cx, |s, _| s.kill());
            self.close_after_kill(vid, addr, cx);
            return;
        }
        let Some(client) = self.host(addr).and_then(|h| h.client.clone()) else {
            return;
        };
        let (stream, mut events) = TermStream::open(
            client,
            lolidesk_proto::TermReq::Attach {
                id,
                size: crate::term::DEFAULT_SIZE,
            },
        );
        stream.send(lolidesk_proto::TermReq::Kill);
        self.background.retain(|t| !t.is_ready());
        self.background.push(cx.spawn(async move |this, cx| {
            // 主机挂断程序后关流；流结束前一直拿着它
            let _keep = stream;
            let wait = async { while let Some(Some(_)) = events.recv().await {} };
            let timeout = cx.background_executor().timer(Duration::from_secs(5));
            futures_select(wait, timeout).await;
            let _ = this.update(cx, |m, cx| m.refresh(addr, false, cx));
        }));
    }

    /// 结束了开着的终端：等一下主机关流，再从窗口里去掉。
    pub fn kill_term(&mut self, view: EntityId, cx: &mut Context<Self>) {
        let Some(t) = self.terms.iter().find(|t| t.entity_id() == view).cloned() else {
            return;
        };
        let addr = t.read(cx).host(cx);
        let session = t.read(cx).session.clone();
        session.update(cx, |s, _| s.kill());
        self.close_after_kill(view, addr, cx);
    }

    fn close_after_kill(&mut self, view: EntityId, addr: SocketAddr, cx: &mut Context<Self>) {
        self.background.retain(|t| !t.is_ready());
        self.background.push(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |m, cx| {
                m.close_term(view, cx);
                m.refresh(addr, false, cx);
            });
        }));
    }

    // ------------------------------------------------------------ 远程桌面

    /// 开一块显示器的远程桌面（同一时间只开一个）。
    pub fn start_desk(
        &mut self,
        addr: SocketAddr,
        display: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(d) = &self.desk {
            let d = d.read(cx);
            if d.host == addr && d.display.id == display && !d.is_stopped() {
                self.select(Selection::Desk, cx);
                return;
            }
        }
        self.stop_desk(cx);
        let Some(h) = self.host(addr) else { return };
        let Some(client) = h.client.clone() else {
            self.toast(ToastKind::Error, format!("{} 没连上", h.title()), cx);
            return;
        };
        let Some(info) = h.displays.iter().find(|d| d.id == display).cloned() else {
            return;
        };
        let title = h.title();
        let video = self.video_for(&info);
        let model = cx.weak_entity();
        let pane =
            cx.new(|cx| DeskPane::new(model, addr, title, info, Some((client, video)), window, cx));
        self.desk = Some(pane);
        self.selection = Selection::Desk;
        cx.notify();
    }

    /// 偏好里的画面参数，帧率夹到这块显示器的刷新率以内。
    pub fn video_for(&self, d: &DisplayInfo) -> VideoSettings {
        let mut v = self.prefs.video();
        v.fps = cap_fps(v.fps, d.refresh);
        v
    }

    pub fn stop_desk(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.desk.take() {
            d.update(cx, |d, cx| d.stop(cx));
        }
        if self.selection == Selection::Desk {
            self.selection = Selection::Home;
        }
        cx.notify();
    }

    /// 换分辨率 / 帧率 / 画质：记下来，开着的远程桌面跟着换。
    pub fn set_video(
        &mut self,
        long_edge: Option<u32>,
        fps: Option<u8>,
        quality: Option<lolidesk_proto::Quality>,
        cx: &mut Context<Self>,
    ) {
        if let Some(e) = long_edge {
            self.prefs.long_edge = e;
        }
        if let Some(f) = fps {
            self.prefs.fps = f;
        }
        if let Some(q) = quality {
            self.prefs.quality = labels::quality_index(q);
        }
        self.prefs.save();
        if let Some(d) = self.desk.clone() {
            let info = d.read(cx).display.clone();
            let video = self.video_for(&info);
            d.update(cx, |d, cx| d.configure(video, cx));
        }
        cx.notify();
    }

    /// 终端字号（放大 / 缩小），记下来。
    pub fn bump_font(&mut self, delta: f32, cx: &mut Context<Self>) {
        let next =
            (self.prefs.font_size + delta).clamp(crate::prefs::FONT_MIN, crate::prefs::FONT_MAX);
        if next != self.prefs.font_size {
            self.prefs.font_size = next;
            self.prefs.save();
            cx.notify();
        }
    }

    // ------------------------------------------------------------ 添加主机

    /// 只握手拿指纹（不发 token）。
    pub fn probe_fingerprint(
        input: String,
        cx: &mut gpui_kit::App,
    ) -> Task<anyhow::Result<(SocketAddr, [u8; 32])>> {
        cx.spawn(async move |_| {
            runtime::run(async move {
                let addr = tokio::task::spawn_blocking(move || net::resolve(&input))
                    .await
                    .context("解析地址")??;
                let fp = net::fingerprint(addr).await?;
                Ok((addr, fp))
            })
            .await
        })
    }

    /// 带着核对过的指纹连上、发 token。
    pub fn trust_and_connect(
        addr: SocketAddr,
        token: String,
        fp: [u8; 32],
        cx: &mut gpui_kit::App,
    ) -> Task<anyhow::Result<Arc<Client>>> {
        let name = client_name();
        cx.spawn(async move |_| {
            runtime::run(async move {
                let (client, _) = net::connect(addr, &token, Some(fp), &name).await?;
                Ok(client)
            })
            .await
        })
    }
}

/// 通知里只放一行（整段输出在网络页上）。
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("失败了");
    let mut s: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        s.push('…');
    }
    s
}

/// 这个地址是不是这台 Mac 自己（回环，或者本机网卡上的地址）。
pub fn is_this_mac(ip: IpAddr) -> bool {
    ip.is_loopback()
        || if_addrs::get_if_addrs()
            .map(|list| list.iter().any(|i| i.ip() == ip))
            .unwrap_or(false)
}

/// 会话列表的一项变了没有（变了才重画侧栏）。
fn same_session(a: &SessionInfo, b: &SessionInfo) -> bool {
    a.id == b.id
        && a.title == b.title
        && a.command == b.command
        && a.foreground == b.foreground
        && a.cwd == b.cwd
        && a.cols == b.cols
        && a.rows == b.rows
        && a.viewers == b.viewers
        && a.exited == b.exited
}

/// 两个 future 谁先完就算完。
async fn futures_select(
    a: impl std::future::Future<Output = ()>,
    b: impl std::future::Future<Output = ()>,
) {
    use std::pin::pin;
    let mut a = pin!(a);
    let mut b = pin!(b);
    std::future::poll_fn(|cx| {
        if a.as_mut().poll(cx).is_ready() || b.as_mut().poll(cx).is_ready() {
            std::task::Poll::Ready(())
        } else {
            std::task::Poll::Pending
        }
    })
    .await
}
