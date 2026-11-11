//! `lolidesk host`：让这台电脑能被连。一个进程持所有终端会话（断线不丢），按需采屏编码。
//! 只监听 lolinet overlay 地址（10.188.0.0/16），自签证书 + 共享 token。
//! 和连别人的客户端是同一个程序 `lolidesk`（desk/client），这里是它的 `host` 子命令。

mod config;
mod cua;
mod desktop;
mod input;
mod launchers;
mod net;
mod procinfo;
mod terminal;

use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Subcommand, Debug)]
pub enum HostCmd {
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
    /// macOS：装成当前用户的 LaunchAgent 并启动（屏幕录制 / 辅助功能权限跟着这个二进制走）；再装一次就是重启
    Install,
    /// macOS：停掉并撤掉 LaunchAgent
    Uninstall,
    /// 开放什么：terminal、desktop（看远程桌面）、control（远程桌面里操作）、cua（agent 截图和键鼠）。
    /// 不带参数是看；`allow cua off` 关一项。马上生效，不用重启
    Allow {
        /// terminal / desktop / control / cua
        what: Option<String>,
        /// on / off
        state: Option<String>,
    },
}

/// 跑一个 `host` 子命令；不给子命令就是前台运行。日志由调用方初始化。
pub fn main(cmd: Option<HostCmd>) -> Result<()> {
    match cmd.unwrap_or(HostCmd::Run { listen: None }) {
        HostCmd::Run { listen } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(run(listen))
        }
        HostCmd::Token { reset } => {
            let cfg = config::HostConfig::load_or_create(reset)?;
            println!("token      {}", cfg.token);
            println!("指纹       {}", cfg.fingerprint_hex());
            println!("目录       {}", cfg.dir.display());
            Ok(())
        }
        HostCmd::Install => config::install_launch_agent(),
        HostCmd::Allow { what, state } => allow(what, state),
        HostCmd::Uninstall => config::uninstall_launch_agent(),
    }
}

fn allow(what: Option<String>, state: Option<String>) -> Result<()> {
    let mut a = config::allow();
    if let Some(w) = what {
        let on = match state.as_deref() {
            Some("on" | "true" | "yes") => true,
            Some("off" | "false" | "no") => false,
            _ => anyhow::bail!("要写 on 或 off：lolidesk host allow {w} on|off"),
        };
        match w.as_str() {
            "terminal" => a.terminal = on,
            "desktop" => a.desktop = on,
            "control" => a.control = on,
            "cua" => a.cua = on,
            _ => anyhow::bail!("没有 {w}：terminal、desktop、control、cua"),
        }
        config::set_allow(a)?;
    }
    let s = |b: bool| if b { "开" } else { "关" };
    println!("终端          {}", s(a.terminal));
    println!("看远程桌面    {}", s(a.desktop));
    println!("远程桌面操作  {}", s(a.control));
    println!(
        "computer use  {}（agent 截图、键鼠；和远程桌面用同一份屏幕录制、辅助功能授权）",
        s(a.cua)
    );
    Ok(())
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
        "lolidesk 主机已启动"
    );
    let shared = net::Shared::new(cfg.token.clone());
    let sessions = shared.sessions.clone();
    let listener = endpoint.clone();
    tokio::select! {
        r = net::serve(listener, shared) => r,
        _ = shutdown() => {
            // launchd 停服务、升级时发 SIGTERM：先把会话存全，重启后回放
            tracing::info!("收到退出信号，存下会话");
            tokio::task::spawn_blocking(move || sessions.save(true)).await?;
            // 告诉客户端连接关了（不发的话，它们要等空闲超时才发现），它们会马上开始重连
            endpoint.close(0u32.into(), b"restart");
            let _ = tokio::time::timeout(Duration::from_secs(1), endpoint.wait_idle()).await;
            Ok(())
        }
    }
}

/// 等 SIGTERM 或 Ctrl-C。
async fn shutdown() {
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = term => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
