# lolidesk：lolinet 内网里的远程终端 + 远程桌面

一个主机进程、一个客户端二进制，两边都在 lolinet（EasyTier overlay，`10.188.0.0/16`）里直连，
不需要中继、账号和公网服务。设计参考了 Vibe42 0.13.3 的做法（拆解笔记 `VIBE42.md` 只放在私有仓库，不随代码公开），只取了和「内网远程 coding」有关的部分：

- **终端**：主机持会话（PTY + alacritty_terminal 模型），客户端接上来先收一份整屏快照再收增量字节。断线不丢、多端同看、重连只是再 Attach。
  客户端发「语义键」，由主机按终端模式（DECCKM、键盘应用模式、括号粘贴）编码；TUI 客户端直接透传本地字节。
- **远程桌面**（macOS 主机）：ScreenCaptureKit 采集 → VideoToolbox 硬件 H.264（低延迟档，不烧光标，空闲帧不编码）→ QUIC datagram 分片 →
  客户端 VideoToolbox 硬解 → egui 贴图。丢帧由客户端请求关键帧恢复。鼠标键盘用 CGEvent 注入，文本按 Unicode 注入不依赖键盘布局。
- **传输**：一条 QUIC 连接（quinn），控制 / 终端 / 桌面各自开流，视频走 datagram。主机自签证书，客户端第一次连时核对并记住指纹（TOFU），之后钉扎；共享 token 鉴权。

| crate | 产物 | 内容 |
|---|---|---|
| `proto/` | 库 | 消息（postcard）、datagram 分片头、语义键编码、猫眼调色板 |
| `host/` | `lolidesk-host` | QUIC 服务、PTY 会话、macOS 采屏编码（`desktop/mac.rs`）、输入注入（`input.rs`）、LaunchAgent 安装 |
| `client/` | `lolidesk` | GUI（egui + wgpu：左栏会话 / 显示器，中间终端格子或桌面画面）和 TUI（`--tui`，在当前终端里接会话） |

## 用

主机（被控的那台，先入网 lolinet）：

```
cargo build --release                 # 产物 target/release/lolidesk-host
./target/release/lolidesk-host token  # 打印 token 和证书指纹（第一次自动生成到 ~/Library/Application Support/lolidesk/host/ 或 ~/.config/lolidesk/host/）
./target/release/lolidesk-host run    # 前台跑，默认监听 <本机 10.188.x.x>:10190
./target/release/lolidesk-host install   # macOS：装成当前用户的 LaunchAgent co.lo-li.lolidesk（日志 ~/Library/Logs/lolidesk/host.log）
```

远程桌面要给 `lolidesk-host` 这个二进制开两个权限：系统设置 → 隐私与安全性 → **屏幕录制**（采屏）和 **辅助功能**（鼠标键盘注入）。
没有屏幕录制时 `ListDisplays` / `Start` 会返回带提示的错误；没有辅助功能时只能看不能控。

客户端：

```
lolidesk <主机地址> --token <token>           # 第一次：核对指纹后记住（~/.config/lolidesk/hosts，600）
lolidesk <主机地址>                           # 之后不用再给 token；默认开窗口并新开一个终端
lolidesk <主机地址> -s 3                      # 接到会话 3
lolidesk <主机地址> -d 0 -q sharp             # 开远程桌面，主显示器，清晰档
lolidesk <主机地址> --tui                     # 不开窗口，在当前终端里接；Ctrl-] 断开（会话留在主机）
lolidesk --list / --forget <主机地址>:10190
```

GUI 里：左栏「＋ 新终端」或点已有会话；点显示器开桌面；流畅 / 均衡 / 清晰三档（1280 / 1920 / 原生长边，码率 4 / 10 / 24 Mbps@1080p）；
「刷新画面」= 请求关键帧。终端区点一下获得键盘；Cmd+C/V 由系统剪贴板处理（粘贴走括号粘贴）。

## 协议（proto/src/lib.rs）

- 每条双向流首字节是流类型：0 控制、1 终端、2 桌面；之后是 `u32 LE 长度 + postcard` 的消息。
- 控制流：`Hello{version, token, client}` → `Welcome{host, os, version, desktop}` / `Denied`；`ListDisplays`、`ListSessions`。
- 终端流：第一条 `Open{cols, rows, command?, cwd?}` 或 `Attach{id, cols, rows}`；主机回 `Opened{id}`、`Snapshot{cols, rows, data}`，之后 `Output` / `Title` / `Bell` / `Exited`。
  客户端发 `Input(Key|Text|Paste|Raw)`、`Resize`、`Kill`。Resize 后主机给所有观看者发新快照（两端模型各自重排会分叉，以主机为准）。
- 桌面流：`Start{display, quality, max_fps}` → `Ready{width, height}`；`Keyframe`、`Quality`、`Input(Move|Button|Scroll|Key|Text)`（坐标 0..1 归一化）；`Stop`。
- datagram：`D5 | frame u32 | index u16 | count u16 | flags u8` + 分片；拼齐后是 `VideoFrame`：`key u8 [sps_len u16 sps pps_len u16 pps] avcc`。AVCC 即 4 字节大端长度前缀的 NALU。

## 分发

独立发包，不编进 `lolinet`。`lolinet` 要保持一个小的静态二进制（Linux 是 musl 静态链接，路由器和容器里也能直接跑）；
lolidesk 要链 ScreenCaptureKit 的 Swift 桥接、系统 Swift 运行时和 wgpu，只能动态链接，体积也大一个量级，并进去会把这两条都破坏掉。

- 包名 `lolidesk-<macos|linux>-<aarch64|x86_64>.tar.gz`，根目录是 `lolidesk-host`、`lolidesk`、`BUILD-INFO`，和 lolinet 的包同一种格式。
- CI（`desk.yml`）在 PR 和 main 上都编 macOS arm64 / x86_64 和 Linux x86_64（Linux 只有终端）。
- 发布是手动的：在 main 上运行 desk 并勾上 release，发一个 `lolidesk-<版本>` 的 GitHub Release。它不占 latest，lolinet 的自动升级认的是 latest。
  准入站的下载点用 `join/deploy.py dist` 传同样的包（带 `.sha256`）。
- 在 macOS 上编要 Xcode 26 以上（macOS 26 SDK）：采屏库带的 Metal 桥接用到了新 SDK 里的类型，旧 Xcode 编不过。
- 安装脚本里的可选安装步骤还没做，现在是手动下载解压。

## 还没做

- Linux / Windows 主机的采屏（PipeWire / DXGI）和客户端解码（VA-API / Media Foundation）；现在非 macOS 两端都只有终端。
- 远程剪贴板同步、文件传输、多显示器同时显示、应用菜单（Vibe42 的 `desktop.actions.v1`）、远程解锁。
- 自适应码率（现在按档位固定；丢帧只请求关键帧，不降档）。FEC。
- GUI 字体：现在是 egui 自带等宽字体；换成 Geist Mono（web/ 已有）并加 Nerd Font 回落。
