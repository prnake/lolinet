//! GUI：egui（wgpu 后端）。左栏会话 / 显示器，中间是终端格子或远程桌面画面。
//! 网络跑在 tokio 线程，和界面之间两条队列：UiCmd 过去，NetEvent 回来（回来时顺手 request_repaint）。

use std::{
    collections::HashMap,
    sync::{mpsc, Arc},
    time::{Duration, Instant},
};

use anyhow::Result;
use eframe::egui::{
    self, Color32, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2,
};
use lolidesk_proto::{
    self as proto, ControlReq, ControlResp, DeskEvent, DeskInput, DeskReq, DisplayInfo, Frag,
    Input, Key, Mods, MouseButton, Quality, SessionInfo, StreamKind, TermEvent, TermReq,
    VideoFrame,
};
use tokio::sync::mpsc as tmpsc;

use crate::{
    decode::{Decoder, Rgba},
    net::Client,
    term::{cell_colors, Model},
};

pub struct Start {
    pub session: Option<u32>,
    pub desktop: Option<u32>,
    pub quality: Quality,
}

enum UiCmd {
    Refresh,
    RefreshSessions,
    OpenTerminal { cols: u16, rows: u16 },
    Attach { id: u32, cols: u16, rows: u16 },
    TermInput(Input),
    TermResize { cols: u16, rows: u16 },
    KillSession,
    StartDesktop { display: u32, quality: Quality },
    StopDesktop,
    DeskInput(DeskInput),
    Keyframe,
    SetQuality(Quality),
}

enum NetEvent {
    Sessions(Vec<SessionInfo>),
    Displays(Vec<DisplayInfo>),
    /// 主机列不出显示器（多半是没给屏幕录制权限）；常驻显示在远程桌面一栏，不进错误列表
    DisplaysError(String),
    Term(TermEvent),
    TermClosed,
    DeskReady {
        width: u32,
        height: u32,
    },
    DeskFrame(Rgba),
    DeskStopped(String),
    Error(String),
    Rtt(Duration),
}

pub fn run(rt: tokio::runtime::Runtime, client: Arc<Client>, start: Start) -> Result<()> {
    let (cmd_tx, cmd_rx) = tmpsc::unbounded_channel::<UiCmd>();
    let (ev_tx, ev_rx) = mpsc::channel::<NetEvent>();
    let repaint: Arc<std::sync::Mutex<Option<egui::Context>>> =
        Arc::new(std::sync::Mutex::new(None));
    let title = format!("lolidesk · {}", client.welcome.host);
    let desktop_supported = client.welcome.desktop && Decoder::supported();
    {
        let (client, repaint) = (client.clone(), repaint.clone());
        rt.spawn(async move { net_worker(client, cmd_rx, ev_tx, repaint).await });
    }
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title(&title)
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([640.0, 400.0]),
        ..Default::default()
    };
    let host = client.welcome.host.clone();
    let client_for_close = client.clone();
    eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            *repaint.lock().unwrap() = Some(cc.egui_ctx.clone());
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(App::new(
                host,
                cmd_tx,
                ev_rx,
                start,
                desktop_supported,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe：{e}"))?;
    client_for_close.conn.close(0u32.into(), b"bye");
    rt.shutdown_timeout(Duration::from_millis(500));
    Ok(())
}

// ---------------------------------------------------------------- 网络线程

async fn net_worker(
    client: Arc<Client>,
    mut rx: tmpsc::UnboundedReceiver<UiCmd>,
    tx: mpsc::Sender<NetEvent>,
    repaint: Arc<std::sync::Mutex<Option<egui::Context>>>,
) {
    let emit = {
        let tx = tx.clone();
        let repaint = repaint.clone();
        move |ev: NetEvent| {
            let _ = tx.send(ev);
            if let Some(ctx) = repaint.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        }
    };
    let emit = Arc::new(emit);
    let mut term: Option<tmpsc::UnboundedSender<TermReq>> = None;
    let mut desk: Option<tmpsc::UnboundedSender<DeskReq>> = None;
    let mut rtt = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { break };
                match cmd {
                    UiCmd::Refresh | UiCmd::RefreshSessions => {
                        let full = matches!(cmd, UiCmd::Refresh);
                        match client.control(ControlReq::ListSessions).await {
                            Ok(ControlResp::Sessions(s)) => emit(NetEvent::Sessions(s)),
                            Ok(ControlResp::Error(e)) | Ok(ControlResp::Denied(e)) => emit(NetEvent::Error(e)),
                            Err(e) => emit(NetEvent::Error(format!("{e:#}"))),
                            _ => {}
                        }
                        if full && client.welcome.desktop {
                            match client.control(ControlReq::ListDisplays).await {
                                Ok(ControlResp::Displays(d)) => emit(NetEvent::Displays(d)),
                                Ok(ControlResp::Error(e)) => emit(NetEvent::DisplaysError(e)),
                                Err(e) => emit(NetEvent::Error(format!("{e:#}"))),
                                _ => {}
                            }
                        }
                    }
                    UiCmd::OpenTerminal { cols, rows } => {
                        term = Some(start_term(client.clone(), TermReq::Open { cols, rows, command: None, cwd: None }, emit.clone()).await);
                    }
                    UiCmd::Attach { id, cols, rows } => {
                        term = Some(start_term(client.clone(), TermReq::Attach { id, cols, rows }, emit.clone()).await);
                    }
                    UiCmd::TermInput(i) => { if let Some(t) = &term { let _ = t.send(TermReq::Input(i)); } }
                    UiCmd::TermResize { cols, rows } => { if let Some(t) = &term { let _ = t.send(TermReq::Resize { cols, rows }); } }
                    UiCmd::KillSession => { if let Some(t) = &term { let _ = t.send(TermReq::Kill); } }
                    UiCmd::StartDesktop { display, quality } => {
                        if let Some(d) = desk.take() { let _ = d.send(DeskReq::Stop); }
                        desk = Some(start_desk(client.clone(), display, quality, emit.clone()).await);
                    }
                    UiCmd::StopDesktop => { if let Some(d) = desk.take() { let _ = d.send(DeskReq::Stop); } }
                    UiCmd::DeskInput(i) => { if let Some(d) = &desk { let _ = d.send(DeskReq::Input(i)); } }
                    UiCmd::Keyframe => { if let Some(d) = &desk { let _ = d.send(DeskReq::Keyframe); } }
                    UiCmd::SetQuality(q) => { if let Some(d) = &desk { let _ = d.send(DeskReq::Quality(q)); } }
                }
            }
            _ = rtt.tick() => emit(NetEvent::Rtt(client.rtt())),
        }
    }
}

type Emit = Arc<dyn Fn(NetEvent) + Send + Sync>;

async fn start_term(
    client: Arc<Client>,
    first: TermReq,
    emit: Emit,
) -> tmpsc::UnboundedSender<TermReq> {
    let (tx, mut rx) = tmpsc::unbounded_channel::<TermReq>();
    tokio::spawn(async move {
        let r: Result<()> = async {
            let (mut send, mut recv) = client.open(StreamKind::Terminal).await?;
            proto::send(&mut send, &first).await?;
            loop {
                tokio::select! {
                    ev = proto::recv::<TermEvent>(&mut recv) => {
                        match ev? {
                            None => break,
                            Some(ev) => emit(NetEvent::Term(ev)),
                        }
                    }
                    req = rx.recv() => {
                        let Some(req) = req else { break };
                        proto::send(&mut send, &req).await?;
                    }
                }
            }
            let _ = send.finish();
            Ok(())
        }
        .await;
        if let Err(e) = r {
            emit(NetEvent::Error(format!("终端流：{e:#}")));
        }
        emit(NetEvent::TermClosed);
    });
    tx
}

async fn start_desk(
    client: Arc<Client>,
    display: u32,
    quality: Quality,
    emit: Emit,
) -> tmpsc::UnboundedSender<DeskReq> {
    let (tx, mut rx) = tmpsc::unbounded_channel::<DeskReq>();
    let keyframe_tx = tx.clone();
    tokio::spawn(async move {
        let r: Result<()> = async {
            let (mut send, mut recv) = client.open(StreamKind::Desktop).await?;
            proto::send(&mut send, &DeskReq::Start { display, quality, max_fps: 30 }).await?;
            let conn = client.conn.clone();
            let emit2 = emit.clone();
            let dgram = tokio::spawn(async move { datagram_loop(conn, keyframe_tx, emit2).await });
            loop {
                tokio::select! {
                    ev = proto::recv::<DeskEvent>(&mut recv) => {
                        match ev? {
                            None => break,
                            Some(DeskEvent::Ready { width, height }) => emit(NetEvent::DeskReady { width, height }),
                            Some(DeskEvent::Stopped(why)) => { emit(NetEvent::DeskStopped(why)); break; }
                            Some(DeskEvent::Error(e)) => emit(NetEvent::Error(format!("远程桌面：{e}"))),
                        }
                    }
                    req = rx.recv() => {
                        let Some(req) = req else { break };
                        let stop = matches!(req, DeskReq::Stop);
                        proto::send(&mut send, &req).await?;
                        if stop { break; }
                    }
                }
            }
            dgram.abort();
            let _ = send.finish();
            Ok(())
        }
        .await;
        if let Err(e) = r {
            emit(NetEvent::Error(format!("桌面流：{e:#}")));
        }
        emit(NetEvent::DeskStopped("已断开".into()));
    });
    tx
}

/// 拼帧中的一帧：各分片、已收到的分片数、是不是关键帧。
type Pending = (Vec<Option<Vec<u8>>>, usize, bool);

/// 收 datagram、按 frame id 拼帧、解码。丢了帧（或拼不齐）就要关键帧。
async fn datagram_loop(conn: quinn::Connection, req: tmpsc::UnboundedSender<DeskReq>, emit: Emit) {
    let mut decoder = Decoder::new();
    let mut pending: HashMap<u32, Pending> = HashMap::new();
    let mut next_expected: Option<u32> = None;
    let mut last_keyframe_req = Instant::now() - Duration::from_secs(10);
    let mut need_key = true;
    loop {
        let d = match conn.read_datagram().await {
            Ok(d) => d,
            Err(_) => break,
        };
        let Some((frag, body)) = Frag::parse(&d) else {
            continue;
        };
        let entry = pending.entry(frag.frame).or_insert_with(|| {
            (
                vec![None; frag.count as usize],
                0,
                frag.flags & proto::FRAG_KEY != 0,
            )
        });
        if (frag.index as usize) < entry.0.len() && entry.0[frag.index as usize].is_none() {
            entry.0[frag.index as usize] = Some(body.to_vec());
            entry.1 += 1;
        }
        if entry.1 < entry.0.len() {
            // 超过 3 帧还没拼齐的丢掉
            pending.retain(|id, _| frag.frame.wrapping_sub(*id) < 4);
            continue;
        }
        let (parts, _, is_key) = pending.remove(&frag.frame).unwrap();
        pending.retain(|id, _| frag.frame.wrapping_sub(*id) < 4);
        let mut data = Vec::new();
        for p in parts {
            data.extend_from_slice(&p.unwrap_or_default());
        }
        let gap = next_expected.is_some_and(|n| n != frag.frame);
        next_expected = Some(frag.frame.wrapping_add(1));
        if is_key {
            need_key = false;
        } else if gap || need_key {
            need_key = true;
            if last_keyframe_req.elapsed() > Duration::from_millis(200) {
                let _ = req.send(DeskReq::Keyframe);
                last_keyframe_req = Instant::now();
            }
            continue;
        }
        let Some(frame) = VideoFrame::parse(&data) else {
            continue;
        };
        match decoder.decode(&frame) {
            Ok(Some(rgba)) => emit(NetEvent::DeskFrame(rgba)),
            Ok(None) => {}
            Err(e) => {
                emit(NetEvent::Error(format!("解码：{e:#}")));
                need_key = true;
            }
        }
    }
}

// ---------------------------------------------------------------- 界面

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Terminal,
    Desktop,
}

struct App {
    host: String,
    cmd: tmpsc::UnboundedSender<UiCmd>,
    ev: mpsc::Receiver<NetEvent>,
    tab: Tab,
    sessions: Vec<SessionInfo>,
    displays: Vec<DisplayInfo>,
    current: Option<u32>,
    model: Option<Model>,
    term_size: (u16, u16),
    pending_start: Option<Start>,
    desktop_supported: bool,
    displays_error: Option<String>,
    desk: Option<DeskView>,
    quality: Quality,
    errors: Vec<(Instant, String)>,
    rtt: Duration,
    last_refresh: Instant,
    font_px: f32,
}

struct DeskView {
    display: u32,
    size: (u32, u32),
    texture: Option<TextureHandle>,
    frames: u64,
    last_frame: Option<Instant>,
    last_move: Option<(f32, f32)>,
    control: bool,
}

impl App {
    fn new(
        host: String,
        cmd: tmpsc::UnboundedSender<UiCmd>,
        ev: mpsc::Receiver<NetEvent>,
        start: Start,
        desktop_supported: bool,
    ) -> Self {
        let _ = cmd.send(UiCmd::Refresh);
        let tab = if start.desktop.is_some() && desktop_supported {
            Tab::Desktop
        } else {
            Tab::Terminal
        };
        let quality = start.quality;
        Self {
            host,
            cmd,
            ev,
            tab,
            sessions: Vec::new(),
            displays: Vec::new(),
            current: None,
            model: None,
            term_size: (0, 0),
            pending_start: Some(start),
            desktop_supported,
            displays_error: None,
            desk: None,
            quality,
            errors: Vec::new(),
            rtt: Duration::ZERO,
            last_refresh: Instant::now(),
            font_px: 14.0,
        }
    }

    fn error(&mut self, e: String) {
        tracing::warn!("{e}");
        self.errors.push((Instant::now(), e));
        if self.errors.len() > 5 {
            self.errors.remove(0);
        }
    }

    fn drain(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.ev.try_recv() {
            match ev {
                NetEvent::Sessions(s) => self.sessions = s,
                NetEvent::DisplaysError(e) => {
                    self.displays.clear();
                    self.displays_error = Some(e);
                }
                NetEvent::Displays(d) => {
                    self.displays_error = None;
                    self.displays = d;
                    if let Some(st) = &self.pending_start {
                        if let Some(d) = st.desktop {
                            if self.desktop_supported && self.desk.is_none() {
                                let id = if d == 0 {
                                    self.displays
                                        .iter()
                                        .find(|x| x.primary)
                                        .or(self.displays.first())
                                        .map(|x| x.id)
                                } else {
                                    Some(d)
                                };
                                if let Some(id) = id {
                                    self.start_desktop(id);
                                }
                            }
                        }
                    }
                }
                NetEvent::Term(ev) => match ev {
                    TermEvent::Opened { id } => {
                        self.current = Some(id);
                        let _ = self.cmd.send(UiCmd::Refresh);
                    }
                    TermEvent::Snapshot { cols, rows, data } => {
                        let m = self.model.get_or_insert_with(|| Model::new(cols, rows));
                        m.snapshot(cols, rows, &data);
                    }
                    TermEvent::Output(b) => {
                        if let Some(m) = &mut self.model {
                            m.feed(&b);
                        }
                    }
                    TermEvent::Title(t) => {
                        if let Some(m) = &mut self.model {
                            m.title = t;
                        }
                    }
                    TermEvent::Bell => {
                        if let Some(m) = &mut self.model {
                            m.bell = true;
                        }
                    }
                    TermEvent::Exited(code) => {
                        self.error(format!(
                            "会话 {} 结束（退出码 {code}）",
                            self.current.unwrap_or(0)
                        ));
                        self.current = None;
                        self.model = None;
                        let _ = self.cmd.send(UiCmd::Refresh);
                    }
                    TermEvent::Error(e) => self.error(e),
                },
                NetEvent::TermClosed => {
                    if self.current.is_some() {
                        self.error("终端流断了，点会话重新接入".into());
                        self.current = None;
                    }
                }
                NetEvent::DeskReady { width, height } => {
                    if let Some(d) = &mut self.desk {
                        d.size = (width, height);
                    }
                }
                NetEvent::DeskFrame(f) => {
                    if let Some(d) = &mut self.desk {
                        let img = egui::ColorImage::from_rgba_unmultiplied(
                            [f.width as usize, f.height as usize],
                            &f.rgba,
                        );
                        match &mut d.texture {
                            Some(t) => t.set(img, TextureOptions::LINEAR),
                            None => {
                                d.texture =
                                    Some(ctx.load_texture("desktop", img, TextureOptions::LINEAR))
                            }
                        }
                        d.frames += 1;
                        d.last_frame = Some(Instant::now());
                    }
                }
                NetEvent::DeskStopped(why) => {
                    if self.desk.take().is_some() {
                        self.error(format!("远程桌面：{why}"));
                    }
                }
                NetEvent::Error(e) => self.error(e),
                NetEvent::Rtt(r) => self.rtt = r,
            }
        }
    }

    fn start_desktop(&mut self, display: u32) {
        self.desk = Some(DeskView {
            display,
            size: (0, 0),
            texture: None,
            frames: 0,
            last_frame: None,
            last_move: None,
            control: true,
        });
        let _ = self.cmd.send(UiCmd::StartDesktop {
            display,
            quality: self.quality,
        });
        self.tab = Tab::Desktop;
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&self.host).strong().size(16.0));
        ui.label(
            egui::RichText::new(format!("RTT {} ms", self.rtt.as_millis()))
                .weak()
                .small(),
        );
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("会话").small().weak());
            if ui.small_button("刷新").clicked() {
                let _ = self.cmd.send(UiCmd::Refresh);
            }
        });
        if ui.button("＋ 新终端").clicked() {
            let (c, r) = if self.term_size.0 > 0 {
                self.term_size
            } else {
                (100, 30)
            };
            self.model = Some(Model::new(c, r));
            let _ = self.cmd.send(UiCmd::OpenTerminal { cols: c, rows: r });
            self.tab = Tab::Terminal;
        }
        let sessions = self.sessions.clone();
        for s in &sessions {
            let selected = self.current == Some(s.id);
            let label = if s.title.is_empty() {
                format!("#{} {}", s.id, s.command)
            } else {
                format!("#{} {}", s.id, s.title)
            };
            let resp = ui.selectable_label(selected, egui::RichText::new(label).monospace());
            if resp.clicked() && !selected {
                let (c, r) = if self.term_size.0 > 0 {
                    self.term_size
                } else {
                    (s.cols, s.rows)
                };
                self.model = Some(Model::new(c, r));
                self.current = Some(s.id);
                let _ = self.cmd.send(UiCmd::Attach {
                    id: s.id,
                    cols: c,
                    rows: r,
                });
                self.tab = Tab::Terminal;
            }
            resp.on_hover_text(format!("{}×{} · {} 人在看", s.cols, s.rows, s.viewers));
        }
        if self.current.is_some() && ui.small_button("结束当前会话").clicked() {
            let _ = self.cmd.send(UiCmd::KillSession);
        }
        ui.separator();
        ui.label(egui::RichText::new("远程桌面").small().weak());
        if !self.desktop_supported {
            ui.label(egui::RichText::new("主机或本机不支持").weak().small());
        } else if let Some(e) = &self.displays_error {
            ui.label(
                egui::RichText::new(e)
                    .color(Color32::from_rgb(0xff, 0xb2, 0x24))
                    .small(),
            );
        }
        let displays = self.displays.clone();
        for d in &displays {
            let on = self.desk.as_ref().is_some_and(|x| x.display == d.id);
            if ui
                .selectable_label(
                    on,
                    format!("{}{}", d.name, if d.primary { " · 主" } else { "" }),
                )
                .clicked()
                && self.desktop_supported
            {
                if on {
                    self.tab = Tab::Desktop;
                } else {
                    self.start_desktop(d.id);
                }
            }
        }
        ui.horizontal(|ui| {
            for (q, name) in [
                (Quality::Smooth, "流畅"),
                (Quality::Balanced, "均衡"),
                (Quality::Sharp, "清晰"),
            ] {
                if ui.selectable_label(self.quality == q, name).clicked() && self.quality != q {
                    self.quality = q;
                    let _ = self.cmd.send(UiCmd::SetQuality(q));
                }
            }
        });
        if self.desk.is_some() {
            if ui.small_button("刷新画面").clicked() {
                let _ = self.cmd.send(UiCmd::Keyframe);
            }
            if ui.small_button("断开桌面").clicked() {
                let _ = self.cmd.send(UiCmd::StopDesktop);
                self.desk = None;
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("字号").small().weak());
            if ui.small_button("−").clicked() {
                self.font_px = (self.font_px - 1.0).max(9.0);
            }
            ui.label(format!("{}", self.font_px as i32));
            if ui.small_button("＋").clicked() {
                self.font_px = (self.font_px + 1.0).min(32.0);
            }
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            let now = Instant::now();
            self.errors
                .retain(|(t, _)| now.duration_since(*t) < Duration::from_secs(12));
            for (_, e) in self.errors.iter().rev() {
                ui.label(
                    egui::RichText::new(e)
                        .color(Color32::from_rgb(0xff, 0xb2, 0x24))
                        .small(),
                );
            }
        });
    }

    fn terminal(&mut self, ui: &mut egui::Ui) {
        let font = FontId::monospace(self.font_px);
        let cell = ui
            .painter()
            .layout_no_wrap("M".to_string(), font.clone(), Color32::WHITE)
            .size();
        let (cw, ch) = (cell.x.max(1.0), cell.y.max(1.0));
        let avail = ui.available_rect_before_wrap();
        let cols = ((avail.width() / cw).floor() as u16).clamp(2, 500);
        let rows = ((avail.height() / ch).floor() as u16).clamp(1, 300);
        if (cols, rows) != self.term_size {
            self.term_size = (cols, rows);
            if let Some(m) = &mut self.model {
                m.resize(cols, rows);
                if self.current.is_some() {
                    let _ = self.cmd.send(UiCmd::TermResize { cols, rows });
                }
            }
        }
        let rect = Rect::from_min_size(avail.min, Vec2::new(cols as f32 * cw, rows as f32 * ch));
        let resp = ui.interact(avail, ui.scope_id().with("term"), Sense::click());
        if resp.clicked() {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        let painter = ui.painter_at(avail);
        painter.rect_filled(avail, 0.0, Color32::BLACK);

        let Some(model) = &mut self.model else {
            painter.text(
                avail.center(),
                egui::Align2::CENTER_CENTER,
                "左边选一个会话，或新开终端",
                font,
                Color32::from_gray(120),
            );
            return;
        };

        // 输入
        if focused {
            let events = ui.input(|i| i.events.clone());
            let alt_screen = model.alt_screen();
            let mut inputs: Vec<Input> = Vec::new();
            let mut scroll_lines = 0i32;
            for ev in events {
                match ev {
                    egui::Event::Text(t) => {
                        if !t.is_empty() {
                            inputs.push(Input::Text(t));
                        }
                    }
                    egui::Event::Paste(t) => inputs.push(Input::Paste(t)),
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if let Some((k, is_char)) = map_key(key) {
                            let mods = mods_of(modifiers);
                            // 可打印字符没按 Ctrl / Alt / Cmd 时由 Text 事件处理，这里只管组合键和功能键
                            if !is_char
                                || modifiers.ctrl
                                || modifiers.alt
                                || modifiers.mac_cmd
                                || modifiers.command
                            {
                                if matches!(k, Key::Char(_))
                                    && (modifiers.mac_cmd || modifiers.command)
                                    && !modifiers.ctrl
                                {
                                    continue; // Cmd+字母留给系统 / 剪贴板
                                }
                                inputs.push(Input::Key { key: k, mods });
                            }
                        }
                    }
                    egui::Event::MouseWheel { unit, delta, .. } => {
                        let lines = match unit {
                            egui::MouseWheelUnit::Line => delta.y,
                            egui::MouseWheelUnit::Point => delta.y / ch,
                            egui::MouseWheelUnit::Page => delta.y * rows as f32,
                        };
                        scroll_lines += lines.round() as i32;
                    }
                    _ => {}
                }
            }
            if scroll_lines != 0 {
                if alt_screen {
                    let key = if scroll_lines > 0 { Key::Up } else { Key::Down };
                    for _ in 0..scroll_lines.unsigned_abs().min(10) {
                        inputs.push(Input::Key {
                            key,
                            mods: Mods::default(),
                        });
                    }
                } else {
                    model.scroll(scroll_lines);
                }
            }
            for i in inputs {
                let _ = self.cmd.send(UiCmd::TermInput(i));
            }
        }

        // 绘制
        let content = model.content();
        let colors = content.colors;
        let origin = rect.min;
        let cursor_pt = content.cursor.point;
        let display_offset = content.display_offset;
        for cell in content.display_iter {
            let c = cell.cell;
            use alacritty_terminal::term::cell::Flags;
            if c.flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let x = origin.x + cell.point.column.0 as f32 * cw;
            let y = origin.y + cell.point.line.0 as f32 * ch;
            let width = if c.flags.contains(Flags::WIDE_CHAR) {
                2.0 * cw
            } else {
                cw
            };
            let (fg, bg) = cell_colors(c.fg, c.bg, c.flags, colors);
            if let Some(bg) = bg {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, ch)),
                    0.0,
                    rgb(bg),
                );
            }
            if c.c != ' ' && c.c != '\0' {
                let mut text = c.c.to_string();
                if let Some(zw) = c.zerowidth() {
                    text.extend(zw.iter());
                }
                painter.text(
                    Pos2::new(x, y),
                    egui::Align2::LEFT_TOP,
                    text,
                    font.clone(),
                    rgb(fg),
                );
            }
            if c.flags.contains(Flags::UNDERLINE) {
                painter.hline(x..=x + width, y + ch - 1.5, Stroke::new(1.0, rgb(fg)));
            }
            if c.flags.contains(Flags::STRIKEOUT) {
                painter.hline(x..=x + width, y + ch / 2.0, Stroke::new(1.0, rgb(fg)));
            }
        }
        if model.show_cursor() && display_offset == 0 {
            let x = origin.x + cursor_pt.column.0 as f32 * cw;
            let y = origin.y + cursor_pt.line.0 as f32 * ch;
            let eye = Color32::from_rgb(0xce, 0xff, 0x1c);
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cw, ch));
            use alacritty_terminal::vte::ansi::CursorShape;
            match (focused, model.cursor_shape()) {
                (false, _) | (_, CursorShape::HollowBlock) => {
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, eye), egui::StrokeKind::Inside);
                }
                (_, CursorShape::Beam) => {
                    painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(2.0, ch)), 0.0, eye);
                }
                (_, CursorShape::Underline) => {
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(x, y + ch - 2.0), Vec2::new(cw, 2.0)),
                        0.0,
                        eye,
                    );
                }
                (_, CursorShape::Hidden) => {}
                _ => {
                    painter.rect_filled(r, 0.0, eye.gamma_multiply(0.85));
                }
            }
        }
        if display_offset > 0 {
            painter.text(
                Pos2::new(avail.max.x - 8.0, avail.min.y + 4.0),
                egui::Align2::RIGHT_TOP,
                format!("↑ {display_offset}"),
                FontId::proportional(12.0),
                Color32::from_gray(160),
            );
        }
        if model.bell {
            model.bell = false;
            ui.ctx().request_repaint_after(Duration::from_millis(80));
        }
    }

    fn desktop(&mut self, ui: &mut egui::Ui) {
        let avail = ui.available_rect_before_wrap();
        let painter = ui.painter_at(avail);
        painter.rect_filled(avail, 0.0, Color32::BLACK);
        let Some(desk) = &mut self.desk else {
            painter.text(
                avail.center(),
                egui::Align2::CENTER_CENTER,
                "左边选一个显示器",
                FontId::proportional(16.0),
                Color32::from_gray(120),
            );
            return;
        };
        let Some(tex) = &desk.texture else {
            painter.text(
                avail.center(),
                egui::Align2::CENTER_CENTER,
                if desk.size.0 > 0 {
                    "等第一帧…"
                } else {
                    "正在连接…"
                },
                FontId::proportional(16.0),
                Color32::from_gray(120),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            return;
        };
        let ts = tex.size_vec2();
        let scale = (avail.width() / ts.x).min(avail.height() / ts.y);
        let size = ts * scale;
        let rect = Rect::from_center_size(avail.center(), size);
        let resp = ui.interact(rect, ui.scope_id().with("desk"), Sense::click_and_drag());
        if resp.clicked() || resp.drag_started() {
            resp.request_focus();
        }
        painter.image(
            tex.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        let focused = resp.has_focus();
        let norm = |p: Pos2| {
            (
                (p.x - rect.min.x) / rect.width(),
                (p.y - rect.min.y) / rect.height(),
            )
        };
        if desk.control {
            if let Some(p) = resp.hover_pos().or_else(|| resp.interact_pointer_pos()) {
                let (x, y) = norm(p);
                if (0.0..=1.0).contains(&x)
                    && (0.0..=1.0).contains(&y)
                    && desk.last_move != Some((x, y))
                {
                    desk.last_move = Some((x, y));
                    let _ = self.cmd.send(UiCmd::DeskInput(DeskInput::Move { x, y }));
                }
            }
            let events = ui.input(|i| i.events.clone());
            for ev in events {
                match ev {
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => {
                        let (x, y) = norm(pos);
                        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                            continue;
                        }
                        let b = match button {
                            egui::PointerButton::Primary => MouseButton::Left,
                            egui::PointerButton::Secondary => MouseButton::Right,
                            egui::PointerButton::Middle => MouseButton::Middle,
                            _ => continue,
                        };
                        let _ = self.cmd.send(UiCmd::DeskInput(DeskInput::Button {
                            x,
                            y,
                            button: b,
                            down: pressed,
                        }));
                    }
                    egui::Event::MouseWheel { unit, delta, .. } if resp.hovered() => {
                        let (dx, dy) = match unit {
                            egui::MouseWheelUnit::Line => (delta.x, delta.y),
                            egui::MouseWheelUnit::Point => (delta.x / 20.0, delta.y / 20.0),
                            egui::MouseWheelUnit::Page => (delta.x * 10.0, delta.y * 10.0),
                        };
                        if let Some((x, y)) = desk.last_move {
                            let _ =
                                self.cmd
                                    .send(UiCmd::DeskInput(DeskInput::Scroll { x, y, dx, dy }));
                        }
                    }
                    egui::Event::Text(t) if focused => {
                        let _ = self.cmd.send(UiCmd::DeskInput(DeskInput::Text(t)));
                    }
                    egui::Event::Key {
                        key,
                        pressed,
                        modifiers,
                        ..
                    } if focused => {
                        if let Some((k, is_char)) = map_key(key) {
                            // 普通字符走 Text；功能键和带 Ctrl / Alt / Cmd 的组合键走 Key
                            if !is_char
                                || modifiers.ctrl
                                || modifiers.alt
                                || modifiers.mac_cmd
                                || modifiers.command
                            {
                                let _ = self.cmd.send(UiCmd::DeskInput(DeskInput::Key {
                                    key: k,
                                    mods: mods_of(modifiers),
                                    down: pressed,
                                }));
                            }
                        }
                    }
                    egui::Event::Paste(t) if focused => {
                        let _ = self.cmd.send(UiCmd::DeskInput(DeskInput::Text(t)));
                    }
                    _ => {}
                }
            }
        }
        let fps_hint = desk
            .last_frame
            .map(|t| t.elapsed() < Duration::from_secs(2))
            .unwrap_or(false);
        let status = format!(
            "{}×{} · {} 帧{}{}",
            desk.size.0,
            desk.size.1,
            desk.frames,
            if fps_hint { "" } else { " · 画面停了" },
            if focused {
                " · 键盘已接管"
            } else {
                " · 点一下画面接管键盘"
            }
        );
        painter.text(
            Pos2::new(avail.min.x + 8.0, avail.max.y - 6.0),
            egui::Align2::LEFT_BOTTOM,
            status,
            FontId::proportional(12.0),
            Color32::from_gray(150),
        );
    }
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn mods_of(m: egui::Modifiers) -> Mods {
    let mut v = 0u8;
    if m.shift {
        v |= Mods::SHIFT;
    }
    if m.ctrl {
        v |= Mods::CTRL;
    }
    if m.alt {
        v |= Mods::ALT;
    }
    if m.mac_cmd || m.command && !m.ctrl {
        v |= Mods::SUPER;
    }
    Mods(v)
}

/// egui 按键 → 协议按键。第二个值表示是不是可打印字符（这些平时由 Text 事件负责）。
fn map_key(k: egui::Key) -> Option<(Key, bool)> {
    use egui::Key as K;
    let fk = match k {
        K::ArrowUp => Key::Up,
        K::ArrowDown => Key::Down,
        K::ArrowLeft => Key::Left,
        K::ArrowRight => Key::Right,
        K::Enter => Key::Enter,
        K::Tab => Key::Tab,
        K::Backspace => Key::Backspace,
        K::Escape => Key::Escape,
        K::Space => return Some((Key::Space, true)),
        K::Insert => Key::Insert,
        K::Delete => Key::Delete,
        K::Home => Key::Home,
        K::End => Key::End,
        K::PageUp => Key::PageUp,
        K::PageDown => Key::PageDown,
        K::F1 => Key::F(1),
        K::F2 => Key::F(2),
        K::F3 => Key::F(3),
        K::F4 => Key::F(4),
        K::F5 => Key::F(5),
        K::F6 => Key::F(6),
        K::F7 => Key::F(7),
        K::F8 => Key::F(8),
        K::F9 => Key::F(9),
        K::F10 => Key::F(10),
        K::F11 => Key::F(11),
        K::F12 => Key::F(12),
        K::Copy | K::Cut | K::Paste => return None,
        other => {
            let name = other.symbol_or_name();
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            return Some((Key::Char(c.to_ascii_lowercase()), true));
        }
    };
    Some((fk, false))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain(&ctx);
        if let Some(st) = self.pending_start.take() {
            if let Some(id) = st.session {
                self.model = Some(Model::new(100, 30));
                self.current = Some(id);
                let _ = self.cmd.send(UiCmd::Attach {
                    id,
                    cols: 100,
                    rows: 30,
                });
            } else if st.desktop.is_none() {
                self.model = Some(Model::new(100, 30));
                let _ = self.cmd.send(UiCmd::OpenTerminal {
                    cols: 100,
                    rows: 30,
                });
            }
            if st.desktop.is_some() {
                self.pending_start = Some(st); // 等显示器列表回来再开
            }
        }
        if self.last_refresh.elapsed() > Duration::from_secs(10) {
            self.last_refresh = Instant::now();
            let _ = self.cmd.send(UiCmd::RefreshSessions);
        }
        egui::Panel::left("side")
            .exact_size(220.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_gray(10))
                    .inner_margin(8.0),
            )
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::top("tabs")
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_gray(10))
                    .inner_margin(6.0),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.tab, Tab::Terminal, "终端");
                    if self.desktop_supported {
                        ui.selectable_value(&mut self.tab, Tab::Desktop, "桌面");
                    }
                    if let Some(m) = &self.model {
                        if !m.title.is_empty() {
                            ui.label(egui::RichText::new(&m.title).weak());
                        }
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(Color32::BLACK))
            .show(ui, |ui| match self.tab {
                Tab::Terminal => self.terminal(ui),
                Tab::Desktop => self.desktop(ui),
            });
        // 终端有输出 / 桌面在推帧时靠 request_repaint 驱动；这里兜底每 250ms 刷一次状态
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}
