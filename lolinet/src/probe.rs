//! 公开探针（设备模式）：准入站首页的设备卡片。只报硬件和位置，每 60 秒签名推给准入站的 /v1/probe/report；
//! 不报主机名、地址、序列号、负载。以前是 probe/reporter.py 的设备模式（要 python3），现在编在 lolinet 里：
//!
//! - 配对码兑换出的配置末尾有一行 `# probe = {...}`（scripts/pair.py 写的：公开 id d-xxxxxxxxxx、这台设备的上报密钥、
//!   上报地址）。`lolinet up -c` 把它存成 /etc/lolinet/probe.json（600），再装系统服务 `lolinet probe run`；
//! - 服务以 root 启动，读完配置就降到 nobody：读硬件（macOS 的 system_profiler / sysctl / sw_vers，Linux 的 /proc、/sys），
//!   问本机内核最近的接入点（easytier-cli peer），用系统 curl 发（参数从 stdin 给，密钥和签名不进进程列表）；
//! - 以前 join/install.sh 装的 python 版（/etc/mesh-probe/reporter.json + co.lo-li.mesh-probe / mesh-probe.service）
//!   在 `lolinet up` 和 `lolinet upgrade` 时接管：配置搬过来（公开 id 不变），python 版撤掉。relay 的不碰。
//!
//! 上报格式和签名和 reporter.py 一样，join/worker.js 验：
//!   X-Probe-Sig = hex(HMAC-SHA256(key, "<ts>." + body))，key 是 pair.py 用主密钥给这个 id 派生的那把。
//! relay 模式（负载、链路）还在 probe/reporter.py；Docker 里的设备（没有 systemd）也还用它。

use std::{
    fs,
    io::{ErrorKind, Read as _, Write as _},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, bail};
use nix::libc;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use crate::{RPC, VERSION, service, tool, upgrade::hex, write_file};

pub const CONFIG: &str = "/etc/lolinet/probe.json";
/// 配对包里探针配置那一行的前缀；它不进 EasyTier 配置
pub const PAIR_LINE: &str = "# probe = ";
/// join/install.sh（2026-11-10 及以前）装的 python 版的配置
const LEGACY_CONFIG: &str = "/etc/mesh-probe/reporter.json";
const INTERVAL: Duration = Duration::from_secs(60);
/// 硬件不常变，6 小时重读一次
const HARDWARE_EVERY: Duration = Duration::from_secs(6 * 3600);
/// 一直发不出去时，错误日志最多一小时一条（日志不轮转）
const ERROR_LOG_EVERY: Duration = Duration::from_secs(3600);
/// 菜单栏 App（no-tun 内核）的管理口；系统服务的是 RPC
const APP_RPC: &str = "127.0.0.1:15897";

// ---------- 配置 ----------

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// 公开 id：d- + 10 位小写十六进制（join/worker.js 的 DEVICE_ID）
fn valid_id(id: &str) -> bool {
    id.len() == 12 && id.starts_with("d-") && id[2..].bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn check(v: &Value) -> Result<()> {
    if field(v, "mode") != "device" {
        bail!("不是设备模式的探针配置（relay 的用 probe/reporter.py）");
    }
    if !valid_id(field(v, "name")) {
        bail!("探针公开 id 不对：{:?}", field(v, "name"));
    }
    if field(v, "key").len() != 64 || unhex(field(v, "key")).is_none() {
        bail!("探针上报密钥不对");
    }
    let url = field(v, "url");
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("探针上报地址不对：{url:?}");
    }
    Ok(())
}

/// 配对得到的配置里的 `# probe = {...}`：没有这一行是 None（配对时 --no-probe）
pub fn from_pairing(text: &str) -> Result<Option<Value>> {
    let Some(line) = text.lines().find_map(|l| l.strip_prefix(PAIR_LINE)) else {
        return Ok(None);
    };
    let v: Value = serde_json::from_str(line.trim()).context("`# probe = ` 那一行不是 JSON")?;
    check(&v)?;
    Ok(Some(v))
}

fn load(path: &Path) -> Result<Value> {
    let raw = fs::read(path).with_context(|| format!("读 {}", path.display()))?;
    let v: Value = serde_json::from_slice(&raw).with_context(|| format!("{} 不是 JSON", path.display()))?;
    check(&v).with_context(|| path.display().to_string())?;
    Ok(v)
}

/// 本机探针的公开 id；Err 是读不到（配置 600，要 root）
pub fn public_id() -> Result<Option<String>, ()> {
    match fs::read(CONFIG) {
        Ok(raw) => Ok(serde_json::from_slice::<Value>(&raw).ok().map(|v| field(&v, "name").to_owned())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// 存成 /etc/lolinet/probe.json。换了公开 id（比如设备改名后重新配对）就先把旧 id 从公开页面撤下
pub fn save(v: &Value) -> Result<()> {
    if let Ok(old) = load(Path::new(CONFIG)) {
        if field(&old, "name") != field(v, "name") {
            bye(&old);
        }
    }
    let text = format!("{}\n", serde_json::to_string_pretty(v)?);
    write_file(CONFIG, text.as_bytes(), 0o600)
}

// ---------- 装 / 接管 / 关 ----------

/// `lolinet up`（force：配置可能刚换过，总是重写服务并重启）和 `lolinet upgrade`（只在缺了、旧了的时候动）调：
/// 接管 python 版；有配置就保证服务装着、是这一版的写法。say 只在有变化时调。
pub fn reconcile(force: bool, say: &dyn Fn(String)) -> Result<()> {
    let adopted = adopt_legacy(say)?;
    if !Path::new(CONFIG).exists() {
        // 配置被删了（或从没配过）：服务也不该在
        if service::probe_state().is_some() {
            service::remove_probe()?;
            say("没有探针配置，探针服务已撤掉".into());
        }
        return Ok(());
    }
    if force || adopted || !service::probe_current() {
        service::install_probe()?;
        let id = public_id().ok().flatten().unwrap_or_default();
        say(format!("公开探针 {id} 已启动（只报硬件和位置）"));
    }
    Ok(())
}

/// python 版（join/install.sh 以前装的）：设备模式的配置搬到 /etc/lolinet/probe.json，服务和 reporter.py 撤掉。
/// 配置是 relay 模式的（relay 上 probe/deploy.py 装的）就什么都不动。返回是否接管了配置
fn adopt_legacy(say: &dyn Fn(String)) -> Result<bool> {
    let mut adopted = false;
    match fs::read(LEGACY_CONFIG) {
        Ok(raw) => {
            let v: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
            if field(&v, "mode") != "device" {
                return Ok(false);
            }
            match check(&v) {
                Ok(()) => {
                    // 以前的 install.sh 只在配对时写它，所以它在就比 /etc/lolinet/probe.json 新
                    save(&v)?;
                    adopted = true;
                }
                Err(e) => say(format!("python 版探针的配置不能用（{e:#}），只撤掉它")),
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).context(format!("读 {LEGACY_CONFIG}")),
    }
    if service::remove_legacy_probe()? || adopted {
        say(format!(
            "python 版公开探针换成了 lolinet 内置的{}",
            if adopted { "（公开 id 不变）" } else { "" }
        ));
    }
    Ok(adopted)
}

/// python 版的配置是设备模式的（relay 上的 reporter.json 没有 mode）
fn legacy_is_device() -> bool {
    fs::read(LEGACY_CONFIG).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).is_some_and(|v| field(&v, "mode") == "device")
}

/// `sudo lolinet probe off`：从公开页面撤下、卸载服务、删配置（python 版的一起）。以后要再装得重新配对
pub fn off() -> Result<()> {
    crate::need_root()?;
    let mut ids = Vec::new();
    for path in [CONFIG, LEGACY_CONFIG] {
        if let Ok(v) = load(Path::new(path)) {
            bye(&v);
            ids.push(field(&v, "name").to_owned());
        }
    }
    service::remove_probe()?;
    if legacy_is_device() {
        service::remove_legacy_probe()?;
    }
    if Path::new(CONFIG).exists() {
        fs::remove_file(CONFIG)?;
    }
    ids.dedup();
    if ids.is_empty() {
        println!("本机没装公开探针");
    } else {
        println!("公开探针已关：{} 已从公开页面撤下；以后要再装得重新配对", ids.join("、"));
    }
    Ok(())
}

// ---------- 服务入口 ----------

fn config_path(config: Option<PathBuf>) -> PathBuf {
    config.unwrap_or_else(|| CONFIG.into())
}

/// `lolinet probe run`：launchd / systemd 调，常驻
pub fn run(config: Option<PathBuf>) -> Result<()> {
    let cfg = match load(&config_path(config)) {
        Ok(c) => c,
        Err(e) => {
            // 配置坏了马上退出的话，KeepAlive / Restart=always 会一直拉起、一直写日志；等一小时再退
            eprintln!("{} 探针没启动：{e:#}", now());
            sleep(ERROR_LOG_EVERY);
            return Err(e);
        }
    };
    drop_privileges()?;
    let mut hw = hardware(&cfg);
    let mut hw_at = Instant::now();
    let mut failing: Option<Instant> = None;
    loop {
        if hw_at.elapsed() >= HARDWARE_EVERY {
            hw = hardware(&cfg);
            hw_at = Instant::now();
        }
        match send(&cfg, &report(&cfg, &hw), 20) {
            Ok(()) => {
                if failing.take().is_some() {
                    eprintln!("{} 上报恢复了", now());
                }
            }
            Err(e) => {
                if failing.is_none_or(|t| t.elapsed() >= ERROR_LOG_EVERY) {
                    eprintln!("{} 上报失败（一直失败的话一小时记一条）：{e:#}", now());
                    failing = Some(Instant::now());
                }
            }
        }
        sleep(INTERVAL);
    }
}

/// `lolinet probe [show|send]`：打印这一轮要报的内容；send 时真发一次
pub fn show(config: Option<PathBuf>, send_it: bool) -> Result<()> {
    let path = config_path(config);
    let cfg = match load(&path) {
        Err(e) if e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == ErrorKind::PermissionDenied) => {
            bail!("{} 是 600，要 root：sudo lolinet probe", path.display())
        }
        Err(e) if e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == ErrorKind::NotFound) => {
            bail!("本机没装公开探针（{} 不存在）：配对时带了探针，sudo lolinet up -c <配置> 才会装", path.display())
        }
        r => r?,
    };
    let payload = report(&cfg, &hardware(&cfg));
    println!("{}", serde_json::to_string_pretty(&payload)?);
    if send_it {
        send(&cfg, &payload, 20)?;
        eprintln!("已上报：{} -> {}", field(&cfg, "name"), field(&cfg, "url"));
    } else {
        eprintln!("（{} 每 60 秒报一次上面这些，发往 {}；sudo lolinet probe send 真发一次）", field(&cfg, "name"), field(&cfg, "url"));
    }
    Ok(())
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// root 启动的：配置读完就降到 nobody，后面采集和发送都不要特权
fn drop_privileges() -> Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    let pw = unsafe { libc::getpwnam(c"nobody".as_ptr()) };
    if pw.is_null() {
        bail!("没有 nobody 用户，不降权就不跑");
    }
    let (uid, gid) = unsafe { ((*pw).pw_uid, (*pw).pw_gid) };
    // 附加组也只留 nobody 的组（macOS 的 setgroups(0, …) 会留下一个 gid 0，不用它）
    unsafe {
        if libc::setgroups(1, &gid) != 0 || libc::setgid(gid) != 0 || libc::setuid(uid) != 0 {
            bail!("降到 nobody 失败：{}", std::io::Error::last_os_error());
        }
        if libc::setuid(0) == 0 {
            bail!("降权后还能回到 root，不跑");
        }
    }
    Ok(())
}

// ---------- 采集：只要硬件和位置 ----------

/// 跑一个命令拿 stdout（失败、超时都是 None）；system_profiler、easytier-cli 偶尔会卡住
fn output(cmd: &mut Command, timeout: Duration) -> Option<String> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let buf = reader.join().ok()?;
                return st.success().then(|| String::from_utf8_lossy(&buf).trim().to_owned());
            }
            Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn cpu_model() -> Option<String> {
    if cfg!(target_os = "macos") {
        return output(Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]), Duration::from_secs(10)).filter(|s| !s.is_empty());
    }
    read("/proc/cpuinfo")?
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.split_whitespace().collect::<Vec<_>>().join(" "))
}

fn mac_hardware() -> Map<String, Value> {
    let hw = output(Command::new("system_profiler").args(["SPHardwareDataType", "-json"]), Duration::from_secs(60))
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.pointer("/SPHardwareDataType/0").cloned())
        .unwrap_or(Value::Null);
    let text = |k: &str| Some(field(&hw, k)).filter(|s| !s.is_empty()).map(str::to_owned);
    // 只取型号和芯片；序列号、UUID 一律不碰
    let model = text("machine_name");
    let chip = text("chip_type").or_else(|| text("cpu_type")).or_else(cpu_model);
    let mem = output(Command::new("sysctl").args(["-n", "hw.memsize"]), Duration::from_secs(10)).and_then(|s| s.parse::<u64>().ok()).filter(|&m| m > 0);
    let ver = output(Command::new("sw_vers").arg("-productVersion"), Duration::from_secs(10)).filter(|s| !s.is_empty());
    let kind = if model.as_deref().is_some_and(|m| m.contains("Book")) { "笔记本" } else { "Mac" };
    let mut m = Map::new();
    m.insert("model".into(), json!(model));
    m.insert("chip".into(), json!(chip));
    m.insert("mem".into(), json!(mem));
    m.insert("os".into(), json!(ver.map_or_else(|| "macOS".to_owned(), |v| format!("macOS {v}"))));
    m.insert("kind".into(), json!(kind));
    m
}

fn linux_hardware(cfg: &Value) -> Map<String, Value> {
    let model = [read("/sys/class/dmi/id/sys_vendor"), read("/sys/class/dmi/id/product_name")].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let mem = read("/proc/meminfo").and_then(|t| {
        let line = t.lines().find(|l| l.starts_with("MemTotal:"))?.to_owned();
        line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb * 1024)
    });
    let os_release = Some(field(cfg, "os_release")).filter(|s| !s.is_empty()).unwrap_or("/etc/os-release");
    let pretty = read(os_release).and_then(|t| {
        t.lines().find_map(|l| l.strip_prefix("PRETTY_NAME=")).map(|v| v.trim().trim_matches('"').to_owned())
    });
    let mut m = Map::new();
    m.insert("model".into(), json!(Some(model).filter(|s| !s.is_empty())));
    m.insert("chip".into(), json!(cpu_model()));
    m.insert("mem".into(), json!(mem));
    m.insert("os".into(), json!(pretty.unwrap_or_else(|| "Linux".into())));
    m.insert("kind".into(), json!("服务器"));
    m
}

fn cores() -> Option<i64> {
    Some(unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) } as i64).filter(|&n| n > 0)
}

/// uname -m：macOS 上是 arm64 / x86_64（和 reporter.py 的 platform.machine() 一样）
fn arch() -> String {
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 {
        return std::env::consts::ARCH.to_owned();
    }
    unsafe { std::ffi::CStr::from_ptr(u.machine.as_ptr()) }.to_string_lossy().into_owned()
}

fn disk_total(path: &str) -> Option<u64> {
    let p = std::ffi::CString::new(path).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(p.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // 字段宽度各平台不一样（macOS 的 f_blocks 是 32 位），统一转 u64
    Some(st.f_blocks as u64 * st.f_frsize as u64)
}

fn hardware(cfg: &Value) -> Map<String, Value> {
    let mut hw = if cfg!(target_os = "macos") { mac_hardware() } else { linux_hardware(cfg) };
    let disks: Vec<Value> = match cfg.get("disks").and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(|d| Some(json!({"label": d.get("label").cloned().unwrap_or(Value::Null), "total": disk_total(d.get("path")?.as_str()?)?})))
            .collect(),
        None => disk_total("/").map(|t| json!({"label": null, "total": t})).into_iter().collect(),
    };
    hw.insert("cores".into(), json!(cores()));
    hw.insert("arch".into(), json!(arch()));
    hw.insert("disks".into(), json!(disks));
    // 配置里写了就以配置为准（比如 DMI 里是厂商代号）
    for k in ["kind", "model", "chip", "os"] {
        if !field(cfg, k).is_empty() {
            hw.insert(k.into(), json!(field(cfg, k)));
        }
    }
    hw
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim_end_matches('%').trim().parse().ok(),
        _ => None,
    }
}

/// 本机内核看到的最近接入点（直连的 relay 里延迟最小的；只用来推断大致位置，公开时换成城市）。读不到就用配置里的 near
fn nearest_relay(cfg: &Value) -> Value {
    let rpcs: Vec<&str> = match field(cfg, "rpc") {
        "" => vec![RPC, APP_RPC],
        r => vec![r],
    };
    for rpc in rpcs {
        let Some(peers) = output(tool("easytier-cli").args(["-p", rpc, "-o", "json", "peer"]), Duration::from_secs(20))
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        else {
            continue;
        };
        let best = peers
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| field(p, "cost") == "p2p")
            .filter(|p| {
                // relay / 基础设施段 10.188.0.0/24
                let ip = field(p, "ipv4").split('/').next().unwrap_or("");
                ip.parse::<Ipv4Addr>().is_ok_and(|a| a.octets()[..3] == [10, 188, 0])
            })
            .filter_map(|p| Some((field(p, "hostname"), num(p.get("lat_ms"))?)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((peer, lat)) = best {
            return json!({"peer": peer, "lat": lat});
        }
    }
    match field(cfg, "near") {
        "" => Value::Null,
        n => json!({"peer": n, "lat": null}),
    }
}

fn report(cfg: &Value, hw: &Map<String, Value>) -> Value {
    let mut device = hw.clone();
    device.insert("location".into(), cfg.get("location").cloned().unwrap_or(Value::Null));
    device.insert("near".into(), nearest_relay(cfg));
    json!({"v": 1, "name": field(cfg, "name"), "device": device})
}

// ---------- 发送 ----------

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key)[..]);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().to_vec()
}

/// 签名后 POST 到配置里的 url。用系统 curl，参数（含签名）从 stdin 的配置给，不出现在进程列表里
fn send(cfg: &Value, payload: &Value, timeout_secs: u32) -> Result<()> {
    let body = serde_json::to_string(payload)?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs().to_string();
    let key = unhex(field(cfg, "key")).context("探针上报密钥不对")?;
    let sig = hex(&hmac_sha256(&key, format!("{ts}.{body}").as_bytes()));
    // curl 配置文件的双引号字符串：只有 \\ 和 \" 要转义（JSON 里的 \n 之类先变成 \\n，curl 读回来还是 \n）
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let mut conf = format!("url = {}\n", q(field(cfg, "url")));
    for h in [
        "Content-Type: application/json".to_owned(),
        format!("User-Agent: lolinet-probe/{VERSION}"),
        format!("X-Probe-Node: {}", field(cfg, "name")),
        format!("X-Probe-Ts: {ts}"),
        format!("X-Probe-Sig: {sig}"),
    ] {
        conf += &format!("header = {}\n", q(&h));
    }
    conf += &format!("data-binary = {}\n", q(&body));
    // -q 必须是第一个参数：不读 ~/.curlrc
    let mut child = Command::new("curl")
        .args(["-q", "-sS", "--connect-timeout", "10", "--max-time", &timeout_secs.to_string(), "-o", "/dev/null", "-w", "%{http_code}", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("跑 curl")?;
    child.stdin.take().context("curl stdin")?.write_all(conf.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        bail!("{}", if err.is_empty() { format!("curl 失败（{}）", out.status) } else { err });
    }
    let code = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !code.starts_with('2') {
        bail!("准入站回 HTTP {code}");
    }
    Ok(())
}

/// 用这个 id 自己的 key 签一条 bye，准入站立刻把它从公开页面删掉（不然 7 天后才消失）。尽力而为
fn bye(cfg: &Value) {
    let name = field(cfg, "name");
    if let Err(e) = send(cfg, &json!({"v": 1, "name": name, "bye": true}), 10) {
        eprintln!("{name}：从公开页面撤下没成功（{e:#}），7 天后自动消失");
    }
}
