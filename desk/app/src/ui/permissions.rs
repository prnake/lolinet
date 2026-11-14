//! 远程桌面的权限引导：主机缺屏幕录制、辅助功能时，侧栏里那台主机下面（窄的）和远程桌面顶上（宽的）各一张卡片。
//! 每个权限一行：图标、名字、「设置」（开了是绿勾，模型每 2 秒问一次主机）；为什么要、在哪儿开都在悬停提示里。
//! 屏幕录制开了以后要重启主机服务才生效：最后一行「重启」。

use gpui_kit::{
    AnyElement, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, component::tooltip::Tooltip, div, prelude::FluentBuilder,
    px,
};
use lolidesk_proto::PermissionKind;

use crate::{
    assets::ic,
    model::{HostState, Model},
    theme::*,
    ui::widgets::{icon, pill_busy, pill_sm},
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
    // 在哪儿开、开给谁：只在「设置」的悬停提示里说
    let where_tip: SharedString = format!(
        "在 {} 的系统设置 → 隐私与安全性里给 lolidesk 打开（主机上跑服务的那个）",
        h.title()
    )
    .into();

    let row_els: Vec<AnyElement> = rows
        .into_iter()
        .map(|r| {
            let model = model.clone();
            let kind = r.kind;
            let why: SharedString = r.why.into();
            let tip = where_tip.clone();
            let end = if r.granted {
                icon(ic::CHECK, 14., eye()).into_any_element()
            } else {
                pill_sm(
                    SharedString::from(format!("perm-open-{key}-{kind:?}")),
                    "设置",
                    false,
                )
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| m.open_permission(addr, kind, cx))
                })
                .into_any_element()
            };
            div()
                .id(SharedString::from(format!("perm-row-{key}-{kind:?}")))
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(if wide { 36. } else { 30. }))
                .tooltip(move |window, cx| Tooltip::new(why.clone()).build(window, cx))
                .child(icon(r.path, 15., if r.granted { eye() } else { warn() }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child(r.name),
                )
                .child(end)
                .into_any_element()
        })
        .collect();

    // 屏幕录制开了（或点过设置）：重启主机服务才生效
    let restart = h.guide_offers_restart().then(|| {
        let model = model.clone();
        let tip = if h.guide.restart {
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
            pill_sm(id, "重启", h.guide.restart)
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.restart_host(addr, cx)))
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(if wide { 36. } else { 30. }))
            .border_t_1()
            .border_color(line1())
            .child(icon(ic::REFRESH, 15., fg2()))
            .child(div().flex_1().text_size(T_SM).child("主机服务"))
            .child(button)
    });

    let card = div()
        .id(SharedString::from(format!("perm-card-{key}")))
        .flex()
        .flex_col()
        .rounded(if wide { R_LG } else { R_MD })
        .bg(ink2())
        .border_1()
        .border_color(warn_line())
        .when(wide, |d| d.px(px(14.)).py(px(4.)).w_full().max_w(px(420.)))
        .when(!wide, |d| {
            d.mx(px(2.)).mt(px(4.)).mb(px(4.)).px(px(10.)).py(px(2.))
        })
        .children(row_els)
        .children(restart);
    Some(card.into_any_element())
}
