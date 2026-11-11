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

/// 装着的主机服务跑的是哪个程序：macOS 读 LaunchAgent `co.lo-li.lolidesk` 的 plist，Linux 读 systemd --user 的
/// `lolidesk.service`（路径、写法和 lolidesk-host 的 config::install_service 一样）。没装服务、读不出来是 None；
/// 程序文件还在不在由调用的人看
pub fn service_program() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        let plist = dirs::home_dir()?.join("Library/LaunchAgents/co.lo-li.lolidesk.plist");
        plist_program(&fs::read_to_string(plist).ok()?)
    } else if cfg!(target_os = "linux") {
        let unit = dirs::config_dir()?.join("systemd/user/lolidesk.service");
        unit_program(&fs::read_to_string(unit).ok()?)
    } else {
        None
    }
}

/// plist 里 ProgramArguments 的第一项
fn plist_program(text: &str) -> Option<PathBuf> {
    let rest = text.split_once("<key>ProgramArguments</key>")?.1;
    let rest = rest.trim_start().strip_prefix("<array>")?;
    let s = rest.trim_start().strip_prefix("<string>")?;
    let s = s.split_once("</string>")?.0;
    let s = s
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// 单元里 ExecStart 的程序：带引号的按 systemd 的转义读回（`\\`、`\"`、`%%`、`$$`），不带的取第一段
fn unit_program(text: &str) -> Option<PathBuf> {
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("ExecStart="))?
        .trim();
    let mut out = String::new();
    if let Some(q) = line.strip_prefix('"') {
        let mut chars = q.chars();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => out.push(chars.next()?),
                c @ ('%' | '$') => {
                    if chars.clone().next() == Some(c) {
                        chars.next();
                    }
                    out.push(c);
                }
                c => out.push(c),
            }
        }
    } else {
        out = line.split_whitespace().next()?.to_string();
    }
    (!out.is_empty()).then(|| PathBuf::from(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_service_program() {
        let plist = "<plist version=\"1.0\"><dict>\n  <key>Label</key><string>co.lo-li.lolidesk</string>\n  <key>ProgramArguments</key><array><string>/Users/u/Apps &amp; Tools/Lolidesk.app/Contents/Helpers/lolidesk</string><string>host</string><string>run</string></array>\n</dict></plist>";
        assert_eq!(
            plist_program(plist),
            Some("/Users/u/Apps & Tools/Lolidesk.app/Contents/Helpers/lolidesk".into())
        );
        assert_eq!(plist_program("<plist></plist>"), None);
        let unit = "[Service]\nExecStart=\"/home/u/My Apps/a\\\"b\\\\c/100%%/$$HOME/lolidesk\" host run\nRestart=always\n";
        assert_eq!(
            unit_program(unit),
            Some("/home/u/My Apps/a\"b\\c/100%/$HOME/lolidesk".into())
        );
        assert_eq!(
            unit_program("ExecStart=/usr/local/bin/lolidesk host run"),
            Some("/usr/local/bin/lolidesk".into())
        );
        assert_eq!(unit_program("[Unit]\n"), None);
    }

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
