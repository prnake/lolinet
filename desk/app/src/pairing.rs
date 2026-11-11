//! 配对的文字和分组：和命令行（desk/client/src/pair.rs 的 `print_access`）一样的说法。纯函数，单测覆盖。

use std::net::SocketAddr;

use lolidesk_proto::{AccessInfo, Allow, CodeInfo, DeviceInfo, GrantInfo, PairLink};

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 多久以前（给人看）
pub fn ago(now: u64, t: u64) -> String {
    let d = now.saturating_sub(t);
    match d {
        0..=59 => "刚才".into(),
        60..=3599 => format!("{} 分钟前", d / 60),
        3600..=86399 => format!("{} 小时前", d / 3600),
        _ => format!("{} 天前", d / 86400),
    }
}

/// 还有多久过期（给人看）
pub fn left(now: u64, t: u64) -> String {
    let d = t.saturating_sub(now);
    match d {
        0 => "已过期".into(),
        1..=3599 => format!("{} 分钟后过期", d.div_ceil(60)),
        3600..=86399 => format!("{} 小时后过期", d / 3600),
        _ => format!("{} 天后过期", d / 86400),
    }
}

/// 地址只留 IP（`10.188.1.2:52314` → `10.188.1.2`）
pub fn ip_of(a: &str) -> String {
    a.parse::<SocketAddr>()
        .map(|s| s.ip().to_string())
        .unwrap_or_else(|_| a.to_string())
}

/// 系统、种类：`macos · app`
pub fn device_kind(d: &DeviceInfo) -> String {
    [d.os.as_str(), d.kind.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// 一台设备的第二行：`macos · app · 10.188.16.11 · 最近 3 分钟前`
pub fn grant_line(g: &GrantInfo, now: u64) -> String {
    let mut parts = vec![];
    let kind = device_kind(&g.device);
    if !kind.is_empty() {
        parts.push(kind);
    }
    if !g.last_addr.is_empty() {
        parts.push(ip_of(&g.last_addr));
    }
    parts.push(format!("最近 {}", ago(now, g.last_seen)));
    parts.join(" · ")
}

/// 一个配对码是谁发的：主人，或者「某台设备的分享链接」
pub fn code_by(info: &AccessInfo, c: &CodeInfo) -> String {
    match &c.created_by {
        None => "主人发的".into(),
        Some(g) => {
            let name = info
                .grants
                .iter()
                .find(|x| &x.id == g)
                .map(|x| x.device.name.clone())
                .unwrap_or_else(|| g.clone());
            format!("{name} 的分享链接")
        }
    }
}

/// 配对码的一行说明：`主人发的 · 用了 1/5 · 6 天后过期 · 只能：终端、看桌面`
pub fn code_line(info: &AccessInfo, c: &CodeInfo, now: u64) -> String {
    let uses = match c.max_uses {
        Some(m) => format!("用了 {}/{m}", c.uses),
        None => format!("用了 {}", c.uses),
    };
    let exp = c
        .expires
        .map(|t| left(now, t))
        .unwrap_or_else(|| "不过期".into());
    let mut parts = vec![code_by(info, c), uses, exp];
    if let Some(a) = c.allow {
        parts.push(format!("只能：{}", a.summary()));
    }
    parts.join(" · ")
}

/// 设备按怎么配对的分组（和 `print_access` 一样）：主人同意的、每个配对码和用它配对的设备、
/// 码不在能看的范围里的（设备看自己时）
pub struct Groups<'a> {
    pub approved: Vec<&'a GrantInfo>,
    pub codes: Vec<(&'a CodeInfo, Vec<&'a GrantInfo>)>,
    pub orphans: Vec<&'a GrantInfo>,
}

pub fn group(info: &AccessInfo) -> Groups<'_> {
    let approved = info.grants.iter().filter(|g| g.code.is_none()).collect();
    let codes = info
        .codes
        .iter()
        .map(|c| {
            let under = info
                .grants
                .iter()
                .filter(|g| g.code.as_deref() == Some(c.id.as_str()))
                .collect();
            (c, under)
        })
        .collect();
    let orphans = info
        .grants
        .iter()
        .filter(|g| {
            g.code
                .as_deref()
                .is_some_and(|c| !info.codes.iter().any(|x| x.id == c))
        })
        .collect();
    Groups {
        approved,
        codes,
        orphans,
    }
}

/// 吊销前给人看的后果
pub fn revoke_note(info: &AccessInfo, id: &str) -> String {
    if id.starts_with("c-") {
        let n = info
            .grants
            .iter()
            .filter(|g| g.code.as_deref() == Some(id))
            .count();
        if n == 0 {
            "这个配对码不能再用了。".into()
        } else {
            format!("这个配对码不能再用了，用它配对的 {n} 台设备也一起断开、失效。")
        }
    } else if info.me.as_deref() == Some(id) {
        "这台电脑在对方那里的配对就没了，要连得重新配对。".into()
    } else {
        "它马上断开、不能再连；它发的分享链接和经那些链接进来的设备也一起失效。".into()
    }
}

// ---------------------------------------------------------------- 新配对码

/// 过期的选项（秒；None 不过期）
pub const EXPIRES: [(Option<u64>, &str); 4] = [
    (None, "不过期"),
    (Some(3600), "1 小时"),
    (Some(86400), "1 天"),
    (Some(7 * 86400), "7 天"),
];

/// 次数的选项（None 不限）
pub const USES: [(Option<u32>, &str); 3] = [(None, "不限"), (Some(1), "1 台"), (Some(5), "5 台")];

/// 分享链接：主机给的地址是它在网里的（拼链接用）；我连它不是经回环的话，用我连它的地址（和命令行一样）
pub fn share_link(
    via: SocketAddr,
    host_addr: &str,
    fingerprint: &str,
    secret: &str,
    name: &str,
) -> PairLink {
    let addr = if via.ip().is_loopback() {
        host_addr.to_string()
    } else {
        via.to_string()
    };
    PairLink {
        addr,
        fingerprint: fingerprint.to_string(),
        code: secret.to_string(),
        name: name.to_string(),
    }
}

/// 添加主机时填的「配对码 / token」：主机 token 是 32 位十六进制，别的当配对码
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    None,
    Token(String),
    Code(String),
}

pub fn classify_secret(s: &str) -> Secret {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        Secret::None
    } else if t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()) {
        Secret::Token(t.to_ascii_lowercase())
    } else {
        Secret::Code(t)
    }
}

/// 设备限制的五个开关（`pair` 只看主机的，不在这里）
pub const LIMITS: [(&str, &str); 5] = [
    ("terminal", "终端"),
    ("desktop", "看桌面"),
    ("control", "操作桌面"),
    ("cua", "agent"),
    ("share", "分享"),
];

pub fn limit_value(a: &Allow, what: &str) -> bool {
    match what {
        "terminal" => a.terminal,
        "desktop" => a.desktop,
        "control" => a.control,
        "cua" => a.cua,
        "share" => a.share,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.into(),
            os: "macos".into(),
            kind: "app".into(),
            version: "x".into(),
        }
    }

    fn grant(id: &str, name: &str, code: Option<&str>) -> GrantInfo {
        GrantInfo {
            id: id.into(),
            device: dev(name),
            code: code.map(Into::into),
            created: 0,
            created_addr: String::new(),
            last_seen: 1000,
            last_addr: "10.188.16.11:52000".into(),
            allow: None,
            online: 0,
        }
    }

    fn code(id: &str, by: Option<&str>) -> CodeInfo {
        CodeInfo {
            id: id.into(),
            label: String::new(),
            created: 0,
            created_by: by.map(Into::into),
            expires: Some(1000 + 7 * 86400),
            max_uses: Some(5),
            uses: 1,
            allow: Some(Allow::view_only()),
        }
    }

    #[test]
    fn times() {
        assert_eq!(ago(1000, 990), "刚才");
        assert_eq!(ago(1000 + 180, 1000), "3 分钟前");
        assert_eq!(ago(1000 + 7200, 1000), "2 小时前");
        assert_eq!(ago(1000 + 3 * 86400, 1000), "3 天前");
        assert_eq!(left(1000, 900), "已过期");
        assert_eq!(left(1000, 1061), "2 分钟后过期");
        assert_eq!(left(0, 7 * 86400), "7 天后过期");
    }

    #[test]
    fn groups_like_the_cli() {
        let info = AccessInfo {
            owner: true,
            me: None,
            grants: vec![
                grant("g-a", "laptop", None),
                grant("g-b", "air", Some("c-1")),
                grant("g-c", "nas", Some("c-2")),
                grant("g-d", "old", Some("c-gone")),
            ],
            codes: vec![code("c-1", None), code("c-2", Some("g-a"))],
            pending: vec![],
        };
        let g = group(&info);
        assert_eq!(g.approved.len(), 1);
        assert_eq!(g.codes.len(), 2);
        assert_eq!(g.codes[0].1[0].id, "g-b");
        assert_eq!(g.orphans[0].id, "g-d");
        assert_eq!(code_by(&info, &info.codes[1]), "laptop 的分享链接");
        assert_eq!(
            code_line(&info, &info.codes[0], 1000),
            "主人发的 · 用了 1/5 · 7 天后过期 · 只能：终端、看桌面"
        );
        assert_eq!(
            grant_line(&info.grants[0], 1000 + 120),
            "macos · app · 10.188.16.11 · 最近 2 分钟前"
        );
        assert!(revoke_note(&info, "c-2").contains("1 台设备"));
        assert!(revoke_note(&info, "g-a").contains("分享链接"));
    }

    #[test]
    fn secrets_and_links() {
        assert_eq!(classify_secret("  "), Secret::None);
        assert_eq!(
            classify_secret("0123456789ABCDEF0123456789abcdef"),
            Secret::Token("0123456789abcdef0123456789abcdef".into())
        );
        assert_eq!(
            classify_secret("abcd-efgh jkmn-pqrs"),
            Secret::Code("abcd-efghjkmn-pqrs".into())
        );
        let fp = "ab".repeat(32);
        // 经回环连的本机：链接里用主机给的网里地址
        let l = share_link(
            "127.0.0.1:10190".parse().unwrap(),
            "10.188.16.10:10190",
            &fp,
            "ABCD-EFGH-JKMN-PQRS",
            "studio",
        );
        assert_eq!(l.addr, "10.188.16.10:10190");
        assert_eq!(PairLink::parse(&l.to_url()), Some(l));
        // 别的主机：用我连它的地址
        let l = share_link(
            "10.188.1.2:10190".parse().unwrap(),
            "0.0.0.0:10190",
            &fp,
            "X",
            "d",
        );
        assert_eq!(l.addr, "10.188.1.2:10190");
        let a = Allow {
            cua: false,
            ..Allow::default()
        };
        assert!(!limit_value(&a, "cua") && limit_value(&a, "terminal"));
    }
}
