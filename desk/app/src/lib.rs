//! Lolidesk.app：lolidesk 的图形客户端，GPUI + gpui-kit 画。
//!
//! 结构：
//! - 纯逻辑（各平台都编、都测）：[`keys`] 物理键位表，[`labels`] 文字和几何，[`video`] 拼帧和统计，[`prefs`] 偏好，
//!   [`lolinet`] 本机 lolinet 命令行（网络页的数据，入网命令的校验），[`pairing`] 配对的文字和分组。
//! - 网络（macOS、Linux）：[`runtime`] 一个 tokio 运行时；[`desk`] 远程桌面的流、拼帧、解码线程；[`term`] 终端会话的流；
//!   [`service`] 本机主机的服务、系统通知、探网里的设备。
//! - 界面（macOS、Linux）：[`model`] 主机、会话、选中项（一个 GPUI 实体，所有视图都看它）；[`ui`] 各个视图。
//!   Linux 上还没有视频解码：远程桌面那一页只说明看不了，别的都一样。
//!
//! tokio ↔ GPUI：网络活在 tokio 的线程上，GPUI 那边 `cx.spawn` 一个任务去 await tokio 的
//! `JoinHandle` / `mpsc` / `Notify`（这些不挑执行器），拿到结果再 `entity.update` 改状态、`cx.notify()` 重画。
//! 画面一帧一帧来：解码线程把最新一帧放进槽里、`Notify` 一下，只有画面那个视图（`Stage`）重画。

pub mod backoff;
pub mod keys;
pub mod labels;
pub mod lolinet;
pub mod pairing;
pub mod prefs;
pub mod termtext;
pub mod video;

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod assets;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod desk;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod model;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod runtime;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod service;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod term;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod theme;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod ui;

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use ui::app::run;

/// 显示在侧栏底下的版本。
pub const VERSION: &str = lolidesk_proto::BUILD;

/// Linux 上窗口的 app_id（Wayland）/ WM_CLASS（X11）：和 lolidesk.desktop 的文件名、StartupWMClass 一样，
/// 任务栏才找得到图标。
pub const APP_ID: &str = "lolidesk";
