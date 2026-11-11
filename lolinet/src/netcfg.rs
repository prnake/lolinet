//! loli.toml 里 lolinet 自己要读、要改的几项：本机 overlay 地址（ipv4）、模式（[flags] no_tun）、本机 SOCKS5 入口（socks5_proxy）。
//!
//! 两种入网模式：
//! - TUN（默认，要 root）：系统里多一张网卡，overlay 地址整段走它；
//! - userspace（无 TUN）：EasyTier 用自己的用户态协议栈，本机程序经 127.0.0.1:18080 的 SOCKS5 进网；
//!   别人发到本机 overlay 地址的包由 EasyTier 转给 127.0.0.1 的同一个端口，所以 ssh、lolidesk、出口节点照样连得上。
//!   容器、没有 /dev/net/tun 的 NAS、手机或电脑上已经有 mihomo / 别的 VPN 占着 TUN 的，用这个。
//!
//! 按行改，不重新序列化：pair.py 写在末尾的 `# credential_id` 这类注释 lolinet status 要读，序列化会丢。

use std::net::{IpAddr, Ipv4Addr};

use anyhow::{Context as _, Result};

/// userspace 模式下本机程序进网用的 SOCKS5（只听回环）
pub const SOCKS: &str = "127.0.0.1:18080";

pub struct Net {
    /// 本机 overlay 地址和前缀（ipv4 = "10.188.16.10/16"）
    pub ip: Option<(Ipv4Addr, u8)>,
    pub userspace: bool,
    /// socks5_proxy 的地址部分（127.0.0.1:18080）
    pub socks: Option<String>,
}

impl Net {
    pub fn parse(text: &str) -> Result<Net> {
        let t: toml::Table = toml::from_str(text).context("配置不是合法的 TOML")?;
        let ip = t.get("ipv4").and_then(|v| v.as_str()).and_then(|s| {
            let (a, p) = s.split_once('/').unwrap_or((s, "32"));
            Some((a.parse().ok()?, p.parse().ok()?))
        });
        let userspace = t
            .get("flags")
            .and_then(|f| f.get("no_tun"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let socks = t
            .get("socks5_proxy")
            .and_then(|v| v.as_str())
            .map(|s| s.trim_start_matches("socks5://").trim_end_matches('/').to_owned());
        Ok(Net { ip, userspace, socks })
    }

    /// overlay 网段，如 10.188.0.0/16
    pub fn cidr(&self) -> Option<String> {
        let (ip, p) = self.ip?;
        let mask = if p == 0 { 0 } else { u32::MAX << (32 - p.min(32)) };
        Some(format!("{}/{p}", Ipv4Addr::from(u32::from(ip) & mask)))
    }

    /// 本机 overlay 地址
    pub fn addr(&self) -> Option<IpAddr> {
        self.ip.map(|(a, _)| a.into())
    }
}

/// 切模式：userspace 加 socks5_proxy 和 [flags] no_tun = true，TUN 把两项去掉
pub fn set_mode(text: &str, userspace: bool) -> String {
    let t = set_top(text, "socks5_proxy", userspace.then(|| format!("\"socks5://{SOCKS}\"")));
    set_in(&t, "flags", "no_tun", userspace.then(|| "true".to_owned()))
}

fn key_of(line: &str) -> Option<&str> {
    let l = line.trim_start();
    if l.is_empty() || l.starts_with('#') || l.starts_with('[') || !l.contains('=') {
        return None;
    }
    Some(l.split('=').next()?.trim())
}

fn is_section(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

/// 顶层键（第一个 [节] 之前）：有就换、None 就删、没有就加在顶层最后一个非空行后面
fn set_top(text: &str, key: &str, value: Option<String>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let end = lines.iter().position(|l| is_section(l)).unwrap_or(lines.len());
    edit(&mut lines, 0, end, key, value);
    join(lines)
}

/// [section] 里的键：有就换、None 就删、没有就加在节头后面；整个节都没有就在文件末尾新建
fn set_in(text: &str, section: &str, key: &str, value: Option<String>) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let head = format!("[{section}]");
    match lines.iter().position(|l| l.trim() == head) {
        Some(h) => {
            let end = lines[h + 1..].iter().position(|l| is_section(l)).map_or(lines.len(), |i| h + 1 + i);
            edit(&mut lines, h + 1, end, key, value);
        }
        None => {
            if let Some(v) = value {
                while lines.last().is_some_and(|l| l.trim().is_empty()) {
                    lines.pop();
                }
                lines.extend(["".into(), head, format!("{key} = {v}")]);
            }
        }
    }
    join(lines)
}

fn edit(lines: &mut Vec<String>, start: usize, end: usize, key: &str, value: Option<String>) {
    let found: Vec<usize> = (start..end).filter(|&i| key_of(&lines[i]) == Some(key)).collect();
    match (found.first(), value) {
        (Some(&i), Some(v)) => {
            lines[i] = format!("{key} = {v}");
            for &j in found[1..].iter().rev() {
                lines.remove(j);
            }
        }
        (Some(_), None) => {
            for &j in found.iter().rev() {
                lines.remove(j);
            }
        }
        (None, Some(v)) => {
            // 节头后面紧接着放；顶层放在最后一个非空、非注释行后面（别插进末尾的 # credential 注释里）
            let at = if start == 0 {
                (start..end).rev().find(|&i| key_of(&lines[i]).is_some()).map_or(start, |i| i + 1)
            } else {
                start
            };
            lines.insert(at, format!("{key} = {v}"));
        }
        (None, None) => {}
    }
}

fn join(lines: Vec<String>) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}
