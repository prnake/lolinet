//! 记住的主机：`<配置目录>/lolidesk/hosts`，每行 `地址<TAB>证书指纹(sha256 hex)<TAB>token[<TAB>主机名]`，600。
//! 主机名是第 4 列（GUI 显示用），旧文件只有 3 列也照样读。

use std::{fs, net::SocketAddr, path::PathBuf};

use anyhow::Result;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Host {
    pub addr: SocketAddr,
    pub fingerprint: String,
    #[serde(skip)]
    pub token: String,
    pub name: String,
}

impl Host {
    pub fn fingerprint_bytes(&self) -> Option<[u8; 32]> {
        hex::decode(&self.fingerprint).ok()?.try_into().ok()
    }
}

pub struct Hosts {
    path: PathBuf,
    items: Vec<Host>,
}

impl Hosts {
    fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("lolidesk")
            .join("hosts")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();
        let items = fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        Ok(Self { path, items })
    }

    pub fn save(&self) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        fs::create_dir_all(self.path.parent().unwrap())?;
        let tmp = self.path.with_extension("tmp");
        let _ = fs::remove_file(&tmp);
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(format(&self.items).as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn get(&self, addr: &SocketAddr) -> Option<Host> {
        self.items.iter().find(|h| &h.addr == addr).cloned()
    }

    /// 记住（或更新）一台主机；name 为空时保留原来的名字。
    pub fn remember(&mut self, addr: SocketAddr, fingerprint: &str, token: &str, name: &str) {
        let old = self.get(&addr).map(|h| h.name).unwrap_or_default();
        self.forget(&addr);
        self.items.push(Host {
            addr,
            fingerprint: fingerprint.to_string(),
            token: token.to_string(),
            name: if name.is_empty() { old } else { name.into() },
        });
    }

    pub fn forget(&mut self, addr: &SocketAddr) {
        self.items.retain(|h| &h.addr != addr);
    }

    pub fn list(&self) -> &[Host] {
        &self.items
    }
}

fn parse(text: &str) -> Vec<Host> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.split('\t');
            let addr = it.next()?.parse().ok()?;
            let fingerprint = it.next()?.to_string();
            let token = it.next()?.to_string();
            let name = it.next().unwrap_or("").to_string();
            Some(Host {
                addr,
                fingerprint,
                token,
                name,
            })
        })
        .collect()
}

fn format(items: &[Host]) -> String {
    items
        .iter()
        .map(|h| {
            if h.name.is_empty() {
                format!("{}\t{}\t{}\n", h.addr, h.fingerprint, h.token)
            } else {
                format!("{}\t{}\t{}\t{}\n", h.addr, h.fingerprint, h.token, h.name)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_and_four_columns() {
        let text = "10.188.1.2:10190\tabcd\ttok\n10.188.1.3:10190\tef01\ttok2\tdesktop\nbad line\n";
        let items = parse(text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "");
        assert_eq!(items[1].name, "desktop");
        assert_eq!(format(&items), text.replace("bad line\n", ""));
    }
}
