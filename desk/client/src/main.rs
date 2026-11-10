//! lolidesk：一个程序两种用法，每台电脑都一样。
//! - `lolidesk host …`：让这台电脑能被连（终端会话 + 远程桌面），见 desk/host；
//! - `lolidesk <主机>`：在当前终端里接别的电脑上的会话（TUI），或者列出会话、显示器、agent。
//!
//! 图形界面（终端 + 远程桌面）是 Lolidesk.app（desk/app），它的包里也带着这个程序。

mod tui;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use lolidesk_core::{hosts::Hosts, net};
use lolidesk_host::HostCmd;

#[derive(Parser)]
#[command(
    name = "lolidesk",
    version,
    about = "lolidesk：连 lolinet 内网里的电脑（lolidesk <主机>），也让这台电脑能被连（lolidesk host）。图形界面是 Lolidesk.app",
    args_conflicts_with_subcommands = true
)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Top>,
    /// 要连的主机：overlay 地址或主机名，可带端口（默认 10190）
    host: Option<String>,
    /// 主机上 `lolidesk host token` 打印的 token；第一次连要给，之后记住
    #[arg(short, long)]
    token: Option<String>,
    /// 接到已有会话；不给就新开一个
    #[arg(short, long)]
    session: Option<u32>,
    /// 新开会话时跑的命令（比如 claude、codex），在登录 shell 里跑，退出后留在 shell
    #[arg(short, long, conflicts_with = "session")]
    command: Option<String>,
    /// 新开会话的工作目录
    #[arg(long, conflicts_with = "session")]
    cwd: Option<String>,
    /// 不询问直接信任新主机的证书指纹
    #[arg(short = 'y', long)]
    yes: bool,
    /// 列出记住的主机
    #[arg(long)]
    list: bool,
    /// 忘掉一台主机（指纹和 token）
    #[arg(long)]
    forget: Option<String>,
    /// 只列出主机上的会话
    #[arg(long)]
    sessions: bool,
    /// 只列出主机上的显示器（顺便检查屏幕录制权限）
    #[arg(long)]
    displays: bool,
    /// 只列出主机上找得到的 agent
    #[arg(long)]
    launchers: bool,
    /// 旧版本的参数：现在默认就是在当前终端里接会话
    #[arg(long, hide = true)]
    tui: bool,
}

#[derive(Subcommand)]
enum Top {
    /// 让这台电脑能被连：run（前台运行，默认）、token、install、uninstall
    Host {
        #[command(subcommand)]
        cmd: Option<HostCmd>,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(Top::Host { cmd }) = args.cmd {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
            )
            .init();
        return lolidesk_host::main(cmd);
    }
    // stderr 会混进终端画面，只留 warn
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("warn".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    if args.list {
        for h in Hosts::load()?.list() {
            println!("{}\t{}\t{}", h.addr, h.name, h.fingerprint);
        }
        return Ok(());
    }
    if let Some(h) = &args.forget {
        let mut hosts = Hosts::load()?;
        let addr = net::resolve(h)?;
        hosts.forget(&addr);
        hosts.save()?;
        println!("已忘掉 {addr}");
        return Ok(());
    }
    let host = args
        .host
        .clone()
        .context("要连哪台？lolidesk <overlay 地址[:端口]>；让这台能被连是 lolidesk host")?;
    let addr = net::resolve(&host)?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut hosts = Hosts::load()?;
    let known = hosts.get(&addr);
    let token = args
        .token
        .clone()
        .or_else(|| known.as_ref().map(|h| h.token.clone()))
        .context("第一次连要 --token（主机上 `lolidesk host token`）")?;
    // 新主机：先只握手拿指纹，核对过再发 token
    let pinned = match known.as_ref().and_then(|h| h.fingerprint_bytes()) {
        Some(fp) => fp,
        None => {
            let fp = rt.block_on(net::fingerprint(addr))?;
            if !args.yes {
                eprint!(
                    "新主机 {addr} 的证书指纹：\n  {}\n和主机上 `lolidesk host token` 打印的指纹对一下。信任并记住？[y/N] ",
                    hex::encode(fp)
                );
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if !matches!(line.trim(), "y" | "Y" | "yes") {
                    bail!("没有信任，退出");
                }
            }
            fp
        }
    };

    let (client, fp) = rt.block_on(net::connect(
        addr,
        &token,
        Some(pinned),
        &format!("lolidesk {}", env!("CARGO_PKG_VERSION")),
    ))?;
    hosts.remember(addr, &hex::encode(fp), &token, &client.welcome.host);
    hosts.save()?;
    eprintln!(
        "已连上 {}（{}，lolidesk {}{}）",
        client.welcome.host,
        client.welcome.os,
        client.welcome.version,
        if client.welcome.persistent {
            "，会话存盘，主机重启后恢复"
        } else {
            ""
        }
    );

    if args.sessions || args.displays || args.launchers {
        use lolidesk_proto::{ControlReq, ControlResp};
        if args.sessions {
            match rt.block_on(client.control(ControlReq::ListSessions))? {
                ControlResp::Sessions(list) if list.is_empty() => println!("没有会话"),
                ControlResp::Sessions(list) => {
                    for s in list {
                        let what = if s.command.is_empty() {
                            "shell"
                        } else {
                            &s.command
                        };
                        let state = match s.exited {
                            Some(code) => format!("已退出（{code}）"),
                            None if s.restored => "恢复的".into(),
                            None => s.foreground.clone(),
                        };
                        println!(
                            "#{}\t{}×{}\t{} 人在看\t{}\t{}\t{}\t{}",
                            s.id, s.cols, s.rows, s.viewers, what, state, s.cwd, s.title
                        );
                    }
                }
                other => println!("{other:?}"),
            }
        }
        if args.displays {
            match rt.block_on(client.control(ControlReq::ListDisplays))? {
                ControlResp::Displays(list) => {
                    for d in list {
                        println!(
                            "{}\t{}×{}\t{} Hz\t{}{}",
                            d.id,
                            d.width,
                            d.height,
                            d.refresh,
                            d.name,
                            if d.primary { "\t主显示器" } else { "" }
                        );
                    }
                }
                ControlResp::Error(e) => println!("主机：{e}"),
                other => println!("{other:?}"),
            }
        }
        if args.launchers {
            match rt.block_on(client.control(ControlReq::ListLaunchers))? {
                ControlResp::Launchers(list) => {
                    for l in list {
                        println!("{}\t{}", l.name, l.command);
                    }
                }
                other => println!("{other:?}"),
            }
        }
        client.conn.close(0u32.into(), b"bye");
        return Ok(());
    }

    let open = tui::Open {
        session: args.session,
        command: args.command.clone(),
        cwd: args.cwd.clone(),
    };
    rt.block_on(tui::run(client, open))
}
