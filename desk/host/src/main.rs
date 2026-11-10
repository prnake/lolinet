//! lolidesk-host：跑在被控电脑上。一个进程持所有终端会话（断线不丢），按需采屏编码。
//! 只监听 lolinet overlay 地址（10.188.0.0/16），自签证书 + 共享 token。

mod config;
mod desktop;
mod input;
mod net;
mod terminal;

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "lolidesk-host", version = lolidesk_proto::BUILD, about = "lolidesk 主机端")]
struct Args {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 前台运行（launchd / systemd 调它）
    Run {
        /// 监听地址；默认本机 10.188.0.0/16 的地址:10190，没有就 0.0.0.0:10190
        #[arg(short, long)]
        listen: Option<SocketAddr>,
    },
    /// 打印（或 --reset 重新生成）客户端要用的 token 和证书指纹
    Token {
        #[arg(long)]
        reset: bool,
    },
    /// macOS：装成当前用户的 LaunchAgent（屏幕录制 / 辅助功能权限跟着这个二进制走）
    Install,
    Uninstall,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    let args = Args::parse();
    match args.cmd.unwrap_or(Cmd::Run { listen: None }) {
        Cmd::Run { listen } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(run(listen))
        }
        Cmd::Token { reset } => {
            let cfg = config::HostConfig::load_or_create(reset)?;
            println!("token      {}", cfg.token);
            println!("指纹       {}", cfg.fingerprint_hex());
            println!("目录       {}", cfg.dir.display());
            Ok(())
        }
        Cmd::Install => config::install_launch_agent(),
        Cmd::Uninstall => config::uninstall_launch_agent(),
    }
}

async fn run(listen: Option<SocketAddr>) -> Result<()> {
    let cfg = config::HostConfig::load_or_create(false)?;
    let addr = listen.unwrap_or_else(|| {
        let ip = config::overlay_ip().unwrap_or_else(|| {
            tracing::warn!("没找到 10.188.0.0/16 的地址，监听 0.0.0.0（只靠 token 保护）");
            "0.0.0.0".parse().unwrap()
        });
        SocketAddr::new(ip, lolidesk_proto::DEFAULT_PORT)
    });
    let server = cfg.quic_server_config()?;
    let endpoint = quinn::Endpoint::server(server, addr).with_context(|| format!("监听 {addr}"))?;
    tracing::info!(
        %addr,
        fingerprint = %cfg.fingerprint_hex(),
        desktop = desktop::supported(),
        "lolidesk-host 已启动"
    );
    let shared = net::Shared::new(cfg.token.clone());
    net::serve(endpoint, shared).await
}
