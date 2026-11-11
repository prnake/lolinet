//! 配对的命令行：
//! - `lolidesk pair <分享链接 | 主机>`：链接带着主机地址、证书指纹和配对码，直接配对；给主机就请那边的人同意
//!   （`--code` 用配对码）。配对上的凭据记在 hosts 里，之后 `lolidesk <主机>` 直接连。
//! - `lolidesk share <主机>`：给一台配对过的电脑发分享链接（别的机器用它配对，不用再同意）。
//! - `lolidesk access <主机>`：在那台电脑上，我和我发的分享链接配对了谁。
//! - `lolidesk host access|approve|deny|code|revoke|limit`：管这台电脑自己的配对。以主人身份（主机 token）
//!   经回环连本机在跑的主机；主机没在跑时 access 读存着的记录，别的要先起主机。

use std::{net::SocketAddr, sync::Arc};

use anyhow::{bail, Context, Result};
use lolidesk_core::{
    hosts::Hosts,
    local,
    net::{self, Client},
};
use lolidesk_host::{access::Store, HostCmd};
use lolidesk_proto::{
    AccessInfo, Allow, ControlReq, ControlResp, DeviceInfo, PairLink, Role, BUILD,
};

fn device() -> DeviceInfo {
    DeviceInfo::this("cli")
}

/// 30m、12h、7d、90s、2w → 秒；never、0 → None（不过期）
pub fn parse_duration(s: &str) -> Result<Option<u64>> {
    let s = s.trim();
    if matches!(s, "never" | "0" | "") {
        return Ok(None);
    }
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num
        .parse()
        .with_context(|| format!("看不懂时长 {s}：写 30m、12h、7d"))?;
    let mul = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 7 * 86400,
        _ => bail!("看不懂时长 {s}：写 30m、12h、7d"),
    };
    Ok(Some(n.checked_mul(mul).context("时长太长了")?))
}

fn now() -> u64 {
    lolidesk_host::access::now()
}

/// 多久以前（给人看）
pub fn ago(t: u64) -> String {
    let d = now().saturating_sub(t);
    match d {
        0..=59 => "刚才".into(),
        60..=3599 => format!("{} 分钟前", d / 60),
        3600..=86399 => format!("{} 小时前", d / 3600),
        _ => format!("{} 天前", d / 86400),
    }
}

/// 还有多久（给人看）
fn left(t: u64) -> String {
    let d = t.saturating_sub(now());
    match d {
        0 => "已过期".into(),
        1..=3599 => format!("{} 分钟后过期", d.div_ceil(60)),
        3600..=86399 => format!("{} 小时后过期", d / 3600),
        _ => format!("{} 天后过期", d / 86400),
    }
}

/// 新主机：先只握手拿指纹，给人核对（或 -y）
pub async fn confirm_fingerprint(
    addr: SocketAddr,
    yes: bool,
    interactive: bool,
) -> Result<[u8; 32]> {
    let fp = net::fingerprint(addr).await?;
    if yes {
        return Ok(fp);
    }
    if !interactive {
        bail!("没和 {addr} 配对过：先在终端里 lolidesk pair {addr}，或者加 -y");
    }
    eprint!(
        "新主机 {addr} 的证书指纹：\n  {}\n能看到对方的话，和它 `lolidesk host token` 打印的对一下。信任？[y/N] ",
        hex::encode(fp)
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !matches!(line.trim(), "y" | "Y" | "yes") {
        bail!("没有信任，退出");
    }
    Ok(fp)
}

/// 配对并把凭据记进 hosts；返回主机名
pub async fn pair_and_remember(
    addr: SocketAddr,
    fp: [u8; 32],
    code: Option<String>,
) -> Result<(String, String)> {
    if code.is_none() {
        eprintln!(
            "已请 {addr} 同意配对：在那台电脑上点「同意」（最多 {} 秒）…",
            lolidesk_proto::PAIR_WAIT_SECS
        );
    }
    let p = net::pair(addr, Some(fp), device(), code).await?;
    let mut hosts = Hosts::load()?;
    hosts.remember(addr, &hex::encode(p.fingerprint), &p.credential, &p.host);
    hosts.save()?;
    Ok((p.host, p.credential))
}

/// `lolidesk pair <链接 | 主机> [--code]`
pub async fn pair_cmd(target: &str, code: Option<String>, yes: bool) -> Result<()> {
    if let Some(link) = PairLink::parse(target) {
        let addr = net::resolve(&link.addr)?;
        let fp: [u8; 32] = hex::decode(&link.fingerprint)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("指纹不对"))?;
        let (host, _) = pair_and_remember(addr, fp, Some(link.code)).await?;
        println!("已和 {host}（{addr}）配对。连它：lolidesk {addr}");
        return Ok(());
    }
    if target.starts_with("lolidesk://") {
        bail!("分享链接不完整：要 lolidesk://pair?h=…&f=…&c=…");
    }
    let addr = net::resolve(target)?;
    let known = Hosts::load()?
        .get(&addr)
        .and_then(|h| h.fingerprint_bytes());
    let fp = match known {
        Some(fp) => fp,
        None => confirm_fingerprint(addr, yes, true).await?,
    };
    let (host, _) = pair_and_remember(addr, fp, code).await?;
    println!("已和 {host}（{addr}）配对，以后连它不用再同意。连它：lolidesk {addr}");
    Ok(())
}

async fn call(c: &Client, req: ControlReq) -> Result<ControlResp> {
    match c.control(req).await? {
        ControlResp::Error(e) | ControlResp::Denied(e) => bail!("{e}"),
        r => Ok(r),
    }
}

async fn access_of(c: &Client) -> Result<AccessInfo> {
    match call(c, ControlReq::Access).await? {
        ControlResp::Access(a) => Ok(a),
        r => bail!("回应不对：{r:?}"),
    }
}

/// 发配对码，拼成分享链接。`via`：我连这台主机用的地址（不是回环的话，链接里用它）
async fn make_link(
    c: &Client,
    fingerprint: &str,
    via: SocketAddr,
    label: String,
    expires: Option<String>,
    uses: Option<u32>,
    view_only: bool,
) -> Result<()> {
    let expires_in = match expires {
        Some(e) => parse_duration(&e)?,
        None => None,
    };
    let req = ControlReq::CreateCode {
        label,
        expires_in,
        max_uses: uses,
        allow: view_only.then(Allow::view_only),
    };
    let ControlResp::Code { info, secret, addr } = call(c, req).await? else {
        bail!("主机没给配对码");
    };
    let addr = if via.ip().is_loopback() {
        addr
    } else {
        via.to_string()
    };
    let link = PairLink {
        addr,
        fingerprint: fingerprint.to_string(),
        code: secret.clone(),
        name: c.welcome.host.clone(),
    };
    println!("配对码    {secret}（{}）", info.id);
    println!("分享链接  {}", link.to_url());
    let mut notes = vec![match info.expires {
        Some(t) => left(t),
        None => "不过期".into(),
    }];
    if let Some(m) = info.max_uses {
        notes.push(format!("最多 {m} 台"));
    }
    if let Some(a) = info.allow {
        notes.push(format!("能：{}", a.summary()));
    }
    println!(
        "          {}；别的机器 lolidesk pair <链接>，或者在 Lolidesk 里导入",
        notes.join("，")
    );
    println!(
        "          吊销：lolidesk host revoke {}（用它配对的设备一起失效）",
        info.id
    );
    Ok(())
}

/// `lolidesk share <主机>`：要先和它配对过
pub async fn share(
    c: Arc<Client>,
    label: String,
    expires: Option<String>,
    uses: Option<u32>,
    view_only: bool,
) -> Result<()> {
    let fp = Hosts::load()?
        .get(&c.addr)
        .map(|h| h.fingerprint)
        .context("这台主机没记住")?;
    make_link(&c, &fp, c.addr, label, expires, uses, view_only).await
}

/// `lolidesk access <主机>`
pub async fn access_remote(c: Arc<Client>) -> Result<()> {
    let info = access_of(&c).await?;
    print_access(&info, &c.welcome.host);
    Ok(())
}

/// 以主人身份连本机在跑的主机
async fn owner() -> Result<Arc<Client>> {
    let l = local::local_host()
        .context("这台电脑还没跑过 lolidesk 主机：lolidesk host install（或 lolidesk host run）")?;
    let (c, _) = net::connect(
        l.addr,
        &l.token,
        Some(l.fingerprint),
        &format!("lolidesk {BUILD}"),
    )
    .await
    .with_context(|| {
        format!(
            "连不上本机的主机 {}：它没在跑（lolidesk host install 起服务）",
            l.addr
        )
    })?;
    Ok(c)
}

/// `lolidesk host access|approve|deny|code|revoke|limit`
pub async fn host_cmd(cmd: HostCmd) -> Result<()> {
    let c = owner().await;
    let c = match (cmd, c) {
        (HostCmd::Access, Err(e)) => {
            // 主机没在跑：读存着的记录（没有等着同意的请求，也不知道谁在线）
            eprintln!("{e:#}\n下面是存着的记录：\n");
            let path = local::host_dir().join("access.json");
            let info = Store::load(&path)?.view(&Role::Owner, &|_| 0);
            print_access(&info, &lolidesk_proto::hostname());
            return Ok(());
        }
        (cmd, c) => (cmd, c?),
    };
    match c {
        (HostCmd::Access, c) => {
            let info = access_of(&c).await?;
            print_access(&info, &c.welcome.host);
        }
        (HostCmd::Approve { id }, c) => {
            call(&c, ControlReq::Decide { id, allow: true }).await?;
            println!("已同意请求 {id}：那台设备配对上了");
        }
        (HostCmd::Deny { id }, c) => {
            call(&c, ControlReq::Decide { id, allow: false }).await?;
            println!("已拒绝请求 {id}");
        }
        (
            HostCmd::Code {
                label,
                expires,
                uses,
                view_only,
            },
            c,
        ) => {
            let fp = hex::encode(
                local::local_host()
                    .context("读不到本机主机的证书")?
                    .fingerprint,
            );
            make_link(&c, &fp, c.addr, label, expires, uses, view_only).await?;
        }
        (HostCmd::Revoke { id }, c) => {
            call(&c, ControlReq::Revoke(id.clone())).await?;
            println!("已吊销 {id}（连着的也断开了）");
        }
        (
            HostCmd::Limit {
                grant,
                what,
                state,
                reset,
            },
            c,
        ) => {
            let allow = if reset {
                None
            } else {
                let (Some(what), Some(state)) = (what, state) else {
                    bail!("lolidesk host limit {grant} <terminal|desktop|control|cua|share> <on|off>，或者 --reset");
                };
                let on = match state.as_str() {
                    "on" | "true" | "yes" => true,
                    "off" | "false" | "no" => false,
                    _ => bail!("要写 on 或 off"),
                };
                let info = access_of(&c).await?;
                let g = info
                    .grants
                    .iter()
                    .find(|g| g.id == grant)
                    .with_context(|| format!("没有设备 {grant}"))?;
                let mut a = g.allow.unwrap_or_default();
                a.set(&what, on).map_err(anyhow::Error::msg)?;
                Some(a)
            };
            call(
                &c,
                ControlReq::Limit {
                    grant: grant.clone(),
                    allow,
                },
            )
            .await?;
            match allow {
                Some(a) => println!("{grant} 现在能：{}（再和主机开放的取交集）", a.summary()),
                None => println!("{grant} 的限制去掉了：和主机开放的一样"),
            }
        }
        (_, _) => bail!("不是管配对的命令"),
    }
    Ok(())
}

fn device_line(d: &DeviceInfo) -> String {
    let mut extra = vec![];
    if !d.os.is_empty() {
        extra.push(d.os.as_str());
    }
    if !d.kind.is_empty() {
        extra.push(d.kind.as_str());
    }
    if extra.is_empty() {
        d.name.clone()
    } else {
        format!("{}（{}）", d.name, extra.join("，"))
    }
}

/// 打印：设备按配对方式分组（主人同意的一组，每个配对码一组，码下面是用它配对的设备和最近登录），再是等着同意的请求
pub fn print_access(info: &AccessInfo, host: &str) {
    let ip = |a: &str| {
        a.parse::<SocketAddr>()
            .map(|s| s.ip().to_string())
            .unwrap_or_else(|_| a.to_string())
    };
    let grant_line = |g: &lolidesk_proto::GrantInfo| {
        let online = if g.online > 0 {
            "  在线".to_string()
        } else {
            String::new()
        };
        let me = if info.me.as_deref() == Some(g.id.as_str()) {
            "  （我）"
        } else {
            ""
        };
        let limit = g
            .allow
            .map(|a| format!("  只能：{}", a.summary()))
            .unwrap_or_default();
        format!(
            "    {}  {}  {}  最近 {}{online}{me}{limit}",
            g.id,
            device_line(&g.device),
            ip(&g.last_addr),
            ago(g.last_seen)
        )
    };
    let who = if info.owner {
        format!("{host} 配对给了 {} 台设备", info.grants.len())
    } else {
        format!("我在 {host} 上")
    };
    println!("{who}");
    let approved: Vec<_> = info.grants.iter().filter(|g| g.code.is_none()).collect();
    if !approved.is_empty() {
        println!("\n  经主人同意");
        for g in approved {
            println!("{}", grant_line(g));
        }
    }
    for c in &info.codes {
        let by = match &c.created_by {
            None => "主人发的".to_string(),
            Some(g) => {
                let name = info
                    .grants
                    .iter()
                    .find(|x| &x.id == g)
                    .map(|x| x.device.name.clone())
                    .unwrap_or_else(|| g.clone());
                format!("{name} 的分享链接")
            }
        };
        let uses = match c.max_uses {
            Some(m) => format!("用了 {}/{m}", c.uses),
            None => format!("用了 {}", c.uses),
        };
        let exp = c.expires.map(left).unwrap_or_else(|| "不过期".into());
        let label = if c.label.is_empty() {
            String::new()
        } else {
            format!("「{}」", c.label)
        };
        let limit = c
            .allow
            .map(|a| format!("，只能：{}", a.summary()))
            .unwrap_or_default();
        println!("\n  配对码 {}{label}  {by}，{uses}，{exp}{limit}", c.id);
        let under: Vec<_> = info
            .grants
            .iter()
            .filter(|g| g.code.as_deref() == Some(c.id.as_str()))
            .collect();
        if under.is_empty() {
            println!("    （还没有设备用它配对）");
        }
        for g in under {
            println!("{}", grant_line(g));
        }
    }
    // 码已经被吊销、设备还在的不会有；码不在我能看的范围里的（设备看自己）单列
    let orphan: Vec<_> = info
        .grants
        .iter()
        .filter(|g| {
            g.code
                .as_deref()
                .is_some_and(|c| !info.codes.iter().any(|x| x.id == c))
        })
        .collect();
    if !orphan.is_empty() {
        println!("\n  用配对码配对");
        for g in orphan {
            println!("{}", grant_line(g));
        }
    }
    if !info.pending.is_empty() {
        println!("\n  等着同意");
        for p in &info.pending {
            println!(
                "    #{}  {}  {}  {}  →  lolidesk host approve {} / deny {}",
                p.id,
                device_line(&p.device),
                ip(&p.addr),
                ago(p.since),
                p.id,
                p.id
            );
        }
    }
    if info.owner {
        println!("\n吊销：lolidesk host revoke <g-… 或 c-…>；发配对码：lolidesk host code --label <给谁>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m").unwrap(), Some(1800));
        assert_eq!(parse_duration("7d").unwrap(), Some(7 * 86400));
        assert_eq!(parse_duration("90").unwrap(), Some(90));
        assert_eq!(parse_duration("never").unwrap(), None);
        assert!(parse_duration("3x").is_err());
        assert!(parse_duration("d").is_err());
    }
}
