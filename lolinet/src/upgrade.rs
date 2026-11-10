//! 自动升级：按发布清单 lolinet.json 把 /usr/local/bin/lolinet 换成发布的版本（公开探针编在里面，一起换）。
//! 每次（不管换没换）顺带把探针理一遍：以前 install.sh 装的 python 版接管过来，服务文件跟上这一版。
//!
//! 清单来源（/etc/lolinet/lolinet.json 的设置，`lolinet up` 写）：
//! - `source` = `github:<owner>/<repo>`：GitHub Releases 最新一版的 lolinet.json；或者直接是清单 URL；
//! - 没写 `source` 就用准入站：`<base_url>/dl/lolinet.json`（join/install.sh 装的设备都记了 base_url）；
//! - 都没有就用 GitHub `prnake/lolinet`。
//!
//! 清单写的是「应该跑哪个版本」，不比新旧：和本机不一样就换（想回退就把旧版本重新发布一次）。
//! 包和清单放在同一个目录（准入站 dl/、GitHub 某个 Release），按清单的地址拼出包的地址。清单的地址只跟「文件名不变」的跳转：
//! GitHub 的 releases/latest/download/ 先跳到 releases/download/<tag>/（钉在这一版上），再跳到一次性的存储地址，
//! 后面那个目录里没有别的文件，不能拿来拼。下载用系统 curl，sha256 不对就不换。

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use crate::{BIN, VERSION, probe, service, write_file};

pub const SETTINGS: &str = "/etc/lolinet/lolinet.json";
pub const DEFAULT_GITHUB: &str = "prnake/lolinet";
pub const INTERVAL_HOURS: u64 = 6;

pub struct Settings {
    pub base_url: Option<String>,
    pub source: Option<String>,
    pub auto: bool,
}

impl Settings {
    /// 没有设置文件（以前装的）时：不知道准入站，自动升级开着，来源退到 GitHub
    pub fn load() -> Self {
        let v: Value = fs::read(SETTINGS).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        Settings { base_url: s("base_url"), source: s("source"), auto: v.get("auto_upgrade").and_then(Value::as_bool).unwrap_or(true) }
    }

    pub fn save(&self) -> Result<()> {
        let v = json!({ "base_url": self.base_url, "source": self.source, "auto_upgrade": self.auto });
        write_file(SETTINGS, format!("{}\n", serde_json::to_string_pretty(&v)?).as_bytes(), 0o644)
    }

    pub fn manifest_url(&self) -> String {
        match (&self.source, &self.base_url) {
            (Some(s), _) => source_url(s),
            (None, Some(b)) => format!("{}/dl/lolinet.json", b.trim_end_matches('/')),
            (None, None) => source_url(&format!("github:{DEFAULT_GITHUB}")),
        }
    }
}

/// `github:<owner>/<repo>` 或清单 URL
pub fn source_url(s: &str) -> String {
    match s.strip_prefix("github:") {
        Some(repo) => format!("https://github.com/{repo}/releases/latest/download/lolinet.json"),
        None => s.to_owned(),
    }
}

pub fn valid_source(s: &str) -> bool {
    match s.strip_prefix("github:") {
        Some(repo) => repo.split('/').count() == 2 && repo.split('/').all(|p| !p.is_empty()),
        None => s.starts_with("https://") || s.starts_with("http://"),
    }
}

/// 清单里的平台名，和发布的包名一致：lolinet-<macos|linux>-<aarch64|x86_64>.tar.gz
fn platform() -> String {
    format!("{}-{}", if cfg!(target_os = "macos") { "macos" } else { "linux" }, std::env::consts::ARCH)
}

/// 临时目录（700），用完删
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Result<Self> {
        let p = std::env::temp_dir().join(format!("lolinet-upgrade-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir(&p)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
        Ok(Tmp(p))
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 下载到 dest（跟随跳转）
fn fetch(url: &str, dest: &Path) -> Result<()> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "20", "--max-time", "600", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .context("跑 curl")?;
    if !out.status.success() {
        bail!("下载 {url} 失败：{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// 清单的稳定地址：从 url 出发，只跟文件名不变的跳转（最多 5 次）。包和探针的地址从它拼
fn pinned(url: &str) -> Result<String> {
    fn name(u: &str) -> &str {
        let base = u.split(['?', '#']).next().unwrap_or(u);
        &base[base.rfind('/').map_or(0, |i| i + 1)..]
    }
    let mut at = url.to_owned();
    for _ in 0..5 {
        let out = Command::new("curl")
            .args(["-fsS", "--retry", "2", "--connect-timeout", "20", "--max-time", "60", "-o", "/dev/null", "-w", "%{redirect_url}"])
            .arg(&at)
            .output()
            .context("跑 curl")?;
        if !out.status.success() {
            bail!("下载 {at} 失败：{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let next = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if next.is_empty() || name(&next) != name(&at) {
            break;
        }
        at = next;
    }
    Ok(at)
}

/// 清单所在目录里的另一个文件
fn sibling(effective: &str, file: &str) -> String {
    let base = effective.split(['?', '#']).next().unwrap_or(effective);
    format!("{}{file}", &base[..=base.rfind('/').unwrap_or(0)])
}

fn sha256(path: &Path) -> Result<String> {
    Ok(hex(&Sha256::digest(fs::read(path)?)))
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn untar(tgz: &Path, dir: &Path, member: &str) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    if !Command::new("tar").arg("-xzf").arg(tgz).arg("-C").arg(dir).arg(member).status()?.success() {
        bail!("解压 {} 失败", tgz.display());
    }
    Ok(dir.join(member))
}

struct Manifest {
    url: String,
    version: String,
    file: String,
    sha256: String,
}

fn manifest(url: &str, tmp: &Path) -> Result<Manifest> {
    let path = tmp.join("lolinet.json");
    let at = pinned(url)?;
    fetch(&at, &path)?;
    let v: Value = serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("{url} 不是 JSON"))?;
    let entry = v.pointer(&format!("/lolinet/{}", platform())).with_context(|| format!("清单里没有 {}", platform()))?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned).with_context(|| format!("清单缺 {k}"));
    Ok(Manifest {
        version: s(&v, "version")?,
        file: s(entry, "file")?,
        sha256: s(entry, "sha256")?,
        url: at,
    })
}

/// lolinet upgrade：--check 只看不换（不要 root），--dry-run 下载校验到试跑为止，--auto 给定时任务用（关了就什么都不做）
pub fn run(check: bool, dry_run: bool, auto: bool, source: Option<String>) -> Result<()> {
    let st = Settings::load();
    if auto && !st.auto {
        return Ok(());
    }
    if !check && !dry_run {
        crate::need_root()?;
    }
    if let Some(s) = &source {
        if !valid_source(s) {
            bail!("--source 要是 github:<owner>/<repo> 或清单 URL");
        }
    }
    let url = source.as_deref().map(source_url).unwrap_or_else(|| st.manifest_url());
    let res = (|| {
        let tmp = Tmp::new()?;
        let m = manifest(&url, &tmp.0)?;
        if m.version == VERSION {
            if !auto {
                println!("lolinet {VERSION} 和发布的一致（{}）", m.url);
            }
        } else if check {
            println!("发布的是 {}，本机 {VERSION}：sudo lolinet upgrade（{}）", m.version, m.url);
        } else {
            replace(&m, &tmp.0, dry_run, auto)?;
        }
        Ok(())
    })();
    // 公开探针：python 版接管过来、服务文件跟上这一版；和这次下载成不成功无关
    if !check && !dry_run {
        let stamp = if auto { format!("{} ", now()) } else { String::new() };
        if let Err(e) = probe::reconcile(false, &|m| println!("{stamp}{m}")) {
            println!("{stamp}公开探针：{e:#}");
        }
    }
    if auto {
        // 定时任务的输出进 /var/log/lolinet/upgrade.log：只记换了什么和出错，平时不写
        if let Err(e) = &res {
            println!("{} 检查升级失败（{url}）：{e:#}", now());
        }
    }
    res
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

fn replace(m: &Manifest, tmp: &Path, dry_run: bool, auto: bool) -> Result<()> {
    let tgz = tmp.join(&m.file);
    fetch(&sibling(&m.url, &m.file), &tgz)?;
    let got = sha256(&tgz)?;
    if got != m.sha256 {
        bail!("{} 的 sha256 不对：{got}，清单写的是 {}", m.file, m.sha256);
    }
    let new = untar(&tgz, &tmp.join("x"), "lolinet")?;
    // 新二进制在本机跑得起来、版本和清单一致，才换
    let out = Command::new(&new).arg("--version").output().context("试跑新版本")?;
    let shown = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || shown != format!("lolinet {}", m.version) {
        bail!("新版本试跑结果不对：{shown:?}，应该是 lolinet {}", m.version);
    }
    if dry_run {
        println!("可以换成 {}（{}，sha256 和试跑都对，--dry-run 没换）", m.version, m.file);
        return Ok(());
    }
    let tmp_bin = format!("{BIN}.tmp");
    let _ = fs::remove_file(&tmp_bin);
    fs::copy(&new, &tmp_bin)?;
    fs::set_permissions(&tmp_bin, fs::Permissions::from_mode(0o755))?;
    fs::rename(&tmp_bin, BIN)?;
    let restarted = service::state().is_some() && service::restart().is_ok();
    service::restart_probe();
    println!(
        "{}lolinet {VERSION} -> {}{}",
        if auto { format!("{} ", now()) } else { String::new() },
        m.version,
        if restarted { "，服务已重启" } else { "" }
    );
    Ok(())
}
