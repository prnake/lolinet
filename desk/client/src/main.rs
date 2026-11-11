//! lolidesk：一个程序两种用法，每台电脑都一样。
//! - `lolidesk host …`：让这台电脑能被连（终端会话 + 远程桌面），见 desk/host；
//! - `lolidesk <主机>`：在当前终端里接别的电脑上的会话（TUI），或者列出会话、显示器、agent；
//! - `lolidesk mcp <主机>` / `lolidesk cua <主机> <工具>`：给 agent 用那台电脑的屏幕和键鼠（computer use，见 cua.rs）。
//!
//! 图形界面（终端 + 远程桌面）是 Lolidesk.app（desk/app），它的包里也带着这个程序。

mod cua;
mod tui;

use std::{path::PathBuf, sync::Arc};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use lolidesk_core::{
    hosts::Hosts,
    net::{self, Client},
};
use lolidesk_host::HostCmd;

#[derive(Parser)]
#[command(
    name = "lolidesk",
    version = lolidesk_proto::BUILD,
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

/// 叫成 lolidesk-host 时（2026-11-10 以前的包是两个程序，老的 LaunchAgent 跑 `lolidesk-host run`，
/// 升级后那个名字是指向 lolidesk 的链接）就是 `lolidesk host …`
fn argv() -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let legacy = v
        .first()
        .and_then(|a| std::path::Path::new(a).file_name())
        .is_some_and(|n| n == "lolidesk-host");
    if legacy {
        v[0] = "lolidesk".into();
        v.insert(1, "host".into());
    }
    v
}

#[derive(Subcommand)]
enum Top {
    /// 让这台电脑能被连：run（前台运行，默认）、token、install、uninstall、allow
    Host {
        #[command(subcommand)]
        cmd: Option<HostCmd>,
    },
    /// 给 agent 的 MCP 服务（stdio）：在 <主机> 上截图、点击、打字。claude mcp add desk -- lolidesk mcp <主机>
    Mcp {
        /// 主机：overlay 地址或主机名，可带端口；要先连过一次（lolidesk <主机> --token …），或者给 --token -y
        host: String,
        #[arg(short, long)]
        token: Option<String>,
        /// 新主机不问，直接信任证书指纹
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// 调一次 computer use 的工具：screenshot、click、move、drag、scroll、type_text、key、cursor、displays
    Cua {
        host: String,
        tool: String,
        /// 参数 JSON，如 '{"x": 100, "y": 200}'
        args: Option<String>,
        /// 截图存到哪（默认 ./screenshot.jpg）
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[arg(short, long)]
        token: Option<String>,
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// 连一台主机：记住过的用记住的 token 和指纹；新主机要 token，先只握手拿指纹，核对过（或 -y）再发 token。
/// interactive = false 时（MCP）不问，新主机没有 -y 就报错
async fn connect(
    host: String,
    token: Option<String>,
    yes: bool,
    interactive: bool,
) -> Result<Arc<Client>> {
    let addr = net::resolve(&host)?;
    let mut hosts = Hosts::load()?;
    let known = hosts.get(&addr);
    let token = token
        .or_else(|| known.as_ref().map(|h| h.token.clone()))
        .context("第一次连要 --token（主机上 `lolidesk host token`）")?;
    let pinned = match known.as_ref().and_then(|h| h.fingerprint_bytes()) {
        Some(fp) => fp,
        None => {
            let fp = net::fingerprint(addr).await?;
            if !yes {
                if !interactive {
                    bail!("没连过 {addr}：先在终端里 lolidesk {host} --token <token> 连一次核对指纹，或者加 -y");
                }
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
    let (client, fp) = net::connect(
        addr,
        &token,
        Some(pinned),
        &format!("lolidesk {}", lolidesk_proto::BUILD),
    )
    .await?;
    hosts.remember(addr, &hex::encode(fp), &token, &client.welcome.host);
    hosts.save()?;
    Ok(client)
}

fn cua_client(host: String, token: Option<String>, yes: bool) -> cua::Cua {
    cua::Cua::new(Box::new(move || {
        let (host, token) = (host.clone(), token.clone());
        Box::pin(connect(host, token, yes, false))
    }))
}

fn main() -> Result<()> {
    let args = Args::parse_from(argv());
    if let Some(Top::Host { cmd }) = args.cmd {
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
            )
            .init();
        return lolidesk_host::main(cmd);
    }
    // stderr 会混进终端画面，只留 warn（MCP 的 stdout 是协议，日志也只能进 stderr）
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("warn".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    match args.cmd {
        Some(Top::Mcp { host, token, yes }) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let label = host.clone();
            return rt.block_on(cua::mcp(cua_client(host, token, yes), label));
        }
        Some(Top::Cua {
            host,
            tool,
            args: a,
            out,
            token,
            yes,
        }) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            return rt.block_on(cua::once(
                cua_client(host, token, yes),
                &tool,
                a.as_deref().unwrap_or(""),
                out,
            ));
        }
        _ => {}
    }

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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let client = rt.block_on(connect(host, args.token.clone(), args.yes, true))?;
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
