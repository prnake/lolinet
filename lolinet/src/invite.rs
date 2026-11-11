//! `lolinet invite`：问内网的管理面要一个配对码，给另一台机器入网用（成员邀请，见 web/agent/mesh_manager.py、docs/design.md）。
//! 不要控制台账号，也不要 root：管理面认的是请求的来源，也就是这台的 overlay 地址；地址由中心分配。
//!
//! 管理面在签发 relay 的 overlay 地址上（nginx 发成 https://manager.<edge_zone>）。它在哪：配对时配置末尾的
//! `# lolinet = {"manager": ..., "manager_ip": ...}`，lolinet up 存进 /etc/lolinet/lolinet.json；或者 --manager 给。
//! 用系统 curl：--resolve 把域名钉到 overlay 地址（有的网络 DNS 给假 IP），userspace 模式经本机的 SOCKS5 出去。

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::{status, upgrade::Settings};

/// 配对包末尾给 lolinet 自己的那一行：管理面的地址
pub const PAIR_LINE: &str = "# lolinet = ";

/// `# lolinet = {...}` -> (manager, manager_ip)
pub fn from_pairing(text: &str) -> Option<(String, Option<String>)> {
    let v: Value = serde_json::from_str(text.lines().find_map(|l| l.strip_prefix(PAIR_LINE))?).ok()?;
    let url = v.get("manager")?.as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://"))?;
    let ip = v.get("manager_ip").and_then(Value::as_str).filter(|ip| ip.parse::<std::net::IpAddr>().is_ok());
    Some((url.trim_end_matches('/').to_owned(), ip.map(str::to_owned)))
}

/// --manager 的写法：https://host 或 https://host@10.188.0.14
fn parse_manager(s: &str) -> Result<(String, Option<String>)> {
    let (url, ip) = match s.rsplit_once('@') {
        Some((u, ip)) if ip.parse::<std::net::IpAddr>().is_ok() => (u, Some(ip.to_owned())),
        _ => (s, None),
    };
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("--manager 要是 https://<管理面域名>，可以带 @<overlay 地址>");
    }
    Ok((url.trim_end_matches('/').to_owned(), ip))
}

/// userspace 模式下本机进网用的 SOCKS5（问本机内核要配置，不用读 600 的 loli.toml）
fn local_socks() -> Option<String> {
    let node = status::node()?;
    let conf = node.get("config")?.as_str()?;
    let no_tun = conf.lines().any(|l| l.split('#').next().unwrap_or("").replace(' ', "") == "no_tun=true");
    if !no_tun {
        return None;
    }
    conf.lines().find_map(|l| {
        let v = l.trim().strip_prefix("socks5_proxy")?.trim_start().strip_prefix('=')?.trim().trim_matches('"');
        Some(v.trim_start_matches("socks5://").trim_end_matches('/').to_owned())
    })
}

pub fn run(kind: String, name: Option<String>, manager: Option<String>, as_json: bool) -> Result<()> {
    let mut st = Settings::load();
    let (url, ip) = match manager.as_deref() {
        Some(m) => {
            let (u, ip) = parse_manager(m)?;
            // 有 root 就记下来，下次不用再给
            if unsafe { nix::libc::geteuid() } == 0 {
                st.manager = Some(u.clone());
                st.manager_ip = ip.clone();
                st.save()?;
            }
            (u, ip)
        }
        None => match st.manager.clone() {
            Some(u) => (u, st.manager_ip.clone()),
            None => bail!(
                "不知道管理面在哪：这台配对时还没有这个功能。lolinet invite --manager https://manager.<edge_zone>@<签发 relay 的 overlay 地址>"
            ),
        },
    };
    let body = json!({"kind": kind, "name": name.unwrap_or_default()}).to_string();
    let mut curl = Command::new("curl");
    curl.args(["-sS", "--connect-timeout", "10", "--max-time", "60", "-X", "POST"])
        .args(["-H", "Content-Type: application/json", "-H", "X-Mesh-Agent: 1", "--data-binary", "@-"])
        .args(["-w", "\n%{http_code}"]);
    if let (Some(ip), Ok(u)) = (&ip, url::Url::parse(&url)) {
        if let Some(host) = u.host_str() {
            let port = u.port_or_known_default().unwrap_or(443);
            curl.arg("--resolve").arg(format!("{host}:{port}:{ip}"));
        }
    }
    if let Some(socks) = local_socks() {
        // --resolve 之后连的是 IP，SOCKS5 不用再解析
        curl.arg("--proxy").arg(format!("socks5://{socks}"));
    }
    curl.arg(format!("{url}/api/mesh/invite"));
    let mut child = curl.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("跑 curl")?;
    child.stdin.take().context("curl stdin")?.write_all(body.as_bytes())?;
    let out = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (resp, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    if !out.status.success() || code == "000" {
        bail!("连不上管理面 {url}{}：{}", ip.map(|i| format!("（{i}）")).unwrap_or_default(), String::from_utf8_lossy(&out.stderr).trim());
    }
    let v: Value = serde_json::from_str(resp).unwrap_or(Value::Null);
    if code != "200" {
        let msg = v.get("message").and_then(Value::as_str).unwrap_or(resp.trim());
        bail!("管理面不给（HTTP {code}）：{msg}");
    }
    if as_json {
        println!("{v}");
        return Ok(());
    }
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let mins = v
        .get("expires_at")
        .and_then(Value::as_i64)
        .map(|t| (t - chrono::Utc::now().timestamp()).max(0) / 60)
        .unwrap_or(15);
    println!("配对码  {}（{mins} 分钟内有效，只能用一次）", s("code"));
    println!("新机器  {} · {} · {}", s("name"), s("ip"), s("kind"));
    println!("在新机器上运行：\n  {}", s("command"));
    Ok(())
}
