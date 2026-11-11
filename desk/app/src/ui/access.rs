//! 「访问」页：谁能连这台主机。本机是主人，看全部：等着同意的请求，经主人同意的设备，每个配对码
//! （谁发的、用了几次、过期、限制）和用它配对的设备，每台设备的最近登录、在线、限制，能吊销、能限制。
//! 别的主机上是「我在这台上的配对」：我自己和我发的分享链接。分组和说法同命令行的 `lolidesk host access`。
//!
//! 「新配对码」（[`NewCodeForm`]）：备注、过期、次数、只能看 → 码、分享链接（能复制）和链接的二维码。
//! 开着这一页时模型每 2 秒问一次主机。

use std::net::SocketAddr;

use gpui_kit::component::{
    WindowExt as _,
    input::{InputEvent, InputState},
    popover::Popover,
    spinner::Spinner,
};
use gpui_kit::{
    AnyElement, App, AppContext, ClipboardItem, Context, Div, Entity, FocusHandle, Focusable,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, prelude::FluentBuilder, px,
};
use lolidesk_proto::{AccessInfo, CodeInfo, GrantInfo, PendingInfo};

use crate::{
    assets::ic,
    model::{HostState, Model, NewCode, ToastKind},
    pairing::{self, EXPIRES, LIMITS, USES},
    theme::*,
    ui::{
        dialogs::{self, Form, error_line, field},
        network::qr,
        widgets::{
            Trigger, badge, caption, checkbox, dot, icon, icon_button, mono, pill, pill_busy,
            pill_sm, switch,
        },
    },
};

// ---------------------------------------------------------------- 页面

pub fn render(model: &Entity<Model>, addr: SocketAddr, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let Some(h) = m.host(addr) else {
        return div().into_any_element();
    };
    let now = pairing::now();
    let host_name = h
        .welcome
        .as_ref()
        .map(|w| w.host.clone())
        .unwrap_or_else(|| h.title());
    let can_code = h.can_share();
    let new_code = can_code.then(|| {
        let model = model.clone();
        pill(
            "acc-new",
            if h.is_owner() {
                "新配对码"
            } else {
                "分享这台…"
            },
            true,
        )
        .on_click(move |_, window, cx| {
            open_new_code(model.clone(), addr, window, cx);
        })
    });
    let bar = super::top_bar(
        "acc-bar",
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(icon(ic::SHIELD, 15., fg2()))
            .child(
                div()
                    .text_size(T_SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child("访问"),
            )
            .child(mono(
                if h.local {
                    format!("本机 · {host_name}")
                } else {
                    format!("{} · {}", h.title(), h.addr)
                },
                T_XS,
                fg3(),
            )),
        div().children(new_code),
    );

    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(28.))
        .max_w(px(880.))
        .pt(px(56.))
        .px(px(56.))
        .pb(px(56.));
    match (&h.access, h.online()) {
        (Some(info), _) => {
            body = body.child(headline(h, info, &host_name));
            body = body.children(sections(model, h, info, now));
        }
        (None, false) => {
            body = body.child(note_card(
                ic::WARNING,
                warn(),
                &format!("{} 没连上", h.title()),
                h.error.clone(),
            ))
        }
        (None, true) => {
            body = body.child(match &h.access_error {
                Some(e) => note_card(ic::WARNING, warn(), "问不到", e.clone()).into_any_element(),
                None => div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(Spinner::new().color(fg3()))
                    .child(div().text_size(T_SM).text_color(fg3()).child("正在问主机…"))
                    .into_any_element(),
            })
        }
    }

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(bar)
        .child(
            div()
                .id("acc-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

fn headline(h: &HostState, info: &AccessInfo, host_name: &str) -> Div {
    let online = info.grants.iter().filter(|g| g.online > 0).count();
    let (a, b) = if info.owner {
        let who = if h.local {
            "这台电脑".to_string()
        } else {
            host_name.to_string()
        };
        let a = if info.grants.is_empty() {
            format!("{who}还没配对给别的设备。")
        } else {
            format!("{who}配对给了 {} 台设备。", info.grants.len())
        };
        let b = match (info.pending.len(), online) {
            (0, 0) if info.grants.is_empty() => {
                "网里的设备点「连接」时这里会问你；也可以发一个配对码。".to_string()
            }
            (0, n) => format!("{n} 台现在连着。"),
            (p, n) => format!("{n} 台现在连着，{p} 个请求在等你同意。"),
        };
        (a, b)
    } else {
        let mine = info
            .codes
            .iter()
            .filter(|c| c.created_by.is_some() && c.created_by == info.me)
            .count();
        let brought = info.grants.iter().filter(|g| g.code.is_some()).count();
        (
            format!("我在 {host_name} 上。"),
            if mine == 0 {
                "还没发过分享链接。".to_string()
            } else {
                format!("我发了 {mine} 个分享链接，经它们进来 {brought} 台设备。")
            },
        )
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(T_2XL)
                .line_height(px(40.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(a),
        )
        .child(
            div()
                .text_size(T_2XL)
                .line_height(px(40.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg3())
                .child(b),
        )
}

fn sections(model: &Entity<Model>, h: &HostState, info: &AccessInfo, now: u64) -> Vec<AnyElement> {
    let addr = h.addr;
    let owner = info.owner;
    let mut out = Vec::new();
    if owner && !info.pending.is_empty() {
        out.push(
            section(
                "等你同意",
                Some(format!("{} 个", info.pending.len())),
                card().border_color(warn_line()).children(
                    info.pending
                        .iter()
                        .enumerate()
                        .map(|(i, p)| pending_row(model, i == 0, p, now)),
                ),
            )
            .into_any_element(),
        );
    }
    let g = pairing::group(info);
    let ctx = RowCtx {
        model,
        addr,
        info,
        now,
    };
    if !g.approved.is_empty() {
        out.push(
            section(
                "经主人同意",
                Some(format!("{} 台", g.approved.len())),
                card().children(
                    g.approved
                        .iter()
                        .enumerate()
                        .map(|(i, gr)| grant_row(&ctx, i == 0, gr)),
                ),
            )
            .into_any_element(),
        );
    }
    for (c, under) in &g.codes {
        out.push(code_section(&ctx, c, under).into_any_element());
    }
    if !g.orphans.is_empty() {
        out.push(
            section(
                "用配对码配对",
                None,
                card().children(
                    g.orphans
                        .iter()
                        .enumerate()
                        .map(|(i, gr)| grant_row(&ctx, i == 0, gr)),
                ),
            )
            .into_any_element(),
        );
    }
    if owner && info.grants.is_empty() && info.codes.is_empty() && info.pending.is_empty() {
        out.push(
            card()
                .child(
                    card_row(true).child(
                        div()
                            .text_size(T_SM)
                            .text_color(fg3())
                            .child("还没有设备配对。别的电脑在侧栏「网里的设备」里点「连接」，这里会弹框问你；或者点右上角「新配对码」。"),
                    ),
                )
                .into_any_element(),
        );
    }
    out.push(
        div()
            .px(px(4.))
            .text_size(T_CAP)
            .text_color(fg3())
            .child(if owner {
                "吊销一台设备：它马上断开，它发的分享链接和经那些链接进来的设备一起失效。吊销配对码：用它配对的设备一起失效。"
            } else {
                "我能吊销自己发的分享链接和经它们进来的设备。解除配对在侧栏这台主机的右键菜单里。"
            })
            .into_any_element(),
    );
    out
}

struct RowCtx<'a> {
    model: &'a Entity<Model>,
    addr: SocketAddr,
    info: &'a AccessInfo,
    now: u64,
}

/// 一节：小标题（右边一行等宽字）+ 内容。
fn section(title: impl Into<SharedString>, right: Option<String>, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(4.))
                .child(caption(title).flex_1())
                .children(right.map(|r| mono(r, T_CAP, fg3()))),
        )
        .child(body)
}

fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(R_LG)
        .bg(ink2())
        .border_1()
        .border_color(line1())
}

fn card_row(first: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(18.))
        .py(px(12.))
        .when(!first, |d| d.border_t_1().border_color(line1()))
}

fn note_card(path: &'static str, color: gpui_kit::Hsla, title: &str, text: String) -> Div {
    card().child(
        card_row(true)
            .items_start()
            .child(icon(path, 18., color))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .text_size(T_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child(title.to_string()),
                    )
                    .child(mono(text, T_CAP, fg3())),
            ),
    )
}

fn pending_row(model: &Entity<Model>, first: bool, p: &PendingInfo, now: u64) -> AnyElement {
    let id = p.id;
    let (m1, m2) = (model.clone(), model.clone());
    let kind = pairing::device_kind(&p.device);
    let sub = [kind, pairing::ip_of(&p.addr), pairing::ago(now, p.since)]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    card_row(first)
        .child(dot(warn(), 6.))
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
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(format!("{} 想连这台", p.device.name)),
                )
                .child(mono(sub, T_CAP, fg3()).truncate()),
        )
        .child(
            pill_sm(("acc-deny", id as usize), "拒绝", false)
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.decide(id, false, cx))),
        )
        .child(
            pill_sm(("acc-allow", id as usize), "同意", true)
                .on_click(move |_, _, cx| m2.update(cx, |m, cx| m.decide(id, true, cx))),
        )
        .into_any_element()
}

fn code_section(ctx: &RowCtx, c: &CodeInfo, under: &[&GrantInfo]) -> Div {
    let title = if c.label.is_empty() {
        "配对码".to_string()
    } else {
        format!("配对码「{}」", c.label)
    };
    let mine = ctx.info.owner || (c.created_by.is_some() && c.created_by == ctx.info.me);
    let revoke = mine.then(|| {
        let (model, addr, id) = (ctx.model.clone(), ctx.addr, c.id.clone());
        let note = pairing::revoke_note(ctx.info, &c.id);
        pill_sm(
            SharedString::from(format!("acc-rc-{}", c.id)),
            "吊销",
            false,
        )
        .text_color(bad())
        .on_click(move |_, window, cx| {
            let (model, id) = (model.clone(), id.clone());
            open_confirm(
                "吊销这个配对码？",
                note.clone(),
                "吊销",
                window,
                cx,
                move |cx| model.update(cx, |m, cx| m.revoke(addr, id.clone(), cx)),
            )
        })
    });
    let expired = c.expires.is_some_and(|t| t <= ctx.now);
    let used_up = c.max_uses.is_some_and(|n| c.uses >= n);
    let meta = card_row(true)
        .py(px(10.))
        .bg(ink3())
        .rounded_t(R_LG)
        .child(icon(
            ic::KEY,
            15.,
            if expired || used_up { fg4() } else { fg2() },
        ))
        .child(
            mono(pairing::code_line(ctx.info, c, ctx.now), T_CAP, fg2())
                .flex_1()
                .min_w_0()
                .truncate(),
        )
        .children(revoke);
    let rows: Vec<AnyElement> = if under.is_empty() {
        vec![
            card_row(false)
                .child(
                    div()
                        .text_size(T_XS)
                        .text_color(fg3())
                        .child("还没有设备用它配对"),
                )
                .into_any_element(),
        ]
    } else {
        under.iter().map(|g| grant_row(ctx, false, g)).collect()
    };
    section(title, Some(c.id.clone()), card().child(meta).children(rows))
}

fn grant_row(ctx: &RowCtx, first: bool, g: &GrantInfo) -> AnyElement {
    let me = ctx.info.me.as_deref() == Some(g.id.as_str());
    let online = g.online > 0;
    let (addr, model) = (ctx.addr, ctx.model.clone());
    // 设备能吊销自己和自己发的码带进来的；主人什么都能吊销
    let by_me = g
        .code
        .as_ref()
        .and_then(|c| ctx.info.codes.iter().find(|x| &x.id == c))
        .is_some_and(|c| c.created_by.is_some() && c.created_by == ctx.info.me);
    let can_revoke = ctx.info.owner || by_me;
    let revoke = (can_revoke && !me).then(|| {
        let id = g.id.clone();
        let note = pairing::revoke_note(ctx.info, &g.id);
        let name = g.device.name.clone();
        pill_sm(
            SharedString::from(format!("acc-rg-{}", g.id)),
            "吊销",
            false,
        )
        .text_color(bad())
        .on_click(move |_, window, cx| {
            let (model, id) = (model.clone(), id.clone());
            open_confirm(
                format!("吊销 {name}？"),
                note.clone(),
                "吊销",
                window,
                cx,
                move |cx| model.update(cx, |m, cx| m.revoke(addr, id.clone(), cx)),
            )
        })
    });
    let limit = ctx
        .info
        .owner
        .then(|| limit_popover(ctx.model.clone(), addr, g.id.clone()));
    card_row(first)
        .child(dot(if online { eye() } else { fg4() }, 6.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(T_SM)
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(g.device.name.clone()),
                        )
                        .when(me, |d| d.child(badge("我", eye(), eye_wash())))
                        .when(online, |d| d.child(mono("在线", T_CAP, eye())))
                        .when_some(g.allow, |d, a| {
                            d.child(badge(format!("只能：{}", a.summary()), warn(), warn_wash()))
                        }),
                )
                .child(mono(pairing::grant_line(g, ctx.now), T_CAP, fg3()).truncate()),
        )
        .children(limit)
        .children(revoke)
        .into_any_element()
}

/// 「限制」：五个开关和「恢复默认」，改了马上发给主机。
fn limit_popover(model: Entity<Model>, addr: SocketAddr, grant: String) -> AnyElement {
    let id = SharedString::from(format!("acc-lim-{grant}"));
    let trigger = Trigger::new(SharedString::from(format!("acc-limt-{grant}")))
        .open_bg(ink4())
        .flex()
        .items_center()
        .h(px(26.))
        .px(px(12.))
        .rounded(R_PILL)
        .bg(ink3())
        .border_1()
        .border_color(line2())
        .text_size(T_XS)
        .font_weight(FontWeight::MEDIUM)
        .hover(|s| s.bg(ink4()))
        .child("限制");
    Popover::new(id)
        .anchor(gpui_kit::Anchor::TopRight)
        .trigger(trigger)
        .content(move |_, _, cx| {
            let current = model
                .read(cx)
                .host(addr)
                .and_then(|h| h.access.as_ref())
                .and_then(|a| a.grants.iter().find(|g| g.id == grant))
                .map(|g| g.allow);
            let allow = current.flatten().unwrap_or_default();
            let limited = current.flatten().is_some();
            let rows = LIMITS.iter().enumerate().map(|(i, (what, label))| {
                let on = pairing::limit_value(&allow, what);
                let (model, grant) = (model.clone(), grant.clone());
                let what = *what;
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .h(px(32.))
                    .child(div().flex_1().text_size(T_SM).child(*label))
                    .child(switch(("lim-sw", i), on, false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            let mut a = allow;
                            let _ = a.set(what, !on);
                            let grant = grant.clone();
                            model.update(cx, |m, cx| m.limit(addr, grant, Some(a), cx));
                            cx.notify();
                        },
                    )))
            });
            let reset = {
                let (model, grant) = (model.clone(), grant.clone());
                div()
                    .id("lim-reset")
                    .text_size(T_XS)
                    .text_color(if limited { fg2() } else { fg4() })
                    .when(limited, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.text_color(fg()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let grant = grant.clone();
                                model.update(cx, |m, cx| m.limit(addr, grant, None, cx));
                                cx.notify();
                            }))
                    })
                    .child("恢复默认（和主机一样）")
            };
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .w(px(220.))
                .child(caption("这台设备能做什么").pb(px(4.)))
                .children(rows)
                .child(
                    div()
                        .mt(px(6.))
                        .pt(px(8.))
                        .border_t_1()
                        .border_color(line1())
                        .child(reset),
                )
        })
        .into_any_element()
}

// ---------------------------------------------------------------- 确认

/// 确认了要做的事
type OnOk = Box<dyn FnOnce(&mut App)>;

/// 确认一件改不回来的事（吊销）。
pub struct ConfirmForm {
    title: SharedString,
    text: String,
    action: &'static str,
    on_ok: Option<OnOk>,
    focus: FocusHandle,
}

impl Focusable for ConfirmForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ConfirmForm {
    fn ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.on_ok.take() {
            f(cx);
        }
        window.close_dialog(cx);
    }
}

impl Form for ConfirmForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ok(window, cx)
    }
}

impl Render for ConfirmForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(T_XS)
                            .text_color(fg2())
                            .child(self.text.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        pill("confirm-cancel", "取消", false)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        div()
                            .id("confirm-ok")
                            .flex()
                            .items_center()
                            .h(px(32.))
                            .px(px(16.))
                            .rounded(R_PILL)
                            .bg(bad())
                            .text_color(solid_ink())
                            .text_size(T_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .child(self.action)
                            .on_click(cx.listener(|f, _, window, cx| f.ok(window, cx))),
                    ),
            )
    }
}

pub fn open_confirm(
    title: impl Into<SharedString>,
    text: String,
    action: &'static str,
    window: &mut Window,
    cx: &mut App,
    on_ok: impl FnOnce(&mut App) + 'static,
) {
    let title = title.into();
    let form = cx.new(|cx| ConfirmForm {
        title,
        text,
        action,
        on_ok: Some(Box::new(on_ok)),
        focus: cx.focus_handle(),
    });
    dialogs::open_form_sized(form, 400., window, cx);
}

// ---------------------------------------------------------------- 新配对码

pub enum CodeState {
    Editing,
    Busy,
    Done(Box<NewCode>),
    Failed(String),
}

/// 「新配对码」/「分享这台…」：备注、过期、次数、只能看 → 码、分享链接和二维码。
pub struct NewCodeForm {
    model: Entity<Model>,
    addr: SocketAddr,
    title: String,
    pub label: Entity<InputState>,
    pub expires: usize,
    pub uses: usize,
    pub view_only: bool,
    pub state: CodeState,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for NewCodeForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl NewCodeForm {
    pub fn new(
        model: Entity<Model>,
        addr: SocketAddr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let h = model.read(cx).host(addr);
        let title = match h {
            Some(h) if h.local => "新配对码".to_string(),
            Some(h) if h.is_owner() => format!("给 {} 发配对码", h.title()),
            Some(h) => format!("分享 {}", h.title()),
            None => "新配对码".to_string(),
        };
        let label =
            cx.new(|cx| InputState::new(window, cx).placeholder("给谁、干什么用（可以不填）"));
        let subs = vec![
            cx.subscribe_in(&label, window, |this, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.create(cx);
                }
            }),
        ];
        Self {
            model,
            addr,
            title,
            label,
            expires: 3,
            uses: 0,
            view_only: false,
            state: CodeState::Editing,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    /// 问主机要配对码。
    pub fn create(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, CodeState::Busy | CodeState::Done(_)) {
            return;
        }
        self.state = CodeState::Busy;
        let label = self.label.read(cx).value().trim().to_string();
        let task = Model::create_code(
            &self.model,
            self.addr,
            label,
            EXPIRES[self.expires].0,
            USES[self.uses].0,
            self.view_only,
            cx,
        );
        let (model, addr) = (self.model.clone(), self.addr);
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |f, cx| {
                f.state = match r {
                    Ok(c) => CodeState::Done(Box::new(c)),
                    Err(e) => CodeState::Failed(crate::runtime::error_text(&e)),
                };
                cx.notify();
            });
            model.update(cx, |m, cx| m.refresh_access(addr, cx));
        })
        .detach();
        cx.notify();
    }

    /// 快照用：直接显示发好的码。
    pub fn show_code(&mut self, code: NewCode, cx: &mut Context<Self>) {
        self.state = CodeState::Done(Box::new(code));
        cx.notify();
    }

    fn copy(&self, text: String, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.model
            .update(cx, |m, cx| m.toast(ToastKind::Success, "已复制", cx));
    }

    fn editing(&self, cx: &mut Context<Self>) -> Div {
        let seg =
            |key: &'static str, opts: Vec<&'static str>, cur: usize, set: fn(&mut Self, usize)| {
                div()
                    .flex()
                    .p(px(3.))
                    .gap(px(2.))
                    .rounded(R_PILL)
                    .bg(ink1())
                    .border_1()
                    .border_color(line2())
                    .children(opts.into_iter().enumerate().map(|(i, label)| {
                        let on = i == cur;
                        div()
                            .id((key, i))
                            .flex()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .h(px(28.))
                            .rounded(R_PILL)
                            .text_size(T_XS)
                            .font_weight(FontWeight::MEDIUM)
                            .when(on, |d| d.bg(ink4()).text_color(fg()))
                            .when(!on, |d| {
                                d.text_color(fg3())
                                    .cursor_pointer()
                                    .hover(|s| s.text_color(fg2()))
                            })
                            .child(label)
                            .on_click(cx.listener(move |f, _, _, cx| {
                                set(f, i);
                                cx.notify()
                            }))
                    }))
            };
        let labeled = |label: &'static str, el: Div| {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(caption(label))
                .child(el)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(field("备注", &self.label))
            .child(labeled(
                "多久后不能再用",
                seg(
                    "code-exp",
                    EXPIRES.iter().map(|e| e.1).collect(),
                    self.expires,
                    |f, i| f.expires = i,
                ),
            ))
            .child(labeled(
                "最多几台设备用它",
                seg(
                    "code-uses",
                    USES.iter().map(|u| u.1).collect(),
                    self.uses,
                    |f, i| f.uses = i,
                ),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().flex().child(
                        checkbox("code-view", self.view_only, "只能看").on_click(cx.listener(
                            |f, _, _, cx| {
                                f.view_only = !f.view_only;
                                cx.notify()
                            },
                        )),
                    ))
                    .child(div().pl(px(24.)).text_size(T_CAP).text_color(fg3()).child(
                        "用它配对的设备能看终端和桌面，不能操作键鼠、不能给 agent 用、不能再分享。",
                    )),
            )
            .child(
                div()
                    .text_size(T_CAP)
                    .text_color(fg3())
                    .child("过期和次数只管新的配对：已经配对上的设备不受影响，要断开就吊销。"),
            )
    }

    fn done(&self, c: &NewCode, cx: &mut Context<Self>) -> Div {
        let url = c.link.to_url();
        let mut notes = vec![match c.info.expires {
            Some(t) => pairing::left(pairing::now(), t),
            None => "不过期".into(),
        }];
        if let Some(m) = c.info.max_uses {
            notes.push(format!("最多 {m} 台"));
        }
        if let Some(a) = c.info.allow {
            notes.push(format!("能：{}", a.summary()));
        }
        let code_text = c.secret.clone();
        let link_text = url.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .gap(px(18.))
                    .child(qr(&url, 172.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(caption("配对码"))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .child(
                                                div()
                                                    .font_family(MONO)
                                                    .text_size(px(22.))
                                                    .line_height(px(30.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(eye())
                                                    .child(c.secret.clone()),
                                            )
                                            .child(icon_button("code-copy", ic::COPY, "复制配对码").on_click(
                                                cx.listener(move |f, _, _, cx| f.copy(code_text.clone(), cx)),
                                            )),
                                    )
                                    .child(mono(
                                        format!("{} · {}", c.info.id, notes.join(" · ")),
                                        T_CAP,
                                        fg3(),
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(T_XS)
                                    .text_color(fg2())
                                    .child("另一台电脑在 Lolidesk 里「＋ → 导入分享链接」，或者跑 lolidesk pair <链接>。也可以在它的「添加主机」里填这台的地址和配对码。"),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(caption("分享链接"))
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(8.))
                            .pl(px(12.))
                            .pr(px(6.))
                            .py(px(6.))
                            .rounded(R_SM)
                            .bg(ink1())
                            .border_1()
                            .border_color(line2())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .py(px(4.))
                                    .font_family(MONO)
                                    .text_size(T_XS)
                                    .line_height(px(18.))
                                    .child(url),
                            )
                            .child(icon_button("link-copy", ic::COPY, "复制链接").on_click(
                                cx.listener(move |f, _, _, cx| f.copy(link_text.clone(), cx)),
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(dot(warn(), 5.))
                    .child(
                        div()
                            .text_size(T_XS)
                            .text_color(fg2())
                            .child("码只显示这一次。不想要了就在「访问」里吊销它，用它配对的设备一起失效。"),
                    ),
            )
    }
}

impl Form for NewCodeForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.state {
            CodeState::Done(_) => window.close_dialog(cx),
            _ => self.create(cx),
        }
    }
}

impl Render for NewCodeForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sub = "别的电脑用配对码或分享链接配对，不用你再点同意。";
        let (body, footer): (AnyElement, AnyElement) = match &self.state {
            CodeState::Done(c) => (
                self.done(c, cx).into_any_element(),
                div()
                    .flex()
                    .justify_end()
                    .child(
                        pill("code-close", "完成", true)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .into_any_element(),
            ),
            state => {
                let busy = matches!(state, CodeState::Busy);
                let err = match state {
                    CodeState::Failed(e) => Some(e.clone()),
                    _ => None,
                };
                (
                    self.editing(cx)
                        .children(error_line(&err))
                        .into_any_element(),
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            pill("code-cancel", "取消", false)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(if busy {
                            pill_busy("code-go", "正在发…", true).into_any_element()
                        } else {
                            pill("code-go", "发配对码", true)
                                .on_click(cx.listener(|f, _, _, cx| {
                                    if matches!(f.state, CodeState::Failed(_)) {
                                        f.state = CodeState::Editing;
                                    }
                                    f.create(cx)
                                }))
                                .into_any_element()
                        })
                        .into_any_element(),
                )
            }
        };
        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(header_owned(self.title.clone(), sub))
            .child(body)
            .child(footer)
    }
}

fn header_owned(title: String, sub: &'static str) -> Div {
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
        .child(div().text_size(T_XS).text_color(fg2()).child(sub))
}

/// 打开「新配对码」/「分享这台…」。
pub fn open_new_code(
    model: Entity<Model>,
    addr: SocketAddr,
    window: &mut Window,
    cx: &mut App,
) -> Entity<NewCodeForm> {
    let form = cx.new(|cx| NewCodeForm::new(model, addr, window, cx));
    dialogs::open_form_sized(form.clone(), 540., window, cx);
    form
}
