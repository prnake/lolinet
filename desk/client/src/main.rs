//! lolidesk：连 lolinet 内网里的 lolidesk-host。默认开 GUI（终端 + 远程桌面），`--tui` 在当前终端里接会话。

mod config;
mod decode;
mod gui;
mod net;
mod term;
mod tui;

use std::net::{SocketAddr, ToSocketAddrs};

use anyhow::{bail, Context, Result};
use clap::Parser;
use lolidesk_proto::Quality;

#[derive(Parser)]
#[command(name = "lolidesk", version = lolidesk_proto::BUILD, about = "lolidesk 客户端")]
struct Args {
    /// 主机：overlay 地址或主机名，可带端口（默认 10190）
    host: Option<String>,
    /// 主机上 `lolidesk-host token` 打印的 token；第一次连要给，之后记住
    #[arg(short, long)]
    token: Option<String>,
    /// 在当前终端里接会话（不开窗口）
    #[arg(long)]
    tui: bool,
    /// 接到已有会话；不给就新开一个
    #[arg(short, long)]
    session: Option<u32>,
    /// 开远程桌面并连到这个显示器（0 = 主显示器）
    #[arg(short, long)]
    desktop: Option<u32>,
    #[arg(short, long, value_enum, default_value_t = QualityArg::Balanced)]
    quality: QualityArg,
    /// 不询问直接信任新主机的证书指纹
    #[arg(short = 'y', long)]
    yes: bool,
    /// 列出记住的主机
    #[arg(long)]
    list: bool,
    /// 忘掉一台主机（指纹和 token）
    #[arg(long)]
    forget: Option<String>,
    /// 只列出主机上的会话，不开窗口
    #[arg(long)]
    sessions: bool,
    /// 只列出主机上的显示器（顺便检查屏幕录制权限），不开窗口
    #[arg(long)]
    displays: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum QualityArg {
    Smooth,
    Balanced,
    Sharp,
}

impl From<QualityArg> for Quality {
    fn from(q: QualityArg) -> Self {
        match q {
            QualityArg::Smooth => Quality::Smooth,
            QualityArg::Balanced => Quality::Balanced,
            QualityArg::Sharp => Quality::Sharp,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let gui = !args.tui;
    // GUI 时日志别往 stderr 刷太多；TUI 时 stderr 会污染屏幕，只留 warn
    let level = if gui { "info" } else { "warn" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(level.parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    if args.list {
        for h in config::Hosts::load()?.list() {
            println!("{}\t{}", h.addr, h.fingerprint);
        }
        return Ok(());
    }
    if let Some(h) = &args.forget {
        let mut hosts = config::Hosts::load()?;
        let addr = resolve(h)?;
        hosts.forget(&addr);
        hosts.save()?;
        println!("已忘掉 {addr}");
        return Ok(());
    }
    let host = args
        .host
        .clone()
        .context("要连哪台？lolidesk <overlay 地址[:端口]>")?;
    let addr = resolve(&host)?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let mut hosts = config::Hosts::load()?;
    let known = hosts.get(&addr);
    let token = args
        .token
        .clone()
        .or_else(|| known.as_ref().map(|h| h.token.clone()))
        .context("第一次连要 --token（主机上 `lolidesk-host token`）")?;
    let pinned = known.as_ref().and_then(|h| h.fingerprint_bytes());

    let (client, fp) = rt.block_on(net::connect(
        addr,
        &token,
        pinned,
        &format!("lolidesk {}", lolidesk_proto::BUILD),
    ))?;
    if pinned.is_none() {
        let fp_hex = hex::encode(fp);
        if !args.yes {
            eprintln!("新主机 {addr}（{}）证书指纹：\n  {fp_hex}\n和主机上 `lolidesk-host token` 打印的指纹对一下。信任并记住？[y/N] ", client.welcome.host);
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if !matches!(line.trim(), "y" | "Y" | "yes") {
                bail!("没有信任，退出");
            }
        }
        hosts.remember(addr, &fp_hex, &token);
        hosts.save()?;
    } else if let Some(t) = &args.token {
        hosts.remember(addr, &hex::encode(fp), t);
        hosts.save()?;
    }
    eprintln!(
        "已连上 {}（{}，lolidesk-host {}）",
        client.welcome.host, client.welcome.os, client.welcome.version
    );

    if args.sessions || args.displays {
        use lolidesk_proto::{ControlReq, ControlResp};
        if args.sessions {
            match rt.block_on(client.control(ControlReq::ListSessions))? {
                ControlResp::Sessions(list) if list.is_empty() => println!("没有会话"),
                ControlResp::Sessions(list) => {
                    for s in list {
                        println!(
                            "#{}\t{}×{}\t{} 人在看\t{}\t{}",
                            s.id, s.cols, s.rows, s.viewers, s.command, s.title
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
                            "{}\t{}×{}{}",
                            d.id,
                            d.width,
                            d.height,
                            if d.primary { "\t主显示器" } else { "" }
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

    let start = gui::Start {
        session: args.session,
        desktop: args.desktop,
        quality: args.quality.into(),
    };
    if gui {
        gui::run(rt, client, start)
    } else {
        rt.block_on(tui::run(client, args.session))
    }
}

fn resolve(host: &str) -> Result<SocketAddr> {
    let with_port = if host.contains(':') && !host.starts_with('[') {
        host.to_string()
    } else {
        format!("{host}:{}", lolidesk_proto::DEFAULT_PORT)
    };
    with_port
        .to_socket_addrs()
        .with_context(|| format!("解析 {host}"))?
        .next()
        .with_context(|| format!("{host} 没有地址"))
}
