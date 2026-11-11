//! 这台电脑自己的 lolidesk 主机：本机的 app 和命令行用主机的 token 以主人身份连它（同一个用户读得到主机目录）。
//! 主机启动时把回环地址写在主机目录的 `local`（见 lolidesk-host 的 config::write_local）。

use std::{fs, net::SocketAddr, path::PathBuf};

use sha2::{Digest, Sha256};

/// 本机主机的连法
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalHost {
    pub addr: SocketAddr,
    /// 主机 token：主人的钥匙
    pub token: String,
    pub fingerprint: [u8; 32],
}

/// 主机目录：`<配置目录>/lolidesk/host`（和 lolidesk-host 的 config_dir 一样）
pub fn host_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lolidesk")
        .join("host")
}

/// 读本机主机的地址、token 和证书指纹。主机从来没在这台跑过（没有 local / token / cert.der）是 None；
/// 有这些文件不代表主机现在在跑
pub fn local_host() -> Option<LocalHost> {
    read(&host_dir())
}

fn read(dir: &std::path::Path) -> Option<LocalHost> {
    let addr = fs::read_to_string(dir.join("local"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let token = fs::read_to_string(dir.join("token"))
        .ok()?
        .trim()
        .to_string();
    let cert = fs::read(dir.join("cert.der")).ok()?;
    if token.is_empty() {
        return None;
    }
    Some(LocalHost {
        addr,
        token,
        fingerprint: Sha256::digest(&cert).into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_host_dir() {
        let dir = std::env::temp_dir().join(format!("lolidesk-local-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir), None);
        fs::write(dir.join("local"), "127.0.0.1:10190\n").unwrap();
        fs::write(dir.join("token"), "abc\n").unwrap();
        fs::write(dir.join("cert.der"), b"cert").unwrap();
        let l = read(&dir).unwrap();
        assert_eq!(l.addr.to_string(), "127.0.0.1:10190");
        assert_eq!(l.token, "abc");
        assert_eq!(l.fingerprint, <[u8; 32]>::from(Sha256::digest(b"cert")));
        let _ = fs::remove_dir_all(&dir);
    }
}
