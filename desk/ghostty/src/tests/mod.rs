//! 测试。要 libghostty-vt：设 LOLIDESK_GHOSTTY_VT，或者有 mygo 下载的缓存；
//! 都没有就打印“跳过”并通过（有的 CI 不下载库）。

mod encode;
mod handler;
mod layout;
mod mirror;
mod render;

/// 载入库；没有就打印跳过，返回 false。
pub(crate) fn lib() -> bool {
    match crate::load() {
        Ok(()) => true,
        Err(e) => {
            eprintln!("跳过：没有 libghostty-vt（{e:#}）");
            false
        }
    }
}

/// from..to 行，每行 `<prefix><i>\r\n`。
pub(crate) fn lines(from: usize, to: usize, prefix: &str) -> String {
    (from..to).map(|i| format!("{prefix}{i}\r\n")).collect()
}
