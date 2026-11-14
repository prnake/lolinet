//! lolidesk：一个程序两种用法，每台电脑都一样。
//! - `lolidesk host …`：让这台电脑能被连（终端会话 + 远程桌面），见 desk/host；
//! - `lolidesk <主机>`：在当前终端里接别的电脑上的会话（TUI），或者列出会话、显示器、agent；
//! - `lolidesk mcp <主机>` / `lolidesk cua <主机> <工具>`：给 agent 用那台电脑的屏幕和键鼠（computer use，见 cua.rs）。
//!
//! 图形界面（终端 + 远程桌面）是 Lolidesk.app（desk/app），它的包里也带着这个程序。

mod cua;
mod pair;
mod tui;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use lolidesk_core::{
    hosts::Hosts,
    local::{self, LocalHost},
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
    /// 只列出主机上开着的窗口（远程桌面可以只推其中一个）
    #[arg(long)]
    windows: bool,
    /// 只列出主机上找得到的 agent
    #[arg(long)]
    launchers: bool,
    /// 看主机的屏幕录制、辅助功能权限开了没有（远程桌面和 computer use 要这两个）
    #[arg(long)]
    permissions: bool,
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
    /// 给 agent 的 MCP 服务（stdio）：在 <主机> 上截图、点击、打字。claude mcp add desk -- lolidesk mcp [<主机>]
    Mcp {
        /// 主机：overlay 地址或主机名，可带端口；要先配对过（lolidesk pair <主机>），或者给 --token -y。
        /// 不给就是这台电脑自己的主机（以主人身份，不用配对）
        host: Option<String>,
        #[arg(short, long)]
        token: Option<String>,
        /// 新主机不问，直接信任证书指纹
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// 和一台电脑配对：给分享链接（lolidesk://pair?…）直接配对；给主机地址就请那边的人点同意（或者 --code 用配对码）
    Pair {
        /// 分享链接，或者主机（overlay 地址或主机名，可带端口）
        target: String,
        /// 配对码（XXXX-XXXX-XXXX-XXXX）
        #[arg(short, long)]
        code: Option<String>,
        /// 新主机不问，直接信任证书指纹
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// 给一台配对过的电脑发分享链接：别的机器用它配对，不用那边再同意（那边的主人看得到、能吊销）
    Share {
        host: String,
        /// 备注：给谁、干什么用
        #[arg(long, default_value = "")]
        label: String,
        /// 多久后不能再用来配对：30m、12h、7d；不给是不过期
        #[arg(long)]
        expires: Option<String>,
        /// 最多几台设备用它配对
        #[arg(long)]
        uses: Option<u32>,
        /// 用它配对的设备只能看
        #[arg(long)]
        view_only: bool,
    },
    /// 在一台电脑上：我（主人的话是全部）配对了哪些设备、发了哪些分享链接
    Access { host: String },
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

/// 这台电脑自己的主机：回环地址，端口和主机目录里 `local` 写的一样。同一个用户读得到它的 token 和证书，
/// 就以主人身份连（和 app 里的「本机」一样），不用配对、不用 --token，也不写进记住的主机（token 重置了照样连得上）
fn local_owner(addr: SocketAddr) -> Option<LocalHost> {
    if !addr.ip().is_loopback() {
        return None;
    }
    local::local_host().filter(|l| l.addr.port() == addr.port())
}

/// 连一台主机：本机的主机直接以主人身份连；记住过的用记住的凭据和指纹。新主机：给了 --token（主机 token）就核对指纹后用它；
/// 没给就请那边的人同意配对（interactive），配对上再连。interactive = false 时（MCP）不问、不配对，没连过就报错
async fn connect(
    host: String,
    token: Option<String>,
    yes: bool,
    interactive: bool,
) -> Result<Arc<Client>> {
    let addr = net::resolve(&host)?;
    if token.is_none() {
        if let Some(l) = local_owner(addr) {
            let (client, _) = net::connect(
                addr,
                &l.token,
                Some(l.fingerprint),
                &format!("lolidesk {}", lolidesk_proto::BUILD),
            )
            .await
            .with_context(|| {
                format!("连不上本机的主机 {addr}：它没在跑（lolidesk host install 起服务）")
            })?;
            return Ok(client);
        }
    }
    let mut hosts = Hosts::load()?;
    let known = hosts.get(&addr);
    let pinned = match known.as_ref().and_then(|h| h.fingerprint_bytes()) {
        Some(fp) => fp,
        None => pair::confirm_fingerprint(addr, yes, interactive).await?,
    };
    let token = match token.or_else(|| known.as_ref().map(|h| h.token.clone())) {
        Some(t) => t,
        None if interactive => pair::pair_and_remember(addr, pinned, None).await?.1,
        None => {
            bail!("没和 {addr} 配对过：先 lolidesk pair {host}（那边的人点同意），或者给 --token")
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
    let access_cmd = matches!(&args.cmd, Some(Top::Host { cmd: Some(c) }) if c.is_access());
    if let (Some(Top::Host { cmd }), false) = (&args.cmd, access_cmd) {
        let cmd = cmd.clone();
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

    let rt = || {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    };
    match args.cmd {
        Some(Top::Host { cmd: Some(cmd) }) => return rt()?.block_on(pair::host_cmd(cmd)),
        Some(Top::Pair { target, code, yes }) => {
            return rt()?.block_on(pair::pair_cmd(&target, code, yes))
        }
        Some(Top::Share {
            host,
            label,
            expires,
            uses,
            view_only,
        }) => {
            return rt()?.block_on(async {
                let c = connect(host, None, false, true).await?;
                pair::share(c, label, expires, uses, view_only).await
            })
        }
        Some(Top::Access { host }) => {
            return rt()?.block_on(async {
                pair::access_remote(connect(host, None, false, true).await?).await
            })
        }
        Some(Top::Mcp { host, token, yes }) => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            // 不给主机：本机主机写在主机目录里的回环地址（没跑过就是默认端口，连的时候再说没在跑）
            let host = host.unwrap_or_else(|| {
                local::local_host()
                    .map(|l| l.addr.to_string())
                    .unwrap_or_else(|| "127.0.0.1".into())
            });
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

    if args.sessions || args.displays || args.windows || args.launchers || args.permissions {
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
        if args.windows {
            // 后加的请求：单独一条控制流问，旧主机不认识也不影响别的
            match rt.block_on(client.control_once(ControlReq::ListWindows)) {
                Ok(ControlResp::Windows(list)) if list.is_empty() => println!("没有开着的窗口"),
                Ok(ControlResp::Windows(list)) => {
                    for w in list {
                        println!(
                            "{}\t{}×{}\t显示器 {}\t{}\t{}{}",
                            w.id,
                            w.width,
                            w.height,
                            w.display,
                            w.app,
                            w.title,
                            if w.minimized { "\t已最小化" } else { "" }
                        );
                    }
                }
                Ok(ControlResp::Error(e)) => println!("主机：{e}"),
                Ok(other) => println!("{other:?}"),
                Err(e) => println!("{e:#}"),
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
        if args.permissions {
            match rt.block_on(client.control(ControlReq::Permissions))? {
                ControlResp::Permissions {
                    screen_recording,
                    accessibility,
                } => {
                    let s = |on: bool| if on { "开了" } else { "没开" };
                    println!("屏幕录制\t{}\t看远程桌面、截图", s(screen_recording));
                    println!("辅助功能\t{}\t远程操作键鼠", s(accessibility));
                    if !(screen_recording && accessibility) {
                        println!(
                            "在主机上：系统设置 → 隐私与安全性，给跑主机服务的 lolidesk 打开；屏幕录制开了要重启服务（lolidesk host install）"
                        );
                    }
                }
                ControlResp::Error(e) => println!("主机：{e}"),
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
