//! 配对：
//! - 有设备请求连这台：整个窗口上弹框问（[`consent_overlay`]，盖在别的对话框上面），几个请求排着队；
//! - 连网里的一台设备：「正在请 X 同意…」（[`PairForm`]），那边点了同意就记住、连上；用配对码的直接配对；
//! - 导入分享链接（[`ImportForm`]）：粘贴 `lolidesk://pair?…`，按链接里的证书指纹认主机，用里面的配对码配对。

use std::net::SocketAddr;

use gpui_kit::component::{
    WindowExt as _,
    input::{InputEvent, InputState},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, anchored, deferred, div, point,
    prelude::FluentBuilder, px,
};
use lolidesk_core::net;
use lolidesk_proto::{PAIR_WAIT_SECS, PairLink};

use crate::{
    assets::ic,
    labels::group_fingerprint,
    model::Model,
    pairing,
    runtime::error_text,
    theme::*,
    ui::{
        dialogs::{self, Form, error_line, field, header},
        widgets::{badge, caption, icon, mono, pill},
    },
};

/// 盖在对话框（10 + 层）之上、菜单（100）之下
const CONSENT_PRIORITY: usize = 60;

// ---------------------------------------------------------------- 有设备请求连这台

/// 本机有配对请求时整个窗口上的框：设备名和地址、它能做什么（图标）、倒计时，同意 / 拒绝。排着队一个一个问。
pub fn consent_overlay(model: &Entity<Model>, window: &Window, cx: &App) -> Option<AnyElement> {
    let m = model.read(cx);
    let p = m.consent.first()?.clone();
    let queued = m.consent.len() - 1;
    let allow = m
        .local_host()
        .and_then(|h| h.welcome.as_ref())
        .map(|w| w.allow);
    let id = p.id;
    let waited = pairing::now().saturating_sub(p.since);
    let left = PAIR_WAIT_SECS.saturating_sub(waited);
    let who = [
        pairing::ip_of(&p.addr),
        p.device.os.clone(),
        [p.device.kind.as_str(), p.device.version.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let (m1, m2) = (model.clone(), model.clone());
    // 同意以后它能做什么：五个图标，能做的亮，悬停看是什么
    let caps = allow.map(|a| {
        let items = [
            (a.terminal, ic::TERMINAL, "终端"),
            (a.desktop, ic::MONITOR, "看桌面"),
            (a.control, ic::POINTER, "操作桌面"),
            (a.cua, ic::SPARKLE, "agent"),
            (a.share, ic::DETACH, "分享"),
        ];
        div()
            .flex()
            .gap(px(8.))
            .children(items.into_iter().enumerate().map(|(i, (on, path, name))| {
                let tip: SharedString = if on {
                    name.into()
                } else {
                    format!("不能{name}").into()
                };
                div()
                    .id(("consent-cap", i))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .h(px(40.))
                    .rounded(R_SM)
                    .bg(ink1())
                    .border_1()
                    .border_color(line1())
                    .child(icon(path, 18., if on { fg() } else { fg4() }))
                    .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            }))
    });
    let card = div()
        .id("consent")
        .occlude()
        .flex()
        .flex_col()
        .gap(px(18.))
        .w(px(400.))
        .p(px(22.))
        .rounded(R_LG)
        .bg(ink2())
        .border_1()
        .border_color(eye_line())
        .shadow(vec![gpui_kit::BoxShadow {
            color: shadow(),
            offset: point(px(0.), px(18.)),
            blur_radius: px(48.),
            spread_radius: px(0.),
            inset: false,
        }])
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(14.))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(40.))
                        .rounded(R_MD)
                        .bg(eye_wash())
                        .border_1()
                        .border_color(eye_line())
                        .child(icon(ic::COMPUTER, 20., eye())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(T_LG)
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(p.device.name.clone()),
                        )
                        .child(mono(who, T_CAP, fg3()).truncate()),
                )
                // 还剩几秒（到了就算拒绝）
                .child(
                    div()
                        .id("consent-left")
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(36.))
                        .rounded_full()
                        .border_2()
                        .border_color(warn_line())
                        .child(mono(left.to_string(), T_XS, warn()))
                        .tooltip(|window, cx| Tooltip::new("到时不答就算拒绝").build(window, cx)),
                ),
        )
        .children(caps)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().when(queued > 0, |d| {
                    d.child(badge(format!("+{queued}"), warn(), warn_wash()))
                }))
                .child(
                    pill("consent-deny", "拒绝", false)
                        .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.decide(id, false, cx))),
                )
                .child(
                    pill("consent-allow", "同意", true)
                        .on_click(move |_, _, cx| m2.update(cx, |m, cx| m.decide(id, true, cx))),
                ),
        );
    let size = window.viewport_size();
    Some(
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .id("consent-scrim")
                    .occlude()
                    .w(size.width)
                    .h(size.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(scrim())
                    .child(card),
            ),
        )
        .with_priority(CONSENT_PRIORITY)
        .into_any_element(),
    )
}

// ---------------------------------------------------------------- 连一台设备

enum PairState {
    Busy,
    Failed(String),
}

/// 「正在请 X 同意…」：在后台配对（不带配对码就等那边的人同意，最多 2 分钟）。
/// 对话框关掉（取消、Esc）就取消：连接断了，主机那边的请求也就没了。
pub struct PairForm {
    model: Entity<Model>,
    pub addr: SocketAddr,
    fingerprint: [u8; 32],
    name: String,
    code: Option<String>,
    state: PairState,
    abort: Option<tokio::task::AbortHandle>,
    _task: Option<Task<()>>,
    focus: FocusHandle,
}

impl Drop for PairForm {
    fn drop(&mut self) {
        if let Some(a) = self.abort.take() {
            a.abort();
        }
    }
}

impl Focusable for PairForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PairForm {
    pub fn new(
        model: Entity<Model>,
        addr: SocketAddr,
        fingerprint: [u8; 32],
        name: String,
        code: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            model,
            addr,
            fingerprint,
            name,
            code,
            state: PairState::Busy,
            abort: None,
            _task: None,
            focus: cx.focus_handle(),
        }
    }

    /// 开始配对。
    pub fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.abort.take() {
            a.abort();
        }
        self.state = PairState::Busy;
        let (task, abort) = Model::pair_task(self.addr, self.fingerprint, self.code.clone(), cx);
        self.abort = Some(abort);
        let model = self.model.clone();
        let addr = self.addr;
        self._task = Some(cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |f, window, cx| {
                f.abort = None;
                match r {
                    Ok(p) => {
                        model.update(cx, |m, cx| m.adopt_paired(addr, p, cx));
                        window.close_dialog(cx);
                    }
                    Err(e) => f.state = PairState::Failed(error_text(&e)),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn title(&self) -> String {
        if self.name.is_empty() {
            pairing::ip_of(&self.addr.to_string())
        } else {
            self.name.clone()
        }
    }
}

impl Form for PairForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state, PairState::Failed(_)) {
            self.start(window, cx);
        }
    }
}

impl Render for PairForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.title();
        let consent = self.code.is_none();
        let (title, sub) = if consent {
            (
                format!("正在请 {name} 同意…"),
                format!("最多等 {} 分钟", PAIR_WAIT_SECS / 60),
            )
        } else {
            (format!("正在和 {name} 配对…"), String::new())
        };
        let status: AnyElement = match &self.state {
            PairState::Busy => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(14.))
                .py(px(12.))
                .rounded(R_SM)
                .bg(ink1())
                .border_1()
                .border_color(line1())
                .child(Spinner::new().color(eye()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(T_SM)
                                .truncate()
                                .child(format!("{name} · {}", self.addr)),
                        )
                        .child(mono(
                            if consent {
                                "等对方回答"
                            } else {
                                "正在换凭据"
                            },
                            T_CAP,
                            fg3(),
                        )),
                )
                .into_any_element(),
            PairState::Failed(e) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .px(px(14.))
                .py(px(12.))
                .rounded(R_SM)
                .bg(bad_wash())
                .border_1()
                .border_color(bad_line())
                .child(div().text_size(T_SM).text_color(bad()).child(e.clone()))
                .into_any_element(),
        };
        let fp = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(caption("对方的证书指纹"))
            .child(
                div()
                    .font_family(MONO)
                    .text_size(T_CAP)
                    .line_height(px(16.))
                    .text_color(fg3())
                    .child(group_fingerprint(&hex::encode(self.fingerprint))),
            );
        let failed = matches!(self.state, PairState::Failed(_));
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .child(
                pill("pair-cancel", if failed { "关闭" } else { "取消" }, false)
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            )
            .when(failed, |d| {
                d.child(
                    pill("pair-retry", "再试一次", true)
                        .on_click(cx.listener(|f, _, window, cx| f.start(window, cx))),
                )
            });
        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(T_LG)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .when(!sub.is_empty(), |d| {
                        d.child(div().text_size(T_XS).text_color(fg2()).child(sub))
                    }),
            )
            .child(status)
            .child(fp)
            .child(footer)
    }
}

/// 打开「正在请 X 同意…」并开始配对。
pub fn open_pairing(
    model: Entity<Model>,
    addr: SocketAddr,
    fingerprint: [u8; 32],
    name: String,
    code: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PairForm> {
    let form = cx.new(|cx| PairForm::new(model, addr, fingerprint, name, code, cx));
    dialogs::open_form(form.clone(), window, cx);
    form.update(cx, |f, cx| f.start(window, cx));
    form
}

/// 快照用：打开「正在请 X 同意…」，不真的配对。
pub fn open_pairing_for_test(
    model: Entity<Model>,
    addr: SocketAddr,
    fingerprint: [u8; 32],
    name: String,
    window: &mut Window,
    cx: &mut App,
) -> Entity<PairForm> {
    let form = cx.new(|cx| PairForm::new(model, addr, fingerprint, name, None, cx));
    dialogs::open_form(form.clone(), window, cx);
    form
}

// ---------------------------------------------------------------- 导入分享链接

/// 「导入分享链接」：粘贴 `lolidesk://pair?…`，看清是哪台，再配对。
pub struct ImportForm {
    model: Entity<Model>,
    pub link: Entity<InputState>,
    error: Option<String>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for ImportForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ImportForm {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let link =
            cx.new(|cx| InputState::new(window, cx).placeholder("lolidesk://pair?h=…&f=…&c=…&n=…"));
        let subs =
            vec![cx.subscribe_in(
                &link,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify()
                    }
                    InputEvent::PressEnter { .. } => this.go(window, cx),
                    _ => {}
                },
            )];
        Self {
            model,
            link,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    fn parsed(&self, cx: &App) -> Option<Result<PairLink, String>> {
        let text = self.link.read(cx).value().trim().to_string();
        if text.is_empty() {
            return None;
        }
        Some(PairLink::parse(&text).ok_or_else(|| {
            if text.starts_with("lolidesk://") {
                "分享链接不完整：要 lolidesk://pair?h=…&f=…&c=…".to_string()
            } else {
                "不是分享链接：要 lolidesk://pair?… 开头".to_string()
            }
        }))
    }

    /// 「配对」：认得链接就换成「正在配对…」。
    fn go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let link = match self.parsed(cx) {
            None => {
                self.error = Some("粘贴别人给的分享链接".into());
                cx.notify();
                return;
            }
            Some(Err(e)) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
            Some(Ok(l)) => l,
        };
        let addr = match net::resolve(&link.addr) {
            Ok(a) => a,
            Err(e) => {
                self.error = Some(error_text(&e));
                cx.notify();
                return;
            }
        };
        let Some(fp) = hex::decode(&link.fingerprint)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
        else {
            self.error = Some("链接里的指纹不对".into());
            cx.notify();
            return;
        };
        let model = self.model.clone();
        window.close_dialog(cx);
        open_pairing(model, addr, fp, link.name, Some(link.code), window, cx);
    }
}

impl Form for ImportForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.go(window, cx)
    }
}

impl Render for ImportForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let preview = match self.parsed(cx) {
            Some(Ok(l)) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(px(14.))
                    .py(px(12.))
                    .rounded(R_SM)
                    .bg(ink1())
                    .border_1()
                    .border_color(line1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(icon(ic::COMPUTER, 16., fg2()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family(MONO)
                                    .text_size(T_SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(if l.name.is_empty() {
                                        l.addr.clone()
                                    } else {
                                        l.name.clone()
                                    }),
                            )
                            .child(mono(l.addr.clone(), T_CAP, fg3())),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(T_CAP)
                            .line_height(px(16.))
                            .text_color(fg3())
                            .child(group_fingerprint(&l.fingerprint)),
                    )
                    .child(
                        div()
                            .text_size(T_CAP)
                            .text_color(fg3())
                            .child("链接带着对方的证书指纹：连上的不是它就不配对。"),
                    ),
            ),
            _ => None,
        };
        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(header("导入分享链接", ""))
            .child(field("分享链接", &self.link))
            .children(preview)
            .children(error_line(&self.error))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        pill("import-cancel", "取消", false)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        pill("import-go", "配对", true)
                            .on_click(cx.listener(|f, _, window, cx| f.go(window, cx))),
                    ),
            )
    }
}

/// 打开「导入分享链接」；`url` 是系统交给 app 的 `lolidesk://…`（先填好，等人点「配对」）。
pub fn open_import(
    model: Entity<Model>,
    url: Option<String>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ImportForm> {
    let form = cx.new(|cx| ImportForm::new(model, window, cx));
    dialogs::open_form(form.clone(), window, cx);
    let input = form.read(cx).link.clone();
    input.update(cx, |i, cx| {
        if let Some(u) = url {
            i.set_value(u, window, cx);
        }
        i.focus(window, cx)
    });
    form
}
