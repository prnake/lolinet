//! 远程桌面采集 + 编码。macOS：ScreenCaptureKit → VideoToolbox 硬件 H.264。其他平台先不支持。

use lolidesk_proto::Quality;

#[cfg(target_os = "macos")]
mod mac;

#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct CaptureConfig {
    pub display: u32,
    pub quality: Quality,
    pub fps: u8,
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(target_os = "macos")]
pub use mac::{list_displays, Capture};

#[cfg(not(target_os = "macos"))]
pub struct Capture;

#[cfg(not(target_os = "macos"))]
impl Capture {
    pub fn start(
        _cfg: CaptureConfig,
        _tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        anyhow::bail!("这个平台还不支持远程桌面")
    }
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }
    pub fn request_keyframe(&self) {}
    pub fn set_quality(&self, _q: Quality) {}
    pub fn stop(self) {}
}

#[cfg(not(target_os = "macos"))]
pub fn list_displays() -> anyhow::Result<Vec<lolidesk_proto::DisplayInfo>> {
    anyhow::bail!("这个平台还不支持远程桌面")
}

/// 按画质档位算编码尺寸（偶数、保持比例）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn target_size(quality: Quality, px_w: u32, px_h: u32) -> (u32, u32) {
    let long = px_w.max(px_h).max(1);
    let cap = match quality {
        Quality::Smooth => 1280,
        Quality::Balanced => 1920,
        Quality::Sharp => 3840,
    };
    let scale = (cap as f64 / long as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(px_w as f64 * scale), even(px_h as f64 * scale))
}

/// 码率（bit/s）：按像素数和档位。1080p 大致 Smooth 4 Mbps / Balanced 10 / Sharp 24。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn bitrate(quality: Quality, w: u32, h: u32) -> u32 {
    let px = (w * h) as f64 / (1920.0 * 1080.0);
    let base = match quality {
        Quality::Smooth => 4.0e6,
        Quality::Balanced => 10.0e6,
        Quality::Sharp => 24.0e6,
    };
    (base * px.max(0.25)).clamp(1.0e6, 80.0e6) as u32
}
