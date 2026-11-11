//! `lolinet invite`：问内网的管理面要一个配对码，给另一台机器（或手机）入网用（成员邀请，见 docs/design.md）。
//! 不要控制台账号，也不要 root：管理面认的是请求的来源，也就是这台的 overlay 地址；地址由中心分配。

use anyhow::Result;
use serde_json::{Value, json};

use crate::manager::Manager;

pub fn run(kind: String, name: Option<String>, manager: Option<String>, as_json: bool) -> Result<()> {
    let m = Manager::locate(manager.as_deref())?;
    let body = json!({"kind": kind, "name": name.unwrap_or_default()}).to_string();
    let v = m.json("POST", "/api/mesh/invite", Some(&body))?;
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
    println!("新设备  {} · {} · {}", s("name"), s("ip"), s("kind"));
    if s("kind") == "phone" {
        println!("在手机浏览器里打开（配对码在 # 后面，不会发给服务器），取到 mihomo 配置后导入 mihomo 客户端：\n  {}", s("command"));
    } else {
        println!("在新机器上运行：\n  {}", s("command"));
    }
    Ok(())
}
