//! 首页（什么都没选时）：主机一台一块，三列。没有主机时只有看板娘和「添加主机」。
//! 少说话：状态是点和延迟，操作是图标；出错原因、系统、版本、会话数在状态的悬停提示里。

use gpui_kit::{
    AnyElement, App, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, component::tooltip::Tooltip, div,
    prelude::FluentBuilder, px,
};

use crate::{
    assets::ic,
    model::{HostState, Link, LocalState, Model},
    term::TermStart,
    theme::*,
    ui::{
        dialogs,
        mascot::{self, Pose},
        sidebar::{link_status, local_status, local_text, retry_note},
        widgets::{dot, icon, icon_pill, mono, pill_busy, square_button},
    },
};

/// 一行几块
const COLS: usize = 3;

pub fn render(model: &Entity<Model>, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let body = if m.hosts.is_empty() {
        empty(model)
    } else {
        let local = m.local.as_ref().map(|l| l.state.clone());
        let tiles: Vec<AnyElement> = m
            .hosts
            .iter()
            .enumerate()
            .map(|(i, h)| tile(i, h, model, local.clone().filter(|_| h.local)))
            .collect();
        let mut grid = div().flex().flex_col().gap(px(12.)).p(px(24.));
        let mut it = tiles.into_iter().peekable();
        while it.peek().is_some() {
            let mut row = div().flex().gap(px(12.));
            for _ in 0..COLS {
                row = row.child(match it.next() {
                    Some(t) => div().flex_1().min_w_0().flex().child(t),
                    None => div().flex_1().min_w_0(),
                });
            }
            grid = grid.child(row);
        }
        div()
            .id("home-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(grid)
            .into_any_element()
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(super::top_bar(
            "home-bar",
            div()
                .text_size(T_SM)
                .font_weight(FontWeight::MEDIUM)
                .child("主机"),
            div(),
        ))
        .child(body)
        .into_any_element()
}

/// 还没有主机：看板娘和一个按钮（要先在那台电脑上装主机，按钮的提示里说）。
fn empty(model: &Entity<Model>) -> AnyElement {
    let model = model.clone();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(20.))
        .pb(px(48.))
        .child(mascot::pose(Pose::Hero, 200.))
        .child(
            icon_pill("home-add", ic::PLUS, "添加主机", true)
                .tooltip(|window, cx| {
                    Tooltip::new("那台电脑上先跑 lolidesk host install").build(window, cx)
                })
                .on_click(move |_, window, cx| dialogs::open_add_host(model.clone(), window, cx)),
        )
        .into_any_element()
}

/// 一台主机：名字、状态点、地址、操作。本机没在跑时是「开启」/「重试」。
fn tile(ix: usize, h: &HostState, model: &Entity<Model>, local: Option<LocalState>) -> AnyElement {
    let addr = h.addr;
    let local_down = local.filter(|st| !h.online() && *st != LocalState::Normal);
    let (label, color) = local_down
        .as_ref()
        .and_then(local_status)
        .unwrap_or_else(|| link_status(h));
    let tip: Option<SharedString> = if let Some(st) = &local_down {
        Some(local_text(st))
    } else if h.online() {
        h.welcome.as_ref().map(|w| {
            format!(
                "{} · lolidesk {} · {} 个会话",
                w.os,
                w.version,
                h.sessions.len()
            )
        })
    } else if !h.error.is_empty() {
        Some(match retry_note(h) {
            Some(n) => format!("{} · {n}", h.error),
            None => h.error.clone(),
        })
    } else {
        None
    }
    .filter(|t| !t.is_empty())
    .map(Into::into);
    let status = div()
        .id(("tile-st", ix))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(6.))
        .child(dot(color, 7.))
        .child(mono(label, T_XS, fg2()))
        .when_some(tip, |d, t| {
            d.tooltip(move |window, cx| Tooltip::new(t.clone()).build(window, cx))
        });

    let actions = if let Some(st) = &local_down {
        let m1 = model.clone();
        match st {
            LocalState::Starting => {
                pill_busy(("card-conn", ix), "开启中…", false).into_any_element()
            }
            LocalState::Off => icon_pill(("card-conn", ix), ic::POWER, "开启", false)
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.set_local_host(true, cx)))
                .into_any_element(),
            _ => square_button(("card-conn", ix), ic::REFRESH, "重试")
                .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.retry_local(cx)))
                .into_any_element(),
        }
    } else if h.online() {
        let m1 = model.clone();
        let term =
            icon_pill(("card-term", ix), ic::TERMINAL, "终端", true).on_click(move |_, _, cx| {
                m1.update(cx, |m, cx| {
                    m.open_term(
                        addr,
                        TermStart::Open {
                            command: None,
                            cwd: None,
                        },
                        cx,
                    )
                })
            });
        let desk = h
            .has_desktop()
            .then(|| h.primary_display().map(|d| d.id))
            .flatten()
            .map(|id| {
                let m2 = model.clone();
                square_button(("card-desk", ix), ic::MONITOR, "远程桌面").on_click(
                    move |_, window, cx| m2.update(cx, |m, cx| m.start_desk(addr, id, window, cx)),
                )
            });
        div()
            .flex()
            .gap(px(8.))
            .child(term)
            .children(desk)
            .into_any_element()
    } else if h.link == Link::Connecting {
        // 正在连：一条来回滑的细线（不是进度百分比）
        mascot::progress(("card-prog", ix), 120., warn()).into_any_element()
    } else {
        let m1 = model.clone();
        let (path, tip) = match h.link {
            Link::Reconnecting => (ic::REFRESH, "现在重连"),
            Link::Failed => (ic::REFRESH, "重试"),
            _ => (ic::PLUG, "连接"),
        };
        square_button(("card-conn", ix), path, tip)
            .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.connect_now(addr, cx)))
            .into_any_element()
    };

    div()
        .id(("card", ix))
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .rounded(R_LG)
        .bg(ink2())
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
                        .truncate()
                        .font_family(MONO)
                        .text_size(T_MD)
                        .font_weight(FontWeight::MEDIUM)
                        .child(h.title()),
                )
                .child(status),
        )
        .child(
            mono(
                if h.local {
                    h.name.clone()
                } else {
                    addr.to_string()
                },
                T_XS,
                fg3(),
            )
            .truncate(),
        )
        .child(div().flex().items_center().h(px(32.)).child(actions))
        .into_any_element()
}
