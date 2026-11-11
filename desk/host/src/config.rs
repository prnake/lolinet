//! 主机配置目录：`~/.config/lolidesk/host/`（macOS 是 `~/Library/Application Support/lolidesk/host/`）。
//! 里面是自签证书（cert.der / key.der）、token 和开放的能力（allow.json，没有就全开），权限 600。

use std::{
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

pub struct HostConfig {
    pub dir: PathBuf,
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
    pub token: String,
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lolidesk")
        .join("host")
}

pub(crate) fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// 本机的 app、命令行从哪里连这台主机（回环地址:端口），写在主机目录的 `local`；lolidesk_core::local 读它
pub fn write_local(addr: std::net::SocketAddr) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    write_private(&dir.join("local"), format!("{addr}\n").as_bytes())
}

/// 开放的能力（allow.json，少写的键按开算）。每条连接、每条流开的时候现读，改了不用重启主机
pub fn allow() -> lolidesk_proto::Allow {
    fs::read(config_dir().join("allow.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn set_allow(a: lolidesk_proto::Allow) -> Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    write_private(&dir.join("allow.json"), &serde_json::to_vec_pretty(&a)?)
}

impl HostConfig {
    pub fn load_or_create(reset_token: bool) -> Result<Self> {
        let dir = config_dir();
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let (cert_p, key_p, tok_p) = (dir.join("cert.der"), dir.join("key.der"), dir.join("token"));
        let (cert, key) = if cert_p.exists() && key_p.exists() {
            (fs::read(&cert_p)?, fs::read(&key_p)?)
        } else {
            let ck =
                rcgen::generate_simple_self_signed(vec![lolidesk_proto::SERVER_NAME.to_string()])?;
            let cert = ck.cert.der().to_vec();
            let key = ck.signing_key.serialize_der();
            write_private(&cert_p, &cert)?;
            write_private(&key_p, &key)?;
            (cert, key)
        };
        let token = if tok_p.exists() && !reset_token {
            fs::read_to_string(&tok_p)?.trim().to_string()
        } else {
            let t = new_token();
            write_private(&tok_p, format!("{t}\n").as_bytes())?;
            t
        };
        Ok(Self {
            dir,
            cert,
            key,
            token,
        })
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(&self.cert).into()
    }

    pub fn fingerprint_hex(&self) -> String {
        hex::encode(self.fingerprint())
    }

    pub fn quic_server_config(&self) -> Result<quinn::ServerConfig> {
        let cert = CertificateDer::from(self.cert.clone());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.clone()));
        let mut tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)?;
        tls.alpn_protocols = vec![lolidesk_proto::ALPN.to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
            .context("QUIC 不接受这份 TLS 配置")?;
        let mut cfg = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        let mut t = quinn::TransportConfig::default();
        t.keep_alive_interval(Some(Duration::from_secs(5)));
        t.max_idle_timeout(Some(Duration::from_secs(30).try_into()?));
        t.datagram_send_buffer_size(16 << 20);
        t.datagram_receive_buffer_size(Some(4 << 20));
        t.stream_receive_window(quinn::VarInt::from_u32(8 << 20));
        t.send_window(32 << 20);
        cfg.transport_config(Arc::new(t));
        Ok(cfg)
    }
}

fn new_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    hex::encode(b)
}

/// 本机在 lolinet overlay 上的地址。
pub fn overlay_ip() -> Option<IpAddr> {
    if_addrs::get_if_addrs()
        .ok()?
        .into_iter()
        .map(|i| i.ip())
        .find(|ip| match ip {
            IpAddr::V4(v4) => v4.octets()[0] == 10 && v4.octets()[1] == 188,
            _ => false,
        })
}

pub fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().trim_end_matches(".local").to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "host".into())
}

/// 装成服务并启动：macOS 是当前用户的 LaunchAgent，Linux 是 systemd --user 单元。再装一次就是重启（换成这次跑的这个程序）
pub fn install_service() -> Result<()> {
    if cfg!(target_os = "macos") {
        install_launch_agent()
    } else if cfg!(target_os = "linux") {
        install_systemd_user()
    } else {
        anyhow::bail!("这个系统上不会装服务：自己让它开机跑 lolidesk host run")
    }
}

/// 停掉并撤掉服务
pub fn uninstall_service() -> Result<()> {
    if cfg!(target_os = "macos") {
        uninstall_launch_agent()
    } else if cfg!(target_os = "linux") {
        uninstall_systemd_user()
    } else {
        anyhow::bail!("这个系统上没有装服务")
    }
}

// ---------------------------------------------------------------- Linux：systemd --user

const UNIT: &str = "lolidesk.service";

/// `$XDG_CONFIG_HOME/systemd/user/lolidesk.service`（默认 ~/.config），systemd --user 从这里读
fn unit_path() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("没有配置目录（HOME 没设？）")?
        .join("systemd/user")
        .join(UNIT))
}

/// 单元里的一个参数：加双引号，`\` 和 `"` 转义；`%` 写成 `%%`、`$` 写成 `$$`，systemd 不拿它当说明符、变量
fn systemd_quote(s: &str) -> Result<String> {
    anyhow::ensure!(
        !s.chars().any(char::is_control),
        "路径里有控制字符，写不进 systemd 单元：{s:?}"
    );
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out.push('"');
    Ok(out)
}

/// systemd --user 单元：跑 `<这个程序> host run`，退出了就再起
fn systemd_unit(exe: &Path) -> Result<String> {
    let exe = exe.to_str().context("程序的路径不是 UTF-8")?;
    Ok(format!(
        "# lolidesk host install 写的；日志：journalctl --user -u lolidesk
[Unit]
Description=lolidesk 主机（让网里的设备连这台电脑）
After=network-online.target

[Service]
ExecStart={} host run
Restart=always
RestartSec=3
Environment=RUST_LOG=info

[Install]
WantedBy=default.target
",
        systemd_quote(exe)?
    ))
}

/// 装的几步：重读单元、开机自启、（重新）启动。再装一次就是换程序重启
const SYSTEMCTL_INSTALL: [&[&str]; 3] = [
    &["--user", "daemon-reload"],
    &["--user", "enable", UNIT],
    &["--user", "restart", UNIT],
];
/// 撤的几步（删单元文件之前、之后）
const SYSTEMCTL_STOP: &[&str] = &["--user", "disable", "--now", UNIT];
const SYSTEMCTL_RELOAD: &[&str] = &["--user", "daemon-reload"];

fn systemctl(args: &[&str]) -> Result<()> {
    let st = std::process::Command::new("systemctl")
        .args(args)
        .status()
        .context("跑不了 systemctl：这台没有 systemd？自己让它开机跑 lolidesk host run")?;
    anyhow::ensure!(st.success(), "systemctl {} 失败（{st}）", args.join(" "));
    Ok(())
}

fn install_systemd_user() -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    let unit = unit_path()?;
    fs::create_dir_all(unit.parent().unwrap())?;
    fs::write(&unit, systemd_unit(&exe)?)?;
    for args in SYSTEMCTL_INSTALL {
        systemctl(args)?;
    }
    println!("已装 systemd --user 单元 {UNIT}（{}）", unit.display());
    println!("日志：journalctl --user -u lolidesk");
    println!("退出登录后也要开着：loginctl enable-linger $USER");
    Ok(())
}

fn uninstall_systemd_user() -> Result<()> {
    let unit = unit_path()?;
    // 没装过时 disable 会失败：不管它，照样删文件
    let _ = systemctl(SYSTEMCTL_STOP);
    if unit.exists() {
        fs::remove_file(&unit)?;
    }
    let _ = systemctl(SYSTEMCTL_RELOAD);
    println!("已撤掉 {UNIT}");
    Ok(())
}

// ---------------------------------------------------------------- macOS：LaunchAgent

const AGENT_LABEL: &str = "co.lo-li.lolidesk";

fn agent_plist() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("没有 HOME")?
        .join("Library/LaunchAgents")
        .join(format!("{AGENT_LABEL}.plist")))
}

pub fn install_launch_agent() -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "macos"),
        "install 只支持 macOS；Linux 请自己写 systemd --user 单元"
    );
    let exe = std::env::current_exe()?.canonicalize()?;
    let logs = dirs::home_dir()
        .context("没有 HOME")?
        .join("Library/Logs/lolidesk");
    fs::create_dir_all(&logs)?;
    let plist = agent_plist()?;
    fs::create_dir_all(plist.parent().unwrap())?;
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- lolidesk host install 写的；日志在 {logs}/host.log -->
<plist version="1.0"><dict>
  <key>Label</key><string>{AGENT_LABEL}</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>host</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{logs}/host.log</string>
  <key>StandardErrorPath</key><string>{logs}/host.log</string>
  <key>EnvironmentVariables</key><dict><key>RUST_LOG</key><string>info</string></dict>
</dict></plist>
"#,
        exe = xml_escape(&exe.display().to_string()),
        logs = xml_escape(&logs.display().to_string())
    );
    fs::write(&plist, body)?;
    let uid = unsafe { libc_getuid() };
    let target = format!("gui/{uid}/{AGENT_LABEL}");
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &target])
        .status();
    let ok = std::process::Command::new("launchctl")
        .args(["bootstrap", &format!("gui/{uid}"), plist.to_str().unwrap()])
        .status()?
        .success();
    anyhow::ensure!(ok, "launchctl bootstrap 失败");
    println!("已装 LaunchAgent {AGENT_LABEL}（{}）", plist.display());
    println!(
        "远程桌面要在「系统设置 → 隐私与安全性」里给 {} 开屏幕录制和辅助功能",
        exe.display()
    );
    Ok(())
}

/// plist 里的文字：路径里可能有 `&`（「Apps & Tools」），不转义 launchd 就读不了这个 plist。
/// lolidesk-core 的 local::service_program 按这个读回
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn uninstall_launch_agent() -> Result<()> {
    let plist = agent_plist()?;
    let uid = unsafe { libc_getuid() };
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{AGENT_LABEL}")])
        .status();
    if plist.exists() {
        fs::remove_file(&plist)?;
    }
    println!("已撤掉 {AGENT_LABEL}");
    Ok(())
}

/// 这个进程是不是 LaunchAgent co.lo-li.lolidesk 起的。launchd 给它起的进程设 XPC_SERVICE_NAME = 标签；
/// 没有的话再问 launchctl 这个服务的 pid 是不是自己。
pub fn under_launch_agent() -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    if std::env::var("XPC_SERVICE_NAME").is_ok_and(|v| v == AGENT_LABEL) {
        return true;
    }
    let uid = unsafe { libc_getuid() };
    std::process::Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{AGENT_LABEL}")])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| {
            launchctl_pid(&String::from_utf8_lossy(&o.stdout)) == Some(std::process::id())
        })
}

/// `launchctl print` 输出里的 `pid = 123`
fn launchctl_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("pid = "))
        .and_then(|p| p.trim().parse().ok())
}

/// 重启主机服务（app 里「重启主机服务」）：交给一个脱离出去的子进程，等一秒让回应先发出去，
/// 再 `launchctl kickstart -k`。launchd 先发 SIGTERM，会话照常存盘，客户端会自己重连。
pub fn restart_launch_agent() -> Result<()> {
    use std::os::unix::process::CommandExt;
    anyhow::ensure!(under_launch_agent(), "主机不是服务方式运行，手动重启");
    let uid = unsafe { libc_getuid() };
    std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "sleep 1; exec launchctl kickstart -k \"$0\"",
            &format!("gui/{uid}/{AGENT_LABEL}"),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        // 自己一个进程组：launchd 停服务时会连同服务的进程组一起杀
        .process_group(0)
        .spawn()
        .context("起 launchctl")?;
    Ok(())
}

extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn systemd_unit_text() {
        let u = super::systemd_unit(Path::new("/usr/local/lib/lolidesk/lolidesk")).unwrap();
        assert!(u.contains("\nExecStart=\"/usr/local/lib/lolidesk/lolidesk\" host run\n"));
        assert!(u.contains("\nRestart=always\n"));
        assert!(u.contains("\nWantedBy=default.target\n"));
        assert!(u.starts_with("# lolidesk host install"));
        // 空格、引号、反斜杠、% 和 $ 都转义了：systemd 读出来还是原来的路径
        let odd =
            super::systemd_unit(Path::new("/home/u/My Apps/a\"b\\c/100%/$HOME/lolidesk")).unwrap();
        assert!(odd.contains(
            "\nExecStart=\"/home/u/My Apps/a\\\"b\\\\c/100%%/$$HOME/lolidesk\" host run\n"
        ));
        // 换行写不进去（会多出一行设置）
        assert!(super::systemd_unit(Path::new("/x\nExecStartPre=/bin/evil")).is_err());
        // 装：重读、自启、（重新）启动；撤：停掉并取消自启
        assert_eq!(
            super::SYSTEMCTL_INSTALL,
            [
                &["--user", "daemon-reload"][..],
                &["--user", "enable", "lolidesk.service"][..],
                &["--user", "restart", "lolidesk.service"][..],
            ]
        );
        assert_eq!(
            super::SYSTEMCTL_STOP,
            ["--user", "disable", "--now", "lolidesk.service"]
        );
    }

    #[test]
    fn launchctl_pid() {
        let text = "gui/501/co.lo-li.lolidesk = {\n\tactive count = 1\n\tpath = /x.plist\n\tstate = running\n\n\tpid = 4242\n\timmediate reason = speculative\n}";
        assert_eq!(super::launchctl_pid(text), Some(4242));
        assert_eq!(super::launchctl_pid("state = not running"), None);
        assert_eq!(super::xml_escape("/A & B/<x>"), "/A &amp; B/&lt;x&gt;");
    }
}
