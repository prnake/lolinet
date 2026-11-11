//! 终端会话：PTY（portable-pty）+ 主机上一个无头的 libghostty-vt 终端，它是会话的唯一真相。
//!
//! - 程序的输出喂给主机的终端；每个接着的客户端有一个 Mirror，有输出后约 4 ms 合成一帧发过去。
//!   帧是转义序列，客户端的终端照着回放就和主机这份一样：滚动历史、光标、模式、标题都在里面。
//!   客户端的终端从不自己排版程序的输出，所以不会和主机的对不上。
//! - 会话只有一个尺寸：听用户最后在用的那个客户端（键入过、Claim 过的）；没人 Claim 时听最后要尺寸的。
//! - 程序退出：退出码 0 关掉会话（像终端里关一个标签）；不是 0 就留着输出，等客户端结束它。
//! - 每 10 秒把会话的命令、目录、尺寸和历史（光标以上的内容）存到配置目录的 sessions/ 下。
//!   主机重启（升级、重启电脑）后回放历史、在原来的目录起一个新 shell；程序本身不会自动重跑。
//!
//! 做法照 egoist/kero 的 kerod（MIT）。

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use lolidesk_ghostty::{self as ghostty, Mirror, Rgb, Terminal};
use lolidesk_proto::{palette, ClearKind, SessionInfo, TermEvent, TermSize};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::procinfo;

/// 输出后等多久再合成一帧：程序重画往往分几次写。
const FRAME_DELAY: Duration = Duration::from_millis(4);
/// 程序开着同步更新（模式 2026）时，隔多久再试。
const SYNC_RETRY: Duration = Duration::from_millis(50);
const SAVE_EVERY: Duration = Duration::from_secs(10);
/// 主机终端留多少滚动历史（libghostty-vt 按字节算）。
const SCROLLBACK: usize = 10 << 20;
/// 一个客户端最多积压多少条：再多说明它跟不上，断开让它重接（重接从第一帧开始）。
pub const VIEWER_QUEUE: usize = 4096;

pub type Events = mpsc::Sender<TermEvent>;

/// 用户的 shell：$SHELL，没有就按系统给个默认。
pub fn user_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        })
}

/// 在登录 + 交互 shell 里跑命令（PATH 和 .zshrc 里的东西都在），命令退出后留在 shell 里，会话不会一闪而过。
pub fn shell_argv(command: &str) -> Vec<String> {
    let shell = user_shell();
    vec![
        shell.clone(),
        "-l".into(),
        "-i".into(),
        "-c".into(),
        format!("{command}; exec {} -l", sh_quote(&shell)),
    ]
}

fn sh_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.,:=+@%".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 客户端给的目录：`~`、`~/x` 和相对路径都从家目录算起（服务进程的当前目录是 /）；不存在就是 None。
fn resolve_dir(dir: Option<&str>) -> Option<String> {
    let home = dirs::home_dir()?;
    let d = dir.map(str::trim).filter(|d| !d.is_empty())?;
    let p = if d == "~" {
        home
    } else if let Some(rest) = d.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(d).is_absolute() {
        PathBuf::from(d)
    } else {
        home.join(d)
    };
    p.is_dir().then(|| p.to_string_lossy().into_owned())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 主机终端的默认颜色（程序用 OSC 10/11/4 问到的）：猫眼调色板，和客户端一样。
fn theme() -> ghostty::Theme {
    let rgb = |c: [u8; 3]| Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    };
    ghostty::Theme {
        foreground: rgb(palette::FG),
        background: rgb(palette::BG),
        cursor: Some(rgb(palette::CURSOR)),
        palette: palette::BASE16.map(rgb),
    }
}

// ---------------------------------------------------------------- 主机终端的回调

/// 往 PTY 写的字节（键入的、查询的应答）都经这个队列交给写线程，按到达顺序写。
/// 不在持着会话锁的时候直接写：程序不读输入时写会卡住，读线程就跟着卡住，程序又等它的输出被读走。
type Writer = std::sync::mpsc::Sender<Vec<u8>>;

/// 终端的回调：查询的应答排进写队列；响铃、通知先攒着，写完一段输出再发给客户端（回调里不能再碰终端）。
struct Effects {
    writer: Writer,
    pending: Arc<Mutex<Vec<TermEvent>>>,
    pwd: Arc<Mutex<String>>,
}

impl ghostty::Handler for Effects {
    fn write_pty(&mut self, bytes: &[u8]) {
        let _ = self.writer.send(bytes.to_vec());
    }
    fn pwd(&mut self, pwd: &str) {
        *self.pwd.lock().unwrap() = pwd.to_string();
    }
    fn bell(&mut self) {
        self.pending.lock().unwrap().push(TermEvent::Bell);
    }
    fn notify(&mut self, title: &str, body: &str) {
        self.pending.lock().unwrap().push(TermEvent::Notify {
            title: title.into(),
            body: body.into(),
        });
    }
}

// ---------------------------------------------------------------- 会话

/// 一个接着的客户端。
struct Viewer {
    mirror: Mirror,
    tx: Events,
    /// 它的终端放得下的尺寸
    asked: Option<TermSize>,
    asked_at: u64,
    /// 用户最后一次在它里面操作的时间（按 stamp 计）
    claimed: u64,
}

struct Inner {
    term: Terminal,
    size: TermSize,
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    shell_pid: Option<u32>,
    viewers: HashMap<u64, Viewer>,
    claimer: Option<u64>,
    stamp: u64,
    /// 有一帧排着了
    framing: bool,
    /// 有输出还没存进历史文件
    dirty: bool,
    exited: Option<i32>,
    /// 用户要结束它（之后程序退出就关掉，不看退出码）
    killing: bool,
    closed: bool,
}

type OnClose = Box<dyn Fn(u32) + Send + Sync>;

pub struct Session {
    pub id: u32,
    command: String,
    started: u64,
    restored: bool,
    me: Weak<Session>,
    inner: Mutex<Inner>,
    writer: Writer,
    pending: Arc<Mutex<Vec<TermEvent>>>,
    pwd: Arc<Mutex<String>>,
    next_viewer: AtomicU64,
    rt: tokio::runtime::Handle,
    on_close: OnClose,
}

/// 开一个会话要的东西。
struct Spec {
    id: u32,
    size: TermSize,
    /// PTY 里跑的命令（None 是登录 shell）和目录
    run: Option<String>,
    cwd: Option<String>,
    /// 恢复的会话：上一次的历史和它结束的时间
    history: Option<(Vec<u8>, String)>,
    /// 会话列表里显示的命令（恢复的会话记着原来的命令，但 PTY 里只起 shell）
    command: String,
    started: u64,
}

impl Session {
    fn spawn(spec: Spec, rt: tokio::runtime::Handle, on_close: OnClose) -> Result<Arc<Self>> {
        let size = TermSize {
            cols: spec.size.cols.clamp(2, 1000),
            rows: spec.size.rows.clamp(1, 500),
            ..spec.size
        };
        let (writer, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let pending = Arc::new(Mutex::new(Vec::new()));
        let pwd = Arc::new(Mutex::new(String::new()));
        let mut term = Terminal::new(
            size.cols,
            size.rows,
            Box::new(Effects {
                writer: writer.clone(),
                pending: pending.clone(),
                pwd: pwd.clone(),
            }),
        )?;
        term.set_scrollback_limit(SCROLLBACK);
        term.set_theme(&theme());
        if size.cell_w > 0 && size.cell_h > 0 {
            term.resize(size.cols, size.rows, size.cell_w as u32, size.cell_h as u32);
        }
        let restored = spec.history.is_some();
        if let Some((h, when)) = &spec.history {
            // 上一次的历史在上面，新 shell 从下面起；历史不带模式和光标，不会把死掉的程序的状态带回来
            term.write(h);
            term.write(format!("\x1b[0;2m── 已恢复 · 会话在 {when} 结束 ──\x1b[0m\r\n").as_bytes());
        }

        let pty = native_pty_system()
            .openpty(pty_size(size))
            .context("openpty")?;
        let mut cmd = match &spec.run {
            Some(c) => {
                CommandBuilder::from_argv(shell_argv(c).into_iter().map(Into::into).collect())
            }
            None => CommandBuilder::new_default_prog(),
        };
        if let Some(d) = resolve_dir(spec.cwd.as_deref())
            .or_else(|| dirs::home_dir().map(|h| h.to_string_lossy().into_owned()))
        {
            cmd.cwd(d);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "lolidesk");
        cmd.env("LOLIDESK", "1");
        cmd.env("LOLIDESK_SESSION", spec.id.to_string());
        // launchd 起的服务没有 LANG：不设的话 shell 和程序会按 ASCII 处理中文
        if std::env::var_os("LANG").is_none() && std::env::var_os("LC_ALL").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }
        let mut child = pty.slave.spawn_command(cmd).context("起 shell")?;
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader()?;
        let mut pty_writer = pty.master.take_writer()?;
        std::thread::Builder::new()
            .name(format!("pty-w-{}", spec.id))
            .spawn(move || {
                // 会话没了（所有发送端都丢了）就结束；程序退出后写会出错，照样把队列读空
                while let Ok(bytes) = write_rx.recv() {
                    if pty_writer.write_all(&bytes).is_ok() {
                        let _ = pty_writer.flush();
                    }
                }
            })?;
        let killer = child.clone_killer();
        let shell_pid = child.process_id();

        let id = spec.id;
        let session = Arc::new_cyclic(|me| Session {
            id,
            command: spec.command,
            started: spec.started,
            restored,
            me: me.clone(),
            inner: Mutex::new(Inner {
                term,
                size,
                master: Some(pty.master),
                killer: Some(killer),
                shell_pid,
                viewers: HashMap::new(),
                claimer: None,
                stamp: 0,
                framing: false,
                dirty: restored,
                exited: None,
                killing: false,
                closed: false,
            }),
            writer,
            pending,
            pwd,
            next_viewer: AtomicU64::new(1),
            rt,
            on_close,
        });

        // 读线程：输出喂给主机终端，排一帧；程序退出后收尾
        let s = session.clone();
        std::thread::Builder::new()
            .name(format!("pty-{id}"))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => s.output(&buf[..n]),
                    }
                }
                let code = child.wait().map(|st| st.exit_code() as i32).unwrap_or(-1);
                s.exited(code);
            })?;
        Ok(session)
    }

    fn output(&self, bytes: &[u8]) {
        let mut st = self.inner.lock().unwrap();
        if st.closed {
            return;
        }
        st.term.write(bytes);
        st.dirty = true;
        let events = std::mem::take(&mut *self.pending.lock().unwrap());
        for ev in events {
            broadcast(&mut st, ev);
        }
        self.frame_soon(&mut st, FRAME_DELAY);
    }

    /// 过一会儿给所有客户端发一帧（期间的输出合成一帧）。
    fn frame_soon(&self, st: &mut Inner, delay: Duration) {
        if st.framing || st.viewers.is_empty() {
            return;
        }
        st.framing = true;
        let me = self.me.clone();
        self.rt.spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(s) = me.upgrade() {
                let mut st = s.inner.lock().unwrap();
                st.framing = false;
                s.frame(&mut st);
            }
        });
    }

    /// 给每个客户端发一帧：把它的终端从上一帧带到现在的样子。跟不上的客户端断开。
    fn frame(&self, st: &mut Inner) {
        let mut due = false;
        let mut gone = Vec::new();
        let Inner { term, viewers, .. } = &mut *st;
        for (id, v) in viewers.iter_mut() {
            let (frame, later) = v.mirror.frame(term);
            due |= later;
            if !frame.is_empty() && v.tx.try_send(TermEvent::Frame(frame)).is_err() {
                gone.push(*id);
            }
        }
        for id in gone {
            tracing::info!(session = self.id, viewer = id, "客户端跟不上，断开");
            drop_viewer(st, id);
        }
        if due {
            self.frame_soon(st, SYNC_RETRY);
        }
    }

    /// 接上一个客户端：先告诉它会话的尺寸，再发第一帧（从头画）。打开它就是在用它：会话改用它的尺寸。
    pub fn attach(&self, tx: Events, size: TermSize) -> u64 {
        let id = self.next_viewer.fetch_add(1, Ordering::Relaxed);
        let mut st = self.inner.lock().unwrap();
        let mut v = Viewer {
            mirror: Mirror::new(),
            tx,
            asked: (size.cols > 0 && size.rows > 0).then_some(size),
            asked_at: 0,
            claimed: 0,
        };
        let _ = v.tx.try_send(TermEvent::Resized {
            cols: st.size.cols,
            rows: st.size.rows,
        });
        let (first, _) = v.mirror.frame(&mut st.term);
        let _ = v.tx.try_send(TermEvent::Frame(first));
        if let Some(code) = st.exited {
            let _ = v.tx.try_send(TermEvent::Exited(code));
        }
        st.viewers.insert(id, v);
        self.claim_locked(&mut st, id);
        id
    }

    pub fn detach(&self, viewer: u64) {
        let mut st = self.inner.lock().unwrap();
        drop_viewer(&mut st, viewer);
        self.fit(&mut st);
    }

    /// 客户端终端放得下的尺寸变了。
    pub fn ask(&self, viewer: u64, size: TermSize) {
        let mut st = self.inner.lock().unwrap();
        st.stamp += 1;
        let stamp = st.stamp;
        if let Some(v) = st.viewers.get_mut(&viewer) {
            v.asked = (size.cols > 0 && size.rows > 0).then_some(size);
            v.asked_at = stamp;
        }
        self.fit(&mut st);
    }

    /// 用户在这个客户端里操作了：会话改用它的尺寸。
    pub fn claim(&self, viewer: u64) {
        let mut st = self.inner.lock().unwrap();
        self.claim_locked(&mut st, viewer);
    }

    fn claim_locked(&self, st: &mut Inner, viewer: u64) {
        if st.claimer == Some(viewer) || !st.viewers.contains_key(&viewer) {
            return;
        }
        st.claimer = Some(viewer);
        st.stamp += 1;
        let stamp = st.stamp;
        if let Some(v) = st.viewers.get_mut(&viewer) {
            v.claimed = stamp;
        }
        self.fit(st);
    }

    /// 会话用哪个客户端的尺寸：最后 Claim 的；都没 Claim 就用最后要尺寸的。
    fn fit(&self, st: &mut Inner) {
        let best = st
            .viewers
            .values()
            .filter_map(|v| v.asked.map(|a| (v.claimed, v.asked_at, a)))
            .max_by_key(|(claimed, asked_at, _)| (*claimed, *asked_at));
        if let Some((_, _, size)) = best {
            self.resize(st, size);
        }
    }

    fn resize(&self, st: &mut Inner, size: TermSize) {
        if st.closed || size.cols == 0 || size.rows == 0 {
            return;
        }
        let size = TermSize {
            cols: size.cols.clamp(2, 1000),
            rows: size.rows.clamp(1, 500),
            ..size
        };
        let (cw, ch) = if size.cell_w > 0 && size.cell_h > 0 {
            (size.cell_w, size.cell_h)
        } else {
            (st.size.cell_w, st.size.cell_h)
        };
        if (size.cols, size.rows) == (st.size.cols, st.size.rows) {
            if (cw, ch) != (st.size.cell_w, st.size.cell_h) {
                // 只是格子的像素变了：程序问像素尺寸、按像素报鼠标时要用
                st.size.cell_w = cw;
                st.size.cell_h = ch;
                st.term.resize(size.cols, size.rows, cw as u32, ch as u32);
                if let Some(m) = &st.master {
                    let _ = m.resize(pty_size(st.size));
                }
            }
            return;
        }
        // 先把旧尺寸下的输出发完，客户端在 Resized 处换尺寸，再收新尺寸的帧
        self.frame(st);
        st.size = TermSize {
            cols: size.cols,
            rows: size.rows,
            cell_w: cw,
            cell_h: ch,
        };
        st.term.resize(size.cols, size.rows, cw as u32, ch as u32);
        if let Some(m) = &st.master {
            let _ = m.resize(pty_size(st.size));
        }
        broadcast(
            st,
            TermEvent::Resized {
                cols: size.cols,
                rows: size.rows,
            },
        );
        st.dirty = true;
        self.frame(st);
    }

    /// 客户端键入的字节（已按终端模式编好码）。键入就是在用它：会话改用它的尺寸。
    pub fn input(&self, viewer: u64, bytes: &[u8]) {
        {
            let mut st = self.inner.lock().unwrap();
            if st.closed {
                return;
            }
            self.claim_locked(&mut st, viewer);
        }
        if !bytes.is_empty() {
            let _ = self.writer.send(bytes.to_vec());
        }
    }

    pub fn clear(&self, what: ClearKind) {
        let mut st = self.inner.lock().unwrap();
        if st.closed {
            return;
        }
        match what {
            ClearKind::Screen => {
                if !st.term.clear_screen() {
                    return;
                }
            }
            ClearKind::Scrollback => st.term.clear_scrollback(),
        }
        st.dirty = true;
        self.frame_soon(&mut st, FRAME_DELAY);
    }

    /// 结束会话：挂断程序，它退出后会话就没了。
    pub fn kill(&self) {
        let mut st = self.inner.lock().unwrap();
        st.killing = true;
        if st.exited.is_some() {
            drop(st);
            self.close();
            return;
        }
        if let Some(k) = st.killer.as_mut() {
            let _ = k.kill();
        }
    }

    /// 程序退出：0 就关掉会话；不是 0 留着输出，告诉客户端。
    fn exited(&self, code: i32) {
        let close = {
            let mut st = self.inner.lock().unwrap();
            if st.closed {
                return;
            }
            st.exited = Some(code);
            st.master = None;
            st.killer = None;
            // 退出前最后的输出先发出去
            self.frame(&mut st);
            broadcast(&mut st, TermEvent::Exited(code));
            code == 0 || st.killing
        };
        tracing::info!(session = self.id, code, "程序退出");
        if close {
            self.close();
        }
    }

    fn close(&self) {
        {
            let mut st = self.inner.lock().unwrap();
            if st.closed {
                return;
            }
            st.closed = true;
            // 丢掉发送端：客户端的流随之结束
            st.viewers.clear();
            if let Some(k) = st.killer.as_mut() {
                let _ = k.kill();
            }
        }
        (self.on_close)(self.id);
    }

    /// 前台程序的进程号（终端前台进程组的组长），没有就是 shell 的。
    fn foreground_pid(st: &Inner) -> Option<i32> {
        st.master
            .as_ref()
            .and_then(|m| m.process_group_leader())
            .or(st.shell_pid.map(|p| p as i32))
    }

    fn cwd(&self, st: &Inner) -> String {
        let from_proc = Self::foreground_pid(st)
            .and_then(procinfo::cwd)
            .or_else(|| st.shell_pid.and_then(|p| procinfo::cwd(p as i32)));
        match from_proc {
            Some(p) => p.to_string_lossy().into_owned(),
            None => {
                // 读不到进程目录时用程序自己报的（OSC 7：file://主机/路径）
                let pwd = self.pwd.lock().unwrap().clone();
                pwd.strip_prefix("file://")
                    .and_then(|rest| rest.find('/').map(|i| rest[i..].to_string()))
                    .unwrap_or(pwd)
            }
        }
    }

    pub fn info(&self) -> SessionInfo {
        let st = self.inner.lock().unwrap();
        let running = st.exited.is_none();
        SessionInfo {
            id: self.id,
            title: st.term.title(),
            command: self.command.clone(),
            foreground: if running {
                Self::foreground_pid(&st)
                    .and_then(procinfo::name)
                    .unwrap_or_default()
            } else {
                String::new()
            },
            cwd: if running {
                self.cwd(&st)
            } else {
                String::new()
            },
            cols: st.size.cols,
            rows: st.size.rows,
            viewers: st.viewers.len() as u32,
            started: self.started,
            restored: self.restored,
            exited: st.exited,
        }
    }

    /// 要存的东西：会话的描述，和有变化时的历史。
    fn snapshot(&self, force: bool) -> Option<(Saved, Option<Vec<u8>>)> {
        let mut st = self.inner.lock().unwrap();
        if st.closed || st.exited.is_some() {
            return None;
        }
        let history = (st.dirty || force).then(|| st.term.history());
        st.dirty = false;
        let cwd = self.cwd(&st);
        Some((
            Saved {
                id: self.id,
                command: self.command.clone(),
                cwd,
                started: self.started,
                cols: st.size.cols,
                rows: st.size.rows,
            },
            history,
        ))
    }
}

fn broadcast(st: &mut Inner, ev: TermEvent) {
    let mut gone = Vec::new();
    for (id, v) in st.viewers.iter() {
        if v.tx.try_send(ev.clone()).is_err() {
            gone.push(*id);
        }
    }
    for id in gone {
        drop_viewer(st, id);
    }
}

fn drop_viewer(st: &mut Inner, id: u64) {
    st.viewers.remove(&id);
    if st.claimer == Some(id) {
        st.claimer = None;
    }
}

fn pty_size(s: TermSize) -> PtySize {
    PtySize {
        rows: s.rows,
        cols: s.cols,
        pixel_width: s.cols.saturating_mul(s.cell_w),
        pixel_height: s.rows.saturating_mul(s.cell_h),
    }
}

// ---------------------------------------------------------------- 存盘

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    id: u32,
    command: String,
    cwd: String,
    started: u64,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SavedState {
    version: u32,
    /// 存盘的时间（Unix 秒）
    saved: u64,
    sessions: Vec<Saved>,
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 存盘时间的本地写法；为一个时间格式不引日期库，交给 date（macOS 的 -r 秒数，GNU 的 -d @秒数）。
fn local_time(secs: u64) -> String {
    let try_date = |args: &[&str]| {
        std::process::Command::new("date")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let s = secs.to_string();
    try_date(&["-r", &s, "+%m-%d %H:%M"])
        .or_else(|| try_date(&["-d", &format!("@{s}"), "+%m-%d %H:%M"]))
        .unwrap_or_else(|| "上次".into())
}

// ---------------------------------------------------------------- 会话表

#[derive(Clone)]
pub struct Sessions {
    inner: Arc<SessionsInner>,
}

struct SessionsInner {
    next: AtomicU32,
    map: Mutex<HashMap<u32, Arc<Session>>>,
    dir: PathBuf,
    rt: tokio::runtime::Handle,
    /// libghostty-vt 载入失败的原因：没有它就开不了终端
    unavailable: Option<String>,
}

impl Sessions {
    /// 载入 libghostty-vt，把上一次存下的会话恢复回来，开始定时存盘。要在 tokio 里调。
    pub fn new(dir: PathBuf) -> Self {
        let unavailable = match ghostty::load() {
            Ok(()) => None,
            Err(e) => {
                tracing::error!("libghostty-vt 载入失败，终端不可用：{e:#}");
                Some(format!("{e:#}"))
            }
        };
        let s = Self {
            inner: Arc::new(SessionsInner {
                next: AtomicU32::new(0),
                map: Mutex::new(HashMap::new()),
                dir,
                rt: tokio::runtime::Handle::current(),
                unavailable,
            }),
        };
        if s.inner.unavailable.is_none() {
            s.restore();
            let me = s.clone();
            s.inner.rt.spawn(async move {
                let mut tick = tokio::time::interval(SAVE_EVERY);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    let me = me.clone();
                    let _ = tokio::task::spawn_blocking(move || me.save(false)).await;
                }
            });
        }
        s
    }

    pub fn persistent(&self) -> bool {
        self.inner.unavailable.is_none()
    }

    fn check(&self) -> Result<()> {
        match &self.inner.unavailable {
            Some(e) => anyhow::bail!("主机上的终端不可用：{e}"),
            None => Ok(()),
        }
    }

    fn history_path(&self, id: u32) -> PathBuf {
        self.inner.dir.join(format!("{id}.vt"))
    }

    fn on_close(&self) -> OnClose {
        let weak = Arc::downgrade(&self.inner);
        Box::new(move |id| {
            if let Some(inner) = weak.upgrade() {
                inner.map.lock().unwrap().remove(&id);
                let _ = std::fs::remove_file(inner.dir.join(format!("{id}.vt")));
            }
        })
    }

    pub fn open(
        &self,
        size: TermSize,
        command: Option<String>,
        cwd: Option<String>,
    ) -> Result<Arc<Session>> {
        self.check()?;
        let command = command.filter(|c| !c.trim().is_empty());
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed) + 1;
        // 持着表锁开会话：程序秒退时，关会话的摘除一定排在下面的 insert 之后
        let mut map = self.inner.map.lock().unwrap();
        let s = Session::spawn(
            Spec {
                id,
                size,
                run: command.clone(),
                cwd,
                history: None,
                command: command.unwrap_or_default(),
                started: now_secs(),
            },
            self.inner.rt.clone(),
            self.on_close(),
        )?;
        map.insert(id, s.clone());
        tracing::info!(id, "开了会话");
        Ok(s)
    }

    pub fn get(&self, id: u32) -> Option<Arc<Session>> {
        self.inner.map.lock().unwrap().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        let sessions: Vec<Arc<Session>> =
            self.inner.map.lock().unwrap().values().cloned().collect();
        let mut v: Vec<_> = sessions.iter().map(|s| s.info()).collect();
        v.sort_by_key(|s| s.id);
        v
    }

    /// 存盘：会话的描述写 state.json，有变化的历史写 <id>.vt。force 时历史全写。
    pub fn save(&self, force: bool) {
        if self.inner.unavailable.is_some() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&self.inner.dir) {
            tracing::warn!("建 {} 失败：{e}", self.inner.dir.display());
            return;
        }
        let sessions: Vec<Arc<Session>> =
            self.inner.map.lock().unwrap().values().cloned().collect();
        let mut state = SavedState {
            version: 1,
            saved: now_secs(),
            sessions: Vec::new(),
        };
        for s in sessions {
            let Some((saved, history)) = s.snapshot(force) else {
                continue;
            };
            if let Some(h) = history {
                if let Err(e) = write_private(&self.history_path(saved.id), &h) {
                    tracing::warn!(id = saved.id, "存历史失败：{e:#}");
                }
            }
            state.sessions.push(saved);
        }
        state.sessions.sort_by_key(|s| s.id);
        match serde_json::to_vec_pretty(&state) {
            Ok(b) => {
                if let Err(e) = write_private(&self.inner.dir.join("state.json"), &b) {
                    tracing::warn!("存会话表失败：{e:#}");
                }
            }
            Err(e) => tracing::warn!("会话表序列化失败：{e}"),
        }
    }

    /// 恢复上一次的会话：回放历史，在原来的目录起新 shell。
    fn restore(&self) {
        let Ok(text) = std::fs::read(self.inner.dir.join("state.json")) else {
            return;
        };
        let state: SavedState = match serde_json::from_slice(&text) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("会话表读不懂，不恢复：{e}");
                return;
            }
        };
        let when = local_time(state.saved);
        let mut map = self.inner.map.lock().unwrap();
        for saved in state.sessions {
            self.inner.next.fetch_max(saved.id, Ordering::Relaxed);
            let history = std::fs::read(self.history_path(saved.id)).unwrap_or_default();
            let result = Session::spawn(
                Spec {
                    id: saved.id,
                    size: TermSize::new(saved.cols, saved.rows),
                    run: None,
                    cwd: Some(saved.cwd.clone()),
                    history: Some((history, when.clone())),
                    command: saved.command.clone(),
                    started: saved.started,
                },
                self.inner.rt.clone(),
                self.on_close(),
            );
            match result {
                Ok(s) => {
                    tracing::info!(id = saved.id, cwd = %saved.cwd, "恢复了会话");
                    map.insert(saved.id, s);
                }
                Err(e) => tracing::warn!(id = saved.id, "恢复会话失败：{e:#}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_from_home() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            resolve_dir(Some("~")),
            Some(home.to_string_lossy().into_owned())
        );
        assert_eq!(resolve_dir(Some("/")), Some("/".into()));
        assert_eq!(resolve_dir(Some("  ")), None);
        assert_eq!(resolve_dir(Some("/no/such/dir")), None);
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("/bin/zsh"), "/bin/zsh");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn saved_state_roundtrip() {
        let st = SavedState {
            version: 1,
            saved: 5,
            sessions: vec![Saved {
                id: 3,
                command: "claude".into(),
                cwd: "/tmp".into(),
                started: 1,
                cols: 80,
                rows: 24,
            }],
        };
        let back: SavedState = serde_json::from_slice(&serde_json::to_vec(&st).unwrap()).unwrap();
        assert_eq!(back.sessions[0].command, "claude");
        assert_eq!(back.sessions[0].cols, 80);
    }

    #[test]
    fn date_formats() {
        assert_ne!(local_time(1_700_000_000), "");
    }
}
