//! 「新建」菜单：登录 shell 里能找到哪些 coding agent。结果缓存一分钟（每次问一遍要起一个交互 shell，慢）。

use std::{
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use lolidesk_proto::Launcher;

use crate::terminal::user_shell;

/// (命令, 显示名)。顺序就是菜单顺序。
const KNOWN: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini CLI"),
    ("opencode", "OpenCode"),
    ("cursor-agent", "Cursor Agent"),
    ("amp", "Amp"),
    ("qwen", "Qwen Code"),
    ("kimi", "Kimi CLI"),
    ("droid", "Droid"),
    ("crush", "Crush"),
    ("pi", "Pi"),
    ("herdr", "herdr"),
];

static CACHE: Mutex<Option<(Instant, Vec<Launcher>)>> = Mutex::new(None);

pub fn list() -> Vec<Launcher> {
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, list)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(60) {
            return list.clone();
        }
    }
    let list = detect();
    *cache = Some((Instant::now(), list.clone()));
    list
}

fn detect() -> Vec<Launcher> {
    let mut out = vec![Launcher {
        id: "shell".into(),
        name: "终端".into(),
        command: String::new(),
    }];
    let names: Vec<&str> = KNOWN.iter().map(|(c, _)| *c).collect();
    let script = format!(
        "for c in {}; do command -v \"$c\" >/dev/null 2>&1 && echo \"$c\"; done",
        names.join(" ")
    );
    let found = run_with_timeout(&user_shell(), &script, Duration::from_secs(8));
    for (cmd, name) in KNOWN {
        if found.iter().any(|f| f == cmd) {
            out.push(Launcher {
                id: (*cmd).into(),
                name: (*name).into(),
                command: (*cmd).into(),
            });
        }
    }
    out
}

/// 在登录 + 交互 shell 里跑一段脚本，取 stdout 的每一行。超时就当什么都没找到。
fn run_with_timeout(shell: &str, script: &str, timeout: Duration) -> Vec<String> {
    let child = Command::new(shell)
        .args(["-l", "-i", "-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return Vec::new();
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                tracing::warn!("找 agent 超时");
                return Vec::new();
            }
        }
    }
    let mut s = String::new();
    if let Some(mut o) = child.stdout.take() {
        use std::io::Read;
        let _ = o.read_to_string(&mut s);
    }
    s.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}
