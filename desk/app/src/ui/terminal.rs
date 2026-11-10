//! 终端面板：会话顶栏（图标、标题、主机 · #编号、目录、断开、⋯ 菜单）和终端（[`TermGrid`]）。
//!
//! 状态：正在接上时右上角一个小条；连接断了终端变暗、「重新连接中…」；程序退出、会话不在了、
//! 断开了时底部一条横幅（重新接入 / 关闭）。终端库没载入时正文是一段说明。

use std::net::SocketAddr;

use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::{
    Anchor, App, AppContext, Context, Entity, EntityId, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window, div, prelude::FluentBuilder, px,
};

use crate::{
    assets::ic,
    labels::{basename, is_agent, session_name},
    model::Model,
    term::{TermPhase, TermSession, TermStart},
    theme::*,
    ui::{
        grid::TermGrid,
        widgets::{Trigger, dot, hover_icon, icon, icon_button, mono, pill},
    },
};

pub struct TerminalView {
    pub session: Entity<TermSession>,
    pub grid: Entity<TermGrid>,
    model: WeakEntity<Model>,
    _subs: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(
        session: Entity<TermSession>,
        model: WeakEntity<Model>,
        cx: &mut Context<Self>,
    ) -> Self {
        let grid = cx.new(|cx| TermGrid::new(session.clone(), model.clone(), cx));
        // 会话的状态变了（不是每一帧）、终端的标题变了才重画顶栏
        let subs = vec![
            cx.observe(&session, |_, _, cx| cx.notify()),
            cx.subscribe(&grid, |_, _, _: &crate::ui::grid::GridEvent, cx| {
                cx.notify()
            }),
        ];
        Self {
            session,
            grid,
            model,
            _subs: subs,
        }
    }

    pub fn host(&self, cx: &App) -> SocketAddr {
        self.session.read(cx).host
    }

    pub fn session_id(&self, cx: &App) -> Option<u32> {
        self.session.read(cx).id
    }

    pub fn is_ended(&self, cx: &App) -> bool {
        self.session.read(cx).is_ended()
    }

    /// 会话开的命令（新开的从 start 来，接上的从主机的会话列表来）。
    pub fn command(&self, cx: &App) -> String {
        let s = self.session.read(cx);
        if let TermStart::Open { command, .. } = &s.start {
            return command.clone().unwrap_or_default();
        }
        let model = self.model.upgrade();
        model
            .and_then(|m| {
                let m = m.read(cx);
                let h = m.host(s.host)?;
                let id = s.id?;
                h.sessions
                    .iter()
                    .find(|x| x.id == id)
                    .map(|x| x.command.clone())
            })
            .unwrap_or_default()
    }

    /// 侧栏和顶栏显示的名字。
    pub fn name(&self, cx: &App) -> String {
        let cmd = self.command(cx);
        let host = self.host(cx);
        let launchers: Vec<lolidesk_proto::Launcher> = match self.model.upgrade() {
            Some(m) => m
                .read(cx)
                .host(host)
                .map(|h| h.launchers.clone())
                .unwrap_or_default(),
            None => Vec::new(),
        };
        session_name(&launchers, &cmd)
    }

    /// 程序设的标题（终端里的，没有就用主机报的）。
    pub fn title(&self, cx: &App) -> String {
        let t = self.grid.read(cx).title();
        if !t.trim().is_empty() {
            return t.to_string();
        }
        self.session.read(cx).title.clone()
    }

    /// 横幅：程序退出、会话不在、断开了。
    fn banner(&self, me: EntityId, phase: &TermPhase, cx: &App) -> Option<gpui_kit::AnyElement> {
        let error = self.session.read(cx).error.clone();
        let (text, reopen) = match phase {
            TermPhase::Exited(code) => {
                (format!("程序已退出（退出码 {code}）· 会话还在主机上"), true)
            }
            TermPhase::Gone => (
                error
                    .clone()
                    .unwrap_or_else(|| "主机上已经没有这个会话了".into()),
                false,
            ),
            TermPhase::Closed => (
                match error {
                    Some(e) => format!("已断开 · {e}"),
                    None => "已断开 · 会话还在主机上".into(),
                },
                true,
            ),
            _ => return None,
        };
        let model = self.model.clone();
        let m2 = self.model.clone();
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(px(16.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("term-banner")
                        .occlude()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .pl(px(16.))
                        .pr(px(6.))
                        .py(px(5.))
                        .rounded(R_PILL)
                        .bg(glass())
                        .border_1()
                        .border_color(line2())
                        .child(dot(fg4(), 6.))
                        .child(div().text_size(T_XS).text_color(fg2()).child(text))
                        .when(reopen, |s| {
                            s.child(pill("term-reopen", "重新接入", true).h(px(28.)).on_click(
                                move |_, _, cx| {
                                    if let Some(m) = model.upgrade() {
                                        m.update(cx, |m, cx| m.reopen_term(me, cx));
                                    }
                                },
                            ))
                        })
                        .child(pill("term-close", "关闭", false).h(px(28.)).on_click(
                            move |_, _, cx| {
                                if let Some(m) = m2.upgrade() {
                                    m.update(cx, |m, cx| m.close_term(me, cx));
                                }
                            },
                        )),
                )
                .into_any_element(),
        )
    }
}

/// 右上角的小状态条。
fn chip(text: &'static str, color: gpui_kit::Hsla) -> gpui_kit::Div {
    div()
        .absolute()
        .top(px(10.))
        .right(px(14.))
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(3.))
        .rounded(R_PILL)
        .bg(glass())
        .border_1()
        .border_color(line2())
        .child(dot(color, 6.))
        .child(div().text_size(T_CAP).text_color(fg2()).child(text))
}

impl Render for TerminalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity_id();
        let s = self.session.read(cx);
        let (host, id, phase) = (s.host, s.id, s.phase.clone());
        let title = self.title(cx);
        let command = self.command(cx);
        let name = self.name(cx);
        let model = self.model.upgrade();
        let (host_title, cwd) = model
            .as_ref()
            .map(|m| {
                let m = m.read(cx);
                let h = m.host(host);
                let cwd = h
                    .zip(id)
                    .and_then(|(h, id)| h.sessions.iter().find(|x| x.id == id))
                    .map(|x| x.cwd.clone())
                    .unwrap_or_default();
                (
                    h.map(|h| h.title()).unwrap_or_else(|| host.to_string()),
                    cwd,
                )
            })
            .unwrap_or_else(|| (host.to_string(), String::new()));

        // 顶栏：程序设了标题就用标题（agent 常把在做的事写进标题）
        let head = if title.trim().is_empty() {
            name.clone()
        } else {
            title.clone()
        };
        let path = if is_agent(&command) {
            ic::SPARKLE
        } else {
            ic::TERMINAL
        };
        let ended = matches!(
            phase,
            TermPhase::Exited(_) | TermPhase::Closed | TermPhase::Gone
        );
        let reconnecting = phase == TermPhase::Reconnecting;

        let detach = {
            let model = self.model.clone();
            icon_button("term-detach", ic::CLOSE, "断开（会话留在主机上）").on_click(
                move |_, _, cx| {
                    if let Some(m) = model.upgrade() {
                        m.update(cx, |m, cx| m.close_term(me, cx));
                    }
                },
            )
        };
        let more = {
            let model = self.model.clone();
            Trigger::new("term-more")
                .tip("更多")
                .open_bg(ink3())
                .group("term-more")
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(R_SM)
                .hover(|s| s.bg(ink3()))
                .child(hover_icon("term-more", ic::MORE, 16., fg2(), fg()))
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                    let (m1, m2, m3) = (model.clone(), model.clone(), model.clone());
                    menu.item(PopupMenuItem::new("放大字号").on_click(move |_, _, cx| {
                        if let Some(m) = m1.upgrade() {
                            m.update(cx, |m, cx| m.bump_font(1.0, cx));
                        }
                    }))
                    .item(PopupMenuItem::new("缩小字号").on_click(move |_, _, cx| {
                        if let Some(m) = m2.upgrade() {
                            m.update(cx, |m, cx| m.bump_font(-1.0, cx));
                        }
                    }))
                    .separator()
                    .item(
                        PopupMenuItem::element(|_, _| {
                            div().text_size(T_SM).text_color(bad()).child("结束会话")
                        })
                        .on_click(move |_, _, cx| {
                            if let Some(m) = m3.upgrade() {
                                m.update(cx, |m, cx| m.kill_term(me, cx));
                            }
                        }),
                    )
                })
        };

        let mut meta = host_title;
        if let Some(id) = id {
            meta.push_str(&format!(" · #{id}"));
        }
        let dir = basename(&cwd).to_string();

        let engine_error = self.grid.read(cx).engine_error().map(str::to_string);
        let body: gpui_kit::AnyElement = match engine_error {
            Some(e) => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .px(px(40.))
                .child(icon(ic::WARNING, 28., warn()))
                .child(
                    div()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child("终端库没载入"),
                )
                .child(
                    div()
                        .max_w(px(560.))
                        .text_center()
                        .text_size(T_XS)
                        .text_color(fg3())
                        .child(e),
                )
                .child(mono(
                    "Lolidesk.app 里它在 Contents/Frameworks；开发时设 LOLIDESK_GHOSTTY_VT 指向 libghostty-vt.dylib",
                    T_CAP,
                    fg4(),
                ))
                .into_any_element(),
            None => div()
                .size_full()
                .when(reconnecting || ended, |d| d.opacity(0.45))
                .child(self.grid.clone())
                .into_any_element(),
        };
        let banner = self.banner(me, &phase, cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(void())
            .child(super::top_bar(
                "term-bar",
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(icon(path, 15., if ended { fg3() } else { eye() }))
                    .child(
                        div()
                            .text_size(T_SM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(head),
                    )
                    .child(mono(meta, T_XS, fg3()).flex_none())
                    .when(!dir.is_empty(), |s| {
                        s.child(mono(dir, T_XS, fg4()).flex_none())
                    }),
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .child(detach)
                    .child(more),
            ))
            .child(
                div()
                    .id("term-body")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(body)
                    .when(phase == TermPhase::Opening, |s| {
                        s.child(chip("正在接上…", fg3()))
                    })
                    .when(reconnecting, |s| s.child(chip("重新连接中…", warn())))
                    .children(banner),
            )
    }
}
