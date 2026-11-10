//! 记住的主机：`~/.config/lolidesk/hosts`，每行 `地址<TAB>证书指纹(sha256 hex)<TAB>token`，600。

use std::{fs, net::SocketAddr, path::PathBuf};

use anyhow::Result;

#[derive(Clone)]
pub struct Host {
    pub addr: SocketAddr,
    pub fingerprint: String,
    pub token: String,
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
        let mut items = Vec::new();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let mut it = line.split('\t');
                if let (Some(a), Some(f), Some(t)) = (it.next(), it.next(), it.next()) {
                    if let Ok(addr) = a.parse() {
                        items.push(Host {
                            addr,
                            fingerprint: f.to_string(),
                            token: t.to_string(),
                        });
                    }
                }
            }
        }
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
        for h in &self.items {
            writeln!(f, "{}\t{}\t{}", h.addr, h.fingerprint, h.token)?;
        }
        f.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn get(&self, addr: &SocketAddr) -> Option<Host> {
        self.items.iter().find(|h| &h.addr == addr).cloned()
    }

    pub fn remember(&mut self, addr: SocketAddr, fingerprint: &str, token: &str) {
        self.forget(&addr);
        self.items.push(Host {
            addr,
            fingerprint: fingerprint.to_string(),
            token: token.to_string(),
        });
    }

    pub fn forget(&mut self, addr: &SocketAddr) {
        self.items.retain(|h| &h.addr != addr);
    }

    pub fn list(&self) -> &[Host] {
        &self.items
    }
}
