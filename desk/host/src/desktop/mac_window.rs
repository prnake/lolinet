//! macOS 的窗口：列出开着的窗口（ScreenCaptureKit 给窗口，辅助功能看哪些最小化了）、
//! 推窗口时每秒看一次它还在不在（CGWindowList，便宜）、把窗口提到最前面（辅助功能：
//! 恢复最小化、激活程序、AXRaise），这样远程的键鼠才落在它上面。

use std::{collections::HashSet, ffi::c_void, ptr::NonNull};

use anyhow::{bail, Context, Result};
use lolidesk_proto::WindowInfo;
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGRect,
};
use objc2_core_graphics::{
    kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowNumber, kCGWindowOwnerPID,
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGGetActiveDisplayList,
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use screencapturekit::prelude::*;

use super::{pick_windows, screen_of, Area, RawWindow, Screen, WinState};

pub fn area_of(r: CGRect) -> Area {
    Area {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    }
}

/// ScreenCaptureKit 自己的 CGRect（同样的字段，不同的类型）
pub fn sc_area(r: screencapturekit::cg::CGRect) -> Area {
    Area {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    }
}

/// 现在接着的显示器：范围（点）和点到像素的倍数
pub fn screens() -> Vec<Screen> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    let err = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    if err != objc2_core_graphics::CGError::Success {
        return Vec::new();
    }
    ids[..n as usize]
        .iter()
        .map(|&id| {
            let bounds = area_of(CGDisplayBounds(id));
            let px_w = CGDisplayCopyDisplayMode(id)
                .map(|m| CGDisplayMode::pixel_width(Some(&m)) as f64)
                .unwrap_or(bounds.w);
            Screen {
                id,
                bounds,
                scale: if bounds.w > 0.0 { px_w / bounds.w } else { 1.0 },
            }
        })
        .collect()
}

/// 主机上开着的窗口（要屏幕录制权限；辅助功能没开时最小化的窗口列不出来）。
/// 客户端每几秒问一次：没权限只回错，不弹系统的授权框（列显示器、开远程桌面时才弹）
pub fn list_windows() -> Result<Vec<WindowInfo>> {
    if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
        bail!("没有屏幕录制权限：系统设置 → 隐私与安全性 → 屏幕录制，勾上 lolidesk，然后重启服务（lolidesk host install）");
    }
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(false)
        .with_exclude_desktop_windows(true)
        .get()
        .context("SCShareableContent")?;
    let raw: Vec<RawWindow> = content
        .windows()
        .into_iter()
        .filter_map(|w| {
            let app = w.owning_application()?;
            Some(RawWindow {
                id: w.window_id(),
                pid: app.process_id(),
                app: app.application_name(),
                bundle: app.bundle_identifier(),
                title: w.title().unwrap_or_default(),
                layer: w.window_layer(),
                on_screen: w.is_on_screen(),
                frame: sc_area(w.frame()),
            })
        })
        .collect();
    // 不在屏幕上的普通窗口里，哪些是最小化了（问它们的程序）
    let pids: HashSet<i32> = raw
        .iter()
        .filter(|w| !w.on_screen && w.layer == 0)
        .map(|w| w.pid)
        .collect();
    let minimized = minimized_windows(&pids);
    Ok(pick_windows(
        raw,
        std::process::id() as i32,
        &minimized,
        &screens(),
    ))
}

/// 这些程序里最小化了的窗口（辅助功能没开就是空的）
fn minimized_windows(pids: &HashSet<i32>) -> HashSet<u32> {
    let mut out = HashSet::new();
    if !trusted() {
        return out;
    }
    for &pid in pids {
        let Some(app) = Ax::app(pid) else { continue };
        for w in app.windows() {
            if w.bool_attr("AXMinimized") == Some(true) {
                if let Some(id) = w.window_id() {
                    out.insert(id);
                }
            }
        }
    }
    out
}

/// 一个窗口的那几项（CGWindowList 的字典）
struct CgWindow {
    id: u32,
    pid: i32,
    layer: i64,
    bounds: CGRect,
    on_screen: bool,
}

fn cg_windows(option: CGWindowListOption, relative_to: u32) -> Vec<CgWindow> {
    let Some(list) = CGWindowListCopyWindowInfo(option, relative_to) else {
        return Vec::new();
    };
    // 每一项是 CFDictionary<CFString, CFType>
    let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
    list.iter()
        .filter_map(|d| {
            let num = |k: &CFString| {
                d.get(k)
                    .and_then(|v| v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64()))
            };
            let id = num(unsafe { kCGWindowNumber })? as u32;
            let mut bounds = CGRect::default();
            let ok = d
                .get(unsafe { kCGWindowBounds })
                .and_then(|v| v.downcast::<CFDictionary>().ok())
                .is_some_and(|b| unsafe {
                    CGRectMakeWithDictionaryRepresentation(Some(&b), &mut bounds)
                });
            if !ok {
                return None;
            }
            let on_screen = d
                .get(unsafe { kCGWindowIsOnscreen })
                .and_then(|v| v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()))
                .unwrap_or(false);
            Some(CgWindow {
                id,
                pid: num(unsafe { kCGWindowOwnerPID }).unwrap_or(0) as i32,
                layer: num(unsafe { kCGWindowLayer }).unwrap_or(0),
                bounds,
                on_screen,
            })
        })
        .collect()
}

fn cg_window(id: u32) -> Option<CgWindow> {
    cg_windows(CGWindowListOption::OptionIncludingWindow, id)
        .into_iter()
        .find(|w| w.id == id)
}

/// 推着的窗口现在怎么样：没了是 None
pub fn window_state(id: u32) -> Option<WinState> {
    let w = cg_window(id)?;
    let area = area_of(w.bounds);
    let screens = screens();
    let screen = screen_of(&area, &screens);
    let scale = screen.map_or(1.0, |s| s.scale);
    Some(WinState {
        area,
        px: (
            (area.w * scale).round() as u32,
            (area.h * scale).round() as u32,
        ),
        on_screen: w.on_screen,
        display: screen.map_or(0, |s| s.id),
    })
}

/// 这个窗口是不是最前面的普通窗口（远程点它之前看一眼，不是就先提上来）
pub fn is_front(id: u32) -> bool {
    cg_windows(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    )
    .into_iter()
    .find(|w| w.layer == 0)
    .is_some_and(|w| w.id == id)
}

/// 把窗口提到最前面：最小化的先恢复，激活它的程序（藏起来的也会出来），再 AXRaise。
/// 要辅助功能权限。
pub fn raise_window(id: u32) -> Result<()> {
    if !trusted() {
        bail!("没有辅助功能权限，提不了窗口（系统设置 → 隐私与安全性 → 辅助功能）");
    }
    let pid = match cg_window(id) {
        Some(w) => w.pid,
        None => bail!("窗口不在了（关掉了？）"),
    };
    let app = Ax::app(pid).context("连不上这个程序（辅助功能）")?;
    let win = app
        .windows()
        .into_iter()
        .find(|w| w.window_id() == Some(id))
        .context("程序没交出这个窗口（辅助功能里找不到）")?;
    if win.bool_attr("AXMinimized") == Some(true) {
        win.set_bool("AXMinimized", false);
    }
    app.set_bool("AXFrontmost", true);
    win.set_bool("AXMain", true);
    win.action("AXRaise");
    Ok(())
}

// ---------------------------------------------------------------- 辅助功能（AXUIElement）

type AXUIElementRef = *const c_void;
type AXError = i32;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: &CFString,
        value: *mut *const c_void,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: &CFString,
        value: &CFType,
    ) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: &CFString) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
    /// 私有但一直在用（yabai、Hammerspoon、Rectangle 都靠它）：辅助功能的窗口 → CGWindowID
    fn _AXUIElementGetWindow(element: AXUIElementRef, out: *mut u32) -> AXError;
}

fn trusted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// 一个 AXUIElement（CF 对象，引用计数由 CFRetained 管）
struct Ax(CFRetained<CFType>);

impl Ax {
    /// 一个程序。卡住的程序最多等 0.3 秒，别让列窗口跟着卡
    fn app(pid: i32) -> Option<Ax> {
        let p = unsafe { AXUIElementCreateApplication(pid) };
        let el = Ax(unsafe { CFRetained::from_raw(NonNull::new(p as *mut CFType)?) });
        unsafe { AXUIElementSetMessagingTimeout(el.ptr(), 0.3) };
        Some(el)
    }

    fn ptr(&self) -> AXUIElementRef {
        let r: &CFType = &self.0;
        r as *const CFType as AXUIElementRef
    }

    fn attr(&self, name: &str) -> Option<CFRetained<CFType>> {
        let key = CFString::from_str(name);
        let mut out: *const c_void = std::ptr::null();
        if unsafe { AXUIElementCopyAttributeValue(self.ptr(), &key, &mut out) } != 0 {
            return None;
        }
        NonNull::new(out as *mut CFType).map(|p| unsafe { CFRetained::from_raw(p) })
    }

    fn windows(&self) -> Vec<Ax> {
        let Some(v) = self.attr("AXWindows") else {
            return Vec::new();
        };
        let Some(arr) = v.downcast_ref::<CFArray>() else {
            return Vec::new();
        };
        let arr: &CFArray<CFType> = unsafe { arr.cast_unchecked() };
        arr.iter().map(Ax).collect()
    }

    fn window_id(&self) -> Option<u32> {
        let mut id = 0u32;
        (unsafe { _AXUIElementGetWindow(self.ptr(), &mut id) } == 0 && id != 0).then_some(id)
    }

    fn bool_attr(&self, name: &str) -> Option<bool> {
        self.attr(name)?
            .downcast_ref::<CFBoolean>()
            .map(|b| b.as_bool())
    }

    fn set_bool(&self, name: &str, value: bool) -> bool {
        let key = CFString::from_str(name);
        let v: &CFType = CFBoolean::new(value);
        unsafe { AXUIElementSetAttributeValue(self.ptr(), &key, v) == 0 }
    }

    fn action(&self, name: &str) -> bool {
        let key = CFString::from_str(name);
        unsafe { AXUIElementPerformAction(self.ptr(), &key) == 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 读真的窗口（CGWindowList 不要屏幕录制权限就给框和在不在屏幕上）：字典解得开、
    /// 窗口的状态对得上、没有的窗口是 None。一个普通窗口都没有的机器（无界面的 CI）跳过
    #[test]
    fn reads_real_windows() {
        assert!(!screens().is_empty(), "一块显示器都没有");
        assert!(window_state(u32::MAX - 7).is_none());
        let on = cg_windows(
            CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
            0,
        );
        let Some(w) = on
            .iter()
            .find(|w| w.layer == 0 && w.bounds.size.width >= 64.0 && w.bounds.size.height >= 48.0)
        else {
            println!("没有屏幕上的普通窗口，跳过");
            return;
        };
        // 刚好关掉了也算过
        let Some(st) = window_state(w.id) else { return };
        assert!(st.on_screen, "{st:?}");
        assert_eq!(st.area, area_of(w.bounds));
        assert!(
            st.px.0 >= st.area.w as u32 && st.px.1 >= st.area.h as u32,
            "{st:?}"
        );
        assert!(screens().iter().any(|s| s.id == st.display), "{st:?}");
        assert!(w.pid > 0);
        // 最前面的普通窗口就是列表里的第一个
        assert!(is_front(on.iter().find(|w| w.layer == 0).unwrap().id));
    }
}
