//! 远程桌面采集 + 编码。macOS：ScreenCaptureKit → VideoToolbox 硬件 H.264。其他平台先不支持。

use std::sync::{atomic::AtomicBool, Arc};

use lolidesk_proto::VideoSettings;

#[cfg(target_os = "macos")]
mod mac;

#[derive(Debug, Clone)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct CaptureConfig {
    pub display: u32,
    pub video: VideoSettings,
    /// 置 true 时下一帧编成关键帧（客户端请求、发送端丢了帧都会置）。
    pub force_key: Arc<AtomicBool>,
}

/// 采集实际用上的参数（按显示器收紧过）。
#[derive(Debug, Clone, Copy)]
pub struct Actual {
    pub width: u32,
    pub height: u32,
    pub fps: u8,
    pub bitrate: u32,
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// 给 agent 的一张截图（JPEG）
pub struct Shot {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub native_width: u32,
    pub native_height: u32,
}

#[cfg(target_os = "macos")]
pub use mac::{list_displays, screenshot, Capture};

#[cfg(not(target_os = "macos"))]
pub fn screenshot(_display: u32, _max_edge: u32, _quality: u8) -> anyhow::Result<Shot> {
    anyhow::bail!("这个平台还不支持截图")
}

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
    pub fn actual(&self) -> Actual {
        Actual {
            width: 0,
            height: 0,
            fps: 0,
            bitrate: 0,
        }
    }
    pub fn stop(self) {}
}

#[cfg(not(target_os = "macos"))]
pub fn list_displays() -> anyhow::Result<Vec<lolidesk_proto::DisplayInfo>> {
    anyhow::bail!("这个平台还不支持远程桌面")
}

/// 帧率夹在 1 到显示器刷新率之间（刷新率不知道就按 120 算）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn clamp_fps(want: u8, refresh: u16) -> u8 {
    let top = if refresh == 0 { 120 } else { refresh.min(240) };
    want.clamp(1, top.min(u8::MAX as u16) as u8)
}

#[cfg(test)]
mod tests {
    use super::clamp_fps;

    #[test]
    fn fps_follows_refresh() {
        assert_eq!(clamp_fps(60, 0), 60);
        assert_eq!(clamp_fps(120, 60), 60);
        assert_eq!(clamp_fps(0, 60), 1);
        assert_eq!(clamp_fps(120, 120), 120);
    }
}
