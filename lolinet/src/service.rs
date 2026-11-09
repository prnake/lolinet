//! 系统服务：macOS 是 launchd LaunchDaemon，Linux 是 systemd，都以 root 跑（TUN 要）。
//! 两套都在所有平台上编译，运行时按系统选，这样在 Mac 上编译也能查 Linux 那份。

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::{Result, bail};

use crate::{BIN, LOG_DIR, upgrade::INTERVAL_HOURS, write_file};

pub struct State {
    pub running: bool,
    pub pid: Option<u32>,
    pub manager: &'static str,
}

const MACOS: bool = cfg!(target_os = "macos");

pub fn install() -> Result<()> {
    if MACOS { launchd::install() } else { systemd::install() }
}

pub fn remove() -> Result<()> {
    remove_upgrade()?;
    if MACOS { launchd::remove() } else { systemd::remove() }
}

/// 自动升级的定时任务（每 INTERVAL_HOURS 小时 `lolinet upgrade --auto`）；enabled = false 时撤掉
pub fn install_upgrade(enabled: bool) -> Result<()> {
    if !enabled {
        return remove_upgrade();
    }
    if MACOS { launchd::install_upgrade() } else { systemd::install_upgrade() }
}

fn remove_upgrade() -> Result<()> {
    if MACOS { launchd::remove_upgrade() } else { systemd::remove_upgrade() }
}

pub fn upgrade_scheduled() -> bool {
    if MACOS { launchd::upgrade_scheduled() } else { systemd::upgrade_scheduled() }
}

/// join/install.sh 装的探针服务（升级换了 reporter.py 之后）；没装就什么都不做
pub fn restart_probe() {
    if MACOS {
        run("launchctl", &["kickstart", "-k", "system/co.lo-li.mesh-probe"]);
    } else {
        run("systemctl", &["try-restart", "mesh-probe"]);
    }
}

/// join/install.sh 以前用 `easytier-cli service install` 装的 easytier 服务：和 lolinet 抢同一张 TUN、同一条凭据，撤掉
pub fn remove_legacy() -> Result<()> {
    if MACOS { launchd::remove_legacy() } else { systemd::remove_legacy() }
}

pub fn restart() -> Result<()> {
    if MACOS { launchd::restart() } else { systemd::restart() }
}

pub fn state() -> Option<State> {
    if MACOS { launchd::state() } else { systemd::state() }
}

fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

mod launchd {
    use super::*;

    const LABEL: &str = "co.lo-li.lolinet";
    const PLIST: &str = "/Library/LaunchDaemons/co.lo-li.lolinet.plist";
    const OLD_LABEL: &str = "easytier";
    const OLD_PLIST: &str = "/Library/LaunchDaemons/easytier.plist";
    const UPGRADE_LABEL: &str = "co.lo-li.lolinet-upgrade";
    const UPGRADE_PLIST: &str = "/Library/LaunchDaemons/co.lo-li.lolinet-upgrade.plist";

    fn load(label: &str, plist: &str) -> Result<()> {
        run("launchctl", &["bootout", &format!("system/{label}")]);
        // bootout 是异步的，紧接着 bootstrap 偶尔报 5: Input/output error，重试几次
        for _ in 0..10 {
            if run("launchctl", &["bootstrap", "system", plist]) {
                return Ok(());
            }
            sleep(Duration::from_secs(1));
        }
        bail!("launchctl bootstrap system {plist} 失败")
    }

    pub fn install() -> Result<()> {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- lolinet up 写的；内核日志在 {LOG_DIR}/easytier.log，这里只收参数错误和崩溃输出 -->
<plist version="1.0"><dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{BIN}</string><string>run</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>{LOG_DIR}/stderr.log</string>
  <key>StandardErrorPath</key><string>{LOG_DIR}/stderr.log</string>
</dict></plist>
"#
        );
        write_file(PLIST, plist.as_bytes(), 0o644)?;
        load(LABEL, PLIST)?;
        println!("launchd {LABEL} 已启动（{PLIST}）");
        Ok(())
    }

    pub fn install_upgrade() -> Result<()> {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- lolinet up 写的：每 {INTERVAL_HOURS} 小时按发布清单自动升级；只记换了什么和出错 -->
<plist version="1.0"><dict>
  <key>Label</key><string>{UPGRADE_LABEL}</string>
  <key>ProgramArguments</key><array><string>{BIN}</string><string>upgrade</string><string>--auto</string></array>
  <key>StartInterval</key><integer>{}</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{LOG_DIR}/upgrade.log</string>
  <key>StandardErrorPath</key><string>{LOG_DIR}/upgrade.log</string>
</dict></plist>
"#,
            INTERVAL_HOURS * 3600
        );
        write_file(UPGRADE_PLIST, plist.as_bytes(), 0o644)?;
        load(UPGRADE_LABEL, UPGRADE_PLIST)?;
        println!("launchd {UPGRADE_LABEL} 已加载：每 {INTERVAL_HOURS} 小时检查升级");
        Ok(())
    }

    pub fn remove_upgrade() -> Result<()> {
        run("launchctl", &["bootout", &format!("system/{UPGRADE_LABEL}")]);
        if Path::new(UPGRADE_PLIST).exists() {
            fs::remove_file(UPGRADE_PLIST)?;
        }
        Ok(())
    }

    pub fn upgrade_scheduled() -> bool {
        run("launchctl", &["print", &format!("system/{UPGRADE_LABEL}")])
    }

    pub fn remove() -> Result<()> {
        run("launchctl", &["bootout", &format!("system/{LABEL}")]);
        if Path::new(PLIST).exists() {
            fs::remove_file(PLIST)?;
        }
        Ok(())
    }

    pub fn remove_legacy() -> Result<()> {
        if Path::new(OLD_PLIST).exists() {
            run("launchctl", &["bootout", &format!("system/{OLD_LABEL}")]);
            fs::remove_file(OLD_PLIST)?;
            println!("已撤掉旧的 launchd {OLD_LABEL} 服务（{OLD_PLIST}）");
        }
        Ok(())
    }

    pub fn restart() -> Result<()> {
        if !run("launchctl", &["kickstart", "-k", &format!("system/{LABEL}")]) {
            bail!("服务没装：sudo lolinet up");
        }
        println!("已重启 launchd {LABEL}");
        Ok(())
    }

    /// `launchctl print` 不要 root；第一处 state / pid 是服务本身的（后面嵌套的是事件源）
    pub fn state() -> Option<State> {
        let out = Command::new("launchctl").args(["print", &format!("system/{LABEL}")]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let field = |k: &str| {
            text.lines().find_map(|l| l.trim().strip_prefix(k)?.strip_prefix(" = ").map(str::to_owned))
        };
        Some(State {
            running: field("state").as_deref() == Some("running"),
            pid: field("pid").and_then(|p| p.parse().ok()),
            manager: "launchd co.lo-li.lolinet",
        })
    }
}

mod systemd {
    use super::*;

    const NAME: &str = "lolinet";
    const UNIT: &str = "/etc/systemd/system/lolinet.service";
    const OLD_NAME: &str = "easytier";
    const OLD_UNIT: &str = "/etc/systemd/system/easytier.service";
    const UPGRADE_SERVICE: &str = "/etc/systemd/system/lolinet-upgrade.service";
    const UPGRADE_TIMER: &str = "/etc/systemd/system/lolinet-upgrade.timer";

    fn need_systemd() -> Result<()> {
        if !Path::new("/run/systemd/system").is_dir() {
            bail!("只支持 systemd（容器、NAS 里用 Docker 版 EasyTier）");
        }
        Ok(())
    }

    pub fn install() -> Result<()> {
        need_systemd()?;
        let unit = format!(
            "# lolinet up 写的；内核日志在 {LOG_DIR}/easytier.log，journald 只收参数错误和崩溃输出
[Unit]
Description=lo-li mesh (lolinet, EasyTier)
After=network-online.target
Wants=network-online.target

[Service]
ExecStart={BIN} run
Restart=always
RestartSec=3
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
"
        );
        write_file(UNIT, unit.as_bytes(), 0o644)?;
        for args in [&["daemon-reload"][..], &["enable", NAME], &["restart", NAME]] {
            if !run("systemctl", args) {
                bail!("systemctl {} 失败", args.join(" "));
            }
        }
        println!("systemd {NAME}.service 已启动");
        Ok(())
    }

    pub fn install_upgrade() -> Result<()> {
        need_systemd()?;
        let service = format!(
            "# lolinet up 写的：按发布清单自动升级，由 lolinet-upgrade.timer 拉起；只记换了什么和出错
[Unit]
Description=lolinet auto upgrade
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
ExecStart={BIN} upgrade --auto
"
        );
        let timer = format!(
            "# lolinet up 写的：开机 15 分钟后一次，之后每 {INTERVAL_HOURS} 小时一次
[Unit]
Description=lolinet auto upgrade every {INTERVAL_HOURS}h

[Timer]
OnBootSec=15min
OnUnitActiveSec={INTERVAL_HOURS}h
RandomizedDelaySec=30min

[Install]
WantedBy=timers.target
"
        );
        write_file(UPGRADE_SERVICE, service.as_bytes(), 0o644)?;
        write_file(UPGRADE_TIMER, timer.as_bytes(), 0o644)?;
        for args in [&["daemon-reload"][..], &["enable", "--now", "lolinet-upgrade.timer"]] {
            if !run("systemctl", args) {
                bail!("systemctl {} 失败", args.join(" "));
            }
        }
        println!("systemd lolinet-upgrade.timer 已启用：每 {INTERVAL_HOURS} 小时检查升级");
        Ok(())
    }

    pub fn remove_upgrade() -> Result<()> {
        run("systemctl", &["disable", "--now", "lolinet-upgrade.timer"]);
        let mut changed = false;
        for f in [UPGRADE_TIMER, UPGRADE_SERVICE] {
            if Path::new(f).exists() {
                fs::remove_file(f)?;
                changed = true;
            }
        }
        if changed {
            run("systemctl", &["daemon-reload"]);
        }
        Ok(())
    }

    pub fn upgrade_scheduled() -> bool {
        run("systemctl", &["is-enabled", "--quiet", "lolinet-upgrade.timer"])
    }

    pub fn remove() -> Result<()> {
        run("systemctl", &["disable", "--now", NAME]);
        if Path::new(UNIT).exists() {
            fs::remove_file(UNIT)?;
            run("systemctl", &["daemon-reload"]);
        }
        Ok(())
    }

    pub fn remove_legacy() -> Result<()> {
        if Path::new(OLD_UNIT).exists() {
            run("systemctl", &["disable", "--now", OLD_NAME]);
            fs::remove_file(OLD_UNIT)?;
            run("systemctl", &["daemon-reload"]);
            println!("已撤掉旧的 systemd {OLD_NAME}.service");
        }
        Ok(())
    }

    pub fn restart() -> Result<()> {
        if !run("systemctl", &["restart", NAME]) {
            bail!("服务没装：sudo lolinet up");
        }
        println!("已重启 systemd {NAME}.service");
        Ok(())
    }

    pub fn state() -> Option<State> {
        let out = Command::new("systemctl")
            .args(["show", NAME, "-p", "LoadState", "-p", "ActiveState", "-p", "MainPID"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('=').map(str::to_owned));
        if field("LoadState").as_deref() != Some("loaded") {
            return None;
        }
        Some(State {
            running: field("ActiveState").as_deref() == Some("active"),
            pid: field("MainPID").and_then(|p| p.parse().ok()).filter(|&p| p != 0),
            manager: "systemd lolinet.service",
        })
    }
}
