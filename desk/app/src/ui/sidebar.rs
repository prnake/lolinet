//! 侧栏：字标和「＋」（添加主机、导入分享链接）、最上面固定的「网络」、主机树（最上面是本机：访问、会话、新建、显示器、窗口；
//! 别的主机：权限引导、会话、新建、显示器、窗口）、网里的设备（点「连接」请对方同意）、底部的在线数和版本。
//! 窗口（照 Vibe42）：主机上开着的窗口，点一个远程桌面就只推它（已经开着就在流上换过去）；正在推的标绿点。
//! 缓存的视图：只在模型变了（`observe`）时重画，画面一帧一帧来时不动。

use std::net::SocketAddr;

use gpui_kit::component::{
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, Context, Entity, EntityId, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, prelude::FluentBuilder, px,
};
use lolidesk_proto::{DeskTarget, DisplayInfo, Launcher, SessionInfo, WindowInfo};

use crate::{
    assets::ic,
    labels::{display_name, is_agent, rtt_label, session_name, session_subtitle, window_label},
    model::{HostState, Link, LocalState, Model, Selection, WindowList},
    service::Found,
    term::TermStart,
    theme::*,
    ui::{
        access, dialogs, pair,
        widgets::{
            Trigger, badge, caption, dot, drag_region, hover_icon, icon, mono, pill_sm, side_row,
        },
    },
};

pub struct Sidebar {
    model: Entity<Model>,
    _sub: Subscription,
}

impl Sidebar {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&model, |_, _, cx| cx.notify());
        Self { model, _sub: sub }
    }
}

/// 这个窗口里开着的一个终端（侧栏要知道哪些会话开着、哪个选中）。
struct OpenTerm {
    view: EntityId,
    host: SocketAddr,
    id: Option<u32>,
    name: String,
    ended: bool,
}

/// 本机主机没连着时的说法（平常的连接中、重连照旧）。
pub fn local_status(state: &LocalState) -> Option<(String, gpui_kit::Hsla)> {
    match state {
        LocalState::Normal => None,
        LocalState::Starting => Some(("开启中".into(), warn())),
        LocalState::Off => Some(("关着".into(), fg4())),
        LocalState::NotRunning(_) => Some(("没在跑".into(), fg4())),
        LocalState::Failed(_) => Some(("开不了".into(), bad())),
    }
}

/// 本机没在跑时给人看的一句话。
pub fn local_text(state: &LocalState) -> String {
    match state {
        LocalState::Normal => String::new(),
        LocalState::Starting => "正在开启本机主机…".into(),
        LocalState::Off => "关着：网里的设备连不了这台，本机也开不了终端".into(),
        LocalState::NotRunning(t) | LocalState::Failed(t) => t.clone(),
    }
}

/// 连接状态的文字和颜色（本机走回环，不显示延迟）。
pub fn link_status(h: &HostState) -> (String, gpui_kit::Hsla) {
    match h.link {
        Link::Online if h.local => ("在线".into(), eye()),
        Link::Online => (rtt_label(h.rtt_ms), eye()),
        Link::Connecting | Link::Reconnecting => ("连接中".into(), warn()),
        Link::Failed => ("连不上".into(), bad()),
        Link::Offline => ("离线".into(), fg4()),
    }
}

/// 重连的说明：「第 3 次重连 · 4 秒后」。
pub fn retry_note(h: &HostState) -> Option<String> {
    if h.retry.attempt == 0 {
        return None;
    }
    let n = h.retry.attempt;
    Some(
        match (h.link, h.retry.wait_secs(std::time::Instant::now())) {
            (Link::Reconnecting, Some(s)) if s > 0 => format!("第 {n} 次重连 · {s} 秒后"),
            _ => format!("第 {n} 次重连…"),
        },
    )
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.clone();
        let m = self.model.read(cx);
        let open: Vec<OpenTerm> = m
            .terms
            .iter()
            .map(|t| {
                let v = t.read(cx);
                OpenTerm {
                    view: t.entity_id(),
                    host: v.host(cx),
                    id: v.session_id(cx),
                    name: {
                        let t = v.title(cx);
                        if t.trim().is_empty() { v.name(cx) } else { t }
                    },
                    ended: v.is_ended(cx),
                }
            })
            .collect();
        let desk = m.desk.as_ref().map(|d| {
            let d = d.read(cx);
            DeskNow {
                host: d.host,
                target: d.target,
                live: !d.is_stopped(),
                pending: d.pending.as_ref().map(|p| p.target),
            }
        });
        let stage_focus = m
            .desk
            .as_ref()
            .map(|d| d.read(cx).stage.read(cx).focus.clone());
        let ctx = TreeCtx {
            model: &model,
            selection: m.selection,
            open: &open,
            desk,
            local: m.local.as_ref().map(|l| l.state.clone()),
            local_pref: m.prefs.local_host,
        };
        let trees: Vec<AnyElement> = m
            .hosts
            .iter()
            .enumerate()
            .map(|(i, h)| host_tree(i, h, &ctx))
            .collect();
        let found: Vec<AnyElement> = m
            .found
            .iter()
            .enumerate()
            .map(|(i, f)| found_row(i, f, &model))
            .collect();
        let (online, total) = (m.online_count(), m.hosts.len());
        let net_row = {
            let model = model.clone();
            let selected = m.selection == Selection::Network;
            let (text, color) = crate::ui::network::summary(m.net.status.as_ref());
            side_row("side-net", selected)
                .h(px(34.))
                .child(icon(ic::NETWORK, 15., if selected { fg() } else { fg2() }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child("网络"),
                )
                .when(!text.is_empty(), |r| {
                    r.child(mono(text, T_CAP, fg3()).flex_none())
                })
                .child(dot(color, 6.))
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| m.select(Selection::Network, cx))
                })
        };

        let add = {
            let model = model.clone();
            Trigger::new("side-add")
                .tip("添加主机、导入分享链接")
                .open_bg(ink4())
                .group("side-add")
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(R_SM)
                .hover(|s| s.bg(ink3()))
                .child(hover_icon("side-add", ic::PLUS, 16., fg2(), fg()))
                .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                    let (m1, m2) = (model.clone(), model.clone());
                    menu.item(
                        PopupMenuItem::new("添加主机…").on_click(move |_, window, cx| {
                            dialogs::open_add_host(m1.clone(), window, cx)
                        }),
                    )
                    .item(
                        PopupMenuItem::new("导入分享链接…").on_click(move |_, window, cx| {
                            pair::open_import(m2.clone(), None, window, cx);
                        }),
                    )
                })
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(ink1())
            .border_r_1()
            .border_color(line1())
            // 点侧栏：键盘从远程画面拿回来
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(f) = &stage_focus
                    && f.is_focused(window)
                {
                    window.blur(cx);
                }
            })
            // 顶上一条：红绿灯右边放字标，整条可以拖窗口
            .child(drag_region(
                div()
                    .id("side-head")
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(BAR_H)
                    .pl(px(86.))
                    .pr(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .font_family(MONO)
                            .text_size(T_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child("lolidesk"),
                    )
                    .child(add),
            ))
            .child(
                div()
                    .id("side-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .px(px(10.))
                            .pt(px(4.))
                            .pb(px(16.))
                            .gap(px(2.))
                            .child(net_row)
                            .child(
                                div()
                                    .px(px(8.))
                                    .pt(px(10.))
                                    .pb(px(6.))
                                    .child(caption("主机")),
                            )
                            .children(trees)
                            .when(!found.is_empty(), |s| {
                                s.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .px(px(8.))
                                        .pt(px(14.))
                                        .pb(px(6.))
                                        .child(caption("网里的设备").flex_1())
                                        .child(mono(format!("{} 台", found.len()), T_CAP, fg4())),
                                )
                                .children(found)
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(8.))
                    .px(px(18.))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(line1())
                    .child(dot(if online > 0 { eye() } else { fg4() }, 6.))
                    .child(mono(format!("{online} / {total}"), T_CAP, fg3()).flex_1())
                    .child(mono(crate::VERSION, T_CAP, fg4())),
            )
    }
}

/// 开着的远程桌面推的是什么（侧栏标出哪一行在推、哪一行正在换过去）
#[derive(Clone, Copy)]
struct DeskNow {
    host: SocketAddr,
    target: DeskTarget,
    live: bool,
    pending: Option<DeskTarget>,
}

impl DeskNow {
    fn streaming(&self, host: SocketAddr, t: DeskTarget) -> bool {
        self.host == host && self.live && self.pending.is_none() && self.target == t
    }

    fn opening(&self, host: SocketAddr, t: DeskTarget) -> bool {
        self.host == host && self.live && self.pending == Some(t)
    }
}

struct TreeCtx<'a> {
    model: &'a Entity<Model>,
    selection: Selection,
    open: &'a [OpenTerm],
    desk: Option<DeskNow>,
    /// 本机主机怎么样了；偏好「让网里的设备能连这台」
    local: Option<LocalState>,
    local_pref: bool,
}

/// 一台主机和它下面的会话、新建、显示器。
fn host_tree(ix: usize, h: &HostState, ctx: &TreeCtx) -> AnyElement {
    let addr = h.addr;
    let local_state = ctx.local.clone().filter(|_| h.local);
    let (status, color) = local_state
        .as_ref()
        .filter(|_| !h.online())
        .and_then(local_status)
        .unwrap_or_else(|| link_status(h));
    let chevron = if h.expanded {
        ic::CHEVRON_DOWN
    } else {
        ic::CHEVRON_RIGHT
    };
    let online = h.online();
    let link = h.link;

    let local = h.local;
    let menu_info = MenuInfo {
        addr,
        link,
        local,
        can_share: h.can_share(),
        paired: h.me().is_some(),
        devices: h.paired_count(),
        local_pref: ctx.local_pref,
    };
    let row = {
        let model = ctx.model.clone();
        let m2 = ctx.model.clone();
        let title = if local {
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_baseline()
                .gap(px(6.))
                .child(
                    div()
                        .flex_none()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child("本机"),
                )
                .child(mono(h.name.clone(), T_CAP, fg3()).truncate())
        } else {
            div()
                .flex_1()
                .min_w_0()
                .font_family(MONO)
                .text_size(T_SM)
                .font_weight(FontWeight::MEDIUM)
                .truncate()
                .child(h.title())
        };
        // 远程桌面有问题（列不出显示器、缺权限）：一个琥珀色的屏幕图标，原因在提示里
        let desk_problem: Option<SharedString> = online
            .then(|| {
                h.displays_error.clone().or_else(|| {
                    (h.perms.is_some() && h.guide_steps() > 0).then(|| "远程桌面缺权限".to_string())
                })
            })
            .flatten()
            .map(Into::into);
        // 没连上：出错原因、第几次重连、几秒后，都在状态的提示里
        let status_tip: Option<SharedString> = if let Some(st) = local_state
            .as_ref()
            .filter(|s| !online && **s != LocalState::Normal)
        {
            Some(local_text(st))
        } else if !online {
            let note = retry_note(h);
            match (h.error.is_empty(), note) {
                (true, None) => None,
                (true, Some(n)) => Some(n),
                (false, None) => Some(h.error.clone()),
                (false, Some(n)) => Some(format!("{} · {n}", h.error)),
            }
        } else {
            None
        }
        .filter(|t| !t.is_empty())
        .map(Into::into);
        side_row(("host", ix), false)
            .h(px(34.))
            .child(icon(chevron, 12., fg3()))
            .child(icon(ic::COMPUTER, 15., if local { fg() } else { fg2() }))
            .child(title)
            .when_some(desk_problem, |r, tip| {
                r.child(
                    div()
                        .id(("host-warn", ix))
                        .flex_none()
                        .child(icon(ic::MONITOR, 13., warn()))
                        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
                )
            })
            .child(
                div()
                    .id(("host-st", ix))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .child(mono(status, T_CAP, fg3()))
                    .child(dot(color, 6.))
                    .when_some(status_tip, |d, tip| {
                        d.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                    }),
            )
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| match link {
                    Link::Online => m.toggle_expanded(addr, cx),
                    Link::Connecting => {}
                    Link::Offline | Link::Failed | Link::Reconnecting => {
                        if let Some(h) = m.host_mut(addr) {
                            h.expanded = true;
                        }
                        if local {
                            m.retry_local(cx);
                        } else {
                            m.connect_now(addr, cx);
                        }
                    }
                })
            })
            .context_menu(move |menu, _, _| host_menu(menu, &m2, &menu_info))
    };

    let mut tree = div().flex().flex_col().child(row);
    if !h.expanded {
        return tree.into_any_element();
    }
    let mut body = div().flex().flex_col().pl(px(14.)).pb(px(6.)).gap(px(1.));
    if let Some(st) = local_state
        .as_ref()
        .filter(|s| !online && **s != LocalState::Normal)
    {
        body = body.children(local_card(st, ctx));
    }
    // 远程桌面还缺权限：引导卡片放在最上面
    let guide = online
        .then(|| crate::ui::permissions::card(h, ctx.model, false))
        .flatten();
    let guiding = guide.is_some();
    body = body.children(guide);
    // 本机：谁能连这台（配对了几台、几个请求在等）
    if local && online {
        body = body.child(access_row(h, ctx));
    }
    // 重连时会话还列着（开着的终端在等），新建和显示器要连上才有
    if online || h.reconnecting() {
        let name = h.title();
        for s in &h.sessions {
            body = body.child(session_row(h, &name, s, ctx));
        }
        // 这个窗口里开着、但主机列表里没有的：新开还没拿到编号的、已经结束的
        for t in ctx.open.iter().filter(|t| {
            t.host == addr && !t.id.is_some_and(|id| h.sessions.iter().any(|s| s.id == id))
        }) {
            body = body.child(local_row(t, ctx));
        }
    }
    if online {
        body = body.child(new_row(ix, addr, &h.launchers, ctx));
        if h.has_desktop() {
            for d in &h.displays {
                body = body.child(display_row(addr, d, ctx));
            }
            // 窗口（照 Vibe42）：点一个只推那个窗口
            if h.wants_windows() {
                body = body.children(windows_section(h, ctx, guiding));
            }
        }
    }
    tree = tree.child(body);
    tree.into_any_element()
}

/// 「窗口」和它下面的窗口。读取中、读不了、主机太旧都在标题行右边（图标），原因在提示里。
fn windows_section(h: &HostState, ctx: &TreeCtx, guiding: bool) -> Vec<AnyElement> {
    let mut out = vec![windows_header(
        h,
        ctx,
        guiding || h.displays_error.is_some(),
    )];
    if h.show_windows && matches!(h.windows_state, WindowList::Ready) {
        out.extend(h.windows.iter().map(|w| window_row(h.addr, w, ctx)));
    }
    out
}

/// 右键菜单要知道的。
#[derive(Clone, Copy)]
struct MenuInfo {
    addr: SocketAddr,
    link: Link,
    local: bool,
    can_share: bool,
    /// 我是这台上配对过的设备（能解除配对）
    paired: bool,
    devices: Option<usize>,
    local_pref: bool,
}

/// 本机的右键菜单：访问、新配对码、刷新 / 重试、「让网里的设备能连这台」。
fn local_menu(menu: PopupMenu, model: &Entity<Model>, i: &MenuInfo) -> PopupMenu {
    let addr = i.addr;
    let (m1, m2, m3, m4) = (model.clone(), model.clone(), model.clone(), model.clone());
    let online = i.link == Link::Online;
    let access = match i.devices {
        Some(n) => format!("访问 · {n} 台设备"),
        None => "访问".into(),
    };
    let menu = menu
        .item(
            PopupMenuItem::new(access)
                .disabled(!online)
                .on_click(move |_, _, cx| {
                    m1.update(cx, |m, cx| m.select(Selection::Access(addr), cx))
                }),
        )
        .item(
            PopupMenuItem::new("新配对码…")
                .disabled(!i.can_share)
                .on_click(move |_, window, cx| {
                    access::open_new_code(m2.clone(), addr, window, cx);
                }),
        )
        .separator();
    let menu = if online {
        menu.item(
            PopupMenuItem::new("刷新")
                .on_click(move |_, _, cx| m3.update(cx, |m, cx| m.refresh(addr, true, cx))),
        )
    } else {
        menu.item(
            PopupMenuItem::new("重试")
                .disabled(i.link == Link::Connecting)
                .on_click(move |_, _, cx| m3.update(cx, |m, cx| m.retry_local(cx))),
        )
    };
    let on = i.local_pref;
    menu.separator().item(
        PopupMenuItem::new("让网里的设备能连这台")
            .checked(on)
            .on_click(move |_, _, cx| m4.update(cx, |m, cx| m.set_local_host(!on, cx))),
    )
}

fn host_menu(menu: PopupMenu, model: &Entity<Model>, i: &MenuInfo) -> PopupMenu {
    if i.local {
        return local_menu(menu, model, i);
    }
    let (addr, link) = (i.addr, i.link);
    let (m1, m2, m3) = (model.clone(), model.clone(), model.clone());
    let (m4, m5, m6) = (model.clone(), model.clone(), model.clone());
    let menu = if link == Link::Online {
        menu.item(
            PopupMenuItem::new("刷新")
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.refresh(addr, true, cx))),
        )
        .item(
            PopupMenuItem::new("断开")
                .on_click(move |_, _, cx| m2.update(cx, |m, cx| m.disconnect(addr, cx))),
        )
    } else if link == Link::Reconnecting || link == Link::Connecting {
        menu.item(
            PopupMenuItem::new("现在重连")
                .disabled(link == Link::Connecting)
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.connect_now(addr, cx))),
        )
        .item(
            PopupMenuItem::new("停止重连")
                .on_click(move |_, _, cx| m2.update(cx, |m, cx| m.disconnect(addr, cx))),
        )
    } else {
        menu.item(
            PopupMenuItem::new("连接")
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.connect_now(addr, cx))),
        )
    };
    let online = link == Link::Online;
    let menu = menu
        .separator()
        .item(
            PopupMenuItem::new("分享这台…")
                .disabled(!i.can_share)
                .on_click(move |_, window, cx| {
                    access::open_new_code(m4.clone(), addr, window, cx);
                }),
        )
        .item(
            PopupMenuItem::new("我在这台上的配对")
                .disabled(!online)
                .on_click(move |_, _, cx| {
                    m5.update(cx, |m, cx| m.select(Selection::Access(addr), cx))
                }),
        )
        .separator();
    let menu = if i.paired {
        menu.item(
            danger_item("解除配对…")
                .disabled(!online)
                .on_click(move |_, window, cx| {
                    let m6 = m6.clone();
                    access::open_confirm(
                        "解除配对？",
                        "在对方那里吊销这台电脑的配对、它发过的分享链接，再忘掉对方。以后要连得重新配对。".into(),
                        "解除配对",
                        window,
                        cx,
                        move |cx| m6.update(cx, |m, cx| m.unpair(addr, cx)),
                    )
                }),
        )
    } else {
        menu
    };
    menu.item(
        danger_item("忘掉这台主机")
            .on_click(move |_, _, cx| m3.update(cx, |m, cx| m.forget(addr, cx))),
    )
}

/// 本机没在跑时的说明：正在开、开不了（重试）、关着（开启）。
fn local_card(st: &LocalState, ctx: &TreeCtx) -> Option<AnyElement> {
    let label = match st {
        LocalState::Normal | LocalState::Starting => return None,
        LocalState::Off => "开启",
        LocalState::NotRunning(_) | LocalState::Failed(_) => "重试",
    };
    let model = ctx.model.clone();
    let enable = *st == LocalState::Off;
    Some(
        div()
            .flex()
            .px(px(10.))
            .py(px(4.))
            .child(
                pill_sm("local-retry", label, false).on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| {
                        if enable {
                            m.set_local_host(true, cx)
                        } else {
                            m.retry_local(cx)
                        }
                    })
                }),
            )
            .into_any_element(),
    )
}

/// 本机下面的「访问」：配对了几台设备，几个请求在等。
fn access_row(h: &HostState, ctx: &TreeCtx) -> AnyElement {
    let addr = h.addr;
    let selected = ctx.selection == Selection::Access(addr);
    let model = ctx.model.clone();
    let pending = h.access.as_ref().map_or(0, |a| a.pending.len());
    let devices = h.paired_count();
    let tip: SharedString = match (devices, pending) {
        (Some(n), 0) => format!("配对了 {n} 台设备"),
        (Some(n), p) => format!("配对了 {n} 台设备 · {p} 个请求在等"),
        (None, p) => format!("{p} 个请求在等"),
    }
    .into();
    side_row("local-access", selected)
        .child(icon(ic::SHIELD, 14., if selected { fg() } else { fg2() }))
        .child(div().flex_1().min_w_0().text_size(T_SM).child("访问"))
        .when(pending > 0, |r| {
            r.child(badge(pending.to_string(), warn(), warn_wash()))
        })
        .children(devices.map(|n| mono(n.to_string(), T_CAP, fg3()).flex_none()))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.select(Selection::Access(addr), cx)))
        .into_any_element()
}

/// 网里一台有 lolidesk 主机、还没配对的设备：「连接」请它同意。
fn found_row(ix: usize, f: &Found, model: &Entity<Model>) -> AnyElement {
    let (addr, fp, name) = (f.addr, f.fingerprint, f.name.clone());
    let model = model.clone();
    let connect = pill_sm(("found-go", ix), "连接", false).on_click(move |_, window, cx| {
        cx.stop_propagation();
        pair::open_pairing(model.clone(), addr, fp, name.clone(), None, window, cx);
    });
    div()
        .id(("found", ix))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(44.))
        .pl(px(10.))
        .pr(px(6.))
        .rounded(R_SM)
        .hover(|s| s.bg(ink2()))
        .child(icon(ic::COMPUTER, 15., fg3()))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .font_family(MONO)
                        .text_size(T_SM)
                        .text_color(fg2())
                        .truncate()
                        .child(if f.name.is_empty() {
                            f.addr.ip().to_string()
                        } else {
                            f.name.clone()
                        }),
                )
                .child(mono(f.addr.ip().to_string(), T_CAP, fg4())),
        )
        .child(connect)
        .into_any_element()
}

/// 红字的菜单项（删、结束）。
pub fn danger_item(label: &'static str) -> PopupMenuItem {
    PopupMenuItem::element(move |_, _| div().text_size(T_SM).text_color(bad()).child(label))
}

fn session_row(h: &HostState, host_name: &str, s: &SessionInfo, ctx: &TreeCtx) -> AnyElement {
    let addr = h.addr;
    let id = s.id;
    let open = ctx
        .open
        .iter()
        .find(|t| t.host == addr && t.id == Some(id) && !t.ended);
    let selected = open.is_some_and(|t| ctx.selection == Selection::Term(t.view));
    let name = session_name(&h.launchers, &s.command);
    let mut sub = session_subtitle(s, host_name);
    let exited = s.exited;
    if let Some(code) = exited {
        sub = if sub.is_empty() {
            format!("已退出（{code}）")
        } else {
            format!("已退出（{code}）· {sub}")
        };
    }
    let path = if is_agent(&s.command) {
        ic::SPARKLE
    } else {
        ic::TERMINAL
    };
    let color = if open.is_some() {
        eye()
    } else if exited.is_some() {
        fg4()
    } else {
        fg2()
    };
    let tip: SharedString = format!(
        "#{} · {}×{} · {} 人在看{}\n{}",
        s.id,
        s.cols,
        s.rows,
        s.viewers,
        if s.restored {
            " · 主机重启后恢复"
        } else {
            ""
        },
        s.cwd
    )
    .into();
    let has_open = open.is_some();
    let (m1, m2) = (ctx.model.clone(), ctx.model.clone());
    side_row(SharedString::from(format!("sess-{addr}-{id}")), selected)
        .h(px(44.))
        .child(icon(path, 15., color))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(T_SM)
                        .when(exited.is_some(), |d| d.text_color(fg2()))
                        .truncate()
                        .child(name),
                )
                .when(!sub.is_empty(), |d| {
                    d.child(mono(sub, T_CAP, fg3()).truncate())
                }),
        )
        .child(mono(format!("#{id}"), T_CAP, fg4()).flex_none())
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, _, cx| {
            m1.update(cx, |m, cx| m.open_term(addr, TermStart::Attach { id }, cx))
        })
        .context_menu(move |menu, _, _| {
            let (a, b, c) = (m2.clone(), m2.clone(), m2.clone());
            let menu = menu.item(PopupMenuItem::new("打开").on_click(move |_, _, cx| {
                a.update(cx, |m, cx| m.open_term(addr, TermStart::Attach { id }, cx))
            }));
            let menu = if has_open {
                menu.item(
                    PopupMenuItem::new("断开（会话留在主机上）").on_click(move |_, _, cx| {
                        b.update(cx, |m, cx| {
                            let views: Vec<EntityId> = m
                                .terms
                                .iter()
                                .filter(|t| {
                                    let v = t.read(cx);
                                    v.host(cx) == addr && v.session_id(cx) == Some(id)
                                })
                                .map(|t| t.entity_id())
                                .collect();
                            for v in views {
                                m.close_term(v, cx);
                            }
                        })
                    }),
                )
            } else {
                menu
            };
            menu.separator().item(
                danger_item("结束会话")
                    .on_click(move |_, _, cx| c.update(cx, |m, cx| m.kill_session(addr, id, cx))),
            )
        })
        .into_any_element()
}

/// 这个窗口里开着、主机列表里还没有（新开的）或已经没有（结束了）的终端。
fn local_row(t: &OpenTerm, ctx: &TreeCtx) -> AnyElement {
    let selected = ctx.selection == Selection::Term(t.view);
    let view = t.view;
    let model = ctx.model.clone();
    let pending = t.id.is_none() && !t.ended;
    let label = if pending {
        "正在打开…".to_string()
    } else {
        "已结束".to_string()
    };
    side_row(("local", view.as_u64() as usize), selected)
        .h(px(44.))
        .child(if pending {
            Spinner::new().color(fg3()).into_any_element()
        } else {
            icon(ic::TERMINAL, 15., fg4()).into_any_element()
        })
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(T_SM)
                        .text_color(fg2())
                        .truncate()
                        .child(t.name.clone()),
                )
                .child(mono(label, T_CAP, fg4())),
        )
        .when_some(t.id, |r, id| {
            r.child(mono(format!("#{id}"), T_CAP, fg4()).flex_none())
        })
        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.select(Selection::Term(view), cx)))
        .into_any_element()
}

/// 「新建」：菜单里是终端和主机上找得到的 agent，最后是自定义命令。
fn new_row(ix: usize, addr: SocketAddr, launchers: &[Launcher], ctx: &TreeCtx) -> AnyElement {
    let model = ctx.model.clone();
    let mut launchers = launchers.to_vec();
    if launchers.is_empty() {
        launchers.push(Launcher {
            id: "shell".into(),
            name: "终端".into(),
            command: String::new(),
        });
    }
    Trigger::new(("new", ix))
        .open_bg(ink3())
        .group(SharedString::from(format!("new-{ix}")))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .px(px(10.))
        .rounded(R_SM)
        .hover(|s| s.bg(ink2()))
        .child(icon(ic::PLUS, 14., fg3()))
        .child(div().text_size(T_SM).text_color(fg2()).child("新建"))
        .dropdown_menu(move |mut menu, _, _| {
            for l in &launchers {
                let model = model.clone();
                let cmd = l.command.clone();
                let (name, shown) = (l.name.clone(), l.command.clone());
                menu = menu.item(
                    PopupMenuItem::element(move |_, _| {
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(24.))
                            .min_w(px(180.))
                            .child(div().text_size(T_SM).child(name.clone()))
                            .when(!shown.is_empty(), |d| {
                                d.child(mono(shown.clone(), T_CAP, fg3()))
                            })
                    })
                    .on_click(move |_, _, cx| {
                        let command = (!cmd.is_empty()).then(|| cmd.clone());
                        model.update(cx, |m, cx| {
                            m.open_term(addr, TermStart::Open { command, cwd: None }, cx)
                        })
                    }),
                );
            }
            let model = model.clone();
            menu.separator()
                .item(
                    PopupMenuItem::new("自定义命令…").on_click(move |_, window, cx| {
                        dialogs::open_custom_command(model.clone(), addr, window, cx)
                    }),
                )
        })
        .into_any_element()
}

fn display_row(addr: SocketAddr, d: &DisplayInfo, ctx: &TreeCtx) -> AnyElement {
    let target = DeskTarget::Display(d.id);
    let streaming = ctx.desk.is_some_and(|n| n.streaming(addr, target));
    let opening = ctx.desk.is_some_and(|n| n.opening(addr, target));
    let selected = (streaming || opening) && ctx.selection == Selection::Desk;
    let model = ctx.model.clone();
    let id = d.id;
    side_row(SharedString::from(format!("disp-{addr}-{id}")), selected)
        .child(icon(
            ic::MONITOR,
            14.,
            if streaming { eye() } else { fg2() },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(T_SM)
                .truncate()
                .child(display_name(&d.name, d.id)),
        )
        .child(mono(format!("{}×{}", d.width, d.height), T_CAP, fg3()).flex_none())
        .when(opening, |r| r.child(Spinner::new().color(fg3())))
        .when(streaming, |r| r.child(dot(eye(), 5.)))
        .on_click(move |_, window, cx| model.update(cx, |m, cx| m.start_desk(addr, id, window, cx)))
        .into_any_element()
}

/// 「窗口」：点一下展开 / 收起这台主机的窗口列表（收起来就不问主机）。右边是窗口数，
/// 或者读取中的转圈、读不了 / 主机太旧的图标。`quiet`：权限卡片、主机行的图标已经在说为什么了，读不了就不再标。
fn windows_header(h: &HostState, ctx: &TreeCtx, quiet: bool) -> AnyElement {
    let addr = h.addr;
    let model = ctx.model.clone();
    let tip: SharedString = match &h.windows_state {
        WindowList::Unsupported => "主机的 lolidesk 太旧，升级后能切换窗口".into(),
        WindowList::Failed(e) if !quiet => format!("读不了窗口：{e}").into(),
        _ if h.show_windows => "收起".into(),
        _ => "展开：只推一个窗口".into(),
    };
    let end: Option<AnyElement> = match &h.windows_state {
        WindowList::Ready => {
            Some(mono(h.windows.len().to_string(), T_CAP, fg4()).into_any_element())
        }
        WindowList::NotYet if h.show_windows => {
            Some(Spinner::new().color(fg4()).into_any_element())
        }
        WindowList::Failed(_) if !quiet => Some(icon(ic::WARNING, 12., warn()).into_any_element()),
        WindowList::Unsupported => Some(icon(ic::WARNING, 12., fg4()).into_any_element()),
        _ => None,
    };
    div()
        .id(SharedString::from(format!("wins-{addr}")))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .h(px(26.))
        .mt(px(4.))
        .px(px(10.))
        .rounded(R_SM)
        .cursor_pointer()
        .hover(|s| s.bg(ink2()))
        .child(icon(
            if h.show_windows {
                ic::CHEVRON_DOWN
            } else {
                ic::CHEVRON_RIGHT
            },
            11.,
            fg3(),
        ))
        .child(caption("窗口").flex_1())
        .children(end)
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.toggle_windows(addr, cx)))
        .into_any_element()
}

/// 主机上的一个窗口：标题（没有就程序名）和程序名；最小化的标出来。点了只推这个窗口。
fn window_row(addr: SocketAddr, w: &WindowInfo, ctx: &TreeCtx) -> AnyElement {
    let target = DeskTarget::Window(w.id);
    let streaming = ctx.desk.is_some_and(|n| n.streaming(addr, target));
    let opening = ctx.desk.is_some_and(|n| n.opening(addr, target));
    let selected = (streaming || opening) && ctx.selection == Selection::Desk;
    let model = ctx.model.clone();
    let id = w.id;
    let name = window_label(w);
    let sub = if w.title.trim().is_empty() {
        String::new()
    } else {
        w.app.clone()
    };
    let tip: SharedString = format!(
        "{}{}\n{}×{} · 窗口 #{}{}",
        w.app,
        if w.title.is_empty() {
            String::new()
        } else {
            format!(" — {}", w.title)
        },
        w.width,
        w.height,
        w.id,
        if w.minimized {
            " · 已最小化（打开时先恢复）"
        } else {
            ""
        }
    )
    .into();
    let color = if streaming {
        eye()
    } else if w.minimized {
        fg4()
    } else {
        fg2()
    };
    side_row(SharedString::from(format!("win-{addr}-{id}")), selected)
        .h(px(40.))
        .child(icon(ic::WINDOW, 14., color))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(T_SM)
                        .when(w.minimized, |d| d.text_color(fg2()))
                        .truncate()
                        .child(name),
                )
                .when(!sub.is_empty(), |d| {
                    d.child(mono(sub, T_CAP, fg3()).truncate())
                }),
        )
        .when(w.minimized, |r| {
            r.child(badge("已最小化", fg3(), ink3()).flex_none())
        })
        .when(opening, |r| r.child(Spinner::new().color(fg3())))
        .when(streaming, |r| r.child(dot(eye(), 5.)))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(move |_, window, cx| {
            model.update(cx, |m, cx| m.start_window(addr, id, window, cx))
        })
        .into_any_element()
}
