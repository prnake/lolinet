//! lolinet：基于 EasyTier 的组网命令行。EasyTier（easytier/build.env 的版本 + patches/）整个编进同一个二进制，
//! 按 argv[0] 分发：叫 easytier-core / easytier-cli 时就是原版（`lolinet up` 把老路径的这两个名字链接过来，
//! no-tun 客户端、老脚本照用）；叫 lolinet 时管后台服务、看状态、跑公开探针。构建见 lolinet/build.sh。

#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
use mimalloc::MiMalloc;

#[cfg(all(feature = "mimalloc", not(feature = "jemalloc")))]
#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

#[cfg(feature = "jemalloc")]
#[global_allocator]
static ALLOC: jemallocator::Jemalloc = jemallocator::Jemalloc;

// 和 easytier-core.rs 一样
#[cfg(not(feature = "jemalloc-prof"))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "malloc_conf")]
pub static malloc_conf: &[u8] = b"dirty_decay_ms:1000,muzzy_decay_ms:1000,retain:false\0";

rust_i18n::i18n!("locales", fallback = "en");

#[path = "../easytier-cli.rs"]
mod et_cli;
mod exit;
mod invite;
mod manager;
mod mihomo;
mod netcfg;
mod probe;
mod service;
mod status;
mod sys;
mod upgrade;

use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

pub const RPC: &str = "127.0.0.1:15888";
#[cfg(unix)]
pub const CONFIG: &str = "/etc/lolinet/loli.toml";
#[cfg(unix)]
pub const BIN: &str = "/usr/local/bin/lolinet";
#[cfg(unix)]
pub const LOG_DIR: &str = "/var/log/lolinet";
// Windows（第一步只前台跑，服务下一步做）：配置和日志在 ProgramData，程序在 Program Files
#[cfg(not(unix))]
pub const CONFIG: &str = r"C:\ProgramData\lolinet\loli.toml";
#[cfg(not(unix))]
pub const BIN: &str = r"C:\Program Files\lolinet\lolinet.exe";
#[cfg(not(unix))]
pub const LOG_DIR: &str = r"C:\ProgramData\lolinet\log";
/// Windows 上换不了 argv[0]：[`tool`] 用这个环境变量告诉子进程当 easytier-core 还是 easytier-cli
#[cfg(not(unix))]
const AS_ENV: &str = "LOLINET_AS";
pub const VERSION: &str = match option_env!("LOLINET_VERSION") {
    Some(v) => v,
    None => "dev",
};
/// join/install.sh（2026-11-09 及以前）装的配置和 easytier-core / easytier-cli 所在目录
const OLD_CONFIG: &str = "/etc/easytier/loli.toml";
const OLD_BIN_DIR: &str = if cfg!(target_os = "macos") { "/usr/local/libexec/easytier" } else { "/opt/easytier" };

#[derive(Parser)]
#[command(name = "lolinet", version = VERSION, about = "lolinet：EasyTier 组网的后台服务和状态（EasyTier 编在同一个二进制里）")]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 服务、本机、接入点和设备（不带子命令时就是这个）
    Status,
    /// 节点表（easytier-cli peer）
    Peers,
    /// 路由表（easytier-cli route）
    Routes,
    /// 装成系统服务并启动（要 root）；会接管 join 脚本以前装的 easytier 服务，默认带自动升级。
    /// 配置末尾有配对时的 `# probe = {...}` 就一起装公开探针
    Up {
        /// 换成这份配置（配对得到的 loli.toml）；不给就用现有的
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// 准入站地址（join/install.sh 经环境变量传进来），自动升级默认看 <它>/dl/lolinet.json；不给就沿用上次的
        #[arg(long, env = "LOLINET_BASE_URL")]
        base_url: Option<String>,
        /// 升级来源：github:<owner>/<repo>（GitHub Releases）、清单 URL，或者 base（用准入站）；不给就沿用上次的
        #[arg(long)]
        upgrade_source: Option<String>,
        /// 自动升级 on / off（默认 on）；不给就沿用上次的
        #[arg(long, value_parser = clap::builder::BoolishValueParser::new())]
        auto_upgrade: Option<bool>,
        /// 无 TUN 入网：本机程序经 127.0.0.1:18080 的 SOCKS5 进网，发到本机 overlay 地址的连接转给 127.0.0.1。
        /// 容器、没有 /dev/net/tun、或者 mihomo 之类已经占着 TUN 时用；不给就沿用配置里的
        #[arg(long, env = "LOLINET_USERSPACE", value_parser = clap::builder::FalseyValueParser::new(), conflicts_with = "tun")]
        userspace: bool,
        /// 换回 TUN 模式（默认）
        #[arg(long)]
        tun: bool,
    },
    /// 按发布清单升级 lolinet 和装了的 lolidesk（要 root）；定时任务每 6 小时跑一次 --auto
    Upgrade {
        /// 只看发布的版本，不下载不换（不要 root）
        #[arg(long)]
        check: bool,
        /// 下载、校验 sha256、试跑新版本，但不换（不要 root）
        #[arg(long)]
        dry_run: bool,
        /// 定时任务用：自动升级关了就什么都不做，平时不输出
        #[arg(long, hide = true)]
        auto: bool,
        /// 这次用别的来源：github:<owner>/<repo> 或清单 URL
        #[arg(long)]
        source: Option<String>,
    },
    /// 停掉并卸载系统服务（要 root），配置和二进制保留
    Down,
    /// 邀请另一台机器入网：问内网管理面要一个配对码，地址由中心分配（不要 root，也不要控制台账号）
    Invite {
        /// desktop（有 TUN 的电脑）、userspace（无 TUN）、phone（mihomo 经 WireGuard 进网）、server（默认只有管理员能邀请）
        #[arg(long, default_value = "desktop", value_parser = ["desktop", "userspace", "phone", "server"])]
        kind: String,
        /// 新机器的名字（小写字母、数字、-）；不给就是 <种类>-<4 位十六进制>
        #[arg(long)]
        name: Option<String>,
        /// 管理面：https://manager.<edge_zone>，可以带 @<签发 relay 的 overlay 地址>；不给就用配对时记下的（root 跑会记住）
        #[arg(long)]
        manager: Option<String>,
        /// 输出 JSON（给 Lolidesk.app 用）
        #[arg(long)]
        json: bool,
    },
    /// 这台电脑的 mihomo 配置：网内地址走 lolinet，规则和出口节点由内网管理面集中下发。
    /// 不带子命令打印配置；on / off 把 mihomo 跑成系统服务（要 root）
    #[command(args_conflicts_with_subcommands = true)]
    Mihomo {
        #[command(subcommand)]
        cmd: Option<MihomoCmd>,
        /// tun 或 userspace；不给就看本机内核是哪种
        #[arg(long, value_parser = ["tun", "userspace"])]
        mode: Option<String>,
        /// 写到这个文件；不给就打到 stdout
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(long)]
        manager: Option<String>,
    },
    /// 出口节点：网里别的机器经这台的 SOCKS5（端口 10180）上网，只对 overlay 开（不带子命令就是 status）
    Exit {
        #[command(subcommand)]
        cmd: Option<ExitCmd>,
    },
    /// 公开探针：准入站首页的设备卡片，只报硬件和位置（不带子命令就是 show）
    Probe {
        #[command(subcommand)]
        cmd: Option<ProbeCmd>,
    },
    /// 重启后台进程（要 root）
    Restart,
    /// 看内核日志
    Logs {
        /// 一直跟着输出
        #[arg(short, long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// 服务入口：前台跑 EasyTier 内核（launchd / systemd 调它）
    Run {
        #[arg(short, long, default_value = CONFIG)]
        config: PathBuf,
    },
    /// 原版 easytier-core，参数原样传
    #[command(disable_help_flag = true)]
    Core {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// 原版 easytier-cli，参数原样传（默认连本机服务的管理口）
    #[command(disable_help_flag = true)]
    Cli {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
}

#[derive(Subcommand)]
enum MihomoCmd {
    /// 取配置、用 mihomo -t 检查，装成系统服务并启动（要 root）
    On {
        /// mihomo 的二进制；不给就用 Lolidesk.app 里带的、系统里装的
        #[arg(long)]
        bin: Option<PathBuf>,
        #[arg(long, value_parser = ["tun", "userspace"])]
        mode: Option<String>,
        #[arg(long)]
        manager: Option<String>,
    },
    /// 停掉并卸载（要 root）
    Off,
    /// 马上从管理面重新取配置，变了就换上、重启 mihomo（要 root；自动升级的定时任务每 6 小时也做一次）
    Refresh,
}

#[derive(Subcommand)]
enum ExitCmd {
    /// 看开没开、地址和服务状态
    Status,
    /// 开（要 root）：装成系统服务，开机自启
    On {
        #[arg(long, default_value_t = exit::PORT)]
        port: u16,
        /// 只让这些网段用（可以给多次），默认整个 overlay 网段
        #[arg(long)]
        allow: Vec<String>,
        /// 也让人经这台去私网地址（10/8、172.16/12、192.168/16 等，相当于子网路由）；回环和链路本地始终不行
        #[arg(long)]
        lan: bool,
    },
    /// 关（要 root）
    Off,
    /// 服务入口；--listen 给了就不读配置（测试用）
    #[command(hide = true)]
    Run {
        #[arg(long)]
        listen: Option<std::net::SocketAddr>,
        #[arg(long)]
        allow: Vec<String>,
        #[arg(long)]
        lan: bool,
    },
}

#[derive(Subcommand)]
enum ProbeCmd {
    /// 打印这一轮要报的内容，不发（配置是 600，要 sudo）
    Show {
        /// 探针配置，默认 /etc/lolinet/probe.json
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// 照常采一次并真发出去，看准入站收没收
    Send {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// 关掉（要 root）：从公开页面撤下、卸载服务、删配置；以后要再装得重新配对
    Off,
    /// 服务入口：常驻，每 60 秒上报一次（launchd / systemd 调它）
    #[command(hide = true)]
    Run {
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

/// 当谁跑：argv[0] 的文件名（Unix 上 easytier-core / easytier-cli 是指向 lolinet 的链接）；
/// Windows 上看 [`AS_ENV`]（看完就删，内核起的子进程不会跟着当错），再看去掉 .exe 的文件名
fn invoked_as() -> Option<String> {
    #[cfg(not(unix))]
    if let Ok(name) = std::env::var(AS_ENV) {
        // 还没起别的线程
        unsafe { std::env::remove_var(AS_ENV) };
        return Some(name);
    }
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let p = Path::new(&argv0);
    let name = if cfg!(unix) { p.file_name() } else { p.file_stem() };
    name.and_then(OsStr::to_str).map(str::to_owned)
}

fn main() -> ExitCode {
    match invoked_as().as_deref() {
        // 和 easytier-core.rs 一样用单线程运行时
        Some("easytier-core") => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(easytier::core::main()),
        Some("easytier-cli") => match et_cli::main() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e:?}");
                ExitCode::FAILURE
            }
        },
        _ => match lolinet(Args::parse()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("lolinet: {e:#}");
                ExitCode::FAILURE
            }
        },
    }
}

fn lolinet(args: Args) -> Result<()> {
    match args.cmd.unwrap_or(Cmd::Status) {
        Cmd::Status => status::show(),
        Cmd::Peers => Err(exec_as("easytier-cli", ["peer"])),
        Cmd::Routes => Err(exec_as("easytier-cli", ["route"])),
        Cmd::Up { config, base_url, upgrade_source, auto_upgrade, userspace, tun } => {
            let mode = if userspace { Some(true) } else if tun { Some(false) } else { None };
            up(config, base_url, upgrade_source, auto_upgrade, mode)
        }
        Cmd::Upgrade { check, dry_run, auto, source } => upgrade::run(check, dry_run, auto, source),
        Cmd::Down => {
            need_root()?;
            service::remove()?;
            println!("服务已停止并卸载；配置 {CONFIG} 留着，sudo lolinet up 重新拉起");
            Ok(())
        }
        Cmd::Restart => {
            need_root()?;
            service::restart()
        }
        Cmd::Invite { kind, name, manager, json } => invite::run(kind, name, manager, json),
        Cmd::Mihomo { cmd, mode, out, manager } => match cmd {
            None => mihomo::run(mode, out, manager),
            Some(MihomoCmd::On { bin, mode, manager }) => mihomo::on(bin, mode, manager),
            Some(MihomoCmd::Off) => mihomo::off(),
            Some(MihomoCmd::Refresh) => mihomo::refresh_cmd(),
        },
        Cmd::Exit { cmd } => match cmd.unwrap_or(ExitCmd::Status) {
            ExitCmd::Status => exit::status(),
            ExitCmd::On { port, allow, lan } => exit::on(port, allow, lan),
            ExitCmd::Off => exit::off(),
            ExitCmd::Run { listen, allow, lan } => exit::run(listen, allow, lan),
        },
        Cmd::Probe { cmd } => match cmd.unwrap_or(ProbeCmd::Show { config: None }) {
            ProbeCmd::Show { config } => probe::show(config, false),
            ProbeCmd::Send { config } => probe::show(config, true),
            ProbeCmd::Off => probe::off(),
            ProbeCmd::Run { config } => probe::run(config),
        },
        Cmd::Logs { follow, lines } => logs(follow, lines),
        Cmd::Run { config } => {
            let dir = log_dir()?;
            Err(exec_as("easytier-core", core_args(&config, &dir)))
        }
        Cmd::Core { args } => Err(exec_as("easytier-core", args)),
        Cmd::Cli { args } => Err(exec_as("easytier-cli", args)),
    }
}

/// 后台进程的 easytier-core 参数。v2.7.0 不认配置文件里的 rpc_portal，命令行给，否则管理口听 0.0.0.0；
/// 日志只写文件（10 MB 一份、留 5 份）。终端那份关掉：打洞超时之类的 warn / error 很密，launchd 收的 stderr 不轮转，
/// 一天能涨好几 MB；参数错误、panic 直接写 stderr，launchd / journald 照样收得到。
/// 日志目录：服务（root）写 /var/log/lolinet；没有 root 跑（容器里、userspace 模式）写不了那里，就写用户自己的
/// $XDG_STATE_HOME/lolinet（默认 ~/.local/state/lolinet）
fn log_dir() -> Result<PathBuf> {
    if fs::create_dir_all(LOG_DIR).is_ok() && sys::writable_dir(Path::new(LOG_DIR)) {
        return Ok(LOG_DIR.into());
    }
    // Windows：%LOCALAPPDATA%\lolinet；Unix：$XDG_STATE_HOME/lolinet
    let state = if cfg!(unix) {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
    } else {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    .with_context(|| format!("没有 HOME，也写不了 {LOG_DIR}"))?;
    let dir = state.join("lolinet");
    fs::create_dir_all(&dir).with_context(|| format!("建 {}", dir.display()))?;
    eprintln!("lolinet：没有权限写 {LOG_DIR}，内核日志写到 {}", dir.display());
    Ok(dir)
}

fn core_args(config: &Path, log_dir: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = vec!["-c".into(), config.into(), "--file-log-dir".into(), log_dir.into()];
    for a in [
        "--rpc-portal", RPC,
        "--file-log-level", "info",
        "--file-log-size", "10",
        "--file-log-count", "5",
        "--console-log-level", "off",
    ] {
        v.push(a.into());
    }
    v
}

fn up(
    config: Option<PathBuf>,
    base_url: Option<String>,
    upgrade_source: Option<String>,
    auto_upgrade: Option<bool>,
    userspace: Option<bool>,
) -> Result<()> {
    need_root()?;
    if let Some(b) = &base_url {
        if !(b.starts_with("https://") || b.starts_with("http://")) {
            bail!("--base-url 要是 http(s):// 开头的准入站地址");
        }
    }
    if let Some(s) = &upgrade_source {
        if s != "base" && !upgrade::valid_source(s) {
            bail!("--upgrade-source 要是 github:<owner>/<repo>、清单 URL 或 base");
        }
    }
    // 目录 755：loli.toml 自己是 600；lolinet.json（升级设置，没有密钥）普通用户也读得到，lolinet status 要显示
    fs::create_dir_all("/etc/lolinet")?;
    sys::chmod("/etc/lolinet", 0o755)?;
    let migrate = !Path::new(CONFIG).exists() && Path::new(OLD_CONFIG).exists();
    let src = config.or_else(|| migrate.then(|| OLD_CONFIG.into()));
    let mut probe_cfg = None;
    let mut lolinet_line = None;
    if let Some(src) = &src {
        if !tool("easytier-core").arg("--check-config").arg("-c").arg(src).status()?.success() {
            bail!("配置检查没过：{}", src.display());
        }
        let text = fs::read_to_string(src).with_context(|| format!("读 {}", src.display()))?;
        // 配对包末尾给探针的那一行（scripts/pair.py 写的公开 id + 上报密钥）单独存，不进 EasyTier 配置
        match probe::from_pairing(&text) {
            Ok(p) => probe_cfg = p,
            Err(e) => eprintln!("配置里的探针那一行用不了，这次不装公开探针：{e:#}"),
        }
        // 配对包末尾给 lolinet 自己的那一行：内网管理面在哪（lolinet invite 用），存进 lolinet.json
        lolinet_line = manager::from_pairing(&text);
        let mut text: String = text
            .lines()
            .filter(|l| !l.starts_with(probe::PAIR_LINE) && !l.starts_with(manager::PAIR_LINE))
            .map(|l| format!("{l}\n"))
            .collect();
        if let Some(u) = userspace {
            text = netcfg::set_mode(&text, u);
        }
        // 装着 mihomo 服务：它经回环 SOCKS5 进网，切回 TUN 也别把这个口关了
        if mihomo::installed().is_some() {
            text = netcfg::set_socks(&text, true);
        }
        write_config(&text)?;
        println!("配置 {} -> {CONFIG}", src.display());
    } else if !Path::new(CONFIG).exists() {
        bail!("没有配置：sudo lolinet up -c <配对得到的 loli.toml>");
    } else if let Some(u) = userspace {
        let text = fs::read_to_string(CONFIG)?;
        let mut new = netcfg::set_mode(&text, u);
        if mihomo::installed().is_some() {
            new = netcfg::set_socks(&new, true);
        }
        if new != text {
            write_config(&new)?;
        }
    }
    let net = netcfg::Net::parse(&fs::read_to_string(CONFIG)?)?;
    if net.userspace {
        println!(
            "模式    userspace（无 TUN）：本机程序经 socks5://{} 进网；发到本机 overlay 地址的连接会转给 127.0.0.1",
            net.socks.as_deref().unwrap_or(netcfg::SOCKS)
        );
    }
    install_self()?;
    service::remove_legacy()?;
    link_legacy_names()?;
    fs::create_dir_all(LOG_DIR)?;
    let mut st = upgrade::Settings::load();
    if let Some(b) = base_url {
        st.base_url = Some(b.trim_end_matches('/').to_owned());
    }
    if let Some(s) = upgrade_source {
        st.source = (s != "base").then_some(s);
    }
    if let Some(a) = auto_upgrade {
        st.auto = a;
    }
    if let Some((m, ip)) = lolinet_line {
        st.manager = Some(m);
        st.manager_ip = ip;
    }
    st.save()?;
    service::install()?;
    service::install_upgrade(st.auto)?;
    // 公开探针：这次的配置带了就换上；以前 install.sh 装的 python 版接管过来。探针出错不影响入网
    let probe_res = (|| {
        if let Some(p) = &probe_cfg {
            probe::save(p)?;
        }
        probe::reconcile(true, &|m| println!("{m}"))
    })();
    if let Err(e) = probe_res {
        eprintln!("公开探针没装上：{e:#}");
    }
    // 出口节点开着的话，模式或地址可能变了：重新拉起
    exit::refresh();
    // 等内核起来、管理口连得上，再给 peer 几秒握手
    for _ in 0..30 {
        if status::node().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    std::thread::sleep(Duration::from_secs(5));
    status::show()
}

/// 换配置前先让 easytier-core 检查一遍：检查过的才换上（先写同目录临时文件，600）
pub(crate) fn write_config(text: &str) -> Result<()> {
    let check = format!("{CONFIG}.check");
    write_file(&check, text.as_bytes(), 0o600)?;
    let ok = tool("easytier-core").arg("--check-config").arg("-c").arg(&check).status()?.success();
    if !ok {
        let _ = fs::remove_file(&check);
        bail!("新配置检查没过（{CONFIG} 没动）");
    }
    fs::rename(&check, CONFIG).with_context(|| format!("改名到 {CONFIG}"))?;
    Ok(())
}

/// 把自己装到 /usr/local/bin/lolinet（先写临时文件再改名，正在跑的旧版本不受影响）
fn install_self() -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if Path::new(BIN).canonicalize().ok().as_deref() == Some(exe.as_path()) {
        return Ok(());
    }
    fs::create_dir_all("/usr/local/bin")?;
    let tmp = format!("{BIN}.tmp");
    fs::copy(&exe, &tmp)?;
    sys::chmod(&tmp, 0o755)?;
    fs::rename(&tmp, BIN)?;
    println!("{} -> {BIN}", exe.display());
    Ok(())
}

/// 老路径的 easytier-core / easytier-cli 换成指向 lolinet 的链接：no-tun 客户端（如菜单栏 App）、
/// 还没换成内置探针的 python 版（reporter.py 找 easytier-cli）和以前的脚本不用改。
fn link_legacy_names() -> Result<()> {
    fs::create_dir_all(OLD_BIN_DIR)?;
    for name in ["easytier-core", "easytier-cli"] {
        let p = Path::new(OLD_BIN_DIR).join(name);
        if fs::read_link(&p).ok().as_deref() == Some(Path::new(BIN)) {
            continue;
        }
        let tmp = Path::new(OLD_BIN_DIR).join(format!(".{name}.lolinet"));
        let _ = fs::remove_file(&tmp);
        sys::symlink(BIN, &tmp)?;
        fs::rename(&tmp, &p)?;
    }
    Ok(())
}

/// 自己换个身份跑：easytier-core / easytier-cli 的原版入口（Unix 换 argv[0]，Windows 设 [`AS_ENV`]）
pub fn tool(name: &str) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap_or_else(|_| BIN.into()));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        c.arg0(name);
    }
    #[cfg(not(unix))]
    c.env(AS_ENV, name);
    c
}

fn exec_as<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(name: &str, args: I) -> anyhow::Error {
    let mut c = tool(name);
    c.args(args);
    exec(c).context(format!("exec {name}"))
}

/// Unix：换成这个程序（不回来）。Windows 没有 exec：起子进程等它退出，带着它的退出码退出
fn exec(mut c: Command) -> anyhow::Error {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        anyhow::Error::new(c.exec())
    }
    #[cfg(not(unix))]
    match c.status() {
        Ok(st) => std::process::exit(st.code().unwrap_or(1)),
        Err(e) => anyhow::Error::new(e),
    }
}

/// 内核日志的最后几行（-f 一直跟着）。Unix 用 tail -F（日志轮转了也跟得上）；Windows 自己读
fn logs(follow: bool, lines: usize) -> Result<()> {
    let file = Path::new(LOG_DIR).join("easytier.log");
    if cfg!(unix) {
        let mut tail = Command::new("tail");
        tail.arg("-n").arg(lines.to_string());
        if follow {
            tail.arg("-F");
        }
        tail.arg(&file);
        return Err(exec(tail));
    }
    let text = fs::read(&file).with_context(|| format!("读 {}", file.display()))?;
    let text = String::from_utf8_lossy(&text);
    let all: Vec<&str> = text.lines().collect();
    for l in &all[all.len().saturating_sub(lines)..] {
        println!("{l}");
    }
    if !follow {
        return Ok(());
    }
    let mut pos = text.len() as u64;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let len = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        // 轮转了（文件变短）：从头读新的那份
        if len < pos {
            pos = 0;
        }
        if len > pos {
            use std::io::{Read as _, Seek as _, SeekFrom};
            let mut f = fs::File::open(&file)?;
            f.seek(SeekFrom::Start(pos))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            pos += buf.len() as u64;
            print!("{}", String::from_utf8_lossy(&buf));
            std::io::stdout().flush()?;
        }
    }
}

pub(crate) fn need_root() -> Result<()> {
    if cfg!(not(unix)) {
        bail!(
            "Windows 上还不会装系统服务（下一步做，见 docs/design.md 的 Windows 一节）。\
             现在先在前台跑内核：lolinet core -c <配置>（无 TUN 入网）"
        );
    }
    if !sys::is_root() {
        bail!("要 root：前面加 sudo");
    }
    Ok(())
}

/// 先写同目录的临时文件（创建时就是目标权限），再改名
pub fn write_file(path: &str, data: &[u8], mode: u32) -> Result<()> {
    let tmp = format!("{path}.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = sys::create_new(&tmp, mode).with_context(|| format!("写 {tmp}"))?;
    f.write_all(data)?;
    f.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("改名到 {path}"))?;
    Ok(())
}
