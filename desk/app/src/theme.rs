//! 猫眼 Catseye：lolinet 的固定美学。取值照抄 `web/src/styles/tokens.css`（那里是唯一的出处）。
//! 纯黑虚空，面板一层比一层亮一点；白胶囊主按钮；猫眼绿只给「在线 / 当前 / 主操作」。
//!
//! 两层：这里的常量给自己画的元素用；[`install`] 把同一套颜色做成 gpui-kit 的主题，
//! 让现成组件（菜单、弹层、输入框、提示、对话框、通知）也是这个样子。

use std::{borrow::Cow, rc::Rc};

use gpui_kit::component::{Theme, ThemeConfig};
use gpui_kit::{App, Hsla, Pixels, Rgba, px, rgb, rgba};

fn c(v: Rgba) -> Hsla {
    v.into()
}

// 底
pub fn void() -> Hsla {
    c(rgb(0x000000))
}
pub fn ink1() -> Hsla {
    c(rgb(0x0a0a0c))
}
pub fn ink2() -> Hsla {
    c(rgb(0x111114))
}
pub fn ink3() -> Hsla {
    c(rgb(0x18181c))
}
pub fn ink4() -> Hsla {
    c(rgb(0x222228))
}
/// rgb(255 255 255 / 0.07)
pub fn line1() -> Hsla {
    c(rgba(0xffffff12))
}
/// rgb(255 255 255 / 0.12)
pub fn line2() -> Hsla {
    c(rgba(0xffffff1f))
}
/// rgb(255 255 255 / 0.22)
pub fn line3() -> Hsla {
    c(rgba(0xffffff38))
}

// 字
pub fn fg() -> Hsla {
    c(rgb(0xf2f2f3))
}
/// 64% 白
pub fn fg2() -> Hsla {
    c(rgba(0xffffffa3))
}
/// 42% 白
pub fn fg3() -> Hsla {
    c(rgba(0xffffff6b))
}
/// 24% 白
pub fn fg4() -> Hsla {
    c(rgba(0xffffff3d))
}

// 白胶囊
pub fn solid() -> Hsla {
    c(rgb(0xf2f2f3))
}
pub fn solid_hover() -> Hsla {
    c(rgb(0xffffff))
}
pub fn solid_ink() -> Hsla {
    c(rgb(0x000000))
}

// 猫眼绿
pub fn eye() -> Hsla {
    c(rgb(0xceff1c))
}
pub fn eye_ink() -> Hsla {
    c(rgb(0x0c0f00))
}
pub fn eye_wash() -> Hsla {
    c(rgba(0xceff1c14))
}
pub fn eye_line() -> Hsla {
    c(rgba(0xceff1c52))
}

// 状态色
pub fn warn() -> Hsla {
    c(rgb(0xffb224))
}
pub fn warn_wash() -> Hsla {
    c(rgba(0xffb22414))
}
pub fn warn_line() -> Hsla {
    c(rgba(0xffb2244d))
}
pub fn bad() -> Hsla {
    c(rgb(0xff5c49))
}
pub fn bad_wash() -> Hsla {
    c(rgba(0xff5c491a))
}
pub fn bad_line() -> Hsla {
    c(rgba(0xff5c4966))
}

/// 浮在画面上的工具条：近黑、86% 不透明
pub fn glass() -> Hsla {
    c(rgba(0x0a0a0cdb))
}
/// 对话框后面压暗
pub fn scrim() -> Hsla {
    c(rgba(0x0000009e))
}
pub fn shadow() -> Hsla {
    c(rgba(0x00000099))
}

// 字体：Geist 管界面，Geist Mono 管一切机器给的东西（地址、指纹、延迟、尺寸）
pub const SANS: &str = "Geist";
pub const MONO: &str = "Geist Mono";

// 字号：操作界面以 13 为正文
pub const T_CAP: Pixels = px(11.);
pub const T_XS: Pixels = px(12.);
pub const T_SM: Pixels = px(13.);
pub const T_MD: Pixels = px(14.);
pub const T_LG: Pixels = px(16.);
pub const T_XL: Pixels = px(22.);
pub const T_2XL: Pixels = px(32.);

// 圆角
pub const R_XS: Pixels = px(6.);
pub const R_SM: Pixels = px(8.);
pub const R_MD: Pixels = px(12.);
pub const R_LG: Pixels = px(16.);
pub const R_PILL: Pixels = px(999.);

/// 侧栏宽度
pub const SIDEBAR_W: Pixels = px(260.);
/// 顶栏高度（红绿灯在这一条里）
pub const BAR_H: Pixels = px(52.);

const FONTS: [&[u8]; 5] = [
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
];

/// gpui-kit 主题：现成组件用的颜色。键名见 gpui-component 的 theme schema。
const THEME: &str = r##"{
  "name": "Catseye",
  "mode": "dark",
  "font.family": "Geist",
  "font.size": 13,
  "mono_font.family": "Geist Mono",
  "mono_font.size": 12,
  "radius": 8,
  "radius.lg": 16,
  "shadow": true,
  "colors": {
    "background": "#000000",
    "foreground": "#f2f2f3",
    "border": "#ffffff1f",
    "input.border": "#ffffff1f",
    "ring": "#ceff1c8c",
    "caret": "#ceff1c",
    "selection.background": "#ceff1c38",
    "overlay": "#0000009e",
    "window.border": "#ffffff1f",

    "popover.background": "#18181c",
    "popover.foreground": "#f2f2f3",
    "muted.background": "#18181c",
    "muted.foreground": "#ffffff6b",
    "accent.background": "#ffffff12",
    "accent.foreground": "#f2f2f3",

    "primary.background": "#f2f2f3",
    "primary.hover.background": "#ffffff",
    "primary.active.background": "#d6d6d8",
    "primary.foreground": "#000000",
    "secondary.background": "#18181c",
    "secondary.hover.background": "#222228",
    "secondary.active.background": "#2a2a31",
    "secondary.foreground": "#f2f2f3",

    "danger.background": "#ff5c49",
    "danger.hover.background": "#ff7a6a",
    "danger.active.background": "#e5503f",
    "danger.foreground": "#000000",
    "warning.background": "#ffb224",
    "warning.foreground": "#000000",
    "success.background": "#ceff1c",
    "success.foreground": "#0c0f00",
    "info.background": "#f2f2f3",
    "info.foreground": "#000000",

    "scrollbar.background": "#00000000",
    "scrollbar.thumb.background": "#ffffff38",
    "scrollbar.thumb.hover.background": "#ffffff5c",
    "title_bar.background": "#0a0a0c",
    "title_bar.border": "#ffffff12",
    "sidebar.background": "#0a0a0c",
    "sidebar.foreground": "#f2f2f3",
    "sidebar.border": "#ffffff12",
    "sidebar.accent.background": "#18181c",
    "sidebar.accent.foreground": "#f2f2f3",
    "sidebar.primary.background": "#f2f2f3",
    "sidebar.primary.foreground": "#000000",
    "list.background": "#00000000",
    "list.hover.background": "#ffffff0a",
    "list.active.background": "#ffffff12",
    "list.active.border": "#ceff1c",
    "skeleton.background": "#18181c",
    "switch.background": "#222228",
    "link": "#ceff1c",
    "link.hover": "#e0ff70",
    "link.active": "#b8e60f",

    "base.red": "#ff5c49",
    "base.green": "#ceff1c",
    "base.yellow": "#ffb224",
    "base.blue": "#3aa8ff",
    "base.magenta": "#e07bff",
    "base.cyan": "#5ee6d0"
  }
}"##;

/// 装字体和主题。在 `gpui_kit::init` 之后调。
pub fn install(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = FONTS.iter().map(|b| Cow::Borrowed(*b)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::warn!("装字体：{e:#}");
    }
    let config: ThemeConfig = match serde_json::from_str(THEME) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("主题：{e:#}");
            return;
        }
    };
    let config = Rc::new(config);
    Theme::update(cx, |theme| {
        theme.apply_config(&config);
        theme.font_family = SANS.into();
        theme.mono_font_family = MONO.into();
        theme.font_size = T_SM;
        theme.radius = R_SM;
        theme.radius_lg = R_LG;
        // 聚焦只把 1 px 的边描成猫眼绿（ring），不在外面再画一圈
        theme.focus_ring = false;
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn theme_json_parses() {
        let c: gpui_kit::component::ThemeConfig = serde_json::from_str(super::THEME).unwrap();
        assert!(c.mode.is_dark());
        assert_eq!(c.font_family.as_deref(), Some("Geist"));
    }
}
