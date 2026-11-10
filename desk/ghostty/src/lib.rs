//! libghostty-vt（Ghostty 的终端核心）的绑定，运行时载入，没有链接期依赖。
//!
//! - 主机：每个会话一个 [`Terminal`]（喂 PTY 的输出），每个连上的客户端一个 [`Mirror`]，
//!   `frame` 出的帧（转义序列）让客户端的终端一格不差地跟上；[`Terminal::history`] 存盘，
//!   主机重启后放在新 shell 上面。
//! - 客户端：自己的 [`Terminal`] 吃帧（Handler 用 `()`，查询由主机回答），[`RenderState`]
//!   读出来画；键、鼠标、粘贴、焦点用 [`KeyEncoder`]、[`MouseEncoder`]、[`paste`]、[`focus`]
//!   按帧设好的模式编码。
//!
//! 库（libghostty-vt.dylib / .so）的来源、怎么找见 README.md。Mirror 和终端的接法移植自
//! mygo（MIT，Copyright (c) 2026 MyGo contributors），各文件开头有说明。
//!
//! 线程：Terminal、RenderState、编码器都是 Send、不是 Sync；一个终端的调用要排队。
//! 回调在 `Terminal::write`（和 resize、reset）里同步跑，不能回头调这个终端。

mod ffi;
mod input;
mod mirror;
mod render;
mod terminal;

#[cfg(test)]
mod tests;

pub use ffi::{library_path, Rgb, RowId, Scrollbar};
pub use input::{
    encode_paste, focus, paste, paste_is_safe, Key, KeyAction, KeyEncoder, KeyEvent, Mods,
    MouseAction, MouseButton, MouseEncoder, MouseEvent, MouseSize, OptionAsAlt,
};
pub use mirror::{Mirror, MOUSE_SHAPES};
pub use render::{
    Cell, Cells, Colors, Cursor, CursorStyle, Dirty, RenderState, Row, Rows, Underline, Width,
};
pub use terminal::{Handler, Mode, Point, Scroll, Selection, Terminal, Theme};

/// 找到并载入 libghostty-vt，只载一次（之后的调用报第一次的结果）。新建终端时也会调；
/// 想早点报“缺库”就先调它。
pub fn load() -> anyhow::Result<()> {
    ffi::lib_checked().map(|_| ())
}

/// 载入的库在哪（还没载入就先载入）。
pub fn loaded_path() -> anyhow::Result<std::path::PathBuf> {
    ffi::lib_checked().map(|l| l.path.clone())
}
