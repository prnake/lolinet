//! QUIC 服务：每条连接先在控制流上 Hello，之后终端流 / 桌面流 / computer use 流各开各的。
//!
//! 谁在连（[`Role`]）：Hello 带主机 token 的是主人（本机 app、本机命令行）；带 `g1.…` 凭据的是一台配对过的设备
//! （access.rs）。设备能做什么 = 主机开放的 ∧ 它自己的限制，每条流开的时候现算，吊销了的设备什么都不能做，
//! 它连着的连接也马上断开。还没凭据的设备可以发 Pair：带配对码直接换凭据，不带就等主人同意（app 里的框，
//! 主人的 app 没开着就弹系统对话框，见 consent.rs）。

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use lolidesk_proto::{
    self as proto, Allow, ControlReq, ControlResp, DeskEvent, DeskInput, DeskReq, DeskTarget,
    DeviceInfo, Frag, PendingInfo, Role, StreamKind, TermEvent, TermReq, VideoSettings, FRAG_KEY,
};
use quinn::{Connection, RecvStream, SendStream};

use crate::{
    access::{self, Store},
    config, consent, cua, desktop, input, launchers, permissions,
    terminal::Sessions,
};

/// 同时最多几个等着同意的配对请求
const MAX_PENDING: usize = 5;
/// 被拒绝之后，同一个地址多久不能再请求
const DENIED_COOLDOWN: Duration = Duration::from_secs(60);
/// 配对码试错：每个地址 10 分钟里最多错几次
const CODE_FAILS: u32 = 10;
const CODE_WINDOW: Duration = Duration::from_secs(600);
/// 主人的 app 多久之内来问过 Access，就算它看着（不另弹系统对话框）
const OWNER_WATCHING: Duration = Duration::from_secs(8);

pub struct Shared {
    token: String,
    host: String,
    pub sessions: Sessions,
    access: Mutex<Store>,
    /// 分享链接里写的主机地址（网里的地址:端口）
    link_addr: String,
    pending: Mutex<Pending>,
    /// 每台配对设备现在的连接：吊销时断开
    conns: Mutex<HashMap<String, Vec<Connection>>>,
    owner_seen: Mutex<Option<Instant>>,
    /// 每个来源地址：配对码试错次数、最近一次被拒绝
    guard: Mutex<HashMap<IpAddr, Guard>>,
}

#[derive(Default)]
struct Pending {
    next: u64,
    items: HashMap<u64, Waiting>,
}

struct Waiting {
    info: PendingInfo,
    ip: IpAddr,
    tx: Option<tokio::sync::oneshot::Sender<bool>>,
    /// 系统对话框的进程（主人在 app 里答复了就关掉它）
    dialog: Option<u32>,
}

#[derive(Default, Clone, Copy)]
struct Guard {
    fails: u32,
    since: Option<Instant>,
    denied: Option<Instant>,
}

impl Shared {
    pub fn new(token: String, link_addr: String) -> Arc<Self> {
        let path = config::config_dir().join("access.json");
        let access = Store::load(&path).unwrap_or_else(|e| {
            // 文件坏了不覆盖它：这次只认主人，配对过的设备都连不上，修好再重启主机
            tracing::error!(
                "{e:#}：配对过的设备这次都连不上，修好 {} 再重启主机",
                path.display()
            );
            Store::memory()
        });
        Arc::new(Self {
            token,
            host: config::hostname(),
            sessions: Sessions::new(config::config_dir().join("sessions")),
            access: Mutex::new(access),
            link_addr,
            pending: Mutex::new(Pending::default()),
            conns: Mutex::new(HashMap::new()),
            owner_seen: Mutex::new(None),
            guard: Mutex::new(HashMap::new()),
        })
    }

    /// 这个角色现在能做什么：主人是主机开放的全部；设备再和它自己的限制取交集；吊销了的什么都不能做
    fn allow_for(&self, role: &Role) -> Allow {
        let host = config::allow();
        match role {
            Role::Owner => host,
            Role::Device(id) => {
                let a = self.access.lock().unwrap();
                match a.grant(id) {
                    Some(g) => host.and(g.allow.unwrap_or_default()),
                    None => Allow::none(),
                }
            }
        }
    }

    fn save(&self, a: &Store) {
        if let Err(e) = a.save() {
            tracing::warn!("存配对记录：{e:#}");
        }
    }

    fn register(&self, id: &str, conn: Connection) {
        let mut c = self.conns.lock().unwrap();
        let list = c.entry(id.to_string()).or_default();
        list.retain(|x| x.close_reason().is_none());
        list.push(conn);
    }

    fn unregister(&self, id: &str, conn: &Connection) {
        let mut c = self.conns.lock().unwrap();
        if let Some(list) = c.get_mut(id) {
            list.retain(|x| x.stable_id() != conn.stable_id() && x.close_reason().is_none());
            if list.is_empty() {
                c.remove(id);
            }
        }
    }

    fn online(&self, id: &str) -> u32 {
        let c = self.conns.lock().unwrap();
        c.get(id).map_or(0, |l| {
            l.iter().filter(|x| x.close_reason().is_none()).count() as u32
        })
    }

    /// 吊销后：断开这些设备现在的连接
    fn disconnect(&self, ids: &[String]) {
        let mut c = self.conns.lock().unwrap();
        for id in ids {
            for conn in c.remove(id).unwrap_or_default() {
                conn.close(3u32.into(), "已吊销".as_bytes());
            }
        }
    }

    fn owner_watching(&self) -> bool {
        self.owner_seen
            .lock()
            .unwrap()
            .is_some_and(|t| t.elapsed() < OWNER_WATCHING)
    }

    fn access_view(&self, role: &Role) -> proto::AccessInfo {
        if *role == Role::Owner {
            *self.owner_seen.lock().unwrap() = Some(Instant::now());
        }
        let mut v = self
            .access
            .lock()
            .unwrap()
            .view(role, &|id| self.online(id));
        if *role == Role::Owner {
            let p = self.pending.lock().unwrap();
            let mut list: Vec<PendingInfo> = p.items.values().map(|w| w.info.clone()).collect();
            list.sort_by_key(|i| i.id);
            v.pending = list;
        }
        v
    }

    /// 主人答复一个配对请求；没有这个请求（过期了、对方走了）返回 false
    fn decide(&self, id: u64, allow: bool) -> bool {
        let mut p = self.pending.lock().unwrap();
        match p.items.get_mut(&id).and_then(|w| w.tx.take()) {
            Some(tx) => tx.send(allow).is_ok(),
            None => false,
        }
    }

    /// 配对码试错太多、刚被拒绝过：现在不接这个地址的请求
    fn guarded(&self, ip: IpAddr, consent: bool) -> Option<String> {
        let mut g = self.guard.lock().unwrap();
        let e = g.entry(ip).or_default();
        if e.since.is_some_and(|t| t.elapsed() > CODE_WINDOW) {
            e.fails = 0;
            e.since = None;
        }
        if e.fails >= CODE_FAILS {
            return Some("配对码试错太多次了，10 分钟后再试".into());
        }
        if consent && e.denied.is_some_and(|t| t.elapsed() < DENIED_COOLDOWN) {
            return Some("刚被拒绝过，过一分钟再请求".into());
        }
        None
    }

    fn note_fail(&self, ip: IpAddr) {
        let mut g = self.guard.lock().unwrap();
        let e = g.entry(ip).or_default();
        e.fails += 1;
        e.since.get_or_insert_with(Instant::now);
    }

    fn note_denied(&self, ip: IpAddr) {
        self.guard.lock().unwrap().entry(ip).or_default().denied = Some(Instant::now());
    }

    fn paired(&self, grant: String, credential: String) -> ControlResp {
        ControlResp::Paired {
            credential,
            host: self.host.clone(),
            grant,
        }
    }

    /// Pair：带配对码直接换凭据；不带就等主人同意
    async fn pair(
        self: &Arc<Self>,
        conn: &Connection,
        device: DeviceInfo,
        code: Option<String>,
    ) -> ControlResp {
        let device = access::clean_device(device);
        let addr = conn.remote_address();
        let ip = addr.ip().to_canonical();
        if let Some(why) = self.guarded(ip, code.is_none()) {
            return ControlResp::Denied(why);
        }
        let Some(code) = code else {
            return self.consent(conn, device, addr).await;
        };
        let r = {
            let mut a = self.access.lock().unwrap();
            let r = a.redeem(&code, device.clone(), &addr.to_string(), access::now());
            if r.is_ok() {
                self.save(&a);
            }
            r
        };
        match r {
            Ok((grant, credential)) => {
                tracing::info!(%grant, name = %device.name, %addr, "用配对码配对");
                self.paired(grant, credential)
            }
            Err(e) => {
                self.note_fail(ip);
                tracing::info!(name = %device.name, %addr, "配对码不对：{e:#}");
                ControlResp::Denied(format!("{e:#}"))
            }
        }
    }

    async fn consent(
        self: &Arc<Self>,
        conn: &Connection,
        device: DeviceInfo,
        addr: SocketAddr,
    ) -> ControlResp {
        if !config::allow().pair {
            return ControlResp::Denied(
                "这台电脑没开放配对请求：找它的主人要配对码或分享链接".into(),
            );
        }
        let ip = addr.ip().to_canonical();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let id = {
            let mut p = self.pending.lock().unwrap();
            if p.items.values().any(|w| w.ip == ip) {
                return ControlResp::Denied("这台设备已经有一个请求在等对方同意了".into());
            }
            if p.items.len() >= MAX_PENDING {
                return ControlResp::Denied("等着同意的请求太多了，稍后再试".into());
            }
            p.next += 1;
            let id = p.next;
            let info = PendingInfo {
                id,
                device: device.clone(),
                addr: addr.to_string(),
                since: access::now(),
            };
            p.items.insert(
                id,
                Waiting {
                    info,
                    ip,
                    tx: Some(tx),
                    dialog: None,
                },
            );
            id
        };
        tracing::info!(id, name = %device.name, %addr, "配对请求：等主人同意");
        // 主人的 app 最近没来看：在这台电脑上弹系统对话框
        if !self.owner_watching() {
            if let Some((child, kind)) = consent::ask(&device, &addr.to_string()) {
                if let Some(w) = self.pending.lock().unwrap().items.get_mut(&id) {
                    w.dialog = Some(child.id());
                }
                let me = self.clone();
                tokio::task::spawn_blocking(move || {
                    if let Some(ans) = consent::wait(child, kind) {
                        me.decide(id, ans);
                    }
                });
            }
        }
        let answer = tokio::select! {
            r = rx => r.ok(),
            _ = tokio::time::sleep(Duration::from_secs(proto::PAIR_WAIT_SECS)) => None,
            _ = conn.closed() => None,
        };
        if let Some(w) = self.pending.lock().unwrap().items.remove(&id) {
            if let Some(pid) = w.dialog {
                consent::close(pid);
            }
        }
        match answer {
            Some(true) => {
                let (grant, credential) = {
                    let mut a = self.access.lock().unwrap();
                    let r = a.issue(device.clone(), None, &addr.to_string(), None, access::now());
                    self.save(&a);
                    r
                };
                tracing::info!(id, %grant, name = %device.name, %addr, "主人同意了配对");
                self.paired(grant, credential)
            }
            Some(false) => {
                self.note_denied(ip);
                tracing::info!(id, name = %device.name, %addr, "主人拒绝了配对");
                ControlResp::Denied("对方拒绝了".into())
            }
            None => ControlResp::Denied(format!("对方 {} 秒内没有回应", proto::PAIR_WAIT_SECS)),
        }
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
    let role: Arc<OnceLock<Role>> = Arc::new(OnceLock::new());
    let r: Result<()> = async {
        loop {
            let (send, recv) = conn.accept_bi().await?;
            let (shared, role, conn) = (shared.clone(), role.clone(), conn.clone());
            tokio::spawn(async move {
                if let Err(e) = handle_stream(send, recv, conn, shared, role).await {
                    tracing::debug!("流结束：{e:#}");
                }
            });
        }
    }
    .await;
    if let Some(Role::Device(id)) = role.get() {
        shared.unregister(id, &conn);
    }
    r
}

/// 不开放时给人看的原因
fn closed(role: &Role, what: &str, flag: &str) -> String {
    match role {
        Role::Owner => format!("这台主机没开放{what}（lolidesk host allow {flag} on）"),
        Role::Device(_) => format!("这台电脑没给你开放{what}"),
    }
}

async fn handle_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    conn: Connection,
    shared: Arc<Shared>,
    role: Arc<OnceLock<Role>>,
) -> Result<()> {
    let mut kind = [0u8; 1];
    recv.read_exact(&mut kind).await?;
    let kind = StreamKind::from_u8(kind[0]).context("未知流类型")?;
    if kind == StreamKind::Control {
        return control(&mut send, &mut recv, &conn, &shared, &role).await;
    }
    let Some(who) = role.get().cloned() else {
        bail!("还没 Hello 就开 {kind:?} 流");
    };
    // 能做什么现算（allow.json 和设备的限制）：改了不用重启主机，新开的流就按新的来
    let allow = shared.allow_for(&who);
    match kind {
        StreamKind::Control => unreachable!(),
        StreamKind::Terminal if !allow.terminal => {
            deny::<TermEvent>(send, TermEvent::Error(closed(&who, "终端", "terminal"))).await
        }
        StreamKind::Desktop if !allow.desktop => {
            deny::<DeskEvent>(send, DeskEvent::Error(closed(&who, "远程桌面", "desktop"))).await
        }
        StreamKind::Terminal => terminal(send, recv, &shared).await,
        StreamKind::Desktop => {
            let (s, r) = (shared.clone(), who.clone());
            desktop_stream(send, recv, conn, move || s.allow_for(&r).control).await
        }
        StreamKind::Cua => {
            let (s, r) = (shared.clone(), who.clone());
            let msg = closed(&who, " computer use", "cua");
            cua::serve(send, recv, move || s.allow_for(&r).cua, msg).await
        }
    }
}

async fn control(
    send: &mut SendStream,
    recv: &mut RecvStream,
    conn: &Connection,
    shared: &Arc<Shared>,
    role: &OnceLock<Role>,
) -> Result<()> {
    while let Some(req) = proto::recv::<ControlReq>(recv).await? {
        let req = match req {
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
                    close_soon(send, conn, 1, b"version");
                    bail!("版本不匹配");
                }
                if role.get().is_some() {
                    proto::send(send, &ControlResp::Denied("这条连接已经 Hello 过了".into()))
                        .await?;
                    continue;
                }
                let addr = conn.remote_address().to_string();
                let who = if constant_eq(token.as_bytes(), shared.token.as_bytes()) {
                    Role::Owner
                } else {
                    let checked = {
                        let mut a = shared.access.lock().unwrap();
                        let r = a.check(&token, &addr, access::now());
                        if r.as_ref().is_some_and(|(_, changed)| *changed) {
                            shared.save(&a);
                        }
                        r
                    };
                    match checked {
                        Some((id, _)) => Role::Device(id),
                        None => {
                            let why = "凭据不对，或者这台设备已经被吊销了（重新配对：lolidesk pair <主机>）";
                            proto::send(send, &ControlResp::Denied(why.into())).await?;
                            close_soon(send, conn, 2, b"token");
                            bail!("凭据不对（{client}）");
                        }
                    }
                };
                if let Role::Device(id) = &who {
                    shared.register(id, conn.clone());
                }
                tracing::info!(client, ?who, "已鉴权");
                let allow = shared.allow_for(&who);
                let _ = role.set(who.clone());
                proto::send(
                    send,
                    &ControlResp::Welcome {
                        host: shared.host.clone(),
                        os: std::env::consts::OS.into(),
                        version: proto::BUILD.into(),
                        desktop: desktop::supported(),
                        persistent: shared.sessions.persistent(),
                        allow,
                        role: who,
                    },
                )
                .await?;
                continue;
            }
            // 配对不用先 Hello：拿到凭据后再在这条流上 Hello
            ControlReq::Pair { device, code } => {
                let resp = shared.pair(conn, device, code).await;
                proto::send(send, &resp).await?;
                continue;
            }
            other => other,
        };
        let Some(who) = role.get().cloned() else {
            proto::send(send, &ControlResp::Denied("先 Hello".into())).await?;
            continue;
        };
        let owner = who == Role::Owner;
        let resp = match req {
            ControlReq::Hello { .. } | ControlReq::Pair { .. } => unreachable!(),
            ControlReq::ListDisplays => match desktop::list_displays() {
                Ok(d) => ControlResp::Displays(d),
                Err(e) => ControlResp::Error(format!("{e:#}")),
            },
            // 窗口标题能看出人在干什么：能看这台桌面的才给
            ControlReq::ListWindows if !shared.allow_for(&who).desktop => {
                ControlResp::Error(closed(&who, "远程桌面", "desktop"))
            }
            ControlReq::ListWindows => {
                match tokio::task::spawn_blocking(desktop::list_windows).await? {
                    Ok(w) => ControlResp::Windows(w),
                    Err(e) => ControlResp::Error(format!("{e:#}")),
                }
            }
            ControlReq::ListSessions => {
                let sessions = shared.sessions.clone();
                ControlResp::Sessions(tokio::task::spawn_blocking(move || sessions.list()).await?)
            }
            ControlReq::ListLaunchers => {
                ControlResp::Launchers(tokio::task::spawn_blocking(launchers::list).await?)
            }
            ControlReq::Permissions => {
                match tokio::task::spawn_blocking(permissions::status).await? {
                    Ok((screen_recording, accessibility)) => ControlResp::Permissions {
                        screen_recording,
                        accessibility,
                    },
                    Err(e) => ControlResp::Error(format!("{e:#}")),
                }
            }
            // 弹系统授权框、重启主机：主人，或者能操作这台桌面的设备
            ControlReq::OpenPermission(_) | ControlReq::RestartHost
                if !owner && !shared.allow_for(&who).control =>
            {
                ControlResp::Error("这台电脑没给你开放操作，不能改它的权限设置".into())
            }
            ControlReq::OpenPermission(kind) => {
                tracing::info!(?kind, "打开权限设置");
                match tokio::task::spawn_blocking(move || permissions::open(kind)).await? {
                    Ok(()) => ControlResp::Ok,
                    Err(e) => ControlResp::Error(format!("{e:#}")),
                }
            }
            ControlReq::RestartHost => {
                // 先回话，再交给脱离出去的子进程重启（它会等一秒）
                if !tokio::task::spawn_blocking(config::under_launch_agent).await? {
                    ControlResp::Error("主机不是服务方式运行，手动重启".into())
                } else {
                    proto::send(send, &ControlResp::Ok).await?;
                    tracing::info!("客户端要求重启主机服务");
                    if let Err(e) = config::restart_launch_agent() {
                        tracing::warn!("重启主机服务：{e:#}");
                    }
                    continue;
                }
            }
            ControlReq::Access => ControlResp::Access(shared.access_view(&who)),
            ControlReq::CreateCode {
                label,
                expires_in,
                max_uses,
                allow,
            } => {
                let by = match &who {
                    Role::Owner => None,
                    Role::Device(id) => Some(id.clone()),
                };
                if by.is_some() && !shared.allow_for(&who).share {
                    ControlResp::Error(
                        "这台电脑没开放分享链接（lolidesk host allow share on）".into(),
                    )
                } else {
                    let r = {
                        let mut a = shared.access.lock().unwrap();
                        let r = a.create_code(
                            &label,
                            by.clone(),
                            expires_in,
                            max_uses,
                            allow,
                            access::now(),
                        );
                        if r.is_ok() {
                            shared.save(&a);
                        }
                        r
                    };
                    match r {
                        Ok((c, secret)) => {
                            tracing::info!(code = %c.id, label = %c.label, by = ?by, "发了配对码");
                            ControlResp::Code {
                                info: proto::CodeInfo {
                                    id: c.id,
                                    label: c.label,
                                    created: c.created,
                                    created_by: c.created_by,
                                    expires: c.expires,
                                    max_uses: c.max_uses,
                                    uses: c.uses,
                                    allow: c.allow,
                                },
                                secret: access::format_code(&secret),
                                addr: shared.link_addr.clone(),
                            }
                        }
                        Err(e) => ControlResp::Error(format!("{e:#}")),
                    }
                }
            }
            ControlReq::Revoke(id) => {
                let r = {
                    let mut a = shared.access.lock().unwrap();
                    let r = a.revoke(&id, &who);
                    if r.is_ok() {
                        shared.save(&a);
                    }
                    r
                };
                match r {
                    Ok(gone) => {
                        tracing::info!(%id, ?gone, by = ?who, "吊销");
                        // 先回话：吊销的可能就是自己
                        proto::send(send, &ControlResp::Ok).await?;
                        shared.disconnect(&gone);
                        continue;
                    }
                    Err(e) => ControlResp::Error(format!("{e:#}")),
                }
            }
            ControlReq::Decide { .. } | ControlReq::Limit { .. } if !owner => {
                ControlResp::Error("只有这台电脑的主人能做".into())
            }
            ControlReq::Decide { id, allow } => {
                if shared.decide(id, allow) {
                    ControlResp::Ok
                } else {
                    ControlResp::Error(format!("没有请求 {id}（可能已经过期，或者对方走了）"))
                }
            }
            ControlReq::Limit { grant, allow } => {
                let mut a = shared.access.lock().unwrap();
                match a.limit(&grant, allow) {
                    Ok(()) => {
                        shared.save(&a);
                        tracing::info!(%grant, ?allow, "改了设备的限制");
                        ControlResp::Ok
                    }
                    Err(e) => ControlResp::Error(format!("{e:#}")),
                }
            }
        };
        proto::send(send, &resp).await?;
    }
    Ok(())
}

/// 拒绝之后关连接：先把流写完，过一会儿再关，不然对方收不到为什么（只看到连接断了）
fn close_soon(send: &mut SendStream, conn: &Connection, code: u32, reason: &'static [u8]) {
    let _ = send.finish();
    let conn = conn.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        conn.close(code.into(), reason);
    });
}

/// 不开放的流：回一句为什么，然后关
async fn deny<T: serde::Serialize>(mut send: SendStream, msg: T) -> Result<()> {
    proto::send(&mut send, &msg).await?;
    send.finish()?;
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

/// 推着的窗口关了、最小化了、藏起来了：改推它所在的显示器时告诉人的话（同 Vibe42）
const WINDOW_GONE: &str = "窗口已关闭或已隐藏，已切换到整个屏幕";

/// 正在推的：采集、推的是什么、实际参数（推窗口时 area 跟着窗口挪）
struct Live {
    capture: desktop::Capture,
    target: DeskTarget,
    actual: desktop::Actual,
}

/// 一条桌面流上的采集、输入和发帧。
struct Desk<'a> {
    send: &'a mut SendStream,
    live: Option<Live>,
    video: VideoSettings,
    injector: Option<input::Injector>,
    tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    force_key: Arc<AtomicBool>,
    /// 推窗口时窗口大小在变：上一次看到的新大小（稳住了才重开采集）
    resizing: Option<(u32, u32)>,
}

impl Desk<'_> {
    async fn event(&mut self, ev: DeskEvent) -> Result<()> {
        proto::send(self.send, &ev).await
    }

    /// 开采集（不发消息）
    async fn capture(&self, target: DeskTarget) -> Result<desktop::Capture> {
        let cfg = desktop::CaptureConfig {
            target,
            video: self.video,
            force_key: self.force_key.clone(),
        };
        let tx = self.tx.clone();
        let c = tokio::task::spawn_blocking(move || desktop::Capture::start(cfg, tx)).await??;
        let a = c.actual();
        tracing::info!(
            ?target,
            video = ?self.video,
            w = a.width,
            h = a.height,
            fps = a.fps,
            bitrate = a.bitrate,
            "远程桌面开始"
        );
        Ok(c)
    }

    fn stop_live(&mut self) {
        if let Some(l) = self.live.take() {
            l.capture.stop();
        }
    }

    /// 开起来了：输入按它换算，告诉客户端（`showing` 时先说推的是什么，再给 Ready）
    async fn went_live(
        &mut self,
        c: desktop::Capture,
        target: DeskTarget,
        showing: Option<String>,
    ) -> Result<()> {
        let a = c.actual();
        let inj = self
            .injector
            .get_or_insert_with(|| input::Injector::new(a.display));
        inj.set_area(a.area);
        inj.set_window(match target {
            DeskTarget::Window(id) => Some(id),
            DeskTarget::Display(_) => None,
        });
        self.live = Some(Live {
            capture: c,
            target,
            actual: a,
        });
        self.resizing = None;
        if let Some(note) = showing {
            self.event(DeskEvent::Showing { target, note }).await?;
        }
        self.event(DeskEvent::Ready {
            width: a.width,
            height: a.height,
            fps: a.fps,
            bitrate: a.bitrate,
        })
        .await
    }

    /// 要推一个窗口：能操作这台桌面的话先把它提到最前面（最小化的恢复、程序激活），
    /// 远程的键鼠才落在它上面；等它出现在屏幕上（最多 1.5 秒，恢复最小化有动画）。
    async fn prepare_window(id: u32, control: bool) -> Result<()> {
        if control {
            if let Err(e) = tokio::task::spawn_blocking(move || desktop::raise_window(id)).await? {
                tracing::debug!("提不了窗口 {id}：{e:#}");
            }
        }
        for _ in 0..15 {
            match tokio::task::spawn_blocking(move || desktop::window_state(id)).await? {
                None => bail!("窗口不在了（关掉了？）"),
                Some(s) if s.on_screen => return Ok(()),
                Some(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        if control {
            bail!("窗口没出现在屏幕上（程序没理恢复的请求，或者它在别的桌面空间）")
        }
        bail!("窗口不在屏幕上（最小化了、程序藏起来了），这台电脑没给你开放操作，恢复不了")
    }

    /// Start（旧客户端，只有显示器）/ Open：开始推
    async fn open(
        &mut self,
        target: DeskTarget,
        video: VideoSettings,
        control: bool,
        announce: bool,
    ) -> Result<()> {
        if self.live.is_some() {
            return self.event(DeskEvent::Error("已经在推了".into())).await;
        }
        self.video = video;
        let first = match target {
            DeskTarget::Window(id) => match Self::prepare_window(id, control).await {
                Ok(()) => self.capture(target).await,
                Err(e) => Err(e),
            },
            DeskTarget::Display(_) => self.capture(target).await,
        };
        match first {
            Ok(c) => {
                let showing = announce.then(String::new);
                self.went_live(c, target, showing).await
            }
            // 窗口打不开：改推主显示器，说为什么
            Err(e) if matches!(target, DeskTarget::Window(_)) => {
                tracing::info!("打不开窗口：{e:#}，改推主显示器");
                let main = DeskTarget::Display(desktop::main_display());
                match self.capture(main).await {
                    Ok(c) => {
                        let note = format!("打不开这个窗口（{e:#}），已切换到整个屏幕");
                        self.went_live(c, main, Some(note)).await
                    }
                    Err(e) => self.fail(e).await,
                }
            }
            Err(e) => self.fail(e).await,
        }
    }

    async fn fail(&mut self, e: anyhow::Error) -> Result<()> {
        tracing::warn!("采屏失败：{e:#}");
        self.event(DeskEvent::Error(format!("{e:#}"))).await
    }

    /// 同一个东西按新参数重开（Configure、窗口大小变了）
    async fn restart(&mut self) -> Result<()> {
        let Some(target) = self.live.as_ref().map(|l| l.target) else {
            return Ok(());
        };
        self.stop_live();
        match self.capture(target).await {
            Ok(c) => self.went_live(c, target, None).await,
            Err(e) => self.fail(e).await,
        }
    }

    /// Select：换推的东西。换不过去就接着推原来的（回 Error）
    async fn select(&mut self, target: DeskTarget, control: bool) -> Result<()> {
        let Some(old) = self.live.as_ref().map(|l| l.target) else {
            return self.event(DeskEvent::Error("还没开始推".into())).await;
        };
        if let DeskTarget::Window(id) = target {
            // 先看窗口能不能推，不行的话原来的接着推，不用停
            if let Err(e) = Self::prepare_window(id, control).await {
                return self
                    .event(DeskEvent::Error(format!("打不开这个窗口：{e:#}")))
                    .await;
            }
        }
        if target == old {
            self.force_key.store(true, Ordering::Relaxed);
            let a = self.live.as_ref().map(|l| l.actual);
            self.event(DeskEvent::Showing {
                target,
                note: String::new(),
            })
            .await?;
            if let Some(a) = a {
                self.event(DeskEvent::Ready {
                    width: a.width,
                    height: a.height,
                    fps: a.fps,
                    bitrate: a.bitrate,
                })
                .await?;
            }
            return Ok(());
        }
        self.stop_live();
        match self.capture(target).await {
            Ok(c) => self.went_live(c, target, Some(String::new())).await,
            Err(e) => {
                let why = format!("换不过去：{e:#}");
                match self.capture(old).await {
                    Ok(c) => {
                        self.event(DeskEvent::Error(why)).await?;
                        self.went_live(c, old, Some(String::new())).await
                    }
                    Err(e2) => self.fail(e2.context(why)).await,
                }
            }
        }
    }

    /// 推窗口时每秒一次：窗口挪了、变大小了、没了
    async fn watch_window(&mut self) -> Result<()> {
        let Some((id, area, source, display)) = self.live.as_ref().and_then(|l| match l.target {
            DeskTarget::Window(id) => Some((id, l.actual.area, l.actual.source, l.actual.display)),
            DeskTarget::Display(_) => None,
        }) else {
            return Ok(());
        };
        let now = tokio::task::spawn_blocking(move || desktop::window_state(id)).await?;
        match desktop::watch(area, source, &mut self.resizing, now) {
            desktop::Watch::Keep => Ok(()),
            desktop::Watch::Move(a) => {
                if let Some(l) = self.live.as_mut() {
                    l.actual.area = a;
                }
                if let Some(inj) = &self.injector {
                    inj.set_area(a);
                }
                Ok(())
            }
            desktop::Watch::Restart => self.restart().await,
            desktop::Watch::Lost => {
                let to = DeskTarget::Display(now.map_or(display, |s| s.display));
                tracing::info!(id, ?to, "推着的窗口没了，改推显示器");
                self.stop_live();
                match self.capture(to).await {
                    Ok(c) => self.went_live(c, to, Some(WINDOW_GONE.into())).await,
                    Err(e) => self.fail(e).await,
                }
            }
        }
    }
}

async fn desktop_stream(
    mut send: SendStream,
    mut recv: RecvStream,
    conn: Connection,
    can_control: impl Fn() -> bool,
) -> Result<()> {
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
    // 请求在自己的任务里读：主循环还要每秒看一眼推着的窗口，读到一半的消息不能被打断
    let (req_tx, mut reqs) = tokio::sync::mpsc::channel::<Result<DeskReq>>(64);
    let reader = tokio::spawn(async move {
        loop {
            let r = proto::recv::<DeskReq>(&mut recv).await;
            let end = !matches!(r, Ok(Some(_)));
            let sent = match r {
                Ok(Some(req)) => req_tx.send(Ok(req)).await,
                Ok(None) => break,
                Err(e) => req_tx.send(Err(e)).await,
            };
            if end || sent.is_err() {
                break;
            }
        }
    });
    let mut desk = Desk {
        send: &mut send,
        live: None,
        video: VideoSettings::default(),
        injector: None,
        tx,
        force_key: force_key.clone(),
        resizing: None,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result: Result<()> = async {
        loop {
            let watching = desk
                .live
                .as_ref()
                .is_some_and(|l| matches!(l.target, DeskTarget::Window(_)));
            let req = tokio::select! {
                r = reqs.recv() => match r {
                    Some(r) => r?,
                    None => break,
                },
                _ = tick.tick(), if watching => {
                    desk.watch_window().await?;
                    continue;
                }
            };
            match req {
                DeskReq::Start { display, video } => {
                    desk.open(DeskTarget::Display(display), video, can_control(), false)
                        .await?
                }
                DeskReq::Open { target, video } => {
                    desk.open(target, video, can_control(), true).await?
                }
                DeskReq::Select(target) => desk.select(target, can_control()).await?,
                DeskReq::Configure(video) => {
                    desk.video = video;
                    desk.restart().await?;
                }
                DeskReq::Stop => break,
                DeskReq::Keyframe => force_key.store(true, Ordering::Relaxed),
                DeskReq::Input(i) => {
                    // 只看不控（allow.json 的 control 关了）：输入丢掉
                    if let Some(inj) = desk.injector.as_ref().filter(|_| can_control()) {
                        // 远程桌面里有人在操作：agent 的 computer use 先让着（input::person_active）
                        if !matches!(i, DeskInput::ReleaseAll) {
                            input::VIEWER_LAST.store(input::now_ms(), Ordering::Relaxed);
                        }
                        inj.apply(i);
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    desk.stop_live();
    if let Some(inj) = desk.injector.take() {
        inj.apply(DeskInput::ReleaseAll);
    }
    reader.abort();
    sender.abort();
    let _ = proto::send(&mut send, &DeskEvent::Stopped("结束".into())).await;
    tracing::info!(dropped = dropped.load(Ordering::Relaxed), "远程桌面结束");
    result
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
