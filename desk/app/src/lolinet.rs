//! 本机的 lolinet：网络页的数据和操作，全靠调 lolinet 命令行（`/usr/local/bin/lolinet`，没有就找 PATH）。
//!
//! - 读的不要 root：`lolinet cli -o json node|peer`（本机节点、别的节点）、`lolinet invite --json`、
//!   `/etc/lolinet/exit.json`（出口节点开着时才有，644）。
//! - 要 root 的（入网、开关出口节点、装到命令行）：macOS 经 osascript 的 `with administrator privileges` 弹系统密码框，
//!   shell 命令只由校验过的几段拼出来（准入站、配对码、本机找到的路径，都加单引号），用户输入的原文不进 shell。
//!   Linux 经 pkexec（polkit 弹密码框），给的是参数表（[`pkexec_join`]、[`pkexec_exit`]、[`pkexec_link`]），不经 shell；
//!   只有入网是 `sh -c` 跑 [`JoinTarget::shell`]，它同样只由校验过、加了单引号的几段拼出来。
//! - Linux 的应用菜单（`~/.local/share/applications/lolidesk.desktop`）不要 root：从包里的 lolidesk.desktop 改出来。
//!
//! 界面只认 [`Lolinet`] 这个接口（测试换成假数据）。这些调用都会阻塞，界面在后台线程里调。

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

/// install.sh 装的位置
pub const BIN: &str = "/usr/local/bin/lolinet";
pub const DESK_BIN: &str = "/usr/local/bin/lolidesk";
/// 入网后的配置（600，但看得到在不在）；老的安装在 /etc/easytier
const CONFIGS: [&str; 2] = ["/etc/lolinet/loli.toml", "/etc/easytier/loli.toml"];
/// 出口节点的设置：开着才有
const EXIT_JSON: &str = "/etc/lolinet/exit.json";
/// 出口节点的默认端口（`lolinet exit on`）
pub const EXIT_PORT: u16 = 10180;
/// Linux：以 root 跑命令（polkit）
pub const PKEXEC: &str = "/usr/bin/pkexec";
/// Linux 的包里，lolidesk-app 旁边的东西（包根目录：lolidesk、libghostty-vt.so、BUILD-INFO，再加这几个）
pub const APP_BIN: &str = "lolidesk-app";
pub const DESKTOP_FILE: &str = "lolidesk.desktop";
pub const ICON_FILE: &str = "lolidesk.svg";
/// 接入点（relay）的网段前缀，和 `lolinet status` 一样
const RELAY_PREFIX: &str = "10.188.0.";

// ---------------------------------------------------------------- 状态

/// 本机节点（`lolinet cli -o json node`）
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    /// overlay 地址，不带前缀长度
    pub ip: String,
    /// 无 TUN（userspace）模式
    pub userspace: bool,
    pub version: String,
}

/// 网里的另一个节点（`lolinet cli -o json peer` 的一行）
#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub name: String,
    pub ip: String,
    /// 直连（p2p）；不是就是经中转
    pub direct: bool,
    /// 直连走的隧道：tcp、udp、wss……
    pub proto: String,
    pub lat_ms: Option<f64>,
    /// 接入点（relay），不是设备
    pub relay: bool,
}

/// 本机入网的情况
#[derive(Debug, Clone, PartialEq)]
pub enum Kernel {
    /// 没装 lolinet
    Missing,
    /// 装了，没入网
    NotJoined,
    /// 入了网，但问不到本机内核（服务没在跑）
    Down(String),
    Up {
        node: Node,
        peers: Vec<Peer>,
    },
}

/// 命令行装了没有
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shell {
    /// /usr/local/bin/lolinet 在不在
    pub lolinet: bool,
    /// /usr/local/bin/lolidesk 在不在；是链接的话指到哪
    pub lolidesk: Option<String>,
    /// app 里带的命令行（macOS：Contents/Helpers/lolidesk；Linux：包里 lolidesk-app 旁边的 lolidesk）；
    /// 不是从 app / 包里跑的是 None
    pub helper: Option<PathBuf>,
    /// Linux 的应用菜单：加了没有（加的是这个包里的 app）。macOS、不是从包里跑的是 None
    pub menu: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub kernel: Kernel,
    /// 出口节点开着：端口
    pub exit: Option<u16>,
    pub shell: Shell,
}

impl Status {
    pub fn node(&self) -> Option<&Node> {
        match &self.kernel {
            Kernel::Up { node, .. } => Some(node),
            _ => None,
        }
    }

    pub fn peers(&self) -> &[Peer] {
        match &self.kernel {
            Kernel::Up { peers, .. } => peers,
            _ => &[],
        }
    }
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn strip_prefix_len(ip: &str) -> String {
    ip.split('/').next().unwrap_or("").to_string()
}

/// 节点的 config（运行中的 TOML）里有 `no_tun = true` 就是 userspace，和 lolinet 自己判断的一样
fn userspace(config: &str) -> bool {
    config
        .lines()
        .any(|l| l.split('#').next().unwrap_or("").replace(' ', "") == "no_tun=true")
}

pub fn parse_node(v: &Value) -> Node {
    Node {
        name: s(v, "hostname"),
        ip: strip_prefix_len(&s(v, "ipv4_addr")),
        userspace: userspace(&s(v, "config")),
        version: s(v, "version"),
    }
}

/// 别的节点：去掉本机（cost = Local）和没有地址的（控制台的网关）。接入点在前，再按名字排。
pub fn parse_peers(v: &Value) -> Vec<Peer> {
    let Some(list) = v.as_array() else {
        return Vec::new();
    };
    let mut out: Vec<Peer> = list
        .iter()
        .filter(|p| s(p, "cost") != "Local")
        .filter_map(|p| {
            let ip = strip_prefix_len(&s(p, "ipv4"));
            if ip.is_empty() {
                return None;
            }
            let direct = s(p, "cost") == "p2p";
            Some(Peer {
                name: s(p, "hostname"),
                relay: ip.starts_with(RELAY_PREFIX),
                lat_ms: s(p, "lat_ms").parse::<f64>().ok().filter(|l| l.is_finite()),
                proto: if direct {
                    s(p, "tunnel_proto")
                } else {
                    String::new()
                },
                direct,
                ip,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.relay
            .cmp(&a.relay)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// exit.json 的端口（文件在就是开着）
pub fn parse_exit(text: &str) -> u16 {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("port").and_then(Value::as_u64))
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(EXIT_PORT)
}

// ---------------------------------------------------------------- 邀请

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteKind {
    /// 有 TUN 的电脑
    Desktop,
    /// 无 TUN 的电脑
    Userspace,
    /// 手机（mihomo 经 WireGuard 进网）
    Phone,
}

impl InviteKind {
    pub const ALL: [InviteKind; 3] = [Self::Desktop, Self::Userspace, Self::Phone];

    /// `lolinet invite --kind` 的值
    pub fn arg(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Userspace => "userspace",
            Self::Phone => "phone",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Desktop => "电脑",
            Self::Userspace => "无 TUN 电脑",
            Self::Phone => "手机",
        }
    }
}

/// `lolinet invite --json` 打的
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Invite {
    pub code: String,
    /// 新机器上跑的命令；手机是 `https://<准入站>/m#CODE` 链接
    pub command: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub kind: String,
    /// 过期时间（Unix 秒）
    #[serde(default)]
    pub expires_at: i64,
}

impl Invite {
    pub fn is_phone(&self) -> bool {
        self.kind == "phone"
    }

    /// 还剩几分钟（向上取整；不知道过期时间就按 15 分钟）
    pub fn minutes_left(&self, now: i64) -> i64 {
        if self.expires_at <= 0 {
            return 15;
        }
        ((self.expires_at - now).max(0) + 59) / 60
    }
}

pub fn parse_invite(out: &[u8]) -> Result<Invite> {
    let inv: Invite = serde_json::from_slice(out).context("lolinet invite 的输出看不懂")?;
    anyhow::ensure!(
        !inv.code.is_empty() && !inv.command.is_empty(),
        "lolinet invite 没给配对码"
    );
    Ok(inv)
}

// ---------------------------------------------------------------- 入网

/// 校验过的入网参数：只有这几段会进 shell
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinTarget {
    /// https://主机[:端口]，主机只有小写字母、数字、点和横线
    pub base: String,
    /// XXXX-XXXX-XXXX
    pub code: String,
    /// 无 TUN 入网（LOLINET_USERSPACE=1）
    pub userspace: bool,
}

/// 准入站地址：`https://[a-z0-9.-]+(:端口)?`，路径只能是空的或 `/i`。
pub fn parse_base(input: &str) -> Result<String, String> {
    let t = input.trim().trim_matches(|c| c == '\'' || c == '"');
    let rest = t
        .strip_prefix("https://")
        .ok_or_else(|| "准入站要以 https:// 开头".to_string())?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    if !matches!(path, "" | "/" | "/i") {
        return Err(format!("准入站地址后面只能是 /i：{path}"));
    }
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (hostport, None),
    };
    let host = host.to_ascii_lowercase();
    let host_ok = !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !host.starts_with(['.', '-'])
        && !host.ends_with(['.', '-'])
        && !host.contains("..");
    if !host_ok {
        return Err(format!("准入站的主机名不对：{hostport}"));
    }
    match port {
        None => Ok(format!("https://{host}")),
        Some(p) => {
            let ok = !p.is_empty()
                && p.len() <= 5
                && p.bytes().all(|b| b.is_ascii_digit())
                && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n));
            if !ok {
                return Err(format!("准入站的端口不对：{p}"));
            }
            Ok(format!(
                "https://{host}:{}",
                p.parse::<u32>().unwrap_or(443)
            ))
        }
    }
}

fn code_char(b: u8) -> bool {
    b.is_ascii_uppercase() || (b'2'..=b'9').contains(&b)
}

/// 配对码：`^[A-Z2-9]{4}-?[A-Z2-9]{4}-?[A-Z2-9]{4}$`（小写先转大写），统一成 XXXX-XXXX-XXXX。
pub fn parse_code(input: &str) -> Result<String, String> {
    let t = input
        .trim()
        .trim_matches(|c| c == '\'' || c == '"')
        .to_ascii_uppercase();
    let b = t.as_bytes();
    let groups: Option<Vec<&[u8]>> = match b.len() {
        12 => Some(vec![&b[0..4], &b[4..8], &b[8..12]]),
        13 if b[4] == b'-' => Some(vec![&b[0..4], &b[5..9], &b[9..13]]),
        13 if b[8] == b'-' => Some(vec![&b[0..4], &b[4..8], &b[9..13]]),
        14 if b[4] == b'-' && b[9] == b'-' => Some(vec![&b[0..4], &b[5..9], &b[10..14]]),
        _ => None,
    };
    match groups {
        Some(g) if g.iter().all(|g| g.iter().all(|c| code_char(*c))) => Ok(g
            .iter()
            .map(|g| String::from_utf8_lossy(g).into_owned())
            .collect::<Vec<_>>()
            .join("-")),
        _ => Err("配对码是 12 位，像 ABCD-EFGH-JKLM（字母和 2-9）".into()),
    }
}

/// 粘贴进来的配对命令：`curl -fsSL https://<准入站>/i | sudo sh -s XXXX-XXXX-XXXX`。
/// 只认这些词：curl、以 - 开头的参数、准入站地址、|、sudo、sh、LOLINET_USERSPACE=1、配对码；别的都不认。
pub fn parse_join_line(line: &str) -> Result<JoinTarget, String> {
    let mut base = None;
    let mut code = None;
    let mut userspace = false;
    for raw in line.split_whitespace() {
        let tok = raw.trim_matches(|c| c == '\'' || c == '"');
        match tok {
            "curl" | "|" | "sudo" | "sh" | "bash" | "$" => continue,
            "LOLINET_USERSPACE=1" => userspace = true,
            t if t.starts_with('-') => continue,
            t if t.starts_with("http://") => return Err("准入站要用 https".into()),
            t if t.starts_with("https://") => {
                if base.is_some() {
                    return Err("命令里有两个地址".into());
                }
                base = Some(parse_base(t)?);
            }
            t => match parse_code(t) {
                Ok(c) if code.is_none() => code = Some(c),
                Ok(_) => return Err("命令里有两个配对码".into()),
                Err(_) => return Err(format!("看不懂命令里的「{t}」")),
            },
        }
    }
    Ok(JoinTarget {
        base: base.ok_or("命令里没有准入站地址（https://…/i）")?,
        code: code.ok_or("命令里没有配对码")?,
        userspace,
    })
}

/// 单引号包起来（里面的单引号换成 '\''）
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl JoinTarget {
    /// 以 root 跑的入网命令：先把脚本下到临时文件（下载失败就是失败，不会把空脚本交给 sh），再带配对码跑。
    pub fn shell(&self) -> String {
        let env = if self.userspace {
            "LOLINET_USERSPACE=1 "
        } else {
            ""
        };
        format!(
            "exec 2>&1; f=$(mktemp) || exit 1; trap 'rm -f \"$f\"' EXIT; curl -fsSL {} -o \"$f\" && {env}sh \"$f\" {}",
            sh_quote(&format!("{}/i", self.base)),
            sh_quote(&self.code)
        )
    }

    /// 给人看的那条命令（和准入站首页上的一样）
    pub fn display(&self) -> String {
        let env = if self.userspace {
            "LOLINET_USERSPACE=1 "
        } else {
            ""
        };
        format!("curl -fsSL {}/i | sudo {env}sh -s {}", self.base, self.code)
    }
}

/// AppleScript：以管理员身份跑一条 shell 命令（弹系统密码框），输出原样带回来。
pub fn admin_script(shell: &str) -> String {
    let escaped = shell.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "do shell script \"{escaped}\" with administrator privileges without altering line endings"
    )
}

/// Linux：pkexec 跑入网脚本。shell 命令是 [`JoinTarget::shell`]（只有校验过、加了单引号的几段）
pub fn pkexec_join(t: &JoinTarget) -> Vec<String> {
    vec![PKEXEC.into(), "/bin/sh".into(), "-c".into(), t.shell()]
}

/// Linux：pkexec 跑 `/usr/local/bin/lolinet exit on|off`（先查过它是 root 的）
pub fn pkexec_exit(on: bool) -> Vec<String> {
    let what = if on { "on" } else { "off" };
    vec![PKEXEC.into(), BIN.into(), "exit".into(), what.into()]
}

/// Linux：pkexec 把包里的 lolidesk 链到 /usr/local/bin/lolidesk（-n：那里是指向目录的链接也直接换掉）
pub fn pkexec_link(helper: &str) -> Vec<String> {
    vec![
        PKEXEC.into(),
        "/bin/ln".into(),
        "-sfn".into(),
        helper.into(),
        DESK_BIN.into(),
    ]
}

/// 要以 root 跑的程序：存在、属于 root、组和其他人都写不了（它所在的目录也一样）
fn root_owned(p: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    for (path, what) in [(p, "文件"), (p.parent().unwrap_or(p), "目录")] {
        let m = std::fs::metadata(path).with_context(|| format!("没有 {}", path.display()))?;
        anyhow::ensure!(m.uid() == 0, "{what} {} 不属于 root", path.display());
        anyhow::ensure!(m.mode() & 0o022 == 0, "{what} {} 别人能写", path.display());
    }
    Ok(())
}

/// 路径不能有控制字符（进 shell 前再加单引号）
fn safe_path(p: &Path) -> Result<String> {
    let s = p.to_str().context("路径不是 UTF-8")?;
    anyhow::ensure!(!s.chars().any(char::is_control), "路径里有控制字符");
    Ok(s.to_string())
}

// ---------------------------------------------------------------- Linux 的包和应用菜单

/// Linux：解开的包里 lolidesk-app 旁边的 lolidesk。要有 BUILD-INFO 才算是包（开发时编出来的不算）
pub fn package_helper(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let helper = dir.join("lolidesk");
    (dir.join("BUILD-INFO").is_file() && helper.is_file()).then_some(helper)
}

/// 桌面文件里的路径：绝对路径，不带引号、反斜杠、$、`、% 和控制字符（这些在 Exec 里要转义，干脆不收）
fn desktop_path(p: &Path) -> Result<String> {
    let s = safe_path(p)?;
    anyhow::ensure!(p.is_absolute(), "{s} 不是绝对路径");
    anyhow::ensure!(
        !s.contains(['"', '\\', '$', '`', '%', '\'']),
        "路径里有引号、反斜杠、$、` 或 %，加不进应用菜单：{s}"
    );
    Ok(s)
}

/// 包里的 lolidesk.desktop 改成这台机器上的：Exec、TryExec、Icon 换成包里的绝对路径，别的照抄。
/// Exec 后面留着 `%u`：点了 lolidesk:// 链接（MimeType 登记了），链接当第一个参数交给 app
pub fn desktop_entry(template: &str, app: &Path, icon: &Path) -> Result<String> {
    let app = desktop_path(app)?;
    let icon = desktop_path(icon)?;
    let mut out = String::new();
    let mut seen = (false, false);
    for line in template.lines() {
        let key = line.split('=').next().unwrap_or("").trim();
        match key {
            "Exec" => {
                seen.0 = true;
                out.push_str(&format!("Exec=\"{app}\" %u\n"));
            }
            "TryExec" => out.push_str(&format!("TryExec={app}\n")),
            "Icon" => {
                seen.1 = true;
                out.push_str(&format!("Icon={icon}\n"));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    anyhow::ensure!(seen == (true, true), "{DESKTOP_FILE} 里没有 Exec 或 Icon");
    Ok(out)
}

/// 应用菜单里的文件：$XDG_DATA_HOME/applications/lolidesk.desktop（默认 ~/.local/share）
pub fn menu_entry_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("applications").join(DESKTOP_FILE))
}

// ---------------------------------------------------------------- 接口

/// 网络页要的一切。都会阻塞：界面在后台线程里调。
pub trait Lolinet: Send + Sync {
    fn status(&self) -> Status;
    /// `lolinet invite --json --kind …`
    fn invite(&self, kind: InviteKind) -> Result<Invite>;
    /// 跑入网脚本（管理员）；返回脚本的输出
    fn join(&self, target: &JoinTarget) -> Result<String>;
    /// `lolinet exit on|off`（管理员）
    fn set_exit(&self, on: bool) -> Result<String>;
    /// 把 app 里的 lolidesk 链到 /usr/local/bin/lolidesk（管理员）
    fn install_cli(&self) -> Result<String>;
    /// Linux：把这个包里的 app 加到应用菜单（不要管理员）
    fn install_menu(&self) -> Result<String> {
        bail!("这个平台没有应用菜单")
    }
}

/// 真的：调本机的 lolinet、osascript（macOS）/ pkexec（Linux）。
pub struct System;

impl System {
    /// /usr/local/bin/lolinet，没有就在 PATH 里找
    pub fn find() -> Option<PathBuf> {
        let bin = Path::new(BIN);
        if bin.is_file() {
            return Some(bin.to_path_buf());
        }
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join("lolinet"))
                .find(|p| p.is_file())
        })
    }

    /// app 里带的命令行。macOS：…/Lolidesk.app/Contents/MacOS/Lolidesk 旁边的 ../Helpers/lolidesk；
    /// Linux：解开的包里 lolidesk-app 旁边的 lolidesk（[`package_helper`]）
    pub fn helper() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        if !cfg!(target_os = "macos") {
            return package_helper(&exe);
        }
        let contents = exe.parent()?.parent()?;
        let helper = contents.join("Helpers/lolidesk");
        (contents.file_name()? == "Contents" && helper.is_file()).then_some(helper)
    }

    /// Linux：从包里跑的话，（应用菜单要的桌面文件，加到菜单里该是什么样）
    fn menu() -> Option<(PathBuf, String)> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        package_helper(&exe)?;
        let dir = exe.parent()?;
        let template = std::fs::read_to_string(dir.join(DESKTOP_FILE)).ok()?;
        let entry = desktop_entry(&template, &exe, &dir.join(ICON_FILE)).ok()?;
        Some((menu_entry_path()?, entry))
    }

    fn shell() -> Shell {
        let desk = Path::new(DESK_BIN);
        Shell {
            lolinet: Path::new(BIN).exists(),
            lolidesk: desk.symlink_metadata().ok().map(|_| {
                std::fs::read_link(desk)
                    .map(|t| t.display().to_string())
                    .unwrap_or_else(|_| DESK_BIN.to_string())
            }),
            helper: Self::helper(),
            menu: Self::menu()
                .map(|(path, entry)| std::fs::read_to_string(path).is_ok_and(|now| now == entry)),
        }
    }
}

/// 跑一个命令，最多等 `limit`；超时就杀掉。返回（退出状态，stdout，stderr）。
pub(crate) fn run(cmd: &mut Command, limit: Duration) -> Result<(ExitStatus, Vec<u8>, String)> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // 输出边读边收，免得管道满了子进程卡住
    let mut out = child.stdout.take().context("没有 stdout")?;
    let mut err = child.stderr.take().context("没有 stderr")?;
    let out_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let err_t = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            bail!("等了 {} 秒没有结果", limit.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = String::from_utf8_lossy(&err_t.join().unwrap_or_default())
        .trim()
        .to_string();
    Ok((status, stdout, stderr))
}

fn cli_json(bin: &Path, what: &str) -> Result<Value> {
    let (st, out, err) = run(
        Command::new(bin).args(["cli", "-o", "json", what]),
        Duration::from_secs(8),
    )?;
    if !st.success() {
        bail!(if err.is_empty() {
            "本机内核问不到".to_string()
        } else {
            err
        });
    }
    serde_json::from_slice(&out).context("lolinet cli 的输出看不懂")
}

/// 经 osascript 以管理员身份跑（弹系统密码框，等用户输完）。点了取消算失败。
fn run_admin(shell: &str) -> Result<String> {
    let (st, out, err) = run(
        Command::new("/usr/bin/osascript").args(["-e", &admin_script(shell)]),
        Duration::from_secs(600),
    )?;
    let out = String::from_utf8_lossy(&out).trim_end().to_string();
    if st.success() {
        return Ok(out);
    }
    if err.contains("-128") {
        bail!("取消了");
    }
    // osascript 的错误是「…: 脚本的输出 (退出码)」，输出在里面
    bail!(if err.is_empty() { out } else { err })
}

/// Linux：经 pkexec 以 root 跑（polkit 弹密码框，等用户输完）。点了取消算失败。输出是 stdout 再接 stderr。
fn run_pkexec(argv: &[String]) -> Result<String> {
    let (prog, args) = argv.split_first().context("空命令")?;
    let (st, out, err) = run(Command::new(prog).args(args), Duration::from_secs(600))
        .with_context(|| format!("跑不了 {prog}：要装 polkit（pkexec）"))?;
    let out = String::from_utf8_lossy(&out).trim_end().to_string();
    let text = [out.as_str(), err.as_str()]
        .iter()
        .filter(|t| !t.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    match st.code() {
        Some(0) => Ok(text),
        // pkexec：126 是用户关了密码框，127 是没拿到授权
        Some(126) => bail!("取消了"),
        Some(127) if text.is_empty() => bail!("没拿到管理员权限"),
        Some(c) if text.is_empty() => bail!("失败了（退出码 {c}）"),
        _ => bail!(if text.is_empty() {
            "失败了".into()
        } else {
            text
        }),
    }
}

impl Lolinet for System {
    fn status(&self) -> Status {
        let exit = std::fs::read_to_string(EXIT_JSON)
            .ok()
            .map(|t| parse_exit(&t));
        let kernel = match Self::find() {
            None => Kernel::Missing,
            Some(bin) => match cli_json(&bin, "node") {
                Ok(node) => Kernel::Up {
                    node: parse_node(&node),
                    peers: cli_json(&bin, "peer")
                        .map(|p| parse_peers(&p))
                        .unwrap_or_default(),
                },
                Err(e) if CONFIGS.iter().any(|c| Path::new(c).exists()) => {
                    Kernel::Down(format!("{e:#}"))
                }
                Err(_) => Kernel::NotJoined,
            },
        };
        Status {
            kernel,
            exit,
            shell: Self::shell(),
        }
    }

    fn invite(&self, kind: InviteKind) -> Result<Invite> {
        let bin = Self::find().context("没装 lolinet：先加入网络")?;
        let (st, out, err) = run(
            Command::new(bin).args(["invite", "--json", "--kind", kind.arg()]),
            Duration::from_secs(30),
        )?;
        if !st.success() {
            bail!(if err.is_empty() {
                "lolinet invite 失败".to_string()
            } else {
                err
            });
        }
        parse_invite(&out)
    }

    fn join(&self, target: &JoinTarget) -> Result<String> {
        if cfg!(target_os = "macos") {
            run_admin(&target.shell())
        } else {
            run_pkexec(&pkexec_join(target))
        }
    }

    fn set_exit(&self, on: bool) -> Result<String> {
        // 以 root 跑的只认装好的 /usr/local/bin/lolinet，而且要是 root 的、别人写不了：
        // 不在 PATH 里找，免得用户目录里随便一个叫 lolinet 的程序借着密码框拿到 root
        let bin = Path::new(BIN);
        root_owned(bin).with_context(|| format!("{BIN} 不能以管理员身份跑"))?;
        if !cfg!(target_os = "macos") {
            return run_pkexec(&pkexec_exit(on));
        }
        let what = if on { "on" } else { "off" };
        run_admin(&format!("{} exit {what} 2>&1", sh_quote(BIN)))
    }

    fn install_cli(&self) -> Result<String> {
        let helper = Self::helper().context(if cfg!(target_os = "macos") {
            "不是从 Lolidesk.app 里跑的，没有可链接的 lolidesk"
        } else {
            "不是从解开的 lolidesk 包里跑的，没有可链接的 lolidesk"
        })?;
        let helper = safe_path(&helper)?;
        if !cfg!(target_os = "macos") {
            run_pkexec(&pkexec_link(&helper))?;
            return Ok(format!("{DESK_BIN} -> {helper}"));
        }
        run_admin(&format!(
            "mkdir -p /usr/local/bin && ln -sf {} {DESK_BIN} && echo {DESK_BIN} '->' {}",
            sh_quote(&helper),
            sh_quote(&helper)
        ))
    }

    fn install_menu(&self) -> Result<String> {
        let (path, entry) =
            Self::menu().context("不是从解开的 lolidesk 包里跑的（旁边要有 lolidesk.desktop）")?;
        let dir = path.parent().context("应用菜单的目录不对")?;
        std::fs::create_dir_all(dir).with_context(|| format!("建不了 {}", dir.display()))?;
        // 先写临时文件再改名：菜单不会读到写了一半的
        let tmp = dir.join(format!(".{DESKTOP_FILE}.{}", std::process::id()));
        std::fs::write(&tmp, &entry).with_context(|| format!("写不了 {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("写不了 {}", path.display()))?;
        Ok(format!("{}\n{}", path.display(), entry.trim_end()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_root_owned_runs_as_root() {
        // 系统自带的程序：root 的、别人写不了
        assert!(root_owned(Path::new("/bin/sh")).is_ok());
        // 用户自己的文件（测试进程不是 root 时）不行
        if unsafe { libc_geteuid() } != 0 {
            let p = std::env::temp_dir().join(format!("lolinet-root-check-{}", std::process::id()));
            std::fs::write(&p, b"#!/bin/sh\n").unwrap();
            assert!(root_owned(&p).is_err());
            let _ = std::fs::remove_file(&p);
        }
        assert!(root_owned(Path::new("/nonexistent/lolinet")).is_err());
    }

    unsafe extern "C" {
        #[link_name = "geteuid"]
        fn libc_geteuid() -> u32;
    }

    #[test]
    fn base_urls() {
        assert_eq!(
            parse_base("https://join.example.com").unwrap(),
            "https://join.example.com"
        );
        assert_eq!(
            parse_base("https://Join.Example.com/i").unwrap(),
            "https://join.example.com"
        );
        assert_eq!(
            parse_base("https://j.example.com:8443/").unwrap(),
            "https://j.example.com:8443"
        );
        for bad in [
            "http://join.example.com",
            "join.example.com",
            "https://",
            "https://join.example.com/x",
            "https://join.example.com/i/x",
            "https://join.example.com:0",
            "https://join.example.com:99999",
            "https://join.example.com:80a",
            "https://jo_in.example.com",
            "https://join.example.com;rm",
            "https://join.example.com/i?x=1",
            "https://user@join.example.com",
            "https://-join.example.com",
            "https://join..example.com",
            "https://$(id).example.com",
        ] {
            assert!(parse_base(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn codes() {
        assert_eq!(parse_code("ABCD-EFGH-JK23").unwrap(), "ABCD-EFGH-JK23");
        assert_eq!(parse_code("abcdefghjk23").unwrap(), "ABCD-EFGH-JK23");
        assert_eq!(parse_code("ABCD-EFGHJK23").unwrap(), "ABCD-EFGH-JK23");
        assert_eq!(parse_code(" 'ABCDEFGH-JK23' ").unwrap(), "ABCD-EFGH-JK23");
        for bad in [
            "ABCD-EFGH-JK2",
            "ABCD-EFGH-JK210",
            "ABCD-EFGH-JK01",
            "ABCD--EFGHJK23",
            "ABCD EFGH JK23",
            "ABCD-EFGH-JK2;",
            "ÄBCD-EFGH-JK23",
            "",
        ] {
            assert!(parse_code(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn join_lines() {
        let t =
            parse_join_line("curl -fsSL https://join.example.com/i | sudo sh -s ABCD-EFGH-JK23")
                .unwrap();
        assert_eq!(
            t,
            JoinTarget {
                base: "https://join.example.com".into(),
                code: "ABCD-EFGH-JK23".into(),
                userspace: false
            }
        );
        let t = parse_join_line(
            "$ curl -fsSL 'https://join.example.com/i' | sudo LOLINET_USERSPACE=1 sh -s -- abcd-efgh-jk23",
        )
        .unwrap();
        assert!(t.userspace);
        assert_eq!(t.code, "ABCD-EFGH-JK23");
        for bad in [
            "curl -fsSL https://join.example.com/i | sudo sh -s ABCD-EFGH-JK23; rm -rf /",
            "curl -fsSL https://join.example.com/i | sudo sh -s ABCD-EFGH-JK23 && reboot",
            "curl -fsSL http://join.example.com/i | sudo sh -s ABCD-EFGH-JK23",
            "curl -fsSL https://join.example.com/x | sudo sh -s ABCD-EFGH-JK23",
            "curl -fsSL https://join.example.com/i | sudo sh",
            "sudo sh -s ABCD-EFGH-JK23",
            "curl https://a.example.com/i https://b.example.com/i | sh -s ABCD-EFGH-JK23",
            "curl -fsSL https://join.example.com/i | sudo sh -s ABCD-EFGH-JK23 ABCD-EFGH-JK24",
            "curl -fsSL https://join.example.com/i | sudo `id` sh -s ABCD-EFGH-JK23",
        ] {
            assert!(parse_join_line(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn join_shell_and_script() {
        let t = JoinTarget {
            base: "https://join.example.com:8443".into(),
            code: "ABCD-EFGH-JK23".into(),
            userspace: true,
        };
        let sh = t.shell();
        assert!(sh.contains("curl -fsSL 'https://join.example.com:8443/i' -o \"$f\""));
        assert!(sh.ends_with("LOLINET_USERSPACE=1 sh \"$f\" 'ABCD-EFGH-JK23'"));
        assert_eq!(
            t.display(),
            "curl -fsSL https://join.example.com:8443/i | sudo LOLINET_USERSPACE=1 sh -s ABCD-EFGH-JK23"
        );
        let script = admin_script(&sh);
        assert!(script.starts_with("do shell script \"exec 2>&1;"));
        assert!(script.contains("trap 'rm -f \\\"$f\\\"' EXIT"));
        assert!(script.ends_with("\" with administrator privileges without altering line endings"));
        // 引号和反斜杠转义后，AppleScript 字符串里不会提前结束
        let s = admin_script(r#"echo "a\"; b"#);
        assert_eq!(
            s,
            r#"do shell script "echo \"a\\\"; b" with administrator privileges without altering line endings"#
        );
        assert_eq!(sh_quote("/Apps/it's/x"), r"'/Apps/it'\''s/x'");
    }

    #[test]
    fn pkexec_argv() {
        let t = JoinTarget {
            base: "https://join.example.com".into(),
            code: "ABCD-EFGH-JK23".into(),
            userspace: false,
        };
        let argv = pkexec_join(&t);
        assert_eq!(argv[..3], ["/usr/bin/pkexec", "/bin/sh", "-c"]);
        assert_eq!(argv.len(), 4);
        assert_eq!(argv[3], t.shell());
        assert_eq!(
            pkexec_exit(true),
            ["/usr/bin/pkexec", "/usr/local/bin/lolinet", "exit", "on"]
        );
        assert_eq!(
            pkexec_exit(false),
            ["/usr/bin/pkexec", "/usr/local/bin/lolinet", "exit", "off"]
        );
        // 路径原样是一个参数：空格、引号、分号都不经 shell
        let odd = "/home/u/my apps/it's;x/lolidesk";
        assert_eq!(
            pkexec_link(odd),
            [
                "/usr/bin/pkexec",
                "/bin/ln",
                "-sfn",
                odd,
                "/usr/local/bin/lolidesk"
            ]
        );
    }

    #[test]
    fn desktop_entries() {
        let template = include_str!("../linux/lolidesk.desktop");
        let app = Path::new("/home/u/lolidesk/lolidesk-app");
        let icon = Path::new("/home/u/lolidesk/lolidesk.svg");
        let e = desktop_entry(template, app, icon).unwrap();
        assert!(e.starts_with("[Desktop Entry]\n"));
        assert!(e.contains("\nExec=\"/home/u/lolidesk/lolidesk-app\" %u\n"));
        // 分享链接 lolidesk:// 交给 app
        assert!(e.contains("\nMimeType=x-scheme-handler/lolidesk;\n"));
        assert!(e.contains("\nTryExec=/home/u/lolidesk/lolidesk-app\n"));
        assert!(e.contains("\nIcon=/home/u/lolidesk/lolidesk.svg\n"));
        // 窗口的 app_id 要和桌面文件的名字、StartupWMClass 一样
        assert_eq!(DESKTOP_FILE, format!("{}.desktop", crate::APP_ID));
        assert!(e.contains(&format!("\nStartupWMClass={}\n", crate::APP_ID)));
        assert_eq!(e.lines().count(), template.lines().count());
        // 有空格的路径可以（Exec 里加了双引号）
        let spaced = desktop_entry(template, Path::new("/home/u/My Apps/lolidesk-app"), icon);
        assert!(
            spaced
                .unwrap()
                .contains("\nExec=\"/home/u/My Apps/lolidesk-app\" %u\n")
        );
        for bad in [
            "lolidesk-app",
            "/home/u/a\"b/lolidesk-app",
            "/home/u/$HOME/lolidesk-app",
            "/home/u/a`id`/lolidesk-app",
            "/home/u/a\\b/lolidesk-app",
            "/home/u/100%/lolidesk-app",
            "/home/u/a\nExec=x/lolidesk-app",
        ] {
            assert!(
                desktop_entry(template, Path::new(bad), icon).is_err(),
                "{bad}"
            );
        }
        assert!(desktop_entry("[Desktop Entry]\nName=x\n", app, icon).is_err());
    }

    #[test]
    fn package_layout() {
        let dir = std::env::temp_dir().join(format!("lolidesk-pkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(APP_BIN);
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(dir.join("lolidesk"), b"").unwrap();
        // 开发时编出来的（旁边没有 BUILD-INFO）不算包
        assert_eq!(package_helper(&exe), None);
        std::fs::write(dir.join("BUILD-INFO"), b"lolidesk test\n").unwrap();
        assert_eq!(package_helper(&exe), Some(dir.join("lolidesk")));
        std::fs::remove_file(dir.join("lolidesk")).unwrap();
        assert_eq!(package_helper(&exe), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn node_and_peers() {
        let node: Value = serde_json::from_str(
            r#"{"hostname":"studio","ipv4_addr":"10.188.16.10/16","version":"2.7.0-abc",
                "config":"ipv4 = \"10.188.16.10/16\"\n[flags]\nno_tun = true # userspace\n"}"#,
        )
        .unwrap();
        assert_eq!(
            parse_node(&node),
            Node {
                name: "studio".into(),
                ip: "10.188.16.10".into(),
                userspace: true,
                version: "2.7.0-abc".into()
            }
        );
        let peers: Value = serde_json::from_str(
            r#"[
              {"hostname":"studio","ipv4":"10.188.16.10","cost":"Local","lat_ms":"0"},
              {"hostname":"mini","ipv4":"10.188.16.7/16","cost":"relay(2)","lat_ms":"48.2","tunnel_proto":""},
              {"hostname":"hk","ipv4":"10.188.0.1","cost":"p2p","lat_ms":"12.5","tunnel_proto":"udp"},
              {"hostname":"console","ipv4":"","cost":"p2p","lat_ms":"1"},
              {"hostname":"desktop","ipv4":"10.188.16.2","cost":"p2p","lat_ms":"-","tunnel_proto":"tcp"}
            ]"#,
        )
        .unwrap();
        let p = parse_peers(&peers);
        let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["hk", "desktop", "mini"]);
        assert!(p[0].relay && p[0].direct && p[0].proto == "udp" && p[0].lat_ms == Some(12.5));
        assert_eq!(p[1].lat_ms, None);
        assert!(!p[2].direct && p[2].ip == "10.188.16.7" && p[2].proto.is_empty());
        assert!(parse_peers(&Value::Null).is_empty());
        assert_eq!(parse_exit(r#"{"port": 10181, "lan": false}"#), 10181);
        assert_eq!(parse_exit("{}"), EXIT_PORT);
    }

    #[test]
    fn invites() {
        let inv = parse_invite(
            br#"{"code":"ABCD-EFGH-JK23","command":"https://join.example.com/m#ABCD-EFGH-JK23",
                 "name":"phone-3f2a","ip":"10.188.48.7","kind":"phone","expires_at":1000900}"#,
        )
        .unwrap();
        assert!(inv.is_phone());
        assert_eq!(inv.minutes_left(1_000_000), 15);
        assert_eq!(inv.minutes_left(1_000_850), 1);
        assert_eq!(inv.minutes_left(2_000_000), 0);
        assert!(parse_invite(br#"{"code":""}"#).is_err());
        assert!(parse_invite(b"not json").is_err());
    }
}
