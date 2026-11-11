//! 有设备请求配对、主人的 app 又没开着（最近没来问）时，在主机这台电脑上直接弹系统对话框问同不同意：
//! macOS 用 osascript 的 display dialog，Linux 用 zenity 或 kdialog（要有图形会话）。都没有就只能等主人
//! 开 app，或者在命令行 `lolidesk host approve <id>`。
//!
//! 设备名、系统是对方自报的：只作为参数传给对话框（osascript 经 `on run argv`），不拼进脚本；
//! zenity 的文字是 Pango 标记，转义 & < >。设 LOLIDESK_NO_DIALOG=1 就不弹（测试、无人值守的服务器）。

use std::process::{Child, Command, Stdio};

use lolidesk_proto::DeviceInfo;

/// 对话框自己多久放弃（比主机等的 120 秒短一点）
const GIVE_UP_SECS: u32 = 110;

#[derive(Clone, Copy)]
pub enum Kind {
    Osascript,
    Zenity,
    Kdialog,
}

pub fn message(device: &DeviceInfo, addr: &str) -> String {
    let os = if device.os.is_empty() {
        "?"
    } else {
        device.os.as_str()
    };
    format!(
        "「{}」（{addr}，{os}）想连这台电脑。\n\n同意后它能看这台的终端和远程桌面、操作键鼠、让 agent 用这台（按这台开放的设置），以后再连不用问。\n随时可以在 Lolidesk 或 lolidesk host access 里吊销。",
        device.name
    )
}

fn which(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

fn pango_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 弹框（不等）。弹不了就是 None
pub fn ask(device: &DeviceInfo, addr: &str) -> Option<(Child, Kind)> {
    if std::env::var_os("LOLIDESK_NO_DIALOG").is_some() {
        return None;
    }
    let msg = message(device, addr);
    let mut cmd;
    let kind;
    if cfg!(target_os = "macos") {
        cmd = Command::new("/usr/bin/osascript");
        cmd.args([
            "-e",
            "on run argv",
            "-e",
            &format!(
                "display dialog (item 1 of argv) with title \"lolidesk\" buttons {{\"拒绝\", \"同意\"}} default button \"拒绝\" cancel button \"拒绝\" giving up after {GIVE_UP_SECS} with icon caution"
            ),
            "-e",
            "end run",
            &msg,
        ]);
        kind = Kind::Osascript;
    } else {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return None;
        }
        if let Some(z) = which("zenity") {
            cmd = Command::new(z);
            cmd.args([
                "--question",
                "--title=lolidesk",
                "--ok-label=同意",
                "--cancel-label=拒绝",
            ])
            .arg(format!("--timeout={GIVE_UP_SECS}"))
            .arg(format!("--text={}", pango_escape(&msg)));
            kind = Kind::Zenity;
        } else {
            cmd = Command::new(which("kdialog")?);
            cmd.args([
                "--title",
                "lolidesk",
                "--yes-label",
                "同意",
                "--no-label",
                "拒绝",
                "--yesno",
            ])
            .arg(&msg);
            kind = Kind::Kdialog;
        }
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    Some((child, kind))
}

/// 等对话框关掉：同意 Some(true)，拒绝 Some(false)，超时、被关掉、弹不出来都是 None（不当拒绝）
pub fn wait(child: Child, kind: Kind) -> Option<bool> {
    let out = child.wait_with_output().ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    match kind {
        // 点「同意」：button returned:同意, gave up:false；点「拒绝」（取消按钮）：错误 -128
        Kind::Osascript if out.status.success() => (stdout.contains("button returned:同意")
            && !stdout.contains("gave up:true"))
        .then_some(true),
        Kind::Osascript => stderr.contains("-128").then_some(false),
        // zenity：0 同意，1 拒绝，5 超时；kdialog：0 是，1 否
        Kind::Zenity | Kind::Kdialog => match out.status.code() {
            Some(0) => Some(true),
            Some(1) => Some(false),
            _ => None,
        },
    }
}

/// 主人在 app 或命令行里答复了：把还开着的对话框关掉
pub fn close(pid: u32) {
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_names_the_device() {
        let d = DeviceInfo {
            name: "laptop".into(),
            os: "linux".into(),
            kind: "app".into(),
            version: "1".into(),
        };
        let m = message(&d, "10.188.16.11:5000");
        assert!(m.starts_with("「laptop」（10.188.16.11:5000，linux）想连这台电脑"));
        assert_eq!(pango_escape("<b>&"), "&lt;b&gt;&amp;");
    }
}
