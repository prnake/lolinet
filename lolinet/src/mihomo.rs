//! `lolinet mihomo`：问内网管理面要这台电脑的 mihomo 配置（docs/design.md「mihomo on phones and desktops」）。
//! 网内地址走 lolinet，规则（拒绝 / 直连 / 经出口）和出口节点由管理面集中下发，mihomo 每 10 分钟经内网自己取。
//! - TUN 模式：网内地址走系统路由（lolinet 的网卡），mihomo 的 TUN 让开 overlay 网段；
//! - userspace 模式：mihomo 当唯一的 TUN，网内地址交给本机 127.0.0.1:18080 的 SOCKS5。
//! lolinet 不带 mihomo（体积），用 mihomo 客户端或者 Lolidesk.app 里带的那份跑这个配置。

use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::{manager, write_file};

pub fn run(mode: Option<String>, out: Option<PathBuf>, manager_flag: Option<String>) -> Result<()> {
    let m = manager::Manager::locate(manager_flag.as_deref())?;
    let mode = mode.unwrap_or_else(|| manager::mode().to_owned());
    let (code, body) = m.call("GET", &format!("/api/mesh/mihomo/config.yaml?mode={mode}"), None)?;
    if code != 200 {
        let msg = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(|s| s.as_str()).map(str::to_owned))
            .unwrap_or_else(|| body.trim().to_owned());
        bail!("管理面不给（HTTP {code}）：{msg}");
    }
    match out {
        Some(p) => {
            write_file(&p.to_string_lossy(), body.as_bytes(), 0o644)?;
            eprintln!("mihomo 配置（{mode} 模式）-> {}", p.display());
        }
        None => print!("{body}"),
    }
    Ok(())
}
