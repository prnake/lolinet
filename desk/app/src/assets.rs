//! 资源：自己的图标（`ld/<名字>.svg`）+ gpui-kit 自带的 Lucide 图标（组件要用）。
//! 图标是 24×24 的线框，按文字颜色画（GPUI 把 SVG 当遮罩，颜色取 `text_color`）。

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

macro_rules! icon {
    ($body:literal) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        )
    };
}

/// 图标名 → SVG。
fn icon_svg(name: &str) -> Option<&'static str> {
    Some(match name {
        "terminal" => icon!(
            r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M7.5 9.5 10.5 12l-3 2.5"/><path d="M13 15h3.5"/>"#
        ),
        "monitor" => icon!(
            r#"<rect x="3" y="4" width="18" height="12" rx="2.5"/><path d="M9 20h6"/><path d="M12 16v4"/>"#
        ),
        "plus" => icon!(r#"<path d="M12 5v14"/><path d="M5 12h14"/>"#),
        // 远程桌面上的一个窗口：带标题栏和两个圆点
        "window" => icon!(
            r#"<rect x="3" y="4.5" width="18" height="15" rx="2.5"/><path d="M3 9h18"/><path d="M6.5 6.8h.01M9.2 6.8h.01" stroke-width="2.2"/>"#
        ),
        "sparkle" => icon!(
            r#"<path d="M12 3.5c.6 3.9 2.6 5.9 6.5 6.5-3.9.6-5.9 2.6-6.5 6.5-.6-3.9-2.6-5.9-6.5-6.5 3.9-.6 5.9-2.6 6.5-6.5z"/><path d="M18.5 16.5v4"/><path d="M16.5 18.5h4"/>"#
        ),
        // 限制：两根滑杆
        "sliders" => icon!(
            r#"<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>"#
        ),
        // 吊销：断开的链
        "unlink" => icon!(
            r#"<path d="m18.8 12.3 1.7-1.7a5 5 0 0 0-7.1-7.1l-1.7 1.7"/><path d="m5.2 11.7-1.7 1.7a5 5 0 0 0 7.1 7.1l1.7-1.7"/><path d="M8 2v3M2 8h3M16 19v3M19 16h3"/>"#
        ),
        // 分流：一条路分成两条
        "split" => icon!(
            r#"<path d="M16 3h5v5"/><path d="M8 3H3v5"/><path d="M12 22v-8.3a4 4 0 0 0-1.17-2.87L3 3"/><path d="m15 9 6-6"/>"#
        ),
        "power" => icon!(r#"<path d="M12 3.5v8"/><path d="M6.6 6.9a7.5 7.5 0 1 0 10.8 0"/>"#),
        "refresh" => {
            icon!(r#"<path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3"/><path d="M19.5 4.5v4h-4"/>"#)
        }
        "expand" => icon!(
            r#"<path d="M4 9V4h5"/><path d="M20 9V4h-5"/><path d="M4 15v5h5"/><path d="M20 15v5h-5"/>"#
        ),
        "fullscreen" => icon!(
            r#"<path d="M4 8.5V4h4.5"/><path d="M20 8.5V4h-4.5"/><path d="M4 15.5V20h4.5"/><path d="M20 15.5V20h-4.5"/><rect x="8" y="8" width="8" height="8" rx="1.5"/>"#
        ),
        "chevron-down" => icon!(r#"<path d="m6.5 9.5 5.5 5.5 5.5-5.5"/>"#),
        "chevron-right" => icon!(r#"<path d="m9.5 6.5 5.5 5.5-5.5 5.5"/>"#),
        "close" => icon!(r#"<path d="M6.5 6.5l11 11"/><path d="M17.5 6.5l-11 11"/>"#),
        "keyboard" => icon!(
            r#"<rect x="2.5" y="6" width="19" height="12" rx="2.5"/><path d="M6.5 10h.01M10 10h.01M13.5 10h.01M17 10h.01M7.5 14h9"/>"#
        ),
        "computer" => {
            icon!(r#"<rect x="4" y="5" width="16" height="11" rx="2"/><path d="M2.5 19h19"/>"#)
        }
        "more" => {
            icon!(r#"<path d="M6 12h.01M12 12h.01M18 12h.01" stroke-width="2.5"/>"#)
        }
        "plug" => icon!(
            r#"<path d="M9 3.5v4M15 3.5v4"/><path d="M6.5 7.5h11v3a5.5 5.5 0 0 1-11 0z"/><path d="M12 16v4.5"/>"#
        ),
        "trash" => icon!(
            r#"<path d="M4.5 7h15"/><path d="M9.5 7V4.5h5V7"/><path d="M6.5 7l1 12.5h9l1-12.5"/>"#
        ),
        "gauge" => icon!(r#"<path d="M4.2 17a8.5 8.5 0 1 1 15.6 0"/><path d="M12 13.5 15.5 9"/>"#),
        "film" => icon!(
            r#"<rect x="3.5" y="5" width="17" height="14" rx="2.5"/><path d="M8 5v14M16 5v14M3.5 12h17"/>"#
        ),
        "warning" => icon!(
            r#"<path d="M12 4 21 19.5H3z"/><path d="M12 10v4.5"/><path d="M12 17.2h.01" stroke-width="2.4"/>"#
        ),
        "detach" => icon!(
            r#"<path d="M14 4.5h5.5V10"/><path d="M19.5 4.5 12 12"/><path d="M18 14v3.5a2 2 0 0 1-2 2H6.5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2H10"/>"#
        ),
        "network" => icon!(
            r#"<circle cx="12" cy="5.5" r="2.5"/><circle cx="5.5" cy="18" r="2.5"/><circle cx="18.5" cy="18" r="2.5"/><path d="M10.8 7.7 6.7 15.8M13.2 7.7l4.1 8.1M8 18h8"/>"#
        ),
        "check" => icon!(r#"<path d="m5.5 12.5 4 4 9-9.5"/>"#),
        "copy" => icon!(
            r#"<rect x="8.5" y="8.5" width="11" height="11" rx="2.5"/><path d="M15.5 8.5V6.5a2 2 0 0 0-2-2h-7a2 2 0 0 0-2 2v7a2 2 0 0 0 2 2h2"/>"#
        ),
        "globe" => icon!(
            r#"<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17"/><path d="M12 3.5c2.4 2.5 3.5 5.3 3.5 8.5s-1.1 6-3.5 8.5c-2.4-2.5-3.5-5.3-3.5-8.5s1.1-6 3.5-8.5z"/>"#
        ),
        "phone" => {
            icon!(r#"<rect x="6.5" y="3" width="11" height="18" rx="2.5"/><path d="M11 17.5h2"/>"#)
        }
        "shield" => icon!(
            r#"<path d="M12 3.5 19 6v5.5c0 4.2-2.9 7.6-7 9-4.1-1.4-7-4.8-7-9V6z"/><path d="m9 12 2.2 2.2L15.5 10"/>"#
        ),
        "pointer" => icon!(r#"<path d="M6 4l12.5 7-5.5 1.5L10.5 18z"/><path d="m13 12.5 4 5"/>"#),
        "key" => icon!(
            r#"<circle cx="8" cy="15.5" r="4"/><path d="m10.9 12.6 8.6-8.6"/><path d="m16.5 7 2.5 2.5"/><path d="m14.2 9.3 2 2"/>"#
        ),
        // Linux 上自己画的窗口按钮
        "win-min" => icon!(r#"<path d="M6.5 12h11"/>"#),
        "win-max" => icon!(r#"<rect x="6.5" y="6.5" width="11" height="11" rx="1.5"/>"#),
        "win-restore" => icon!(
            r#"<rect x="6" y="9" width="9" height="9" rx="1.5"/><path d="M9 9V7.5A1.5 1.5 0 0 1 10.5 6H16.5A1.5 1.5 0 0 1 18 7.5V13.5A1.5 1.5 0 0 1 16.5 15H15"/>"#
        ),
        _ => return None,
    })
}

/// 图标的资源路径（`svg().path(..)` 用）。
pub mod ic {
    pub const TERMINAL: &str = "ld/terminal.svg";
    pub const MONITOR: &str = "ld/monitor.svg";
    pub const WINDOW: &str = "ld/window.svg";
    pub const PLUS: &str = "ld/plus.svg";
    pub const SPARKLE: &str = "ld/sparkle.svg";
    pub const POWER: &str = "ld/power.svg";
    pub const REFRESH: &str = "ld/refresh.svg";
    pub const EXPAND: &str = "ld/expand.svg";
    pub const FULLSCREEN: &str = "ld/fullscreen.svg";
    pub const CHEVRON_DOWN: &str = "ld/chevron-down.svg";
    pub const CHEVRON_RIGHT: &str = "ld/chevron-right.svg";
    pub const CLOSE: &str = "ld/close.svg";
    pub const KEYBOARD: &str = "ld/keyboard.svg";
    pub const COMPUTER: &str = "ld/computer.svg";
    pub const MORE: &str = "ld/more.svg";
    pub const PLUG: &str = "ld/plug.svg";
    pub const TRASH: &str = "ld/trash.svg";
    pub const GAUGE: &str = "ld/gauge.svg";
    pub const FILM: &str = "ld/film.svg";
    pub const WARNING: &str = "ld/warning.svg";
    pub const DETACH: &str = "ld/detach.svg";
    pub const NETWORK: &str = "ld/network.svg";
    pub const CHECK: &str = "ld/check.svg";
    pub const COPY: &str = "ld/copy.svg";
    pub const GLOBE: &str = "ld/globe.svg";
    pub const PHONE: &str = "ld/phone.svg";
    pub const SHIELD: &str = "ld/shield.svg";
    pub const POINTER: &str = "ld/pointer.svg";
    pub const KEY: &str = "ld/key.svg";
    pub const SPLIT: &str = "ld/split.svg";
    pub const SLIDERS: &str = "ld/sliders.svg";
    pub const UNLINK: &str = "ld/unlink.svg";
    pub const WIN_MIN: &str = "ld/win-min.svg";
    pub const WIN_MAX: &str = "ld/win-max.svg";
    pub const WIN_RESTORE: &str = "ld/win-restore.svg";
}

/// 看板娘的图（`mascot/<名字>.png`，透明底，按显示大小的两倍存）：用 Codex（OpenAI 的生图模型）画的，
/// 品红底抠成透明。`run` 是 8 帧跑步横着拼成的一条，见 [`crate::ui::mascot`]。
fn mascot_png(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "hero" => include_bytes!("../assets/mascot/hero.png"),
        "think" => include_bytes!("../assets/mascot/think.png"),
        "sleep" => include_bytes!("../assets/mascot/sleep.png"),
        "run" => include_bytes!("../assets/mascot/run.png"),
        _ => return None,
    })
}

/// 应用的资源：先找自己的图标和看板娘，再交给 gpui-kit 的。
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path
            .strip_prefix("ld/")
            .and_then(|p| p.strip_suffix(".svg"))
        {
            return Ok(icon_svg(name).map(|s| Cow::Borrowed(s.as_bytes())));
        }
        if let Some(name) = path
            .strip_prefix("mascot/")
            .and_then(|p| p.strip_suffix(".png"))
        {
            return Ok(mascot_png(name).map(Cow::Borrowed));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_load() {
        for name in [
            "terminal",
            "monitor",
            "sparkle",
            "chevron-down",
            "more",
            "warning",
            "network",
            "check",
            "copy",
            "globe",
            "phone",
            "shield",
            "pointer",
            "key",
            "split",
            "sliders",
            "unlink",
            "win-min",
            "win-max",
            "win-restore",
        ] {
            let svg = AppAssets.load(&format!("ld/{name}.svg")).unwrap().unwrap();
            assert!(
                svg.starts_with(b"<svg") && svg.ends_with(b"</svg>"),
                "{name}"
            );
        }
        assert!(AppAssets.load("ld/nope.svg").unwrap().is_none());
    }
}
