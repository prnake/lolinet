//! 这台电脑：本机的 lolidesk 主机（读主机目录、装 / 撤服务）、系统通知、探网里的设备。
//!
//! 界面只认 [`Service`] 这个接口（测试换成假的，不起进程、不连网）。真的实现：
//! - 本机主机：读主机目录（[`lolidesk_core::local::local_host`]）；装、撤服务跑 `<程序> host install|uninstall`
//!   （macOS 是 LaunchAgent，Linux 是 systemd --user）。程序是已经装着的服务用的那个，没装才用 app 带的；
//!   开发时编出来的没有 app 带的，也没装服务，就什么都不跑。
//! - 系统通知：窗口不在前台时，有设备请求配对就发一条。macOS 用 osascript 的 `on run argv`，Linux 用 notify-send；
//!   文字只当参数传，不拼进脚本（设备名是对方自报的）。
//! - 探设备：只握手拿证书指纹（[`net::fingerprint`]），2 秒超时。
//!
//! 装、撤服务、发通知都会阻塞：界面在后台线程里调。

use std::{
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use lolidesk_core::{local::LocalHost, net};

/// 探一台设备多久算没有
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// 同时最多探几台
pub const PROBE_MAX: usize = 8;
/// 同一台多久才再探一次（秒）
pub const PROBE_EVERY: u64 = 60;

pub type Probe = Pin<Box<dyn Future<Output = Option<[u8; 32]>> + Send>>;

/// 这台电脑上要起进程、连网的事。
pub trait Service: Send + Sync {
    /// 本机主机的连法（主机目录里的地址、token、证书指纹）；主机从来没在这台跑过是 None
    fn find(&self) -> Option<LocalHost>;
    /// app 带的 lolidesk（macOS：Contents/Helpers；Linux：包里 lolidesk-app 旁边）；开发时编的没有
    fn helper(&self) -> Option<PathBuf>;
    /// 已经装着的主机服务跑的程序（文件还在才算）。没装服务是 None。
    /// app 不把它换成别的程序：屏幕录制、辅助功能的授权是跟着程序走的
    fn installed(&self) -> Option<PathBuf>;
    /// `<program> host install`：装成服务并启动（装着的话就是重启）；返回它的输出
    fn install(&self, program: &Path) -> Result<String>;
    /// `<program> host uninstall`
    fn uninstall(&self, program: &Path) -> Result<String>;
    /// 发一条系统通知（不等它）
    fn notify(&self, text: &str);
    /// 只握手拿证书指纹；没有 lolidesk 主机、超时是 None。在网络运行时上跑
    fn probe(&self, addr: SocketAddr) -> Probe;
}

/// 真的：读主机目录，跑 app 带的 lolidesk、osascript / notify-send。
pub struct Real;

impl Service for Real {
    fn find(&self) -> Option<LocalHost> {
        lolidesk_core::local::local_host()
    }

    fn helper(&self) -> Option<PathBuf> {
        crate::lolinet::System::helper()
    }

    fn installed(&self) -> Option<PathBuf> {
        lolidesk_core::local::service_program().filter(|p| p.is_file())
    }

    fn install(&self, program: &Path) -> Result<String> {
        if not_installed_yet(program) {
            bail!("Lolidesk 还在下载的位置或安装盘里：先把它拖进「应用程序」，再从那里打开");
        }
        run_helper(program, "install")
    }

    fn uninstall(&self, program: &Path) -> Result<String> {
        run_helper(program, "uninstall")
    }

    fn notify(&self, text: &str) {
        let Some(argv) = notify_argv(text, find_in_path("notify-send")) else {
            return;
        };
        let (prog, args) = argv.split_first().expect("至少有程序名");
        let child = Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        // 收掉子进程，免得留僵尸
        if let Ok(mut c) = child {
            std::thread::spawn(move || {
                let _ = c.wait();
            });
        }
    }

    fn probe(&self, addr: SocketAddr) -> Probe {
        Box::pin(async move {
            tokio::time::timeout(PROBE_TIMEOUT, net::fingerprint(addr))
                .await
                .ok()?
                .ok()
        })
    }
}

/// `<helper> host install|uninstall`，最多等 30 秒；输出是 stdout 再接 stderr
fn run_helper(helper: &Path, what: &str) -> Result<String> {
    let (status, out, err) = crate::lolinet::run(
        Command::new(helper).args(["host", what]),
        Duration::from_secs(30),
    )
    .with_context(|| format!("跑 {} host {what}", helper.display()))?;
    let out = String::from_utf8_lossy(&out).trim().to_string();
    let text = [out, err]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if status.success() {
        Ok(text)
    } else if text.is_empty() {
        bail!("lolidesk host {what} 失败（{status}）")
    } else {
        bail!("{text}")
    }
}

/// PATH 里的一个程序
fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// 发通知的命令（参数表，不经 shell）。macOS：osascript 的 `on run argv`，文字是参数，不进脚本；
/// Linux：有 notify-send 才发。
pub fn notify_argv(text: &str, notify_send: Option<PathBuf>) -> Option<Vec<String>> {
    // 一行、不长：通知里放不下，控制字符也不要
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(200)
        .collect();
    if cfg!(target_os = "macos") {
        Some(vec![
            "/usr/bin/osascript".into(),
            "-e".into(),
            "on run argv".into(),
            "-e".into(),
            "display notification (item 1 of argv) with title \"Lolidesk\"".into(),
            "-e".into(),
            "end run".into(),
            text,
        ])
    } else {
        let bin = notify_send?;
        Some(vec![
            bin.display().to_string(),
            "--app-name=Lolidesk".into(),
            "Lolidesk".into(),
            text,
        ])
    }
}

// ---------------------------------------------------------------- 网里的设备

/// 网里一台有 lolidesk 主机的设备（握手拿到了证书指纹）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// lolinet 里的主机名
    pub name: String,
    pub addr: SocketAddr,
    pub fingerprint: [u8; 32],
}

/// 这次要探哪些地址：lolinet 里的设备（不是接入点），去掉本机、记住的主机、正在探的、`PROBE_EVERY` 秒内探过的，
/// 最多补到 [`PROBE_MAX`] 个同时在探。`last` 是（地址，上次探的 Unix 秒）。
pub fn probe_candidates(
    peers: &[crate::lolinet::Peer],
    me: Option<&str>,
    known: &[std::net::IpAddr],
    probing: &[std::net::IpAddr],
    last: &[(std::net::IpAddr, u64)],
    now: u64,
) -> Vec<(String, std::net::IpAddr)> {
    let room = PROBE_MAX.saturating_sub(probing.len());
    peers
        .iter()
        .filter(|p| !p.relay)
        .filter_map(|p| Some((p.name.clone(), p.ip.parse::<std::net::IpAddr>().ok()?)))
        .filter(|(_, ip)| me.is_none_or(|m| m != ip.to_string()))
        .filter(|(_, ip)| !known.contains(ip) && !probing.contains(ip))
        .filter(|(_, ip)| {
            !last
                .iter()
                .any(|(a, t)| a == ip && now.saturating_sub(*t) < PROBE_EVERY)
        })
        .take(room)
        .collect()
}

/// 这个程序还没装好，不能装进服务（服务记的是程序的路径，这两种过后就没了）：
/// - 从「下载」里直接开的：macOS 把 app 挪到一个临时位置跑（App Translocation）；
/// - 从 DMG 里直接开的：app 旁边是 DMG 里那个 Applications 链接（desk/scripts/bundle-dmg.sh），推出 DMG 路径就没了。
fn not_installed_yet(program: &Path) -> bool {
    if program.to_string_lossy().contains("/AppTranslocation/") {
        return true;
    }
    let Some(app) = program
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
    else {
        return false;
    };
    app.parent()
        .map(|dir| dir.join("Applications"))
        .and_then(|l| std::fs::read_link(l).ok())
        .is_some_and(|to| to == Path::new("/Applications"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lolinet::Peer;

    #[cfg(unix)]
    #[test]
    fn not_installed_from_dmg_or_translocation() {
        let tmp = std::env::temp_dir().join(format!("lolidesk-dmg-{}", std::process::id()));
        let helper = tmp.join("Lolidesk.app/Contents/Helpers/lolidesk");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        // 装好的（旁边没有 Applications 链接）可以装进服务
        assert!(!not_installed_yet(&helper));
        assert!(!not_installed_yet(Path::new("/usr/local/bin/lolidesk")));
        // DMG 里：旁边有指向 /Applications 的链接
        std::os::unix::fs::symlink("/Applications", tmp.join("Applications")).unwrap();
        assert!(not_installed_yet(&helper));
        std::fs::remove_dir_all(&tmp).unwrap();
        assert!(not_installed_yet(Path::new(
            "/private/var/folders/x/AppTranslocation/ABC/d/Lolidesk.app/Contents/Helpers/lolidesk"
        )));
    }

    fn peer(name: &str, ip: &str, relay: bool) -> Peer {
        Peer {
            name: name.into(),
            ip: ip.into(),
            direct: true,
            proto: "udp".into(),
            lat_ms: Some(1.0),
            relay,
        }
    }

    #[test]
    fn notification_is_argv_only() {
        let evil = "a\"; do shell script \"rm -rf ~\" --\nb";
        let argv = notify_argv(evil, Some("/usr/bin/notify-send".into())).unwrap();
        // 设备名只在最后一个参数里，换行换成了空格；脚本是固定的几行
        let last = argv.last().unwrap();
        assert_eq!(last, "a\"; do shell script \"rm -rf ~\" -- b");
        assert!(argv[..argv.len() - 1].iter().all(|a| !a.contains("rm -rf")));
        if cfg!(target_os = "macos") {
            assert_eq!(argv[0], "/usr/bin/osascript");
            assert_eq!(argv[2], "on run argv");
        } else {
            assert_eq!(argv[0], "/usr/bin/notify-send");
            assert!(notify_argv("x", None).is_none());
        }
        let long = "字".repeat(500);
        assert_eq!(
            notify_argv(&long, Some("/x".into()))
                .unwrap()
                .last()
                .unwrap()
                .chars()
                .count(),
            200
        );
    }

    #[test]
    fn which_peers_to_probe() {
        let peers = vec![
            peer("relay-hk", "10.188.0.1", true),
            peer("studio", "10.188.16.10", false),
            peer("desktop", "10.188.1.2", false),
            peer("mini", "10.188.1.7", false),
            peer("nas", "10.188.32.4", false),
            peer("phone", "", false),
        ];
        let ip = |s: &str| s.parse::<std::net::IpAddr>().unwrap();
        // 本机、记住的、正在探的、刚探过的都不探；接入点不探
        let got = probe_candidates(
            &peers,
            Some("10.188.16.10"),
            &[ip("10.188.1.2")],
            &[ip("10.188.1.7")],
            &[(ip("10.188.32.4"), 1000)],
            1030,
        );
        assert!(got.is_empty(), "{got:?}");
        // 60 秒后再探
        let got = probe_candidates(
            &peers,
            Some("10.188.16.10"),
            &[ip("10.188.1.2")],
            &[ip("10.188.1.7")],
            &[(ip("10.188.32.4"), 1000)],
            1060,
        );
        assert_eq!(got, vec![("nas".to_string(), ip("10.188.32.4"))]);
        // 最多 8 个同时在探
        let many: Vec<Peer> = (1..=20)
            .map(|i| peer(&format!("p{i}"), &format!("10.188.2.{i}"), false))
            .collect();
        assert_eq!(probe_candidates(&many, None, &[], &[], &[], 0).len(), 8);
        let busy: Vec<std::net::IpAddr> = (1..=6).map(|i| ip(&format!("10.188.9.{i}"))).collect();
        assert_eq!(probe_candidates(&many, None, &[], &busy, &[], 0).len(), 2);
    }
}
