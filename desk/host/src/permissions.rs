//! 远程桌面（和 computer use）要的两个系统权限：屏幕录制（采屏）、辅助功能（注入键鼠）。
//! 查的时候只问不弹框（`CGPreflightScreenCaptureAccess`、`AXIsProcessTrusted`）；
//! 用户在 app 里点「在主机上打开设置」才弹系统的授权框、打开设置页。
//! 授权跟着这个二进制走：给的是跑主机服务的那个 `lolidesk`。

use anyhow::Result;
use lolidesk_proto::PermissionKind;

/// 现在有没有：（屏幕录制，辅助功能）
#[cfg(target_os = "macos")]
pub fn status() -> Result<(bool, bool)> {
    Ok((mac::screen_recording(), mac::accessibility()))
}

#[cfg(not(target_os = "macos"))]
pub fn status() -> Result<(bool, bool)> {
    anyhow::bail!("这个平台没有远程桌面")
}

/// 弹系统的授权框（第一次时，也把 lolidesk 加进列表里），再打开这一项的设置页。
#[cfg(target_os = "macos")]
pub fn open(kind: PermissionKind) -> Result<()> {
    match kind {
        PermissionKind::ScreenRecording => mac::request_screen_recording(),
        PermissionKind::Accessibility => mac::request_accessibility(),
    }
    let ok = std::process::Command::new("/usr/bin/open")
        .arg(kind.settings_url())
        .status()?
        .success();
    anyhow::ensure!(ok, "打不开系统设置");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn open(_kind: PermissionKind) -> Result<()> {
    anyhow::bail!("这个平台没有远程桌面")
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFString, CFType};
    use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: *const CFDictionary) -> u8;
        static kAXTrustedCheckOptionPrompt: &'static CFString;
    }

    pub fn screen_recording() -> bool {
        CGPreflightScreenCaptureAccess()
    }

    pub fn accessibility() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    pub fn request_screen_recording() {
        CGRequestScreenCaptureAccess();
    }

    /// 带 prompt 问一次：没授权时系统弹框，并把这个程序加进「辅助功能」的列表
    pub fn request_accessibility() {
        unsafe {
            let yes: &CFType = CFBoolean::new(true);
            let key: &CFString = kAXTrustedCheckOptionPrompt;
            let opts = CFDictionary::<CFString, CFType>::from_slices(&[key], &[yes]);
            AXIsProcessTrustedWithOptions(opts.as_opaque());
        }
    }
}
