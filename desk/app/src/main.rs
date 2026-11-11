//! Lolidesk.app 的入口。界面在库里（`lolidesk_app`）。

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    lolidesk_app::run();

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("lolidesk-app 只有 macOS 和 Linux 版；这台机器上用命令行客户端 lolidesk。");
        std::process::exit(1);
    }
}
