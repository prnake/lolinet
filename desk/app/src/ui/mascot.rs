//! 看板娘：首页、第一次用、正在连、出错、结束了各有一个样子（图在 `assets/mascot/`，见 [`crate::assets`]）。
//! 只是陪衬：放在空着的地方，不挡操作，也不替代文字说明。

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt, ElementId, Hsla, IntoElement, ParentElement, Styled, div, img, px,
};

use crate::theme::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pose {
    /// 笑着站好：首页、第一次用
    Hero,
    /// 托着下巴想：出错了
    Think,
    /// 闭着眼：结束了、断开了
    Sleep,
}

impl Pose {
    fn path(self) -> &'static str {
        match self {
            Pose::Hero => "mascot/hero.png",
            Pose::Think => "mascot/think.png",
            Pose::Sleep => "mascot/sleep.png",
        }
    }

    /// 宽 / 高（和图一致，免得图载入前后布局跳）
    fn aspect(self) -> f32 {
        match self {
            Pose::Hero => 447. / 720.,
            Pose::Think => 225. / 520.,
            Pose::Sleep => 229. / 520.,
        }
    }
}

/// 站着的看板娘，高 `h`。
pub fn pose(p: Pose, h: f32) -> impl IntoElement {
    img(p.path()).flex_none().w(px(h * p.aspect())).h(px(h))
}

const RUN_FRAMES: usize = 8;
/// run.png 里一帧的大小（8 帧横着拼成一条）
const RUN_W: f32 = 197.;
const RUN_H: f32 = 260.;

/// 跑步，高 `h`：整条图放在只露一帧宽的框里，按时间往左挪，一圈 0.64 秒。
/// 一张图换位置，不是八张图轮流换，所以不会因为载入而闪。
pub fn running(id: impl Into<ElementId>, h: f32) -> impl IntoElement {
    let w = h * RUN_W / RUN_H;
    div()
        .flex_none()
        .relative()
        .w(px(w))
        .h(px(h))
        .overflow_hidden()
        .child(
            img("mascot/run.png")
                .absolute()
                .top_0()
                .left_0()
                .w(px(w * RUN_FRAMES as f32))
                .h(px(h))
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(640)).repeat(),
                    move |el, t| {
                        let i = ((t * RUN_FRAMES as f32) as usize).min(RUN_FRAMES - 1);
                        el.left(px(-(i as f32) * w))
                    },
                ),
        )
}

/// 说不准还要多久的进度条：宽 `w`，一小段 `color` 在轨道里来回滑。
pub fn progress(id: impl Into<ElementId>, w: f32, color: Hsla) -> impl IntoElement {
    let seg = (w * 0.3).round();
    div()
        .flex_none()
        .relative()
        .w(px(w))
        .h(px(3.))
        .rounded(R_PILL)
        .bg(line1())
        .overflow_hidden()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(seg))
                .h(px(3.))
                .rounded(R_PILL)
                .bg(color)
                .with_animation(
                    id,
                    Animation::new(Duration::from_millis(1600)).repeat(),
                    move |el, t| {
                        // 两头慢、中间快地来回
                        let x = (1. - (t * std::f32::consts::TAU).cos()) / 2.;
                        el.left(px(x * (w - seg)))
                    },
                ),
        )
}
