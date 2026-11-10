//! 远程桌面：顶栏、画面（[`Stage`]）、浮在底部的胶囊工具条（[`Toolbar`]）。
//!
//! 重画的范围：新的一帧只通知 `Stage`。GPUI 会连带重跑祖先的 render（这里的 `DeskPane` 和窗口根），
//! 但它们都很浅；侧栏和工具条是缓存的视图（`.cached`），不重画也不重新排版。
//!
//! 画面上的输入：指针按画面矩形归一化到 0..1（工具条上的归工具条，它 `occlude`）；点画面把键盘交给远端；
//! 键盘全部按物理键位发（[`crate::keys`]），Tab / Shift+Tab / ⌘ 组合键也是——`DeskStage` 这一层
//! 用 `NoAction` 把外层（gpui-kit 的 Root、窗口的快捷键）的绑定挡掉，按键才会落到这里的监听上。
//! 失焦、窗口失去激活时发 `ReleaseAll`。

use std::{cell::Cell, net::SocketAddr, rc::Rc, sync::Arc, time::Duration};

use gpui_kit::component::{
    menu::{DropdownMenu, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::{
    Anchor, App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton as GMouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Render,
    ScrollEvent, SharedString, StatefulInteractiveElement, StyleRefinement, Styled, Subscription,
    Task, WeakEntity, Window, canvas, div, prelude::FluentBuilder, px, surface,
};
use lolidesk_core::{decode::Frame, net::Client};
use lolidesk_proto::{DeskInput, DisplayInfo, MouseButton, VideoSettings};

use crate::{
    assets::ic,
    desk::{DeskSender, DeskShared, DeskStream, DeskStreamEvent},
    keys::{self, ModifierTracker},
    labels::{self, Rect, display_name, fit, normalise, normalise_clamped, quality_name},
    model::Model,
    theme::*,
    ui::widgets::{Trigger, dot, icon, mono, pill, vdivider},
    video::StatsSnapshot,
};

/// 画面那一层的按键上下文：这里绑 `NoAction` 挡住外层的快捷键。
pub const STAGE_CONTEXT: &str = "DeskStage";

/// 实际生效的编码参数（主机的 Ready）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub bitrate: u32,
}

pub struct DeskPane {
    model: WeakEntity<Model>,
    pub host: SocketAddr,
    pub host_title: String,
    pub display: DisplayInfo,
    stream: Option<DeskStream>,
    pub stage: Entity<Stage>,
    toolbar: Entity<Toolbar>,
    pub encoding: Option<Encoding>,
    /// 换画面参数中（等新的 Ready）
    pub reconfiguring: bool,
    pub error: Option<String>,
    pub stopped: bool,
    /// 连接断了，等重连后重新开始
    pub suspended: bool,
    pub stats: StatsSnapshot,
    client: Option<Arc<Client>>,
    /// 第几次开始（旧流迟到的事不算）
    run: u32,
    _tasks: Vec<Task<()>>,
}

impl DeskPane {
    /// `conn` 是 None 时不连主机（快照、测试）。
    pub fn new(
        model: WeakEntity<Model>,
        host: SocketAddr,
        host_title: String,
        display: DisplayInfo,
        conn: Option<(Arc<Client>, VideoSettings)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let stage = cx.new(|cx| Stage::new(window, cx));
        let toolbar = cx.new(|_| Toolbar {
            pane: WeakEntity::new_invalid(),
            model: model.clone(),
        });
        let me = cx.weak_entity();
        toolbar.update(cx, |t, _| t.pane = me);
        let mut pane = Self {
            model,
            host,
            host_title,
            display,
            stream: None,
            stage,
            toolbar,
            encoding: None,
            reconfiguring: false,
            error: None,
            stopped: false,
            suspended: false,
            stats: StatsSnapshot::default(),
            client: None,
            run: 0,
            _tasks: Vec::new(),
        };
        if let Some((client, video)) = conn {
            pane.start(client, video, cx);
        }
        pane
    }

    fn start(&mut self, client: Arc<Client>, video: VideoSettings, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        self._tasks.clear();
        self.client = Some(client.clone());
        let (stream, mut events) = DeskStream::start(client, self.display.id, video);
        let shared = stream.shared();
        let sender = stream.sender();
        self.stage.update(cx, |s, cx| s.attach(sender, shared, cx));
        self.stream = Some(stream);
        // 流上来的事
        self._tasks.push(cx.spawn(async move |this, cx| {
            while let Some(ev) = events.recv().await {
                let closed = matches!(ev, DeskStreamEvent::Closed);
                let alive = this.update(cx, |p, cx| {
                    if p.run == run {
                        p.on_event(ev, cx)
                    }
                });
                if alive.is_err() || closed {
                    break;
                }
            }
        }));
        // 每秒取一次统计，只让工具条重画
        self._tasks.push(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |p, cx| {
                    let Some(s) = p.stream.as_mut() else {
                        return false;
                    };
                    let stats = s.stats();
                    if stats != p.stats {
                        p.stats = stats;
                        p.toolbar.update(cx, |_, cx| cx.notify());
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn on_event(&mut self, ev: DeskStreamEvent, cx: &mut Context<Self>) {
        match ev {
            DeskStreamEvent::Ready {
                width,
                height,
                fps,
                bitrate,
            } => {
                self.encoding = Some(Encoding {
                    width,
                    height,
                    fps,
                    bitrate,
                });
                self.reconfiguring = false;
                self.error = None;
            }
            DeskStreamEvent::Stopped(why) => {
                if !why.is_empty() {
                    self.error = Some(why);
                }
                self.stopped = true;
            }
            DeskStreamEvent::Error(e) => self.error = Some(e),
            DeskStreamEvent::Closed => self.stopped = true,
        }
        // 流断是因为连接断了：不算结束，等重连
        if self.stopped
            && self
                .client
                .as_ref()
                .is_some_and(|c| c.conn.close_reason().is_some())
        {
            self.stopped = false;
            self.suspended = true;
            self.error = None;
            if let Some(m) = self.model.upgrade() {
                let host = self.host;
                cx.defer(move |cx| m.update(cx, |m, cx| m.check_host(host, cx)));
            }
        }
        if self.stopped {
            self.stream = None;
            self.stage.update(cx, |s, cx| s.detach(cx));
        }
        self.toolbar.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// 连接断了：停掉流，画面留着，等重连。
    pub fn suspend(&mut self, cx: &mut Context<Self>) {
        if self.stopped {
            return;
        }
        if let Some(mut s) = self.stream.take() {
            s.stop();
        }
        self.run += 1;
        self.suspended = true;
        self.stage.update(cx, |s, cx| s.detach(cx));
        cx.notify();
    }

    /// 重连上了：同一块显示器、同样的参数重新开始。
    pub fn resume(&mut self, client: Arc<Client>, video: VideoSettings, cx: &mut Context<Self>) {
        self.suspended = false;
        self.stopped = false;
        self.error = None;
        self.encoding = None;
        self.reconfiguring = false;
        self.start(client, video, cx);
        cx.notify();
    }

    pub fn configure(&mut self, video: VideoSettings, cx: &mut Context<Self>) {
        if let Some(s) = &self.stream {
            s.configure(video);
            self.reconfiguring = true;
        }
        self.toolbar.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    pub fn keyframe(&self) {
        if let Some(s) = &self.stream {
            s.keyframe();
        }
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(mut s) = self.stream.take() {
            s.stop();
        }
        self.stopped = true;
        self.stage.update(cx, |s, cx| s.detach(cx));
        cx.notify();
    }

    /// 快照、测试用：设成「已经在放」的样子。
    pub fn set_playing_for_test(
        &mut self,
        encoding: Encoding,
        stats: StatsSnapshot,
        frame: Option<Frame>,
        cx: &mut Context<Self>,
    ) {
        self.encoding = Some(encoding);
        self.stats = stats;
        if let Some(f) = frame {
            self.stage.update(cx, |s, cx| s.set_frame(f, cx));
        }
        cx.notify();
    }

    fn status(&self) -> Option<gpui_kit::AnyElement> {
        let base = div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.));
        if self.stopped {
            let model = self.model.clone();
            let (host, display) = (self.host, self.display.id);
            return Some(
                base.bg(void())
                    .child(
                        div()
                            .text_size(T_SM)
                            .text_color(fg2())
                            .child("远程桌面已结束"),
                    )
                    .when_some(self.error.clone(), |s, e| {
                        s.child(
                            div()
                                .max_w(px(520.))
                                .text_size(T_XS)
                                .text_color(fg3())
                                .text_center()
                                .child(e),
                        )
                    })
                    .child(
                        pill("desk-restart", "重新连接", true).on_click(move |_, window, cx| {
                            if let Some(m) = model.upgrade() {
                                m.update(cx, |m, cx| m.start_desk(host, display, window, cx));
                            }
                        }),
                    )
                    .into_any_element(),
            );
        }
        if let Some(e) = self.error.as_ref().filter(|_| !self.suspended) {
            return Some(
                base.child(icon(ic::WARNING, 28., warn()))
                    .child(
                        div()
                            .max_w(px(520.))
                            .text_size(T_SM)
                            .text_color(fg2())
                            .text_center()
                            .child(e.clone()),
                    )
                    .into_any_element(),
            );
        }
        let text = if self.suspended {
            "连接断了，重新连接中…"
        } else if self.encoding.is_none() {
            "正在打开远程桌面…"
        } else if self.reconfiguring {
            "正在换画面参数…"
        } else {
            return None;
        };
        Some(
            base.child(Spinner::new().color(fg3()))
                .child(div().text_size(T_XS).text_color(fg3()).child(text))
                .into_any_element(),
        )
    }
}

impl Render for DeskPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.stage.read(cx).focus.is_focused(window);
        let right = if focused {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(3.))
                .rounded(R_PILL)
                .bg(eye_wash())
                .border_1()
                .border_color(eye_line())
                .child(icon(ic::KEYBOARD, 13., eye()))
                .child(div().text_size(T_CAP).text_color(eye()).child("键盘在远端"))
        } else {
            div()
                .text_size(T_CAP)
                .text_color(fg3())
                .child("点画面把键盘交给远端")
        };
        let status = self.status();
        let stage_focus = self.stage.read(cx).focus.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(void())
            .child(
                super::top_bar(
                    "desk-bar",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .min_w_0()
                        .child(icon(ic::MONITOR, 15., fg2()))
                        .child(
                            div()
                                .text_size(T_SM)
                                .font_weight(gpui_kit::FontWeight::MEDIUM)
                                .truncate()
                                .child(display_name(&self.display.name, self.display.id)),
                        )
                        .child(mono(self.host_title.clone(), T_XS, fg3()).truncate()),
                    right,
                )
                // 点顶栏：键盘从远程画面拿回来
                .on_mouse_down(GMouseButton::Left, move |_, window, cx| {
                    if stage_focus.is_focused(window) {
                        window.blur(cx);
                    }
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.stage.clone())
                    .children(status)
                    .child(
                        self.toolbar.clone().cached(
                            StyleRefinement::default()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom(px(16.))
                                .h(px(42.)),
                        ),
                    ),
            )
    }
}

// ---------------------------------------------------------------- 画面

pub struct Stage {
    pub focus: FocusHandle,
    frame: Option<Frame>,
    /// 画面的像素尺寸（最新一帧的）
    pic: (u32, u32),
    sender: Option<DeskSender>,
    /// 这一层在窗口里的位置（每帧 prepaint 时记下）
    bounds: Rc<Cell<Bounds<Pixels>>>,
    last_move: Option<(f32, f32)>,
    mods: ModifierTracker,
    pump: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl Focusable for Stage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Stage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let subs = vec![
            cx.on_focus(&focus, window, |_, _, cx| cx.notify()),
            cx.on_blur(&focus, window, |s, _, cx| {
                // 键盘离开画面：远端别留着按下的键
                s.release_all();
                cx.notify();
            }),
            cx.observe_window_activation(window, |s, window, cx| {
                if !window.is_window_active() {
                    s.release_all();
                    cx.notify();
                }
            }),
        ];
        Self {
            focus,
            frame: None,
            pic: (0, 0),
            sender: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            last_move: None,
            mods: ModifierTracker::default(),
            pump: None,
            _subs: subs,
        }
    }

    /// 接上一条流：输入发给它，画面从它的槽里取。
    pub fn attach(&mut self, sender: DeskSender, shared: Arc<DeskShared>, cx: &mut Context<Self>) {
        self.sender = Some(sender);
        self.pump = Some(cx.spawn(async move |this, cx| {
            loop {
                shared.frame_ready.notified().await;
                if shared.is_closed() {
                    break;
                }
                let Some(frame) = shared.take_frame() else {
                    continue;
                };
                if this.update(cx, |s, cx| s.set_frame(frame, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    /// 测试用：输入发到这里。
    pub fn set_sender(&mut self, sender: DeskSender) {
        self.sender = Some(sender);
    }

    pub fn detach(&mut self, cx: &mut Context<Self>) {
        self.release_all();
        self.sender = None;
        self.pump = None;
        cx.notify();
    }

    pub fn set_frame(&mut self, frame: Frame, cx: &mut Context<Self>) {
        self.pic = frame.size();
        self.frame = Some(frame);
        cx.notify();
    }

    fn send(&self, input: DeskInput) {
        if let Some(s) = &self.sender {
            s.input(input);
        }
    }

    fn release_all(&mut self) {
        self.mods.reset();
        self.last_move = None;
        self.send(DeskInput::ReleaseAll);
    }

    /// 画面在窗口里的矩形（等比放进这一层）。
    fn picture(&self) -> Rect {
        let b = self.bounds.get();
        let r = Rect::new(
            f32::from(b.origin.x),
            f32::from(b.origin.y),
            f32::from(b.size.width),
            f32::from(b.size.height),
        );
        fit(r, self.pic.0, self.pic.1)
    }

    fn point(p: gpui_kit::Point<Pixels>) -> (f32, f32) {
        (f32::from(p.x), f32::from(p.y))
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, _: &mut Context<Self>) {
        if self.sender.is_none() {
            return;
        }
        let (px, py) = Self::point(ev.position);
        if let Some(p) = normalise(self.picture(), px, py)
            && self.last_move != Some(p)
        {
            self.last_move = Some(p);
            self.send(DeskInput::Move { x: p.0, y: p.1 });
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (px, py) = Self::point(ev.position);
        let Some((x, y)) = normalise(self.picture(), px, py) else {
            return;
        };
        if !self.focus.is_focused(window) {
            // 点了画面：键盘交给远端
            self.focus.focus(window, cx);
        }
        if let Some(button) = map_button(ev.button) {
            self.send(DeskInput::Button {
                x,
                y,
                button,
                down: true,
            });
        }
        cx.stop_propagation();
    }

    fn mouse_up(&mut self, ev: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        let (px, py) = Self::point(ev.position);
        let (x, y) = normalise_clamped(self.picture(), px, py);
        if let Some(button) = map_button(ev.button) {
            self.send(DeskInput::Button {
                x,
                y,
                button,
                down: false,
            });
        }
    }

    fn scroll(&mut self, ev: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        let pic = self.picture();
        let (px, py) = Self::point(ev.position);
        let Some((x, y)) = normalise(pic, px, py) else {
            return;
        };
        let d = ev.delta.pixel_delta(px_line());
        if let Some((dx, dy)) = labels::scroll_fraction(pic, f32::from(d.x), f32::from(d.y))
            && (dx != 0.0 || dy != 0.0)
        {
            self.send(DeskInput::Scroll { x, y, dx, dy });
        }
        cx.stop_propagation();
    }

    fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        // ⌘Q 留在本地（退出）
        if m.platform && !m.control && !m.alt && ev.keystroke.key == "q" {
            return;
        }
        cx.stop_propagation();
        let Some(code) = physical_code(&ev.keystroke.key) else {
            return;
        };
        let mods = keys::mods(m.shift, m.control, m.alt, m.platform);
        self.send(DeskInput::Key {
            code: code.into(),
            down: true,
            mods,
        });
        if mods.sup() {
            // ⌘ 按着时 macOS 不给松开事件：按下就当一次敲击
            self.send(DeskInput::Key {
                code: code.into(),
                down: false,
                mods,
            });
        }
    }

    fn key_up(&mut self, ev: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let m = &ev.keystroke.modifiers;
        let Some(code) = physical_code(&ev.keystroke.key) else {
            return;
        };
        self.send(DeskInput::Key {
            code: code.into(),
            down: false,
            mods: keys::mods(m.shift, m.control, m.alt, m.platform),
        });
    }

    fn modifiers_changed(
        &mut self,
        ev: &ModifiersChangedEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = &ev.modifiers;
        let now = keys::mods(m.shift, m.control, m.alt, m.platform);
        let vk = keys::current_key_code();
        for (code, down) in self.mods.change(vk, now.0) {
            self.send(DeskInput::Key {
                code: code.into(),
                down,
                mods: now,
            });
        }
        cx.stop_propagation();
    }
}

/// 一行滚动换成多少像素（老式滚轮按行报）。
fn px_line() -> Pixels {
    px(16.)
}

/// 这次按键的物理键位：先读 NSEvent 的键码，读不到按键名猜。
fn physical_code(key: &str) -> Option<&'static str> {
    keys::current_key_code()
        .and_then(keys::vk_to_code)
        .or_else(|| keys::key_name_to_code(key))
}

fn map_button(b: GMouseButton) -> Option<MouseButton> {
    match b {
        GMouseButton::Left => Some(MouseButton::Left),
        GMouseButton::Right => Some(MouseButton::Right),
        GMouseButton::Middle => Some(MouseButton::Middle),
        _ => None,
    }
}

impl Render for Stage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bounds = self.bounds.clone();
        div()
            .id("stage")
            .key_context(STAGE_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(void())
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_any_mouse_down(cx.listener(Self::mouse_down))
            .on_mouse_up(GMouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(GMouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up(GMouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(GMouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(GMouseButton::Right, cx.listener(Self::mouse_up))
            .on_scroll(cx.listener(Self::scroll))
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .when_some(self.frame.as_ref(), |s, f| {
                s.child(
                    surface(f.0.clone())
                        .absolute()
                        .size_full()
                        .object_fit(ObjectFit::Contain),
                )
            })
    }
}

// ---------------------------------------------------------------- 工具条

/// 浮在画面底部的胶囊：显示器、分辨率、帧率、画质、刷新画面、全屏、统计、断开。
pub struct Toolbar {
    pane: WeakEntity<DeskPane>,
    model: WeakEntity<Model>,
}

/// 工具条上带文字的下拉。
fn tool_menu(id: &'static str, path: &'static str, label: String, tip: &'static str) -> Trigger {
    Trigger::new(id)
        .tip(tip)
        .open_bg(ink4())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(32.))
        .pl(px(9.))
        .pr(px(10.))
        .rounded(R_PILL)
        .hover(|s| s.bg(ink4()))
        .child(icon(path, 15., fg2()))
        .child(mono(label, T_XS, fg()).whitespace_nowrap())
        .child(icon(ic::CHEVRON_DOWN, 12., fg3()))
}

/// 菜单里机器给的值（分辨率、帧率）用等宽字。
fn mono_item(text: String) -> gpui_kit::Div {
    mono(text, T_SM, fg())
}

/// 工具条上的圆形图标按钮。
fn tool_button(
    id: &'static str,
    path: &'static str,
    tip: &'static str,
    color: gpui_kit::Hsla,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(32.))
        .rounded(R_PILL)
        .cursor_pointer()
        .hover(|s| s.bg(ink4()))
        .active(|s| s.opacity(0.8))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(icon(path, 16., color))
}

impl Render for Toolbar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let row = div().size_full().flex().justify_center().items_center();
        let (Some(pane), Some(model)) = (self.pane.upgrade(), self.model.upgrade()) else {
            return row;
        };
        let p = pane.read(cx);
        let m = model.read(cx);
        let prefs = m.prefs.clone();
        let d = p.display.clone();
        let displays = m
            .host(p.host)
            .map(|h| h.displays.clone())
            .unwrap_or_default();
        let host = p.host;
        let stats = p.stats;
        let enc = p.encoding;
        let live = enc.is_some() && !p.stopped;
        let fps_now = labels::cap_fps(prefs.fps, d.refresh);

        // 显示器
        let display_menu = {
            let model = self.model.clone();
            let current = d.id;
            tool_menu(
                "tb-display",
                ic::MONITOR,
                display_name(&d.name, d.id),
                "显示器",
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for x in &displays {
                    let id = x.id;
                    let model = model.clone();
                    let label = if x.refresh > 0 {
                        format!(
                            "{} · {}×{} · {} Hz",
                            display_name(&x.name, x.id),
                            x.width,
                            x.height,
                            x.refresh
                        )
                    } else {
                        format!("{} · {}×{}", display_name(&x.name, x.id), x.width, x.height)
                    };
                    menu = menu.item(PopupMenuItem::new(label).checked(id == current).on_click(
                        move |_, window, cx| {
                            if id != current
                                && let Some(m) = model.upgrade()
                            {
                                m.update(cx, |m, cx| m.start_desk(host, id, window, cx));
                            }
                        },
                    ));
                }
                menu
            })
        };

        // 分辨率
        let res_menu = {
            let model = self.model.clone();
            let opts = labels::resolution_options(d.width, d.height);
            let current = if prefs.long_edge >= d.width.max(d.height) {
                0
            } else {
                prefs.long_edge
            };
            tool_menu(
                "tb-res",
                ic::EXPAND,
                labels::resolution_label(d.width, d.height, prefs.long_edge)
                    .trim_start_matches("原生 ")
                    .to_string(),
                "分辨率（编码画面的大小）",
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for (edge, label) in &opts {
                    let (edge, model) = (*edge, model.clone());
                    let text = label.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| mono_item(text.clone()))
                            .checked(edge == current)
                            .on_click(move |_, _, cx| {
                                if let Some(m) = model.upgrade() {
                                    m.update(cx, |m, cx| m.set_video(Some(edge), None, None, cx));
                                }
                            }),
                    );
                }
                menu
            })
        };

        // 帧率
        let fps_menu = {
            let model = self.model.clone();
            let opts = labels::fps_options(d.refresh);
            tool_menu(
                "tb-fps",
                ic::FILM,
                format!("{fps_now} fps"),
                "最高帧率（画面不动时不发帧）",
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for f in &opts {
                    let (f, model) = (*f, model.clone());
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| mono_item(format!("{f} fps")))
                            .checked(f == fps_now)
                            .on_click(move |_, _, cx| {
                                if let Some(m) = model.upgrade() {
                                    m.update(cx, |m, cx| m.set_video(None, Some(f), None, cx));
                                }
                            }),
                    );
                }
                menu
            })
        };

        // 画质
        let q_menu = {
            let model = self.model.clone();
            let current = prefs.quality();
            tool_menu(
                "tb-q",
                ic::GAUGE,
                quality_name(current).to_string(),
                "画质（码率）",
            )
            .dropdown_menu_with_anchor(Anchor::BottomLeft, move |mut menu, _, _| {
                for (q, name, hint) in labels::QUALITIES {
                    let model = model.clone();
                    menu = menu.item(
                        PopupMenuItem::element(move |_, _| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(div().text_size(T_SM).child(name))
                                .child(div().text_size(T_CAP).text_color(fg3()).child(hint))
                        })
                        .checked(q == current)
                        .on_click(move |_, _, cx| {
                            if let Some(m) = model.upgrade() {
                                m.update(cx, |m, cx| m.set_video(None, None, Some(q), cx));
                            }
                        }),
                    );
                }
                menu
            })
        };

        let refresh = {
            let pane = self.pane.clone();
            tool_button("tb-kf", ic::REFRESH, "刷新画面（要一个关键帧）", fg2()).on_click(
                move |_, _, cx| {
                    if let Some(p) = pane.upgrade() {
                        p.read(cx).keyframe();
                    }
                },
            )
        };
        let fullscreen = tool_button("tb-fs", ic::FULLSCREEN, "全屏", fg2())
            .on_click(|_, window, _| window.toggle_fullscreen());
        let stop = {
            let model = self.model.clone();
            tool_button("tb-stop", ic::POWER, "断开远程桌面", bad()).on_click(move |_, _, cx| {
                if let Some(m) = model.upgrade() {
                    m.update(cx, |m, cx| m.stop_desk(cx));
                }
            })
        };

        row.child(
            div()
                .id("toolbar")
                .occlude()
                .flex()
                .items_center()
                .gap(px(2.))
                .p(px(5.))
                .rounded(R_PILL)
                .bg(glass())
                .border_1()
                .border_color(line2())
                .shadow(vec![gpui_kit::BoxShadow {
                    color: shadow(),
                    offset: gpui_kit::point(px(0.), px(12.)),
                    blur_radius: px(40.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .child(display_menu)
                .child(vdivider())
                .child(res_menu)
                .child(fps_menu)
                .child(q_menu)
                .child(vdivider())
                .child(refresh)
                .child(fullscreen)
                .child(vdivider())
                .child(stats_view(stats, enc, live))
                .child(stop),
        )
    }
}

/// 实时帧率 · 码率 · 往返延迟，等宽、定宽；悬停看细节。
fn stats_view(s: StatsSnapshot, enc: Option<Encoding>, live: bool) -> impl IntoElement {
    let color = if !live {
        fg4()
    } else if s.rtt_ms > 80.0 || (s.dropped > 0 && s.fps < 5.0) {
        warn()
    } else {
        eye()
    };
    let tip: SharedString = match enc {
        Some(e) => format!(
            "编码 {}×{} @ {} fps，目标码率 {:.1} Mbps\n解码 {:.1} ms/帧 · 丢帧 {} · 关键帧请求 {}",
            e.width,
            e.height,
            e.fps,
            e.bitrate as f64 / 1e6,
            s.decode_ms,
            s.dropped,
            s.keyframes
        ),
        None => format!(
            "还没收到画面\n丢帧 {} · 关键帧请求 {}",
            s.dropped, s.keyframes
        ),
    }
    .into();
    let cell = |text: String, w: f32| {
        div()
            .w(px(w))
            .flex()
            .justify_end()
            .child(mono(text, T_XS, fg2()).whitespace_nowrap())
    };
    div()
        .id("tb-stats")
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .h(px(32.))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(dot(color, 6.))
        .child(cell(format!("{:.0} fps", s.fps), 46.))
        .child(cell(format!("{:.1} Mbps", s.mbps), 70.))
        .child(cell(format!("{:.0} ms", s.rtt_ms), 44.))
}
