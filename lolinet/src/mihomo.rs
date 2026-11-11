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
//!
//! 服务跑的是拷到 /etc/lolinet/mihomo/mihomo 的那份（root 的，755），不是找到的那个文件：拖进「应用程序」的 app、
//! Homebrew 的目录都是用户自己能写的，root 服务直接跑那里的程序，用户身份的任何进程换掉它就拿到 root。
//! 拷之前不以 root 跑原来那个文件（连 -v 都不跑），检查、试配置都用拷过来的。

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::{CONFIG, manager, netcfg, service, write_file};

pub const DIR: &str = "/etc/lolinet/mihomo";
pub const SETTINGS: &str = "/etc/lolinet/mihomo.json";
/// 服务跑的那份 mihomo（root 的拷贝）
pub const BIN: &str = "/etc/lolinet/mihomo/mihomo";

/// 把找到的 mihomo 拷成 root 的 [`BIN`]（755）：先拷到旁边、跑拷贝的 -v 确认是 mihomo，才换上（不是 mihomo 的话
/// 原来装着的那份不动）。目录一路往上都得是 root 的、别人写不了，不然拷了也白拷。返回 -v 的第一行
fn install_bin(src: &Path) -> Result<String> {
    fs::create_dir_all(DIR)?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o755))?;
    for dir in Path::new(DIR).ancestors() {
        let m = fs::metadata(dir).with_context(|| format!("看 {}", dir.display()))?;
        if m.uid() != 0 || m.mode() & 0o022 != 0 {
            bail!("{} 不是只有 root 能写（owner {}，权限 {:o}）：mihomo 不能放这里跑成 root 服务", dir.display(), m.uid(), m.mode() & 0o777);
        }
    }
    let data = fs::read(src).with_context(|| format!("读 {}", src.display()))?;
    let staging = format!("{BIN}.new");
    write_file(&staging, &data, 0o755)?;
    // write_file 按 umask 建文件：权限再明确设一次；root 建的文件 owner 就是 root
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))?;
    let ver = match version(Path::new(&staging)) {
        Ok(v) => v,
        Err(_) => {
            let _ = fs::remove_file(&staging);
            bail!("{} 不像是 mihomo（拷过来跑 -v 不对）", src.display());
        }
    };
    fs::rename(&staging, BIN)?;
    Ok(ver)
}

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
    let src = find_bin(bin)?;
    let mode = mode.unwrap_or_else(|| manager::mode().to_owned());
    let body = fetch(&mode, manager_flag.as_deref())?;
    // 先拷成 root 的再跑（-v、-t、服务都用拷贝）：原来那个文件可能是用户写得了的
    let ver = install_bin(&src)?;
    let bin = PathBuf::from(BIN);
    if ensure_socks()? {
        service::restart()?;
        println!("lolinet 打开了 127.0.0.1:18080（mihomo 经它进网），已重启 lolinet");
    }
    put_config(&bin, &body)?;
    let st = json!({"bin": bin, "from": src, "mode": mode});
    write_file(SETTINGS, format!("{}\n", serde_json::to_string_pretty(&st)?).as_bytes(), 0o644)?;
    service::install_mihomo(&bin)?;
    println!("mihomo 已作为系统服务启动（{ver}，{mode} 模式，配置 {DIR}/config.yaml）");
    println!("  跑的是拷到 {BIN} 的那份（root 的），拷自 {}；换了版本再 sudo lolinet mihomo on", src.display());
    println!("  规则、线路绑哪台出口由管理面集中下发，mihomo 每 10 分钟自己取；");
    println!("  管理面加了线路要换配置：自动升级的定时任务（每 6 小时）会做，马上要就 sudo lolinet mihomo refresh");
    Ok(())
}

/// 先写到旁边让 mihomo -t 检查，过了才换上
fn put_config(bin: &Path, body: &str) -> Result<()> {
    fs::create_dir_all(DIR)?;
    let staging = format!("{DIR}/config.yaml.new");
    write_file(&staging, body.as_bytes(), 0o644)?;
    let t = Command::new(bin).args(["-t", "-d", DIR, "-f", &staging]).output()?;
    if !t.status.success() {
        let _ = fs::remove_file(&staging);
        bail!("mihomo 说配置不对：{}", String::from_utf8_lossy(&t.stdout).trim());
    }
    fs::rename(&staging, format!("{DIR}/config.yaml"))?;
    Ok(())
}

/// 装着 mihomo 服务的话：重新取配置，变了（管理面加了线路、改了节点地址）就换上、重启 mihomo。
/// 返回换没换；没装 mihomo 是 Ok(false)。线路里的规则、线路绑哪台不用这一步，mihomo 自己每 10 分钟取
pub fn refresh() -> Result<bool> {
    let Ok(raw) = fs::read(SETTINGS) else {
        return Ok(false);
    };
    let v: Value = serde_json::from_slice(&raw).with_context(|| format!("{SETTINGS} 不是 JSON"))?;
    let bin = PathBuf::from(v.get("bin").and_then(Value::as_str).context("mihomo.json 里没有 bin")?);
    // 只跑 root 的那份拷贝：别的路径（手改的、旧格式）不以 root 跑
    if bin != Path::new(BIN) {
        bail!("mihomo.json 里的程序是 {}，不是 {BIN}：重新 sudo lolinet mihomo on", bin.display());
    }
    let mode = v.get("mode").and_then(Value::as_str).unwrap_or("tun").to_owned();
    let body = fetch(&mode, None)?;
    if fs::read_to_string(format!("{DIR}/config.yaml")).is_ok_and(|now| now == body) {
        return Ok(false);
    }
    version(&bin)?;
    put_config(&bin, &body)?;
    service::install_mihomo(&bin)?;
    Ok(true)
}

pub fn refresh_cmd() -> Result<()> {
    crate::need_root()?;
    if !Path::new(SETTINGS).exists() {
        bail!("没装 mihomo 服务：sudo lolinet mihomo on");
    }
    if refresh()? {
        println!("mihomo 配置换成了管理面现在的，已重启 mihomo");
    } else {
        println!("mihomo 配置和管理面现在的一样，没动");
    }
    Ok(())
}

pub fn off() -> Result<()> {
    crate::need_root()?;
    service::remove_mihomo()?;
    if Path::new(SETTINGS).exists() {
        fs::remove_file(SETTINGS)?;
    }
    let _ = fs::remove_file(BIN);
    println!("mihomo 服务已停止并卸载；{DIR} 里的规则缓存留着");
    Ok(())
}

/// 装着的话是哪个 mihomo（给 lolinet status）
pub fn installed() -> Option<String> {
    let v: Value = serde_json::from_slice(&fs::read(SETTINGS).ok()?).ok()?;
    let from = v.get("from").and_then(Value::as_str).map(|f| format!("（拷自 {f}）")).unwrap_or_default();
    Some(format!(
        "{}{from} · {} 模式",
        v.get("bin").and_then(Value::as_str).unwrap_or("?"),
        v.get("mode").and_then(Value::as_str).unwrap_or("?")
    ))
}
