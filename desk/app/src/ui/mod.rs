//! 视图。窗口 = 侧栏（缓存的视图）+ 右边（首页 / 网络 / 访问 / 终端 / 远程桌面），本机有配对请求时整个窗口上弹框。

pub mod access;
pub mod app;
pub mod boxdraw;
pub mod desktop;
pub mod dialogs;
pub mod grid;
pub mod home;
pub mod mascot;
pub mod network;
pub mod pair;
pub mod permissions;
pub mod sidebar;
pub mod terminal;
pub mod widgets;

use gpui_kit::{
    Div, InteractiveElement, IntoElement, ParentElement, Stateful, Styled, div,
    prelude::FluentBuilder, px,
};

use crate::theme::*;

/// 右边内容区的顶栏（能拖窗口）：左边标题，右边操作（Linux 上窗口自己画边框时再加窗口按钮）。
pub fn top_bar(id: &'static str, left: impl IntoElement, right: impl IntoElement) -> Stateful<Div> {
    widgets::drag_region(
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(BAR_H)
            .pl(px(18.))
            .pr(px(14.))
            .border_b_1()
            .border_color(line1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .flex_1()
                    .min_w_0()
                    .child(left),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_none()
                    .child(right)
                    .when(cfg!(target_os = "linux"), |d| {
                        d.child(widgets::WindowControls)
                    }),
            ),
    )
}
