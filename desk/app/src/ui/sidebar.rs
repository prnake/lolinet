//! 侧栏：字标和添加按钮、最上面固定的「网络」、主机树（权限引导、会话、新建、显示器）、底部的在线数和版本。
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
use lolidesk_proto::{DisplayInfo, Launcher, SessionInfo};

use crate::{
    assets::ic,
    labels::{display_name, is_agent, rtt_label, session_name, session_subtitle},
    model::{HostState, Link, Model, Selection},
    term::TermStart,
    theme::*,
    ui::{
        dialogs,
        widgets::{Trigger, caption, dot, drag_region, icon, icon_button, mono, side_row},
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

/// 连接状态的文字和颜色。
pub fn link_status(h: &HostState) -> (String, gpui_kit::Hsla) {
    match h.link {
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
            (d.host, d.display.id, !d.is_stopped())
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
        };
        let trees: Vec<AnyElement> = m
            .hosts
            .iter()
            .enumerate()
            .map(|(i, h)| host_tree(i, h, &ctx))
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
            icon_button("side-add", ic::PLUS, "添加主机")
                .on_click(move |_, window, cx| dialogs::open_add_host(model.clone(), window, cx))
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
                            .when(trees.is_empty(), |s| {
                                s.child(
                                    div()
                                        .px(px(8.))
                                        .py(px(4.))
                                        .text_size(T_XS)
                                        .text_color(fg3())
                                        .child("还没有主机。点右上角 ＋ 添加。"),
                                )
                            })
                            .children(trees),
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
                    .child(mono(format!("{online} / {total} 在线"), T_CAP, fg3()).flex_1())
                    .child(mono(crate::VERSION, T_CAP, fg4())),
            )
    }
}

struct TreeCtx<'a> {
    model: &'a Entity<Model>,
    selection: Selection,
    open: &'a [OpenTerm],
    desk: Option<(SocketAddr, u32, bool)>,
}

/// 一台主机和它下面的会话、新建、显示器。
fn host_tree(ix: usize, h: &HostState, ctx: &TreeCtx) -> AnyElement {
    let addr = h.addr;
    let (status, color) = link_status(h);
    let chevron = if h.expanded {
        ic::CHEVRON_DOWN
    } else {
        ic::CHEVRON_RIGHT
    };
    let online = h.online();
    let link = h.link;

    let row = {
        let model = ctx.model.clone();
        let m2 = ctx.model.clone();
        side_row(("host", ix), false)
            .h(px(34.))
            .child(icon(chevron, 12., fg3()))
            .child(icon(ic::COMPUTER, 15., fg2()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(MONO)
                    .text_size(T_SM)
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(h.title()),
            )
            .child(mono(status, T_CAP, fg3()).flex_none())
            .child(dot(color, 6.))
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| match link {
                    Link::Online => m.toggle_expanded(addr, cx),
                    Link::Connecting => {}
                    Link::Offline | Link::Failed | Link::Reconnecting => {
                        if let Some(h) = m.host_mut(addr) {
                            h.expanded = true;
                        }
                        m.connect_now(addr, cx);
                    }
                })
            })
            .context_menu(move |menu, _, _| host_menu(menu, &m2, addr, link))
    };

    let mut tree = div().flex().flex_col().child(row);
    if !h.expanded {
        return tree.into_any_element();
    }
    let mut body = div().flex().flex_col().pl(px(14.)).pb(px(6.)).gap(px(1.));
    let note = retry_note(h);
    if !online && (!h.error.is_empty() || note.is_some()) {
        // 不再重试的是红字；还在重连的是安静的灰字，带次数和倒计时
        let failed = h.link == Link::Failed;
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(10.))
                .pt(px(2.))
                .pb(px(6.))
                .text_size(T_CAP)
                .text_color(if failed { bad() } else { fg3() })
                .when(!h.error.is_empty(), |d| d.child(h.error.clone()))
                .when_some(note, |d, n| d.child(mono(n, T_CAP, warn()))),
        );
    }
    // 远程桌面还缺权限：引导卡片放在最上面
    let guide = online
        .then(|| crate::ui::permissions::card(h, ctx.model, false))
        .flatten();
    let guiding = guide.is_some();
    body = body.children(guide);
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
            // 卡片已经在说缺屏幕录制了，列不出显示器的那条就不重复
            if let Some(e) = h.displays_error.as_ref().filter(|_| !guiding) {
                body = body.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mx(px(2.))
                        .mt(px(4.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(R_SM)
                        .bg(warn_wash())
                        .border_1()
                        .border_color(warn_line())
                        .child(icon(ic::MONITOR, 14., warn()))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(T_CAP)
                                .text_color(warn())
                                .child(e.clone()),
                        ),
                );
            }
        }
    }
    tree = tree.child(body);
    tree.into_any_element()
}

fn host_menu(menu: PopupMenu, model: &Entity<Model>, addr: SocketAddr, link: Link) -> PopupMenu {
    let (m1, m2, m3) = (model.clone(), model.clone(), model.clone());
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
    menu.separator().item(
        danger_item("忘掉这台主机")
            .on_click(move |_, _, cx| m3.update(cx, |m, cx| m.forget(addr, cx))),
    )
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
    let streaming = ctx
        .desk
        .is_some_and(|(h, id, live)| h == addr && id == d.id && live);
    let selected = streaming && ctx.selection == Selection::Desk;
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
        .when(streaming, |r| r.child(dot(eye(), 5.)))
        .on_click(move |_, window, cx| model.update(cx, |m, cx| m.start_desk(addr, id, window, cx)))
        .into_any_element()
}

/// 给首页用：主机卡片上的状态徽章。
pub fn status_badge(h: &HostState) -> impl IntoElement {
    use crate::ui::widgets::badge;
    match h.link {
        Link::Online => badge(rtt_label(h.rtt_ms), eye(), eye_wash()),
        Link::Connecting | Link::Reconnecting => badge("连接中", warn(), warn_wash()),
        Link::Failed => badge("连不上", bad(), bad_wash()),
        Link::Offline => badge("离线", fg3(), ink3()),
    }
}
