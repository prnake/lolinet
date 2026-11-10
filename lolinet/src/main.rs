//! lolinet：基于 EasyTier 的组网命令行。EasyTier（easytier/build.env 的版本 + patches/）整个编进同一个二进制，
//! 按 argv[0] 分发：叫 easytier-core / easytier-cli 时就是原版（`lolinet up` 把老路径的这两个名字链接过来，
//! 探针、no-tun 客户端、老脚本照用）；叫 lolinet 时管后台服务、看状态。构建见 lolinet/build.sh。

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
mod service;
mod status;
mod upgrade;

use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Write as _,
    os::unix::{
        fs::{OpenOptionsExt as _, PermissionsExt as _},
        process::CommandExt as _,
    },
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

pub const RPC: &str = "127.0.0.1:15888";
pub const CONFIG: &str = "/etc/lolinet/loli.toml";
pub const BIN: &str = "/usr/local/bin/lolinet";
pub const LOG_DIR: &str = "/var/log/lolinet";
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
    /// 装成系统服务并启动（要 root）；会接管 join 脚本以前装的 easytier 服务，默认带自动升级
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
    },
    /// 按发布清单升级 lolinet、探针和装了的 lolidesk（要 root）；定时任务每 6 小时跑一次 --auto
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

fn main() -> ExitCode {
    let argv0 = std::env::args_os().next().unwrap_or_default();
    match Path::new(&argv0).file_name().and_then(OsStr::to_str) {
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
        Cmd::Up { config, base_url, upgrade_source, auto_upgrade } => up(config, base_url, upgrade_source, auto_upgrade),
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
        Cmd::Logs { follow, lines } => {
            let mut tail = Command::new("tail");
            tail.arg("-n").arg(lines.to_string());
            if follow {
                tail.arg("-F");
            }
            Err(tail.arg(Path::new(LOG_DIR).join("easytier.log")).exec().into())
        }
        Cmd::Run { config } => {
            fs::create_dir_all(LOG_DIR).with_context(|| format!("建 {LOG_DIR}"))?;
            Err(exec_as("easytier-core", core_args(&config)))
        }
        Cmd::Core { args } => Err(exec_as("easytier-core", args)),
        Cmd::Cli { args } => Err(exec_as("easytier-cli", args)),
    }
}

/// 后台进程的 easytier-core 参数。v2.7.0 不认配置文件里的 rpc_portal，命令行给，否则管理口听 0.0.0.0；
/// 日志只写文件（10 MB 一份、留 5 份）。终端那份关掉：打洞超时之类的 warn / error 很密，launchd 收的 stderr 不轮转，
/// 一天能涨好几 MB；参数错误、panic 直接写 stderr，launchd / journald 照样收得到。
fn core_args(config: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = vec!["-c".into(), config.into()];
    for a in [
        "--rpc-portal", RPC,
        "--file-log-dir", LOG_DIR,
        "--file-log-level", "info",
        "--file-log-size", "10",
        "--file-log-count", "5",
        "--console-log-level", "off",
    ] {
        v.push(a.into());
    }
    v
}

fn up(config: Option<PathBuf>, base_url: Option<String>, upgrade_source: Option<String>, auto_upgrade: Option<bool>) -> Result<()> {
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
    fs::set_permissions("/etc/lolinet", fs::Permissions::from_mode(0o755))?;
    let migrate = !Path::new(CONFIG).exists() && Path::new(OLD_CONFIG).exists();
    let src = config.or_else(|| migrate.then(|| OLD_CONFIG.into()));
    if let Some(src) = &src {
        if !tool("easytier-core").arg("--check-config").arg("-c").arg(src).status()?.success() {
            bail!("配置检查没过：{}", src.display());
        }
        let text = fs::read_to_string(src).with_context(|| format!("读 {}", src.display()))?;
        // 配对包末尾给探针的那一行不进 EasyTier 配置（join/install.sh 也是这么处理的）
        let text: String = text.lines().filter(|l| !l.starts_with("# probe = ")).map(|l| format!("{l}\n")).collect();
        write_file(CONFIG, text.as_bytes(), 0o600)?;
        println!("配置 {} -> {CONFIG}", src.display());
    } else if !Path::new(CONFIG).exists() {
        bail!("没有配置：sudo lolinet up -c <配对得到的 loli.toml>");
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
    st.save()?;
    service::install()?;
    service::install_upgrade(st.auto)?;
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

/// 把自己装到 /usr/local/bin/lolinet（先写临时文件再改名，正在跑的旧版本不受影响）
fn install_self() -> Result<()> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if Path::new(BIN).canonicalize().ok().as_deref() == Some(exe.as_path()) {
        return Ok(());
    }
    fs::create_dir_all("/usr/local/bin")?;
    let tmp = format!("{BIN}.tmp");
    fs::copy(&exe, &tmp)?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    fs::rename(&tmp, BIN)?;
    println!("{} -> {BIN}", exe.display());
    Ok(())
}

/// 老路径的 easytier-core / easytier-cli 换成指向 lolinet 的链接：探针（reporter.py 找 easytier-cli）、
/// no-tun 客户端（如菜单栏 App）和以前的脚本不用改。
fn link_legacy_names() -> Result<()> {
    fs::create_dir_all(OLD_BIN_DIR)?;
    for name in ["easytier-core", "easytier-cli"] {
        let p = Path::new(OLD_BIN_DIR).join(name);
        if fs::read_link(&p).ok().as_deref() == Some(Path::new(BIN)) {
            continue;
        }
        let tmp = Path::new(OLD_BIN_DIR).join(format!(".{name}.lolinet"));
        let _ = fs::remove_file(&tmp);
        std::os::unix::fs::symlink(BIN, &tmp)?;
        fs::rename(&tmp, &p)?;
    }
    Ok(())
}

/// 自己换个 argv[0] 跑：easytier-core / easytier-cli 的原版入口
pub fn tool(name: &str) -> Command {
    let mut c = Command::new(std::env::current_exe().unwrap_or_else(|_| BIN.into()));
    c.arg0(name);
    c
}

fn exec_as<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(name: &str, args: I) -> anyhow::Error {
    anyhow::Error::new(tool(name).args(args).exec()).context(format!("exec {name}"))
}

pub(crate) fn need_root() -> Result<()> {
    if unsafe { nix::libc::geteuid() } != 0 {
        bail!("要 root：前面加 sudo");
    }
    Ok(())
}

/// 先写同目录的临时文件（创建时就是目标权限），再改名
pub fn write_file(path: &str, data: &[u8], mode: u32) -> Result<()> {
    let tmp = format!("{path}.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)
        .with_context(|| format!("写 {tmp}"))?;
    f.write_all(data)?;
    f.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("改名到 {path}"))?;
    Ok(())
}
