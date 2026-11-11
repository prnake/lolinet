//! 谁能连这台电脑：`<主机目录>/access.json`（600），配对过的设备（grant）和配对码（code）。
//!
//! - 主机自己的 token 是主人的钥匙（本机 app、本机命令行），不在这里。
//! - 设备的凭据 `g1.<id>.<32 位十六进制>` 只在配对时给一次，这里只存 sha256。
//! - 配对码 16 位（大写字母和 2-9，去掉长得像的），也只存 sha256；可以有过期时间、次数上限、能力限制。
//!   过期、用满只挡以后的配对，已经用它配对的设备不受影响。
//! - 吊销一个码：用它配对的设备一起失效。吊销一台设备：它发的码（分享链接）和用那些码配对的设备一起失效。
//! - 设备能做什么 = 主机开放的 ∧ 它自己的限制（主人设的，或者配对码带的）。
//!
//! 这里只管数据，网络、同意框在 net.rs。

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use lolidesk_proto::{AccessInfo, Allow, CodeInfo, DeviceInfo, GrantInfo, Role};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 配对码的字母表（和 lolinet 的配对码一样，没有 0 O 1 I L）
pub const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 16;
/// 设备每次连上都记最近登录；地址没变时最多这么久写一次盘
const SEEN_EVERY: u64 = 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    hash: String,
    pub device: DeviceInfo,
    /// 用哪个配对码配对的；主人同意的是 None
    pub code: Option<String>,
    pub created: u64,
    pub created_addr: String,
    pub last_seen: u64,
    pub last_addr: String,
    #[serde(default)]
    pub allow: Option<Allow>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Code {
    pub id: String,
    hash: String,
    pub label: String,
    pub created: u64,
    /// 谁发的：主人是 None，设备（分享链接）是它的 g-id
    pub created_by: Option<String>,
    pub expires: Option<u64>,
    pub max_uses: Option<u32>,
    pub uses: u32,
    #[serde(default)]
    pub allow: Option<Allow>,
}

#[derive(Default, Serialize, Deserialize)]
struct Data {
    #[serde(default)]
    grants: Vec<Grant>,
    #[serde(default)]
    codes: Vec<Code>,
}

pub struct Store {
    /// None：只在内存里（测试，或者文件坏了不敢覆盖）
    path: Option<PathBuf>,
    data: Data,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sha(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn random_hex(bytes: usize) -> String {
    use rand::RngCore;
    let mut b = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

fn random_code() -> String {
    use rand::Rng;
    let mut r = rand::rng();
    (0..CODE_LEN)
        .map(|_| CODE_ALPHABET[r.random_range(0..CODE_ALPHABET.len())] as char)
        .collect()
}

/// 配对码统一成大写、只留字母数字（XXXX-XXXX-XXXX-XXXX、小写、带空格的都认）
pub fn normalize_code(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// 给人看的：每 4 位一个 -
pub fn format_code(s: &str) -> String {
    s.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// 设备自报的东西只当显示用：去掉控制字符、截短
pub fn clean_device(d: DeviceInfo) -> DeviceInfo {
    fn clip(s: &str, n: usize) -> String {
        let s: String = s.chars().filter(|c| !c.is_control()).take(n).collect();
        s.trim().to_string()
    }
    let name = clip(&d.name, 64);
    DeviceInfo {
        name: if name.is_empty() {
            "unknown".into()
        } else {
            name
        },
        os: clip(&d.os, 32),
        kind: clip(&d.kind, 16),
        version: clip(&d.version, 32),
    }
}

impl Store {
    pub fn load(path: &Path) -> Result<Store> {
        let data = match fs::read(path) {
            Ok(b) => serde_json::from_slice(&b)
                .with_context(|| format!("{} 不是合法的 JSON", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(e) => return Err(e).with_context(|| format!("读 {}", path.display())),
        };
        Ok(Store {
            path: Some(path.to_path_buf()),
            data,
        })
    }

    pub fn memory() -> Store {
        Store {
            path: None,
            data: Data::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::config::write_private(path, &serde_json::to_vec_pretty(&self.data)?)
    }

    fn new_id(&self, prefix: &str) -> String {
        loop {
            let id = format!("{prefix}-{}", random_hex(4));
            let used = self.data.grants.iter().any(|g| g.id == id)
                || self.data.codes.iter().any(|c| c.id == id);
            if !used {
                return id;
            }
        }
    }

    pub fn grant(&self, id: &str) -> Option<&Grant> {
        self.data.grants.iter().find(|g| g.id == id)
    }

    pub fn grants(&self) -> &[Grant] {
        &self.data.grants
    }

    pub fn codes(&self) -> &[Code] {
        &self.data.codes
    }

    /// 给一台设备签凭据，返回（设备 id，凭据）。凭据只这一次给出去
    pub fn issue(
        &mut self,
        device: DeviceInfo,
        code: Option<String>,
        addr: &str,
        allow: Option<Allow>,
        now: u64,
    ) -> (String, String) {
        let id = self.new_id("g");
        let secret = random_hex(16);
        self.data.grants.push(Grant {
            id: id.clone(),
            hash: sha(&secret),
            device: clean_device(device),
            code,
            created: now,
            created_addr: addr.to_string(),
            last_seen: now,
            last_addr: addr.to_string(),
            allow,
        });
        (id.clone(), format!("g1.{id}.{secret}"))
    }

    /// Hello 带来的凭据对不对；对的话记下最近登录。返回（设备 id，要不要写盘）
    pub fn check(&mut self, credential: &str, addr: &str, now: u64) -> Option<(String, bool)> {
        let rest = credential.strip_prefix("g1.")?;
        let (id, secret) = rest.split_once('.')?;
        let g = self.data.grants.iter_mut().find(|g| g.id == id)?;
        if !same(&g.hash, &sha(secret)) {
            return None;
        }
        let changed = g.last_addr != addr || now.saturating_sub(g.last_seen) >= SEEN_EVERY;
        g.last_seen = now;
        g.last_addr = addr.to_string();
        Some((g.id.clone(), changed))
    }

    /// 用配对码配对：码对、没过期、没用满，就签一条凭据（带着码的能力限制）
    pub fn redeem(
        &mut self,
        code: &str,
        device: DeviceInfo,
        addr: &str,
        now: u64,
    ) -> Result<(String, String)> {
        let code = normalize_code(code);
        if code.len() != CODE_LEN {
            bail!("配对码是 {CODE_LEN} 位");
        }
        let h = sha(&code);
        let c = self
            .data
            .codes
            .iter_mut()
            .find(|c| same(&c.hash, &h))
            .context("配对码不对，或者已经被吊销了")?;
        if c.expires.is_some_and(|e| now >= e) {
            bail!("配对码已经过期了");
        }
        if c.max_uses.is_some_and(|m| c.uses >= m) {
            bail!("配对码已经用满了");
        }
        c.uses += 1;
        let (id, allow) = (c.id.clone(), c.allow);
        Ok(self.issue(device, Some(id), addr, allow, now))
    }

    /// 发一个配对码。设备发的（分享链接）不能比它自己的权限大
    pub fn create_code(
        &mut self,
        label: &str,
        created_by: Option<String>,
        expires_in: Option<u64>,
        max_uses: Option<u32>,
        allow: Option<Allow>,
        now: u64,
    ) -> Result<(Code, String)> {
        let label: String = label.chars().filter(|c| !c.is_control()).take(64).collect();
        if max_uses == Some(0) {
            bail!("次数上限至少是 1");
        }
        if expires_in.is_some_and(|e| e == 0 || e > 10 * 365 * 86400) {
            bail!("过期时间要在 1 秒到 10 年之间");
        }
        let creator_limit = match &created_by {
            Some(g) => self.grant(g).context("这台设备已经被吊销了")?.allow,
            None => None,
        };
        let allow = match (allow, creator_limit) {
            (None, None) => None,
            (a, c) => Some(a.unwrap_or_default().and(c.unwrap_or_default())),
        };
        let secret = random_code();
        let code = Code {
            id: self.new_id("c"),
            hash: sha(&secret),
            label: label.trim().to_string(),
            created: now,
            created_by,
            expires: expires_in.map(|e| now + e),
            max_uses,
            uses: 0,
            allow,
        };
        self.data.codes.push(code.clone());
        Ok((code, secret))
    }

    /// 谁能吊销什么：主人什么都行；设备能吊销自己、自己发的码、用自己的码配对的设备
    fn may_revoke(&self, id: &str, by: &Role) -> bool {
        let me = match by {
            Role::Owner => return true,
            Role::Device(me) => me,
        };
        if id == me {
            return true;
        }
        let mine = |code: &str| {
            self.data
                .codes
                .iter()
                .any(|c| c.id == code && c.created_by.as_deref() == Some(me))
        };
        if id.starts_with("c-") {
            return mine(id);
        }
        self.grant(id)
            .and_then(|g| g.code.as_deref())
            .is_some_and(mine)
    }

    /// 吊销（连带）。返回失效的设备 id（要断开它们的连接）
    pub fn revoke(&mut self, id: &str, by: &Role) -> Result<Vec<String>> {
        let known = self.grant(id).is_some() || self.data.codes.iter().any(|c| c.id == id);
        if !known {
            bail!("没有 {id}（可能已经吊销了）");
        }
        if !self.may_revoke(id, by) {
            bail!("只有这台电脑的主人能吊销 {id}");
        }
        let (mut gq, mut cq): (Vec<String>, Vec<String>) = if id.starts_with("c-") {
            (vec![], vec![id.to_string()])
        } else {
            (vec![id.to_string()], vec![])
        };
        let mut gone = Vec::new();
        loop {
            if let Some(c) = cq.pop() {
                let before = self.data.codes.len();
                self.data.codes.retain(|x| x.id != c);
                if self.data.codes.len() < before {
                    gq.extend(
                        self.data
                            .grants
                            .iter()
                            .filter(|g| g.code.as_deref() == Some(c.as_str()))
                            .map(|g| g.id.clone()),
                    );
                }
                continue;
            }
            if let Some(g) = gq.pop() {
                let before = self.data.grants.len();
                self.data.grants.retain(|x| x.id != g);
                if self.data.grants.len() < before {
                    cq.extend(
                        self.data
                            .codes
                            .iter()
                            .filter(|c| c.created_by.as_deref() == Some(g.as_str()))
                            .map(|c| c.id.clone()),
                    );
                    gone.push(g);
                }
                continue;
            }
            break;
        }
        Ok(gone)
    }

    /// 主人给一台设备设限制（None 是和主机一样）
    pub fn limit(&mut self, grant: &str, allow: Option<Allow>) -> Result<()> {
        let g = self
            .data
            .grants
            .iter_mut()
            .find(|g| g.id == grant)
            .with_context(|| format!("没有设备 {grant}"))?;
        g.allow = allow;
        Ok(())
    }

    /// 这台设备的限制（None 是没有限制）；设备已经不在了也是 None，调用方先确认它还在
    pub fn limit_of(&self, grant: &str) -> Option<Allow> {
        self.grant(grant).and_then(|g| g.allow)
    }

    /// 给 Access 的：主人看全部；设备看自己、自己发的码、用这些码配对的设备
    pub fn view(&self, role: &Role, online: &dyn Fn(&str) -> u32) -> AccessInfo {
        let me = match role {
            Role::Owner => None,
            Role::Device(id) => Some(id.as_str()),
        };
        let my_code = |c: &Code| me.is_none() || c.created_by.as_deref() == me;
        let codes: Vec<CodeInfo> = self
            .data
            .codes
            .iter()
            .filter(|c| my_code(c))
            .map(|c| CodeInfo {
                id: c.id.clone(),
                label: c.label.clone(),
                created: c.created,
                created_by: c.created_by.clone(),
                expires: c.expires,
                max_uses: c.max_uses,
                uses: c.uses,
                allow: c.allow,
            })
            .collect();
        let visible = |g: &Grant| match me {
            None => true,
            Some(me) => {
                g.id == me
                    || g.code
                        .as_deref()
                        .is_some_and(|c| codes.iter().any(|x| x.id == c))
            }
        };
        let grants = self
            .data
            .grants
            .iter()
            .filter(|g| visible(g))
            .map(|g| GrantInfo {
                id: g.id.clone(),
                device: g.device.clone(),
                code: g.code.clone(),
                created: g.created,
                created_addr: g.created_addr.clone(),
                last_seen: g.last_seen,
                last_addr: g.last_addr.clone(),
                allow: g.allow,
                online: online(&g.id),
            })
            .collect();
        AccessInfo {
            owner: me.is_none(),
            me: me.map(str::to_string),
            grants,
            codes,
            pending: Vec::new(),
        }
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
            version: "t".into(),
        }
    }

    #[test]
    fn credentials() {
        let mut s = Store::memory();
        let (id, cred) = s.issue(dev("laptop"), None, "10.188.16.11:5000", None, 100);
        assert!(cred.starts_with(&format!("g1.{id}.")));
        assert_eq!(
            s.check(&cred, "10.188.16.11:5000", 110),
            Some((id.clone(), false))
        );
        // 地址变了要写盘；过了一分钟也要
        assert_eq!(
            s.check(&cred, "10.188.16.11:6000", 120),
            Some((id.clone(), true))
        );
        assert_eq!(
            s.check(&cred, "10.188.16.11:6000", 200),
            Some((id.clone(), true))
        );
        assert_eq!(s.grant(&id).unwrap().last_addr, "10.188.16.11:6000");
        // 密钥错一位、id 不对、不是 g1 格式都不行；主机 token 那种十六进制串也不行
        let flip = if cred.ends_with('0') { '1' } else { '0' };
        let bad = format!("{}{flip}", &cred[..cred.len() - 1]);
        assert_eq!(s.check(&bad, "x", 1), None);
        assert_eq!(s.check("g1.g-00000000.00", "x", 1), None);
        assert_eq!(s.check("0123456789abcdef0123456789abcdef", "x", 1), None);
        // 文件里只有哈希
        let json = serde_json::to_string(&s.data).unwrap();
        assert!(!json.contains(cred.rsplit('.').next().unwrap()));
    }

    #[test]
    fn codes_expire_and_run_out() {
        let mut s = Store::memory();
        let (c, secret) = s
            .create_code("ci", None, Some(60), Some(2), None, 1000)
            .unwrap();
        assert_eq!(secret.len(), CODE_LEN);
        assert!(secret.bytes().all(|b| CODE_ALPHABET.contains(&b)));
        // 小写、带横线、带空格都认
        let shown = format_code(&secret).to_lowercase().replace('-', " - ");
        let (g1, _) = s.redeem(&shown, dev("a"), "1.1.1.1:1", 1010).unwrap();
        assert_eq!(s.grant(&g1).unwrap().code.as_deref(), Some(c.id.as_str()));
        s.redeem(&secret, dev("b"), "1.1.1.2:1", 1020).unwrap();
        assert!(s
            .redeem(&secret, dev("c"), "x", 1030)
            .unwrap_err()
            .to_string()
            .contains("用满"));
        let (_, s2) = s.create_code("", None, Some(60), None, None, 1000).unwrap();
        assert!(s
            .redeem(&s2, dev("d"), "x", 1060)
            .unwrap_err()
            .to_string()
            .contains("过期"));
        assert!(s.redeem("AAAA", dev("e"), "x", 1).is_err());
        assert!(s.redeem("AAAABBBBCCCCDDDD", dev("e"), "x", 1).is_err());
        assert!(s.create_code("", None, None, Some(0), None, 1).is_err());
    }

    #[test]
    fn shared_links_cannot_exceed_the_sharer() {
        let mut s = Store::memory();
        let (viewer, _) = s.issue(dev("v"), None, "x", Some(Allow::view_only()), 1);
        let (c, secret) = s
            .create_code("share", Some(viewer.clone()), None, None, None, 1)
            .unwrap();
        assert_eq!(c.allow, Some(Allow::view_only()));
        let (g, _) = s.redeem(&secret, dev("w"), "x", 2).unwrap();
        assert_eq!(s.limit_of(&g), Some(Allow::view_only()));
        // 主人发的码：不设限就是没有限制
        let (c2, _) = s.create_code("full", None, None, None, None, 1).unwrap();
        assert_eq!(c2.allow, None);
    }

    #[test]
    fn revoke_cascades_and_respects_who_asks() {
        let mut s = Store::memory();
        let (a, _) = s.issue(dev("a"), None, "x", None, 1);
        let (b, _) = s.issue(dev("b"), None, "x", None, 1);
        let (ca, sa) = s
            .create_code("a 的分享", Some(a.clone()), None, None, None, 1)
            .unwrap();
        let (a1, _) = s.redeem(&sa, dev("a1"), "x", 2).unwrap();
        let (ca2, sa2) = s
            .create_code("a1 的分享", Some(a1.clone()), None, None, None, 2)
            .unwrap();
        let (a2, _) = s.redeem(&sa2, dev("a2"), "x", 3).unwrap();

        // b 管不了 a 的东西；a 能吊销自己发的码
        assert!(s.revoke(&ca.id, &Role::Device(b.clone())).is_err());
        assert!(s.revoke(&a1, &Role::Device(b.clone())).is_err());
        // 看到的：a 看见自己、自己的码和 a1；看不见 b、a1 发的码
        let v = s.view(&Role::Device(a.clone()), &|_| 0);
        let ids: Vec<_> = v.grants.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, [a.as_str(), a1.as_str()]);
        assert_eq!(v.codes.len(), 1);
        assert!(!v.owner && v.pending.is_empty());

        // 吊销 a 的码：a1、a1 发的码、a2 都没了，a 和 b 还在
        let mut gone = s.revoke(&ca.id, &Role::Device(a.clone())).unwrap();
        gone.sort();
        let mut want = vec![a1.clone(), a2.clone()];
        want.sort();
        assert_eq!(gone, want);
        assert!(s.codes().iter().all(|c| c.id != ca2.id));
        assert!(s.grant(&a).is_some() && s.grant(&b).is_some());
        // 主人吊销 b；设备也能吊销自己
        assert_eq!(
            s.revoke(&b, &Role::Owner).unwrap(),
            std::slice::from_ref(&b)
        );
        assert_eq!(
            s.revoke(&a, &Role::Device(a.clone())).unwrap(),
            std::slice::from_ref(&a)
        );
        assert!(s.revoke(&a, &Role::Owner).is_err());
    }

    #[test]
    fn device_info_is_display_only() {
        let d = clean_device(DeviceInfo {
            name: "evil\u{1b}[31m\nname".repeat(20),
            os: "".into(),
            kind: "app".into(),
            version: "1".into(),
        });
        assert!(!d.name.chars().any(char::is_control));
        assert!(d.name.chars().count() <= 64);
        assert_eq!(clean_device(DeviceInfo::default()).name, "unknown");
    }

    #[test]
    fn file_round_trip() {
        let dir = std::env::temp_dir().join(format!("lolidesk-access-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("access.json");
        let mut s = Store::load(&p).unwrap();
        let (id, cred) = s.issue(dev("a"), None, "x", None, 1);
        s.save().unwrap();
        let mut t = Store::load(&p).unwrap();
        assert_eq!(t.check(&cred, "x", 2).map(|r| r.0), Some(id));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
