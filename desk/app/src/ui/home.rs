//! 首页（什么都没选时）：两行大标题，主机卡片（两列），没有主机时是「添加主机」。

use gpui_kit::{
    AnyElement, App, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, div, px,
};

use crate::{
    assets::ic,
    model::{HostState, Link, Model},
    term::TermStart,
    theme::*,
    ui::{
        dialogs,
        sidebar::status_badge,
        widgets::{icon, mono, pill},
    },
};

pub fn render(model: &Entity<Model>, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let headline = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(T_2XL)
                .line_height(px(40.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("远程终端，远程桌面。"),
        )
        .child(
            div()
                .text_size(T_2XL)
                .line_height(px(40.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg3())
                .child("lolinet 内网里的电脑，就在手边。"),
        );

    let body = if m.hosts.is_empty() {
        let model = model.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(div().flex().child(pill("home-add", "添加主机", true).on_click(
                move |_, window, cx| dialogs::open_add_host(model.clone(), window, cx),
            )))
            .child(mono(
                "在要连的电脑上跑 lolidesk host install，再用 lolidesk host token 打出的 token 添加它。",
                T_XS,
                fg3(),
            ))
            .into_any_element()
    } else {
        // 两列：一行两张卡
        let cards: Vec<AnyElement> = m
            .hosts
            .iter()
            .enumerate()
            .map(|(i, h)| host_card(i, h, model))
            .collect();
        let mut grid = div().flex().flex_col().gap(px(12.));
        let mut it = cards.into_iter();
        while let Some(a) = it.next() {
            let b = it.next();
            grid = grid.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(div().flex_1().min_w_0().flex().child(a))
                    .child(match b {
                        Some(b) => div().flex_1().min_w_0().flex().child(b),
                        None => div().flex_1().min_w_0(),
                    }),
            );
        }
        grid.into_any_element()
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(super::top_bar("home-bar", div(), div()))
        .child(
            div()
                .id("home-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(28.))
                        .max_w(px(880.))
                        .pt(px(72.))
                        .px(px(56.))
                        .pb(px(56.))
                        .child(headline)
                        .child(body),
                ),
        )
        .into_any_element()
}

/// 首页的一台主机：状态、地址、一键开终端 / 桌面。
fn host_card(ix: usize, h: &HostState, model: &Entity<Model>) -> AnyElement {
    let addr = h.addr;
    let actions = if h.online() {
        let m1 = model.clone();
        let term = pill(("card-term", ix), "新终端", true).on_click(move |_, _, cx| {
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
                pill(("card-desk", ix), "远程桌面", false).on_click(move |_, window, cx| {
                    m2.update(cx, |m, cx| m.start_desk(addr, id, window, cx))
                })
            });
        div().flex().gap(px(8.)).child(term).children(desk)
    } else {
        let m1 = model.clone();
        let busy = h.link == Link::Connecting;
        let retrying = h.link == Link::Reconnecting;
        div().flex().gap(px(8.)).child(if busy {
            crate::ui::widgets::pill_busy(("card-conn", ix), "连接中…", false).into_any_element()
        } else {
            pill(
                ("card-conn", ix),
                if retrying { "现在重连" } else { "连接" },
                false,
            )
            .on_click(move |_, _, cx| m1.update(cx, |m, cx| m.connect_now(addr, cx)))
            .into_any_element()
        })
    };
    let meta = match (&h.welcome, h.online()) {
        (Some(w), true) => Some(
            mono(
                format!(
                    "{} · lolidesk {} · {} 个会话{}",
                    w.os,
                    w.version,
                    h.sessions.len(),
                    if w.persistent {
                        " · 重启不丢会话"
                    } else {
                        ""
                    }
                ),
                T_CAP,
                fg3(),
            )
            .into_any_element(),
        ),
        _ if !h.error.is_empty() => Some(
            div()
                .text_size(T_CAP)
                .text_color(fg3())
                .child(match crate::ui::sidebar::retry_note(h) {
                    Some(n) => format!("{} · {n}", h.error),
                    None => h.error.clone(),
                })
                .into_any_element(),
        ),
        _ => None,
    };
    div()
        .id(("card", ix))
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(14.))
        .p(px(18.))
        .rounded(R_LG)
        .bg(ink2())
        .border_1()
        .border_color(line1())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(36.))
                        .rounded(R_MD)
                        .bg(ink3())
                        .child(icon(ic::COMPUTER, 18., fg2())),
                )
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
                                .text_size(T_MD)
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(h.title()),
                        )
                        .child(mono(addr.to_string(), T_CAP, fg3())),
                )
                .child(status_badge(h)),
        )
        .child(actions)
        .children(meta)
        .into_any_element()
}
