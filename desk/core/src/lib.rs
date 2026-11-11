//! lolidesk 客户端核心：QUIC 连接（证书指纹钉扎）、记住的主机、H.264 硬解。
//!
//! 命令行 / TUI（`client/`）直接用；GUI（`app/`，Go + mygo）经 `app/core/rust` 的 C ABI 用
//! （那个 crate 编成 cdylib `liblolidesk_ffi`，mygo 要求它放在 Go 包的目录里）。

pub mod decode;
pub mod hosts;
pub mod local;
pub mod net;

pub use net::{connect, fingerprint, resolve, Client, Welcome};
