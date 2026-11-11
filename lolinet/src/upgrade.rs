//! 自动升级：按发布清单 lolinet.json 把 /usr/local/bin/lolinet 换成发布的版本（公开探针编在里面，一起换）。
//! 每次（不管换没换）顺带把探针理一遍：以前 install.sh 装的 python 版接管过来，服务文件跟上这一版。
//! 装了 lolidesk 的再按 lolidesk.json 换 /usr/local/lib/lolidesk/（lolidesk + libghostty-vt）。
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
//!
//! lolidesk 的清单跟着同一个来源：准入站、清单 URL 是 lolinet.json 同目录的 lolidesk.json；GitHub 上 lolidesk 的
//! Release 不占 releases/latest，问 API 拿最新的 lolidesk-* Release，用它里面的 lolidesk.json。

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
/// lolidesk 装在这里（程序和 libghostty-vt 放一起，程序在旁边找库），/usr/local/bin/lolidesk 是指过来的链接
pub const DESK_DIR: &str = "/usr/local/lib/lolidesk";
const DESK_LINK: &str = "/usr/local/bin/lolidesk";
/// 2026-11-10 以前的包是两个程序，装在 /usr/local/bin；现在的 lolidesk 被叫成 lolidesk-host 时就是 `lolidesk host`，
/// 所以老的服务（LaunchAgent 跑 lolidesk-host run）换成指过来的链接照样起得来
const DESK_LEGACY: [&str; 2] = ["/usr/local/bin/lolidesk-host", "/usr/local/bin/lolidesk"];
const DESK_LIB: &str = if cfg!(target_os = "macos") { "libghostty-vt.dylib" } else { "libghostty-vt.so" };
const DESK_MANIFEST: &str = "lolidesk.json";

pub struct Settings {
    pub base_url: Option<String>,
    pub source: Option<String>,
    pub auto: bool,
    /// 内网管理面（lolinet invite 问它要配对码）：https://manager.<edge_zone> 和签发 relay 的 overlay 地址
    pub manager: Option<String>,
    pub manager_ip: Option<String>,
}

impl Settings {
    /// 没有设置文件（以前装的）时：不知道准入站，自动升级开着，来源退到 GitHub
    pub fn load() -> Self {
        let v: Value = fs::read(SETTINGS).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
        Settings {
            base_url: s("base_url"),
            source: s("source"),
            auto: v.get("auto_upgrade").and_then(Value::as_bool).unwrap_or(true),
            manager: s("manager"),
            manager_ip: s("manager_ip"),
        }
    }

    pub fn save(&self) -> Result<()> {
        let v = json!({
            "base_url": self.base_url,
            "source": self.source,
            "auto_upgrade": self.auto,
            "manager": self.manager,
            "manager_ip": self.manager_ip,
        });
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

/// 和 fetch 一样，只是 404（没发布）返回 false
fn fetch_if_found(url: &str, dest: &Path) -> Result<bool> {
    let out = Command::new("curl")
        .args(["-sSL", "--retry", "2", "--connect-timeout", "20", "--max-time", "60", "-w", "%{http_code}", "-o"])
        .arg(dest)
        .arg(url)
        .output()
        .context("跑 curl")?;
    if !out.status.success() {
        bail!("下载 {url} 失败：{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    match String::from_utf8_lossy(&out.stdout).trim() {
        "200" => Ok(true),
        "404" => Ok(false),
        code => bail!("下载 {url} 失败：HTTP {code}"),
    }
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
    // 来源是 GitHub 的话是哪个仓库：lolidesk 的清单要问它的 API（见 desk_manifest_url）
    let github = match source.as_deref().or(st.source.as_deref()) {
        Some(s) => s.strip_prefix("github:").map(str::to_owned),
        None => st.base_url.is_none().then(|| DEFAULT_GITHUB.to_owned()),
    };
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
        desk(&m, github.as_deref(), &tmp.0, check, dry_run, auto)?;
        Ok(())
    })();
    // 公开探针：python 版接管过来、服务文件跟上这一版；和这次下载成不成功无关
    if !check && !dry_run {
        let stamp = if auto { format!("{} ", now()) } else { String::new() };
        if let Err(e) = probe::reconcile(false, &|m| println!("{stamp}{m}")) {
            println!("{stamp}公开探针：{e:#}");
        }
        // mihomo 服务：管理面加了线路、出口换了地址，配置要换（规则本身 mihomo 自己取）
        match crate::mihomo::refresh() {
            Ok(true) => println!("{stamp}mihomo 配置换成了管理面现在的，已重启 mihomo"),
            Ok(false) => {}
            Err(e) => println!("{stamp}mihomo 配置没换：{e:#}"),
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

/// 定时任务的输出带时间（进日志），手动跑不带
fn stamp(auto: bool) -> String {
    if auto { format!("{} ", now()) } else { String::new() }
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
        stamp(auto),
        m.version,
        if restarted { "，服务已重启" } else { "" }
    );
    Ok(())
}


/// lolidesk.json 的地址。GitHub：最新的 lolidesk-* Release 里那份（API 按发布时间从新到旧列），没发布过就是 None；
/// 别的来源（准入站 dl/、清单 URL）：和 lolinet.json 同一个目录
fn desk_manifest_url(m: &Manifest, github: Option<&str>, tmp: &Path) -> Result<Option<String>> {
    let Some(repo) = github else {
        return Ok(Some(sibling(&m.url, DESK_MANIFEST)));
    };
    let api = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    let path = tmp.join("releases.json");
    fetch(&api, &path)?;
    let v: Value = serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("{api} 不是 JSON"))?;
    let flag = |r: &Value, k: &str| r.get(k).and_then(Value::as_bool).unwrap_or(false);
    let tag = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| !flag(r, "draft") && !flag(r, "prerelease"))
        .filter_map(|r| r.get("tag_name").and_then(Value::as_str))
        .find(|t| t.starts_with("lolidesk-"));
    Ok(tag.map(|t| format!("https://github.com/{repo}/releases/download/{t}/{DESK_MANIFEST}")))
}

/// 本机二进制自己报的版本：`<名字> <版本>` 里的版本，跑不起来就是 None
fn bin_version(path: &str, name: &str) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    let shown = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    shown.strip_prefix(&format!("{name} ")).filter(|_| out.status.success()).map(str::to_owned)
}

/// 装了 lolidesk 才管（没装的不装）：/usr/local/lib/lolidesk/lolidesk，或者老的 /usr/local/bin 下的两个程序。
/// 版本和 lolidesk.json 不一样就把整个包（lolidesk + libghostty-vt）换进 /usr/local/lib/lolidesk/，
/// /usr/local/bin 下的 lolidesk、lolidesk-host（老的）换成指过去的链接。
/// 不重启在跑的主机：终端会话在主机进程里，重启就全断了。换的是文件，主机下次启动才用新版本。
/// macOS 上换了程序，屏幕录制和辅助功能的授权可能要重新给（授权认的是那个二进制，见 docs/lolidesk.md）。
fn desk(m: &Manifest, github: Option<&str>, tmp: &Path, check: bool, dry_run: bool, auto: bool) -> Result<()> {
    let main = format!("{DESK_DIR}/lolidesk");
    let installed = Path::new(&main).exists() || DESK_LEGACY.iter().any(|p| Path::new(p).exists());
    if !installed {
        return Ok(());
    }
    let path = tmp.join(DESK_MANIFEST);
    let url = match desk_manifest_url(m, github, tmp)? {
        Some(url) if fetch_if_found(&url, &path)? => url,
        _ => {
            if !auto {
                println!("这个来源没有发布 lolidesk，本机的没动");
            }
            return Ok(());
        }
    };
    let v: Value = serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("{url} 不是 JSON"))?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned).with_context(|| format!("{url} 缺 {k}"));
    let entry = v.pointer(&format!("/lolidesk/{}", platform())).with_context(|| format!("{url} 里没有 {}", platform()))?;
    let (version, file, want) = (s(&v, "version")?, s(entry, "file")?, s(entry, "sha256")?);
    if !file.starts_with("lolidesk-") || file.contains('/') {
        bail!("{url} 里的包名不对：{file}");
    }
    // 要换：新位置没装、版本不对，或者 /usr/local/bin 下还是老的独立程序（不是指过来的链接）
    let cur = bin_version(&main, "lolidesk");
    let legacy: Vec<&str> =
        DESK_LEGACY.iter().copied().filter(|p| fs::symlink_metadata(p).is_ok_and(|m| !m.file_type().is_symlink())).collect();
    if cur.as_deref() == Some(version.as_str()) && legacy.is_empty() {
        if !auto {
            println!("lolidesk {version} 和发布的一致");
        }
        return Ok(());
    }
    let was = match (&cur, legacy.is_empty()) {
        (Some(c), true) => c.clone(),
        (Some(c), false) => format!("{c}，另有老的 {}", legacy.join("、")),
        (None, _) => format!("老的 {}", legacy.join("、")),
    };
    if check {
        println!("lolidesk 发布的是 {version}，本机 {was}：sudo lolinet upgrade（{url}）");
        return Ok(());
    }
    let tgz = tmp.join(&file);
    fetch(&sibling(&url, &file), &tgz)?;
    let got = sha256(&tgz)?;
    if got != want {
        bail!("{file} 的 sha256 不对：{got}，{DESK_MANIFEST} 写的是 {want}");
    }
    // 解出来、在本机试跑（它在旁边找 libghostty-vt）、版本和清单一致，才换
    let stage = tmp.join("lolidesk");
    let bin = untar(&tgz, &stage, "lolidesk")?;
    let lib = untar(&tgz, &stage, DESK_LIB)?;
    if bin_version(&bin.to_string_lossy(), "lolidesk").as_deref() != Some(version.as_str()) {
        bail!("新的 lolidesk 在本机试跑结果不对，应该是 lolidesk {version}");
    }
    if dry_run {
        println!("lolidesk 可以换成 {version}（{file}，sha256 和试跑都对，--dry-run 没换）");
        return Ok(());
    }
    fs::create_dir_all(DESK_DIR)?;
    // 先换库再换程序：新程序起来时旁边一定是配套的库
    write_file(&format!("{DESK_DIR}/{DESK_LIB}"), &fs::read(&lib)?, 0o644)?;
    write_file(&main, &fs::read(&bin)?, 0o755)?;
    for link in std::iter::once(DESK_LINK).chain(legacy.iter().copied().filter(|p| *p != DESK_LINK)) {
        relink(link, &main)?;
    }
    println!("{}lolidesk {was} -> {version}（在跑的主机下次启动才用新版本）", stamp(auto));
    Ok(())
}

/// link 换成指向 target 的符号链接（先建临时链接再改名）
fn relink(link: &str, target: &str) -> Result<()> {
    if fs::read_link(link).ok().as_deref() == Some(Path::new(target)) {
        return Ok(());
    }
    let tmp = format!("{link}.lolinet");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp)?;
    fs::rename(&tmp, link)?;
    Ok(())
}
