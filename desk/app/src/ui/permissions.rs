//! 远程桌面的权限引导：主机缺屏幕录制、辅助功能时，侧栏里那台主机下面（窄的）和远程桌面顶上（宽的）各一张卡片。
//! 每个没开的权限一行，开了变绿（模型每 2 秒问一次主机）；屏幕录制开了以后要重启主机服务才生效。

use gpui_kit::{
    AnyElement, Entity, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px,
};
use lolidesk_proto::PermissionKind;

use crate::{
    assets::ic,
    model::{HostState, Model},
    theme::*,
    ui::widgets::{icon, mono, pill_busy, pill_sm},
};

struct Row {
    kind: PermissionKind,
    name: &'static str,
    why: &'static str,
    path: &'static str,
    granted: bool,
}

/// 这台主机的引导卡片；不用引导（权限都开了、没有远程桌面、没连上）是 None。
pub fn card(h: &HostState, model: &Entity<Model>, wide: bool) -> Option<AnyElement> {
    let steps = h.guide_steps();
    let perms = h.perms?;
    if steps == 0 {
        return None;
    }
    let mut rows = Vec::new();
    if h.guide.screen || !perms.screen {
        rows.push(Row {
            kind: PermissionKind::ScreenRecording,
            name: "屏幕录制",
            why: "看远程桌面、给 agent 截图",
            path: ic::MONITOR,
            granted: perms.screen,
        });
    }
    if h.needs_access() && (h.guide.access || !perms.access) {
        rows.push(Row {
            kind: PermissionKind::Accessibility,
            name: "辅助功能",
            why: "远程操作键盘和鼠标",
            path: ic::POINTER,
            granted: perms.access,
        });
    }
    let key = format!("{}-{}", h.addr, if wide { "w" } else { "n" });
    let addr = h.addr;

    let title = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon(ic::SHIELD, 15., warn()))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(if wide { T_MD } else { T_SM })
                .font_weight(FontWeight::MEDIUM)
                .child(format!("远程桌面还差 {steps} 步")),
        );
    let lead = wide.then(|| {
        div()
            .text_size(T_XS)
            .text_color(fg3())
            .child(format!(
                "在 {} 的「系统设置 → 隐私与安全性」里给 lolidesk 打开。授权跟着主机上跑服务的那个 lolidesk 走。",
                h.title()
            ))
    });

    let row_els: Vec<AnyElement> = rows
        .into_iter()
        .map(|r| {
            let model = model.clone();
            let kind = r.kind;
            let button = (!r.granted).then(|| {
                pill_sm(
                    SharedString::from(format!("perm-open-{key}-{kind:?}")),
                    "在主机上打开设置",
                    false,
                )
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| m.open_permission(addr, kind, cx))
                })
            });
            let (state, color): (&str, Hsla) = if r.granted {
                ("已开", eye())
            } else {
                ("没开", warn())
            };
            let mark = if r.granted {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded_full()
                    .bg(eye_wash())
                    .border_1()
                    .border_color(eye_line())
                    .child(icon(ic::CHECK, 11., eye()))
            } else {
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .child(icon(r.path, 15., warn()))
            };
            if wide {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .h(px(36.))
                    .child(mark)
                    .child(
                        div()
                            .flex_none()
                            .w(px(64.))
                            .text_size(T_SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child(r.name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(T_XS)
                            .text_color(fg3())
                            .child(r.why),
                    )
                    .child(match button {
                        Some(b) => b.into_any_element(),
                        None => mono(state, T_CAP, color).into_any_element(),
                    })
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(mark)
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(T_SM)
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(r.name),
                            )
                            .child(mono(state, T_CAP, color)),
                    )
                    .child(
                        div()
                            .pl(px(26.))
                            .text_size(T_CAP)
                            .text_color(fg3())
                            .child(r.why),
                    )
                    .children(button.map(|b| div().flex().pl(px(26.)).child(b)))
                    .into_any_element()
            }
        })
        .collect();

    let restart = h.guide_offers_restart().then(|| {
        let model = model.clone();
        let note = if h.guide.restart {
            "屏幕录制开了，重启主机服务才生效"
        } else {
            "勾上 lolidesk 以后，重启主机服务才生效"
        };
        let id = SharedString::from(format!("perm-restart-{key}"));
        let button = if h.guide.restarting {
            pill_busy(id, "重启中…", true)
                .h(px(26.))
                .px(px(12.))
                .text_size(T_XS)
        } else {
            pill_sm(id, "重启主机服务", h.guide.restart)
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.restart_host(addr, cx)))
        };
        let text = div()
            .text_size(if wide { T_XS } else { T_CAP })
            .text_color(fg2())
            .child(note);
        let body = if wide {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().flex_1().min_w_0().child(text))
                .child(button)
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(text)
                .child(div().flex().child(button))
        };
        div()
            .pt(px(if wide { 10. } else { 12. }))
            .border_t_1()
            .border_color(line1())
            .child(body)
    });

    let card = div()
        .id(SharedString::from(format!("perm-card-{key}")))
        .flex()
        .flex_col()
        .rounded(if wide { R_LG } else { R_MD })
        .bg(ink2())
        .border_1()
        .border_color(warn_line())
        .when(wide, |d| {
            d.gap(px(6.))
                .px(px(18.))
                .py(px(14.))
                .w_full()
                .max_w(px(760.))
        })
        .when(!wide, |d| {
            d.gap(px(12.)).mx(px(2.)).mt(px(4.)).mb(px(4.)).p(px(12.))
        })
        .child(title)
        .children(lead)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(if wide { 0. } else { 12. }))
                .when(wide, |d| d.pt(px(4.)))
                .children(row_els),
        )
        .children(restart);
    Some(card.into_any_element())
}
