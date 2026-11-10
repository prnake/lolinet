//! Lolidesk.app：lolidesk 的图形客户端，GPUI + gpui-kit 画。
//!
//! 结构：
//! - 纯逻辑（各平台都编、都测）：[`keys`] 物理键位表，[`labels`] 文字和几何，[`video`] 拼帧和统计，[`prefs`] 偏好。
//! - 网络（macOS）：[`runtime`] 一个 tokio 运行时；[`desk`] 远程桌面的流、拼帧、解码线程；[`term`] 终端会话的流。
//! - 界面（macOS）：[`model`] 主机、会话、选中项（一个 GPUI 实体，所有视图都看它）；[`ui`] 各个视图。
//!
//! tokio ↔ GPUI：网络活在 tokio 的线程上，GPUI 那边 `cx.spawn` 一个任务去 await tokio 的
//! `JoinHandle` / `mpsc` / `Notify`（这些不挑执行器），拿到结果再 `entity.update` 改状态、`cx.notify()` 重画。
//! 画面一帧一帧来：解码线程把最新一帧放进槽里、`Notify` 一下，只有画面那个视图（`Stage`）重画。

pub mod backoff;
pub mod keys;
pub mod labels;
pub mod prefs;
pub mod termtext;
pub mod video;

#[cfg(target_os = "macos")]
pub mod assets;
#[cfg(target_os = "macos")]
pub mod desk;
#[cfg(target_os = "macos")]
pub mod model;
#[cfg(target_os = "macos")]
pub mod runtime;
#[cfg(target_os = "macos")]
pub mod term;
#[cfg(target_os = "macos")]
pub mod theme;
#[cfg(target_os = "macos")]
pub mod ui;

#[cfg(target_os = "macos")]
pub use ui::app::run;

/// 显示在侧栏底下的版本。
pub const VERSION: &str = lolidesk_proto::BUILD;
