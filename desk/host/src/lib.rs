//! `lolidesk host`：让这台电脑能被连。一个进程持所有终端会话（断线不丢），按需采屏编码。
//! 只监听 lolinet overlay 地址（10.188.0.0/16），自签证书 + 共享 token。
//! 和连别人的客户端是同一个程序 `lolidesk`（desk/client），这里是它的 `host` 子命令。

pub mod access;
mod config;
mod consent;
mod cua;
mod desktop;
mod input;
mod launchers;
mod net;
mod permissions;
mod procinfo;
mod terminal;

use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use clap::Subcommand;

#[derive(Subcommand, Debug, Clone)]
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
    /// 开放什么：terminal、desktop（看远程桌面）、control（远程桌面里操作）、cua（agent 截图和键鼠）、
    /// pair（没配对的设备能请求配对）、share（配对过的设备能发分享链接）。
    /// 不带参数是看；`allow cua off` 关一项。马上生效，不用重启
    Allow {
        /// terminal / desktop / control / cua / pair / share
        what: Option<String>,
        /// on / off
        state: Option<String>,
    },
    /// 谁能连这台电脑：配对过的设备（最近登录）、配对码（用它配对了哪些）、等着同意的请求
    Access,
    /// 同意一个配对请求（id 看 lolidesk host access）
    Approve { id: u64 },
    /// 拒绝一个配对请求
    Deny { id: u64 },
    /// 发一个配对码，打印码和分享链接。别的机器 `lolidesk pair <链接>`（或在 app 里导入）就配对上，不用再同意
    Code {
        /// 备注：给谁、干什么用
        #[arg(long, default_value = "")]
        label: String,
        /// 多久后不能再用来配对：30m、12h、7d；不给是不过期（已经配对的设备不受影响）
        #[arg(long)]
        expires: Option<String>,
        /// 最多几台设备用它配对
        #[arg(long)]
        uses: Option<u32>,
        /// 用它配对的设备只能看：不能操作键鼠、不能给 agent 用、不能再分享
        #[arg(long)]
        view_only: bool,
    },
    /// 吊销一台设备（g-…）或一个配对码（c-…，用它配对的设备一起失效）
    Revoke { id: String },
    /// 限制一台设备能做什么：`limit g-xxxx cua off`；`--reset` 去掉限制（和主机一样）
    Limit {
        grant: String,
        /// terminal / desktop / control / cua / share
        what: Option<String>,
        /// on / off
        state: Option<String>,
        #[arg(long)]
        reset: bool,
    },
}

impl HostCmd {
    /// 管配对的那几个：要连上在跑的主机（lolidesk 命令行处理，见 client 的 pair.rs）
    pub fn is_access(&self) -> bool {
        matches!(
            self,
            Self::Access
                | Self::Approve { .. }
                | Self::Deny { .. }
                | Self::Code { .. }
                | Self::Revoke { .. }
                | Self::Limit { .. }
        )
    }
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
        // 管配对的命令由 lolidesk 处理（要连上在跑的主机），走不到这里
        _ => anyhow::bail!("管配对的命令要用 lolidesk 跑"),
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
        a.set(&w, on).map_err(anyhow::Error::msg)?;
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
    println!(
        "配对请求      {}（网里的设备点「连接」，这台弹框问你同不同意）",
        s(a.pair)
    );
    println!(
        "分享链接      {}（配对过的设备能给别的机器发配对码）",
        s(a.share)
    );
    Ok(())
}

async fn run(listen: Option<SocketAddr>) -> Result<()> {
    let cfg = config::HostConfig::load_or_create(false)?;
    let addr = listen.unwrap_or_else(|| {
        let ip = config::overlay_ip().unwrap_or_else(|| {
            tracing::warn!("没找到 10.188.0.0/16 的地址，监听 0.0.0.0（没配对的设备连不进来，配对请求要主人同意）");
            "0.0.0.0".parse().unwrap()
        });
        SocketAddr::new(ip, lolidesk_proto::DEFAULT_PORT)
    });
    let server = cfg.quic_server_config()?;
    let endpoint =
        quinn::Endpoint::server(server.clone(), addr).with_context(|| format!("监听 {addr}"))?;
    // 本机的 app 和命令行从回环连（不管有没有入网）。主地址已经覆盖回环（0.0.0.0、127.0.0.1）就不用再听一个
    let local = SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), addr.port());
    let mut endpoints = vec![endpoint];
    if !(addr.ip().is_unspecified() || addr.ip().is_loopback()) {
        match quinn::Endpoint::server(server, local) {
            Ok(e) => endpoints.push(e),
            Err(e) => tracing::warn!("听不了 {local}（本机的 app 连不上这台主机）：{e}"),
        }
    }
    let local = if addr.ip().is_loopback() { addr } else { local };
    if let Err(e) = config::write_local(local) {
        tracing::warn!("记不下本机地址：{e:#}");
    }
    // 分享链接里的地址：别的机器连这台用的（网里的地址）
    let link = if addr.ip().is_unspecified() {
        config::overlay_ip().map_or(addr, |ip| SocketAddr::new(ip, addr.port()))
    } else {
        addr
    };
    tracing::info!(
        %addr,
        %local,
        fingerprint = %cfg.fingerprint_hex(),
        desktop = desktop::supported(),
        "lolidesk 主机已启动"
    );
    let shared = net::Shared::new(cfg.token.clone(), link.to_string());
    let sessions = shared.sessions.clone();
    let mut serving = tokio::task::JoinSet::new();
    for e in &endpoints {
        serving.spawn(net::serve(e.clone(), shared.clone()));
    }
    tokio::select! {
        r = serving.join_next() => r.map_or(Ok(()), |r| r.map_err(anyhow::Error::from).and_then(|r| r)),
        _ = shutdown() => {
            // launchd 停服务、升级时发 SIGTERM：先把会话存全，重启后回放
            tracing::info!("收到退出信号，存下会话");
            tokio::task::spawn_blocking(move || sessions.save(true)).await?;
            // 告诉客户端连接关了（不发的话，它们要等空闲超时才发现），它们会马上开始重连
            for e in &endpoints {
                e.close(0u32.into(), b"restart");
            }
            for e in &endpoints {
                let _ = tokio::time::timeout(Duration::from_secs(1), e.wait_idle()).await;
            }
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
