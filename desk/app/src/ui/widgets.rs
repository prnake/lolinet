//! 小部件：图标、状态点、小标题、胶囊按钮、图标按钮、徽章、侧栏行、菜单触发器、拖窗口的区域。

use std::cell::Cell;

use gpui_kit::component::{Selectable, menu::DropdownMenu, tooltip::Tooltip};
use gpui_kit::{
    AnyElement, App, BoxShadow, Decorations, Div, ElementId, FontWeight, Hsla, InteractiveElement,
    Interactivity, IntoElement, MouseButton, ParentElement, Pixels, RenderOnce, SharedString,
    Stateful, StatefulInteractiveElement, StyleRefinement, Styled, Svg, Window, div, point,
    prelude::FluentBuilder, px, svg,
};

use crate::{assets::ic, theme::*};

pub fn icon(path: &'static str, size: f32, color: Hsla) -> Svg {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(color)
}

/// 状态点。
pub fn dot(color: Hsla, size: f32) -> Div {
    div().size(px(size)).flex_none().rounded_full().bg(color)
}

/// 小标题：11 号等宽、最淡的字色。
pub fn caption(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(MONO)
        .text_size(T_CAP)
        .text_color(fg3())
        .child(text.into())
}

/// 等宽字（机器给的东西）。
pub fn mono(text: impl Into<SharedString>, size: Pixels, color: Hsla) -> Div {
    div()
        .font_family(MONO)
        .text_size(size)
        .text_color(color)
        .child(text.into())
}

/// 胶囊按钮：主操作白底黑字，次要的深底描边。
pub fn pill(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    let b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(16.))
        .rounded(R_PILL)
        .text_size(T_SM)
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .child(label.into());
    if primary {
        b.bg(solid())
            .text_color(solid_ink())
            .hover(|s| s.bg(solid_hover()))
            .active(|s| s.opacity(0.85))
    } else {
        b.bg(ink3())
            .border_1()
            .border_color(line2())
            .text_color(fg())
            .hover(|s| s.bg(ink4()))
            .active(|s| s.opacity(0.85))
    }
}

/// 不能点的胶囊按钮（忙着的时候）。
pub fn pill_busy(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    let b = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(32.))
        .px(px(16.))
        .rounded(R_PILL)
        .text_size(T_SM)
        .font_weight(FontWeight::MEDIUM)
        .opacity(0.6)
        .child(label.into());
    if primary {
        b.bg(solid()).text_color(solid_ink())
    } else {
        b.bg(ink3())
            .border_1()
            .border_color(line2())
            .text_color(fg())
    }
}

/// 小号胶囊按钮（卡片里的行内操作）：26 高、12 号字。
pub fn pill_sm(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    pill(id, label, primary)
        .h(px(26.))
        .px(px(12.))
        .text_size(T_XS)
}

/// 开关：开是猫眼绿底、黑钮，关是深底、灰钮。`busy` 时半透明、不能点。
pub fn switch(id: impl Into<ElementId>, on: bool, busy: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .w(px(36.))
        .h(px(20.))
        .p(px(2.))
        .rounded(R_PILL)
        .border_1()
        .when(on, |d| d.justify_end().bg(eye()).border_color(eye()))
        .when(!on, |d| d.bg(ink4()).border_color(line2()))
        .when(busy, |d| d.opacity(0.5))
        .when(!busy, |d| d.cursor_pointer())
        .child(
            div()
                .size(px(14.))
                .rounded_full()
                .bg(if on { eye_ink() } else { fg2() }),
        )
}

/// 勾选框 + 文字。
pub fn checkbox(
    id: impl Into<ElementId>,
    on: bool,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(16.))
                .rounded(px(4.))
                .border_1()
                .when(on, |d| d.bg(eye()).border_color(eye()))
                .when(!on, |d| d.bg(ink1()).border_color(line3()))
                .when(on, |d| {
                    d.child(icon(crate::assets::ic::CHECK, 12., eye_ink()))
                }),
        )
        .child(div().text_size(T_SM).child(label.into()))
}

/// 28×28 的方形图标按钮，带提示。
pub fn icon_button(
    id: impl Into<ElementId>,
    path: &'static str,
    tip: &'static str,
) -> Stateful<Div> {
    let group: SharedString = format!("ib-{path}").into();
    div()
        .id(id)
        .group(group.clone())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(R_SM)
        .cursor_pointer()
        .hover(|s| s.bg(ink3()))
        .active(|s| s.bg(ink4()))
        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
        .child(hover_icon(group, path, 16., fg2(), fg()))
}

/// 图标：组（`group`）悬停时换颜色。GPUI 的 SVG 不继承文字颜色，所以要自己设。
pub fn hover_icon(
    group: impl Into<SharedString>,
    path: &'static str,
    size: f32,
    color: Hsla,
    hover: Hsla,
) -> Svg {
    icon(path, size, color).group_hover(group, move |s| s.text_color(hover))
}

/// 徽章：小号等宽字、胶囊、淡底。
pub fn badge(text: impl Into<SharedString>, color: Hsla, wash: Hsla) -> Div {
    div()
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded(R_PILL)
        .bg(wash)
        .font_family(MONO)
        .text_size(T_CAP)
        .text_color(color)
        .child(text.into())
}

/// 侧栏的一行：悬停亮一层；选中时再亮一层、左边一道猫眼绿。
pub fn side_row(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.))
        .h(px(32.))
        .px(px(10.))
        .rounded(R_SM)
        .text_color(fg())
        .cursor_pointer()
        .when(selected, |s| s.bg(ink3()).shadow(vec![inset_bar()]))
        .when(!selected, |s| s.hover(|s| s.bg(ink2())))
}

/// 选中行左边那道 2 px 的猫眼绿：内阴影，跟着圆角弯。
pub fn inset_bar() -> BoxShadow {
    BoxShadow {
        color: eye(),
        offset: point(px(2.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(0.),
        inset: true,
    }
}

/// 一条细分隔线（竖的）。
pub fn vdivider() -> Div {
    div()
        .w(px(1.))
        .h(px(18.))
        .mx(px(4.))
        .flex_none()
        .bg(line2())
}

// ---------------------------------------------------------------- 拖窗口

thread_local! {
    /// 在可拖区域里按下了左键、还没动：动了就开始拖窗口
    static DRAG_ARMED: Cell<bool> = const { Cell::new(false) };
}

/// 让一块区域能拖窗口（双击放大），和 gpui-kit 的 TitleBar 一样：按下后一动才开始拖，
/// 不耽误区域里按钮的点击。
pub fn drag_region(el: Stateful<Div>) -> Stateful<Div> {
    el.on_mouse_down(MouseButton::Left, |ev, window, _| {
        if ev.click_count >= 2 {
            DRAG_ARMED.with(|d| d.set(false));
            window.titlebar_double_click();
        } else {
            DRAG_ARMED.with(|d| d.set(true));
        }
    })
    .on_mouse_up(MouseButton::Left, |_, _, _| {
        DRAG_ARMED.with(|d| d.set(false))
    })
    .on_mouse_down_out(|_, _, _| DRAG_ARMED.with(|d| d.set(false)))
    .on_mouse_move(|ev, window, _| {
        if ev.pressed_button == Some(MouseButton::Left) && DRAG_ARMED.with(|d| d.replace(false)) {
            window.start_window_move();
        }
    })
}

/// 窗口按钮（最小化、最大化、关窗口）：只在 Linux 上窗口自己画边框时有
/// （Wayland 合成器不给标题栏，比如 GNOME）。窗口管理器画标题栏时、macOS 上什么都不画。
#[derive(IntoElement)]
pub struct WindowControls;

impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let own = cfg!(target_os = "linux")
            && matches!(window.window_decorations(), Decorations::Client { .. });
        if !own {
            // 不占地方：不进 flex 排版，也就不多出一段 gap
            return div().absolute().into_any_element();
        }
        let can = window.window_controls();
        let max = window.is_maximized();
        // 按下不给外面的拖窗口区域
        let button = |id: &'static str, path: &'static str, tip: &'static str| {
            icon_button(id, path, tip).on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(2.))
            .ml(px(6.))
            .when(can.minimize, |d| {
                d.child(
                    button("win-min", ic::WIN_MIN, "最小化")
                        .on_click(|_, window, _| window.minimize_window()),
                )
            })
            .when(can.maximize, |d| {
                let (path, tip) = if max {
                    (ic::WIN_RESTORE, "还原")
                } else {
                    (ic::WIN_MAX, "最大化")
                };
                d.child(button("win-max", path, tip).on_click(|_, window, _| window.zoom_window()))
            })
            .child(
                button("win-close", ic::CLOSE, "关闭")
                    .on_click(|_, window, _| window.remove_window()),
            )
            .into_any_element()
    }
}

// ---------------------------------------------------------------- 菜单触发器

/// 能挂 gpui-kit 下拉菜单的任意外观（[`DropdownMenu`] 要 Selectable + InteractiveElement）。
#[derive(IntoElement)]
pub struct Trigger {
    base: Stateful<Div>,
    style: StyleRefinement,
    children: Vec<AnyElement>,
    selected: bool,
    open_bg: Option<Hsla>,
    tip: Option<&'static str>,
}

impl Trigger {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(id),
            style: StyleRefinement::default(),
            children: Vec::new(),
            selected: false,
            open_bg: None,
            tip: None,
        }
    }

    /// 菜单开着时的底色。
    pub fn open_bg(mut self, c: Hsla) -> Self {
        self.open_bg = Some(c);
        self
    }

    /// 悬停提示（菜单开着时不显示，免得盖住菜单）。
    pub fn tip(mut self, tip: &'static str) -> Self {
        self.tip = Some(tip);
        self
    }
}

impl Selectable for Trigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for Trigger {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for Trigger {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl InteractiveElement for Trigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl StatefulInteractiveElement for Trigger {}

impl DropdownMenu for Trigger {}

impl RenderOnce for Trigger {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        use gpui_kit::{TestSupportExt as _, component::StyledExt as _};
        let open = self.selected;
        let open_bg = self.open_bg;
        let tip = self.tip.filter(|_| !open);
        self.base
            .when_some(tip, |s, tip| {
                s.tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            })
            .cursor_pointer()
            .refine_style(&self.style)
            .when_some(open_bg.filter(|_| open), |s, c| s.bg(c))
            .children(self.children)
            // 测试能按 id 找到它（不开 test-support 时什么都不做）
            .test_support()
    }
}
