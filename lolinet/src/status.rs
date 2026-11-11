//! `lolinet status`：服务、升级、探针、本机、接入点（10.188.0.0/24）和设备。数据来自内置 easytier-cli 的 JSON 输出，
//! 和 web/agent、probe/reporter.py 读的是同一份。

use std::fs;

use anyhow::Result;
use serde_json::Value;

use crate::{CONFIG, RPC, VERSION, probe, service, tool, upgrade};

fn cli_json(sub: &str) -> Option<Value> {
    let out = tool("easytier-cli").args(["-p", RPC, "-o", "json", sub]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

pub fn node() -> Option<Value> {
    cli_json("node")
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_owned()
}

/// 按终端显示宽度补空格（中文占两格）
fn pad(text: &str, width: usize) -> String {
    let w: usize = text.chars().map(|c| if c as u32 >= 0x2e80 { 2 } else { 1 }).sum();
    format!("{text}{}", " ".repeat(width.saturating_sub(w)))
}

pub fn show() -> Result<()> {
    println!("lolinet {VERSION} · EasyTier {}", easytier::common::constants::EASYTIER_VERSION);
    match service::state() {
        Some(st) if st.running => println!(
            "服务    运行中 · {}{}",
            st.manager,
            st.pid.map(|p| format!(" · pid {p}")).unwrap_or_default()
        ),
        Some(st) => println!("服务    没在运行 · {}（sudo lolinet restart）", st.manager),
        None => println!("服务    没装（sudo lolinet up）"),
    }
    let st = upgrade::Settings::load();
    match (st.auto, service::upgrade_scheduled()) {
        (true, true) => println!("升级    自动，每 {} 小时 · {}", upgrade::INTERVAL_HOURS, st.manifest_url()),
        (true, false) => println!("升级    定时任务没装（sudo lolinet up）· {}", st.manifest_url()),
        (false, _) => println!("升级    关（sudo lolinet up --auto-upgrade on）；手动 sudo lolinet upgrade"),
    }
    // 公开 id 在 600 的配置里，不加 sudo 只看得到服务状态
    let id = match probe::public_id() {
        Ok(Some(id)) => format!(" · {id}"),
        _ => String::new(),
    };
    match service::probe_state() {
        Some(p) if p.running => println!("探针    运行中{id} · {} · 只报硬件和位置", p.manager),
        Some(p) => println!("探针    没在运行{id} · {}（sudo lolinet up）", p.manager),
        None if service::legacy_probe_installed() => println!("探针    python 版（sudo lolinet up 换成内置的）"),
        None if !id.is_empty() => println!("探针    配了{id}，服务没装（sudo lolinet up）"),
        None => println!("探针    没装（配对时带了探针才装）"),
    }
    let (Some(node), Some(Value::Array(peers))) = (node(), cli_json("peer")) else {
        println!("内核    管理口 {RPC} 连不上");
        return Ok(());
    };
    let nat = peers.iter().find(|p| s(p, "cost") == "Local").map(|p| s(p, "nat_type")).unwrap_or_default();
    println!("本机    {}  {}  NAT {nat}", s(&node, "hostname"), s(&node, "ipv4_addr"));
    println!("模式    {}", mode());
    println!("凭证    {}", credential());
    if let Some(e) = crate::exit::Settings::load() {
        let st = match service::exit_state() {
            Some(st) if st.running => "运行中".to_owned(),
            Some(_) => "没在运行（sudo lolinet exit on）".to_owned(),
            None => "服务没装（sudo lolinet exit on）".to_owned(),
        };
        let ip = s(&node, "ipv4_addr");
        let ip = ip.split('/').next().unwrap_or("");
        println!("出口    socks5://{ip}:{} · {st}", e.port);
    }
    if let Some(m) = crate::mihomo::installed() {
        let st = match service::mihomo_state() {
            Some(st) if st.running => "运行中",
            Some(_) => "没在运行（sudo lolinet mihomo on）",
            None => "服务没装（sudo lolinet mihomo on）",
        };
        println!("mihomo  {m} · {st}");
    }
    // 没有地址的是控制台（network.issuer 那台 relay）的 Gateway 实例，不算节点
    let (relays, devices): (Vec<&Value>, Vec<&Value>) = peers
        .iter()
        .filter(|p| s(p, "cost") != "Local" && !s(p, "ipv4").is_empty())
        .partition(|p| s(p, "ipv4").starts_with("10.188.0."));
    for (title, unit, list) in [("接入点", "个", relays), ("设备", "台", devices)] {
        let direct = list.iter().filter(|p| s(p, "cost") == "p2p").count();
        println!("{}{} {unit}在线，直连 {direct}", pad(title, 8), list.len());
        for p in list {
            let link = if s(p, "cost") == "p2p" { format!("直连 {}", s(p, "tunnel_proto")) } else { "中转".into() };
            let lat = s(p, "lat_ms").parse::<f64>().map(|l| format!("{l:.0} ms")).unwrap_or_else(|_| "-".into());
            println!(
                "  {} {} {} {:>8}  丢包 {}",
                pad(&s(p, "hostname"), 12),
                pad(&s(p, "ipv4"), 14),
                pad(&link, 10),
                lat,
                s(p, "loss_rate")
            );
        }
    }
    Ok(())
}

/// TUN 还是 userspace。配置是 600，不是 root 读不到，就问本机内核运行中的配置（管理口不要 root）
fn mode() -> String {
    // TUN 模式也可能开着回环 SOCKS5：lolinet mihomo on 开的，mihomo 经它进网
    let tun = |socks: Option<String>| match socks {
        Some(s) => format!("TUN · 另开了 socks5://{s}（给 mihomo 用）"),
        None => "TUN".to_owned(),
    };
    let userspace = |socks: Option<String>| {
        format!("userspace（无 TUN）· 本机程序经 socks5://{} 进网", socks.as_deref().unwrap_or(crate::netcfg::SOCKS))
    };
    match fs::read_to_string(CONFIG).ok().and_then(|t| crate::netcfg::Net::parse(&t).ok()) {
        Some(n) if n.userspace => userspace(n.socks),
        Some(n) => tun(n.socks),
        None if crate::manager::mode() == "userspace" => userspace(crate::manager::local_socks()),
        None => tun(crate::manager::local_socks_any()),
    }
}

/// pair.py 在配置末尾写的 `# credential_id` / `# credential_expiry_unix`；配置是 600，不是 root 读不到
fn credential() -> String {
    let text = match fs::read_to_string(CONFIG) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return "sudo lolinet 才看得到".into(),
        Err(_) => return format!("没有 {CONFIG}"),
    };
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix(" = ").map(str::trim).map(str::to_owned));
    let id = field("# credential_id").unwrap_or_else(|| "?".into());
    let Some(expiry) = field("# credential_expiry_unix").and_then(|e| e.parse::<i64>().ok()) else {
        return id;
    };
    let days = (expiry - chrono::Utc::now().timestamp()) / 86400;
    let date = chrono::DateTime::from_timestamp(expiry, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
    if days < 0 { format!("{id}，已过期（{date}）") } else { format!("{id}，还剩 {days} 天（{date} 到期）") }
}
