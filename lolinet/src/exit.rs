//! 出口节点：网里别的机器借这台的网络出去。一个内置的 SOCKS5 服务（RFC 1928 的 CONNECT 和 UDP ASSOCIATE，不要认证），
//! 默认端口 10180。手机上的 mihomo、浏览器、`curl --socks5-hostname` 都能按规则挑一个出口用（见 docs/design.md）。
//!
//! - 听哪里：TUN 模式只听本机 overlay 地址；userspace 模式听 127.0.0.1，EasyTier 把发到本机 overlay 地址的包转给它。
//!   公网上连不到：地址不是公网的，而且来源不在允许的网段（默认 overlay 网段和回环）就在握手前关掉。
//! - 去哪里：不让借出口打本机和内网，回环、链路本地（169.254.169.254 的云元数据）、组播、私网和 CGNAT 地址一律拒绝，
//!   `--lan` 才放开私网（相当于子网路由）。
//! - 性能：TCP 两边 TCP_NODELAY，copy_bidirectional 64 KiB 缓冲；UDP 一个关联一对 socket，逐包转发，QUIC、DNS 都能走。
//! - 服务：co.lo-li.lolinet-exit / lolinet-exit.service，root 启动，读完配置、绑好端口就降到 nobody。只记出错。
//!
//! 配置 /etc/lolinet/exit.json：{"port": 10180, "allow": ["10.188.0.0/16"], "lan": false}，allow 不写就是 overlay 网段。

use std::{
    collections::HashMap,
    fs,
    io::ErrorKind,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::Path,
    rc::Rc,
    str::FromStr as _,
    time::Duration,
};

use anyhow::{Context as _, Result};
use cidr::{IpCidr, IpInet};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};

use crate::{CONFIG as NET_CONFIG, netcfg::Net, service, write_file};

pub const CONFIG: &str = "/etc/lolinet/exit.json";
pub const PORT: u16 = 10180;
const HANDSHAKE: Duration = Duration::from_secs(10);
const DIAL: Duration = Duration::from_secs(10);
/// 同时最多这么多条连接，再多就先不 accept（文件描述符留给内核和别的服务）
const MAX_CONNS: usize = 4096;
const BUF: usize = 64 << 10;

#[derive(Clone)]
pub struct Settings {
    pub port: u16,
    /// 允许的来源网段；空就是 overlay 网段 + 回环
    pub allow: Vec<String>,
    /// 放开私网目标（子网路由）
    pub lan: bool,
}

impl Settings {
    pub fn load() -> Option<Settings> {
        let v: Value = serde_json::from_slice(&fs::read(CONFIG).ok()?).ok()?;
        Some(Settings {
            port: v.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).unwrap_or(PORT),
            allow: v
                .get("allow")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default(),
            lan: v.get("lan").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    fn save(&self) -> Result<()> {
        let v = json!({"port": self.port, "allow": self.allow, "lan": self.lan});
        write_file(CONFIG, format!("{}\n", serde_json::to_string_pretty(&v)?).as_bytes(), 0o644)
    }
}

/// 读 loli.toml：本机地址、模式。配置是 600，要 root
fn net() -> Result<Net> {
    Net::parse(&fs::read_to_string(NET_CONFIG).with_context(|| format!("读 {NET_CONFIG}（要 root）"))?)
}

/// 这台的出口在网里的地址（别人填进 mihomo / curl 的那个）
fn shown_addr(net: &Net, port: u16) -> String {
    net.addr().map(|a| format!("{a}:{port}")).unwrap_or_else(|| format!("<本机 overlay 地址>:{port}"))
}

pub fn on(port: u16, allow: Vec<String>, lan: bool) -> Result<()> {
    crate::need_root()?;
    for a in &allow {
        parse_cidr(a)?;
    }
    let net = net()?;
    Settings { port, allow, lan }.save()?;
    service::install_exit()?;
    println!(
        "出口节点已开：socks5://{}（{}模式，{}）",
        shown_addr(&net, port),
        if net.userspace { "userspace " } else { "TUN " },
        if lan { "放开了私网目标" } else { "只出公网" }
    );
    println!("  别的机器试一下：curl --socks5-hostname {} https://example.com -I", shown_addr(&net, port));
    register(port);
    Ok(())
}

/// 在内网管理面登记（mihomo 的出口列表从那里来）；连不上只提示，不影响出口本身
fn register(port: u16) {
    let res = crate::manager::Manager::locate(None)
        .and_then(|m| m.json("POST", "/api/mesh/exit", Some(&json!({ "port": port }).to_string())));
    match (res, port) {
        (Ok(_), 0) => println!("  已从管理面的出口列表撤下"),
        (Ok(_), _) => println!("  已登记到管理面：手机和电脑的 mihomo 会在「出口」里看到这台"),
        (Err(e), _) => println!("  没登记到管理面（mihomo 的出口列表里不会有这台）：{e:#}"),
    }
}

pub fn off() -> Result<()> {
    crate::need_root()?;
    service::remove_exit()?;
    if Path::new(CONFIG).exists() {
        fs::remove_file(CONFIG)?;
    }
    println!("出口节点已关");
    register(0);
    Ok(())
}

/// `lolinet up` 换了配置或模式后调：开着的就重启，好用上新的地址
pub fn refresh() {
    if Settings::load().is_some() && service::exit_state().is_some() {
        let _ = service::install_exit();
    }
}

pub fn status() -> Result<()> {
    let Some(s) = Settings::load() else {
        println!("出口节点  没开（sudo lolinet exit on）");
        return Ok(());
    };
    let addr = net().map(|n| shown_addr(&n, s.port)).unwrap_or_else(|_| format!("<overlay 地址>:{}", s.port));
    let state = match service::exit_state() {
        Some(st) if st.running => format!("运行中 · {}", st.manager),
        Some(st) => format!("没在运行 · {}（sudo lolinet exit on）", st.manager),
        None => "服务没装（sudo lolinet exit on）".into(),
    };
    println!("出口节点  socks5://{addr} · {state}");
    println!(
        "  来源 {} · 目标 {}",
        if s.allow.is_empty() { "overlay 网段和回环".into() } else { s.allow.join("、") },
        if s.lan { "公网和私网" } else { "只有公网" }
    );
    Ok(())
}

// ---------- 服务 ----------

/// 服务入口。listen / allow 给了就用（测试、手工跑），没给就按 exit.json 和 loli.toml
pub fn run(listen: Option<SocketAddr>, allow: Vec<String>, lan: bool) -> Result<()> {
    let s = Settings::load();
    let lan = lan || s.as_ref().is_some_and(|s| s.lan);
    let (listen, acl) = match listen {
        Some(l) => {
            let mut acl = Acl::default();
            for a in &allow {
                acl.0.push(parse_cidr(a)?);
            }
            if acl.0.is_empty() {
                acl.loopback();
            }
            (l, acl)
        }
        None => {
            let s = s.context("出口节点没开：sudo lolinet exit on")?;
            let net = net()?;
            let ip = if net.userspace {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            } else {
                net.addr().context("配置里没有 ipv4（本机 overlay 地址）")?
            };
            let mut acl = Acl::default();
            for a in allow.iter().chain(&s.allow) {
                acl.0.push(parse_cidr(a)?);
            }
            if acl.0.is_empty() {
                acl.0.push(parse_cidr(&net.cidr().context("配置里没有 ipv4")?)?);
                acl.loopback();
            }
            (SocketAddr::new(ip, s.port), acl)
        }
    };
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    rt.block_on(async move {
        // TUN 模式开机时网卡可能还没起来，绑不上就等
        let mut warned = false;
        let listener = loop {
            match bind(listen) {
                Ok(l) => break l,
                Err(e) if e.kind() == ErrorKind::AddrNotAvailable => {
                    if !warned {
                        eprintln!("{} 等 {} 可用（EasyTier 还没起来？）", now(), listen.ip());
                        warned = true;
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => return Err(anyhow::Error::new(e).context(format!("听 {listen}"))),
            }
        };
        crate::probe::drop_privileges()?;
        serve(listener, Rc::new(acl), lan).await
    })
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let s = socket2::Socket::new(socket2::Domain::for_address(addr), socket2::Type::STREAM, None)?;
    s.set_reuse_address(true)?;
    s.set_nonblocking(true)?;
    s.bind(&addr.into())?;
    s.listen(1024)?;
    TcpListener::from_std(s.into())
}

#[derive(Default)]
struct Acl(Vec<IpCidr>);

impl Acl {
    fn loopback(&mut self) {
        self.0.push(IpCidr::from_str("127.0.0.0/8").unwrap());
        self.0.push(IpCidr::from_str("::1/128").unwrap());
    }

    fn allows(&self, ip: IpAddr) -> bool {
        let ip = canonical(ip);
        self.0.iter().any(|c| c.contains(&ip))
    }
}

/// 10.188.16.10/16 这种带主机位的也认（取网段）
fn parse_cidr(s: &str) -> Result<IpCidr> {
    IpCidr::from_str(s)
        .or_else(|_| IpInet::from_str(s).map(|i| i.network()))
        .map_err(|e| anyhow::anyhow!("网段写错了：{s}（{e}）"))
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// 目标地址能不能借出口去：不打本机、不打元数据、默认不打私网
fn dest_ok(ip: IpAddr, lan: bool) -> bool {
    match canonical(ip) {
        IpAddr::V4(a) => {
            let o = a.octets();
            let private = a.is_private() || (o[0] == 100 && (64..128).contains(&o[1]));
            !(a.is_loopback() || a.is_unspecified() || a.is_link_local() || a.is_multicast() || a.is_broadcast() || o[0] == 0)
                && (lan || !private)
        }
        IpAddr::V6(a) => {
            let s = a.segments();
            let link_local = s[0] & 0xffc0 == 0xfe80;
            let unique_local = s[0] & 0xfe00 == 0xfc00;
            !(a.is_loopback() || a.is_unspecified() || a.is_multicast() || link_local) && (lan || !unique_local)
        }
    }
}

async fn serve(listener: TcpListener, acl: Rc<Acl>, lan: bool) -> Result<()> {
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONNS));
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            loop {
                let permit = slots.clone().acquire_owned().await?;
                let (sock, peer) = match listener.accept().await {
                    Ok(x) => x,
                    Err(e) => {
                        // EMFILE 之类：歇一下再接
                        eprintln!("{} accept：{e}", now());
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                };
                if !acl.allows(peer.ip()) {
                    continue;
                }
                tokio::task::spawn_local(async move {
                    let _permit = permit;
                    let _ = handle(sock, peer, lan).await;
                });
            }
        })
        .await
}

enum Dst {
    Ip(SocketAddr),
    Domain(String, u16),
}

impl Dst {
    async fn resolve(&self) -> std::io::Result<Vec<SocketAddr>> {
        match self {
            Dst::Ip(a) => Ok(vec![*a]),
            Dst::Domain(d, p) => Ok(tokio::net::lookup_host((d.as_str(), *p)).await?.collect()),
        }
    }
}

const REP_OK: u8 = 0;
const REP_FAIL: u8 = 1;
const REP_NOT_ALLOWED: u8 = 2;
const REP_NET_UNREACH: u8 = 3;
const REP_HOST_UNREACH: u8 = 4;
const REP_REFUSED: u8 = 5;
const REP_CMD: u8 = 7;
const REP_ATYP: u8 = 8;

async fn handle(mut c: TcpStream, peer: SocketAddr, lan: bool) -> Result<()> {
    c.set_nodelay(true)?;
    let local = c.local_addr()?;
    let (cmd, dst) = match timeout(HANDSHAKE, handshake(&mut c)).await {
        Ok(Ok(x)) => x,
        Ok(Err(Bad::Reply(code))) => return reply(&mut c, code, None).await,
        _ => return Ok(()),
    };
    match cmd {
        1 => connect(c, dst, lan).await,
        3 => udp(c, peer, local, lan).await,
        _ => reply(&mut c, REP_CMD, None).await,
    }
}

enum Bad {
    /// 握手读到一半出错：回这个码再关
    Reply(u8),
    /// 不是 SOCKS5 或者连接断了：直接关
    Close,
}

impl From<std::io::Error> for Bad {
    fn from(_: std::io::Error) -> Bad {
        Bad::Close
    }
}

async fn handshake(c: &mut TcpStream) -> std::result::Result<(u8, Dst), Bad> {
    let mut head = [0u8; 2];
    c.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(Bad::Close);
    }
    let mut methods = vec![0u8; head[1] as usize];
    c.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        c.write_all(&[5, 0xff]).await?;
        return Err(Bad::Close);
    }
    c.write_all(&[5, 0]).await?;
    let mut req = [0u8; 4];
    c.read_exact(&mut req).await?;
    if req[0] != 5 {
        return Err(Bad::Close);
    }
    let dst = match req[3] {
        1 => {
            let mut b = [0u8; 6];
            c.read_exact(&mut b).await?;
            Dst::Ip(SocketAddr::new(Ipv4Addr::new(b[0], b[1], b[2], b[3]).into(), u16::from_be_bytes([b[4], b[5]])))
        }
        4 => {
            let mut b = [0u8; 18];
            c.read_exact(&mut b).await?;
            let ip: [u8; 16] = b[..16].try_into().unwrap();
            Dst::Ip(SocketAddr::new(Ipv6Addr::from(ip).into(), u16::from_be_bytes([b[16], b[17]])))
        }
        3 => {
            let mut n = [0u8; 1];
            c.read_exact(&mut n).await?;
            let mut b = vec![0u8; n[0] as usize + 2];
            c.read_exact(&mut b).await?;
            let port = u16::from_be_bytes([b[n[0] as usize], b[n[0] as usize + 1]]);
            b.truncate(n[0] as usize);
            Dst::Domain(String::from_utf8(b).map_err(|_| Bad::Reply(REP_ATYP))?, port)
        }
        _ => return Err(Bad::Reply(REP_ATYP)),
    };
    Ok((req[1], dst))
}

async fn reply(c: &mut TcpStream, code: u8, bound: Option<SocketAddr>) -> Result<()> {
    let mut b = vec![5, code, 0];
    put_addr(&mut b, bound.unwrap_or_else(|| SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0)));
    c.write_all(&b).await?;
    Ok(())
}

fn put_addr(b: &mut Vec<u8>, a: SocketAddr) {
    match canonical(a.ip()) {
        IpAddr::V4(ip) => {
            b.push(1);
            b.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            b.push(4);
            b.extend_from_slice(&ip.octets());
        }
    }
    b.extend_from_slice(&a.port().to_be_bytes());
}

fn code_of(e: &std::io::Error) -> u8 {
    match e.kind() {
        ErrorKind::ConnectionRefused => REP_REFUSED,
        ErrorKind::TimedOut => REP_HOST_UNREACH,
        _ => match e.raw_os_error().and_then(crate::sys::unreachable) {
            Some(false) => REP_NET_UNREACH,
            Some(true) => REP_HOST_UNREACH,
            None => REP_FAIL,
        },
    }
}

async fn connect(mut c: TcpStream, dst: Dst, lan: bool) -> Result<()> {
    let addrs = match timeout(DIAL, dst.resolve()).await {
        Ok(Ok(a)) if !a.is_empty() => a,
        _ => return reply(&mut c, REP_HOST_UNREACH, None).await,
    };
    let addrs: Vec<SocketAddr> = addrs.into_iter().filter(|a| dest_ok(a.ip(), lan)).collect();
    if addrs.is_empty() {
        return reply(&mut c, REP_NOT_ALLOWED, None).await;
    }
    // 解析出的地址挨个试（IPv4 / IPv6 都有时，先通的那个）
    let mut last = REP_HOST_UNREACH;
    let mut out = None;
    for a in addrs {
        match timeout(DIAL, TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                out = Some(s);
                break;
            }
            Ok(Err(e)) => last = code_of(&e),
            Err(_) => last = REP_HOST_UNREACH,
        }
    }
    let Some(mut out) = out else {
        return reply(&mut c, last, None).await;
    };
    out.set_nodelay(true)?;
    reply(&mut c, REP_OK, Some(out.local_addr()?)).await?;
    let _ = tokio::io::copy_bidirectional_with_sizes(&mut c, &mut out, BUF, BUF).await;
    Ok(())
}

/// UDP ASSOCIATE：关联跟着这条 TCP 走，TCP 断了关联就结束。
/// 进来的包只认 TCP 那一端的主机；出去用一对 socket（IPv4 / IPv6），回包带上来源地址包回 SOCKS5 的 UDP 头。
async fn udp(mut c: TcpStream, peer: SocketAddr, local: SocketAddr, lan: bool) -> Result<()> {
    let relay = match UdpSocket::bind(SocketAddr::new(local.ip(), 0)).await {
        Ok(s) => s,
        Err(_) => return reply(&mut c, REP_FAIL, None).await,
    };
    let bound = relay.local_addr()?;
    // userspace 模式：客户端连的是本机 overlay 地址，127.0.0.1 对它没用；回 0.0.0.0，客户端按惯例用它连过来的地址
    let shown = if bound.ip().is_loopback() { SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), bound.port()) } else { bound };
    let out4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    let out6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).await.ok();
    reply(&mut c, REP_OK, Some(shown)).await?;

    const ROOM: usize = 22; // 最长的 UDP 头（IPv6）
    let mut inb = vec![0u8; 65536];
    let mut back4 = vec![0u8; 65536 + ROOM];
    let mut back6 = vec![0u8; 65536 + ROOM];
    let mut ctl = [0u8; 64];
    let mut client: Option<SocketAddr> = None;
    let mut names: HashMap<(String, u16), SocketAddr> = HashMap::new();
    loop {
        tokio::select! {
            r = c.read(&mut ctl) => match r {
                Ok(0) | Err(_) => return Ok(()),
                Ok(_) => {}
            },
            r = relay.recv_from(&mut inb) => {
                let (n, from) = r?;
                if canonical(from.ip()) != canonical(peer.ip()) {
                    continue;
                }
                client = Some(from);
                let Some((dst, off)) = parse_udp(&inb[..n]) else { continue };
                let to = match dst {
                    Dst::Ip(a) => a,
                    Dst::Domain(d, p) => {
                        let key = (d, p);
                        match names.get(&key) {
                            Some(a) => *a,
                            None => {
                                let found = match timeout(DIAL, tokio::net::lookup_host((key.0.as_str(), key.1))).await {
                                    Ok(Ok(mut it)) => it.next(),
                                    _ => None,
                                };
                                let Some(a) = found else { continue };
                                if names.len() >= 256 {
                                    names.clear();
                                }
                                names.insert(key, a);
                                a
                            }
                        }
                    }
                };
                if !dest_ok(to.ip(), lan) {
                    continue;
                }
                let to = SocketAddr::new(canonical(to.ip()), to.port());
                let _ = match (to, &out6) {
                    (SocketAddr::V4(_), _) => out4.send_to(&inb[off..n], to).await,
                    (SocketAddr::V6(_), Some(s6)) => s6.send_to(&inb[off..n], to).await,
                    _ => continue,
                };
            }
            r = out4.recv_from(&mut back4[ROOM..]) => {
                let (n, from) = r?;
                if let Some(cl) = client {
                    let start = wrap(&mut back4, ROOM, from);
                    let _ = relay.send_to(&back4[start..ROOM + n], cl).await;
                }
            }
            r = async { match &out6 { Some(s) => s.recv_from(&mut back6[ROOM..]).await, None => std::future::pending().await } } => {
                let (n, from) = r?;
                if let Some(cl) = client {
                    let start = wrap(&mut back6, ROOM, from);
                    let _ = relay.send_to(&back6[start..ROOM + n], cl).await;
                }
            }
        }
    }
}

/// SOCKS5 UDP 头：RSV(2) FRAG(1) ATYP 地址 端口；分片的不支持（丢）
fn parse_udp(b: &[u8]) -> Option<(Dst, usize)> {
    if b.len() < 4 || b[2] != 0 {
        return None;
    }
    match b[3] {
        1 if b.len() >= 10 => {
            let ip = Ipv4Addr::new(b[4], b[5], b[6], b[7]);
            Some((Dst::Ip(SocketAddr::new(ip.into(), u16::from_be_bytes([b[8], b[9]]))), 10))
        }
        4 if b.len() >= 22 => {
            let ip: [u8; 16] = b[4..20].try_into().ok()?;
            Some((Dst::Ip(SocketAddr::new(Ipv6Addr::from(ip).into(), u16::from_be_bytes([b[20], b[21]]))), 22))
        }
        3 if b.len() >= 5 => {
            let n = b[4] as usize;
            if b.len() < 5 + n + 2 {
                return None;
            }
            let name = std::str::from_utf8(&b[5..5 + n]).ok()?.to_owned();
            Some((Dst::Domain(name, u16::from_be_bytes([b[5 + n], b[6 + n]])), 7 + n))
        }
        _ => None,
    }
}

/// 在 buf[room..] 的数据前面写 UDP 头，返回包的起点
fn wrap(buf: &mut [u8], room: usize, from: SocketAddr) -> usize {
    let mut h = vec![0, 0, 0];
    put_addr(&mut h, from);
    let start = room - h.len();
    buf[start..room].copy_from_slice(&h);
    start
}

