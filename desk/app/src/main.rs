//! Lolidesk.app 的入口。界面在库里（`lolidesk_app`）。

fn main() {
    #[cfg(target_os = "macos")]
    lolidesk_app::run();

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("lolidesk-app 只有 macOS 版；这台机器上用命令行客户端 lolidesk。");
        std::process::exit(1);
    }
}
