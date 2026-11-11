//! 问内网的管理面（web/agent/mesh_manager.py，签发 relay 上，nginx 发成 https://manager.<edge_zone>）：
//! lolinet invite（要配对码）、lolinet mihomo（要 mihomo 配置）、lolinet exit on / off（登记出口）。
//! 管理面认的是请求的来源，也就是这台的 overlay 地址，所以不要 root、不要账号。
//!
//! 它在哪：配对时配置末尾的 `# lolinet = {"manager": ..., "manager_ip": ...}`，lolinet up 存进 /etc/lolinet/lolinet.json；
//! 或者 --manager https://host[@ip] 给（root 跑会记住）。用系统 curl：--resolve 把域名钉到 overlay 地址
//! （有的网络 DNS 给假 IP），userspace 模式经本机的 SOCKS5 出去。

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

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
fn parse(s: &str) -> Result<(String, Option<String>)> {
    let (url, ip) = match s.rsplit_once('@') {
        Some((u, ip)) if ip.parse::<std::net::IpAddr>().is_ok() => (u, Some(ip.to_owned())),
        _ => (s, None),
    };
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("--manager 要是 https://<管理面域名>，可以带 @<overlay 地址>");
    }
    Ok((url.trim_end_matches('/').to_owned(), ip))
}

pub struct Manager {
    pub url: String,
    ip: Option<String>,
}

impl Manager {
    /// --manager 给了就用（root 跑顺手记下来），没给就用配对时记下的
    pub fn locate(flag: Option<&str>) -> Result<Manager> {
        let mut st = Settings::load();
        if let Some(m) = flag {
            let (url, ip) = parse(m)?;
            if crate::sys::is_root() {
                st.manager = Some(url.clone());
                st.manager_ip = ip.clone();
                st.save()?;
            }
            return Ok(Manager { url, ip });
        }
        match st.manager {
            Some(url) => Ok(Manager { url, ip: st.manager_ip }),
            None => bail!(
                "不知道管理面在哪：这台配对时还没有这个功能。加 --manager https://manager.<edge_zone>@<签发 relay 的 overlay 地址>"
            ),
        }
    }

    /// 发一个请求，返回 (HTTP 状态, 响应体)；连不上是 Err
    pub fn call(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String)> {
        let mut curl = Command::new("curl");
        curl.args(["-sS", "--connect-timeout", "10", "--max-time", "60", "-X", method])
            .args(["-H", "X-Mesh-Agent: 1", "-w", "\n%{http_code}"]);
        if body.is_some() {
            curl.args(["-H", "Content-Type: application/json", "--data-binary", "@-"]);
        }
        if let (Some(ip), Ok(u)) = (&self.ip, url::Url::parse(&self.url)) {
            if let Some(host) = u.host_str() {
                let port = u.port_or_known_default().unwrap_or(443);
                curl.arg("--resolve").arg(format!("{host}:{port}:{ip}"));
            }
        }
        if let Some(socks) = local_socks() {
            // --resolve 之后连的是 IP，SOCKS5 不用再解析
            curl.arg("--proxy").arg(format!("socks5://{socks}"));
        }
        curl.arg(format!("{}{path}", self.url));
        let mut child = curl.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("跑 curl")?;
        let mut stdin = child.stdin.take().context("curl stdin")?;
        if let Some(b) = body {
            stdin.write_all(b.as_bytes())?;
        }
        drop(stdin);
        let out = child.wait_with_output()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let (resp, code) = text.rsplit_once('\n').unwrap_or(("", &text));
        let code: u16 = code.trim().parse().unwrap_or(0);
        if !out.status.success() || code == 0 {
            bail!(
                "连不上管理面 {}{}：{}",
                self.url,
                self.ip.as_deref().map(|i| format!("（{i}）")).unwrap_or_default(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok((code, resp.to_owned()))
    }

    /// 要 200 的 JSON 调用；别的状态把管理面给的原因报出来
    pub fn json(&self, method: &str, path: &str, body: Option<&str>) -> Result<Value> {
        let (code, resp) = self.call(method, path, body)?;
        let v: Value = serde_json::from_str(&resp).unwrap_or(Value::Null);
        if code != 200 {
            let msg = v.get("message").and_then(Value::as_str).unwrap_or(resp.trim());
            bail!("管理面不给（HTTP {code}）：{msg}");
        }
        Ok(v)
    }
}

/// 本机内核的配置文本（问管理口，不用读 600 的 loli.toml）
fn node_config() -> Option<String> {
    status::node()?.get("config")?.as_str().map(str::to_owned)
}

/// userspace 模式下本机进网用的 SOCKS5；TUN 模式或者本机内核没在跑是 None
pub fn local_socks() -> Option<String> {
    let conf = node_config()?;
    if !userspace(&conf) {
        return None;
    }
    socks_of(&conf)
}

/// 本机内核开着的回环 SOCKS5，不管哪种模式（TUN 模式下 lolinet mihomo on 会开）
pub fn local_socks_any() -> Option<String> {
    socks_of(&node_config()?)
}

/// 配置文本里 socks5_proxy 的地址部分（127.0.0.1:18080）
fn socks_of(conf: &str) -> Option<String> {
    conf.lines().find_map(|l| {
        let v = l.trim().strip_prefix("socks5_proxy")?.trim_start().strip_prefix('=')?.trim().trim_matches('"');
        Some(v.trim_start_matches("socks5://").trim_end_matches('/').to_owned())
    })
}

fn userspace(conf: &str) -> bool {
    conf.lines().any(|l| l.split('#').next().unwrap_or("").replace(' ', "") == "no_tun=true")
}

/// 这台是 userspace 还是 TUN（问不到本机内核就当 TUN）
pub fn mode() -> &'static str {
    match node_config() {
        Some(c) if userspace(&c) => "userspace",
        _ => "tun",
    }
}
