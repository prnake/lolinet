//! `lolinet mihomo`：问内网管理面要这台电脑的 mihomo 配置（docs/design.md「mihomo on phones and desktops」）。
//! 网内地址走 lolinet，规则（拒绝 / 直连 / 经出口）和出口节点由管理面集中下发，mihomo 每 10 分钟经内网自己取。
//! - TUN 模式：两个 TUN，mihomo 的让开 overlay 网段，普通程序访问网内直接走 lolinet 的网卡；
//! - userspace 模式：mihomo 当唯一的 TUN，网内地址交给本机 127.0.0.1:18080 的 SOCKS5。
//! 两种模式下 mihomo 自己拨出去的连接（出口节点、拉规则、走 7890 端口的程序访问网内）都经 127.0.0.1:18080：
//! mihomo 开了 auto-detect-interface 后，往外拨的连接都绑在物理网卡上（只有回环例外），直接拨 10.188.x.x 到不了
//! lolinet 的 TUN。所以 TUN 模式下 `lolinet mihomo on` 也给 lolinet 打开这个回环 SOCKS5。
//!
//! `sudo lolinet mihomo on`：把 mihomo 跑成系统服务（TUN 要 root）：co.lo-li.lolinet-mihomo / lolinet-mihomo.service，
//! 工作目录 /etc/lolinet/mihomo（config.yaml + mihomo 自己下载的规则、地理库）。lolinet 不带 mihomo（体积）：
//! 用 --bin 给，或者用 Lolidesk.app 里带的那份、系统里装的那份。

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::{CONFIG, manager, netcfg, service, write_file};

pub const DIR: &str = "/etc/lolinet/mihomo";
pub const SETTINGS: &str = "/etc/lolinet/mihomo.json";

fn fetch(mode: &str, manager_flag: Option<&str>) -> Result<String> {
    let m = manager::Manager::locate(manager_flag)?;
    let (code, body) = m.call("GET", &format!("/api/mesh/mihomo/config.yaml?mode={mode}"), None)?;
    if code != 200 {
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| body.trim().to_owned());
        bail!("管理面不给（HTTP {code}）：{msg}");
    }
    Ok(body)
}

pub fn run(mode: Option<String>, out: Option<PathBuf>, manager_flag: Option<String>) -> Result<()> {
    let mode = mode.unwrap_or_else(|| manager::mode().to_owned());
    let body = fetch(&mode, manager_flag.as_deref())?;
    match out {
        Some(p) => {
            write_file(&p.to_string_lossy(), body.as_bytes(), 0o644)?;
            eprintln!("mihomo 配置（{mode} 模式）-> {}", p.display());
        }
        None => print!("{body}"),
    }
    if mode == "tun" && manager::local_socks_any().is_none() {
        eprintln!(
            "注意：这份配置经 lolinet 的 127.0.0.1:18080 进网，这台的 lolinet 还没开它。sudo lolinet mihomo on 会开，\
             自己跑 mihomo 的话先 sudo lolinet up --userspace，或者等 mihomo on 帮你开"
        );
    }
    Ok(())
}

/// TUN 模式下给 lolinet 打开回环 SOCKS5（改了配置就重启 lolinet）。返回改没改
pub fn ensure_socks() -> Result<bool> {
    let text = fs::read_to_string(CONFIG).with_context(|| format!("读 {CONFIG}"))?;
    let net = netcfg::Net::parse(&text)?;
    if net.socks.is_some() {
        return Ok(false);
    }
    crate::write_config(&netcfg::set_socks(&text, true))?;
    Ok(true)
}

/// 找 mihomo：--bin、Lolidesk.app 里带的、系统里装的
fn find_bin(bin: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(b) = bin {
        return b.canonicalize().with_context(|| format!("没有 {}", b.display()));
    }
    let mut cands: Vec<PathBuf> = vec![
        "/Applications/Lolidesk.app/Contents/Helpers/mihomo".into(),
        "/usr/local/bin/mihomo".into(),
        "/opt/homebrew/bin/mihomo".into(),
        "/usr/bin/mihomo".into(),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        cands.extend(std::env::split_paths(&path).map(|d| d.join("mihomo")));
    }
    cands
        .into_iter()
        .find(|p| p.is_file())
        .context("找不到 mihomo：装 Lolidesk.app（带着它），或者 --bin 指过去")
}

/// mihomo -v 的第一行（Mihomo Meta v1.19.33 …）；跑不起来就不是 mihomo
fn version(bin: &Path) -> Result<String> {
    let out = Command::new(bin).arg("-v").output().with_context(|| format!("跑 {} -v", bin.display()))?;
    let v = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned();
    if !out.status.success() || !v.to_lowercase().contains("mihomo") {
        bail!("{} 不像是 mihomo（-v 输出 {v:?}）", bin.display());
    }
    Ok(v)
}

pub fn on(bin: Option<PathBuf>, mode: Option<String>, manager_flag: Option<String>) -> Result<()> {
    crate::need_root()?;
    let bin = find_bin(bin)?;
    let ver = version(&bin)?;
    let mode = mode.unwrap_or_else(|| manager::mode().to_owned());
    let body = fetch(&mode, manager_flag.as_deref())?;
    if ensure_socks()? {
        service::restart()?;
        println!("lolinet 打开了 127.0.0.1:18080（mihomo 经它进网），已重启 lolinet");
    }
    fs::create_dir_all(DIR)?;
    // 先写到旁边检查（mihomo -t），过了才换上
    let staging = format!("{DIR}/config.yaml.new");
    write_file(&staging, body.as_bytes(), 0o644)?;
    let t = Command::new(&bin).args(["-t", "-d", DIR, "-f", &staging]).output()?;
    if !t.status.success() {
        let _ = fs::remove_file(&staging);
        bail!("mihomo 说配置不对：{}", String::from_utf8_lossy(&t.stdout).trim());
    }
    fs::rename(&staging, format!("{DIR}/config.yaml"))?;
    let st = json!({"bin": bin, "mode": mode});
    write_file(SETTINGS, format!("{}\n", serde_json::to_string_pretty(&st)?).as_bytes(), 0o644)?;
    service::install_mihomo(&bin)?;
    println!("mihomo 已作为系统服务启动（{ver}，{mode} 模式，配置 {DIR}/config.yaml）");
    println!("  规则和出口节点由管理面集中下发，mihomo 每 10 分钟自己取；控制面板 http://127.0.0.1:9090（没开就没有）");
    Ok(())
}

pub fn off() -> Result<()> {
    crate::need_root()?;
    service::remove_mihomo()?;
    if Path::new(SETTINGS).exists() {
        fs::remove_file(SETTINGS)?;
    }
    println!("mihomo 服务已停止并卸载；{DIR} 留着（规则缓存）");
    Ok(())
}

/// 装着的话是哪个 mihomo（给 lolinet status）
pub fn installed() -> Option<String> {
    let v: Value = serde_json::from_slice(&fs::read(SETTINGS).ok()?).ok()?;
    Some(format!(
        "{} · {} 模式",
        v.get("bin").and_then(Value::as_str).unwrap_or("?"),
        v.get("mode").and_then(Value::as_str).unwrap_or("?")
    ))
}
