//! 离屏渲染快照：用 GPUI 的无窗口上下文和真的离屏渲染器（macOS 是 Metal，Linux 是 wgpu）画各个界面，
//! 设了 `LOLIDESK_SNAPSHOTS=<目录>` 就把 PNG 存进去（人看设计用）。
//! 顺带测按键路由：画面拿到焦点后 Tab / Shift+Tab / ⌘C / ⌘H 都发给远端，不被窗口拿走（只在 macOS：
//! Linux 上还没有远程画面，远程桌面那一页只说明看不了）。
//!
//! GPUI 的 Mac 平台只能在主线程建，所以这个测试不用 libtest（`harness = false`）。

fn main() {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    gui::run();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    println!("snapshots: 跳过（只有 macOS 和 Linux 有界面）");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod gui {
    #![allow(unsafe_code)]
    // 远程画面上的输入只在 macOS 上测：Linux 上那些帮手和引入用不到
    #![cfg_attr(not(target_os = "macos"), allow(unused_imports, dead_code))]

    /// 终端的快照和测试（tests/gui/term_cases.rs）。
    mod term_cases;

    #[cfg(target_os = "macos")]
    use std::ptr::{self, NonNull};
    use std::{
        net::SocketAddr,
        panic::{AssertUnwindSafe, catch_unwind},
        path::PathBuf,
        sync::Arc,
    };

    use gpui_kit::{
        AnyWindowHandle, AppContext, Bounds, Entity, HeadlessAppContext, InputEvent as _,
        Modifiers, MouseButton, ScrollDelta, ScrollEvent, TouchPhase, Window, WindowBounds,
        WindowOptions, point, px, size,
        test::{TestEventExt, TestWindowExt},
    };
    use lolidesk_app::{
        assets::AppAssets,
        desk::DeskSender,
        lolinet::{
            Invite, InviteKind, JoinTarget, Kernel, Lolinet, Node, Peer, Shell, Status as NetStatus,
        },
        model::{HostState, Model, Perms, Selection},
        prefs::Prefs,
        ui::{
            app::{Workspace, init},
            desktop::{DeskPane, Encoding},
            dialogs::{AddHostForm, open_form},
            network::InviteState,
        },
        video::StatsSnapshot,
    };
    use lolidesk_core::{decode::Frame, net::Welcome};
    use lolidesk_proto::{
        DeskInput, DeskReq, DisplayInfo, Launcher, MouseButton as PButton, SessionInfo,
    };
    #[cfg(target_os = "macos")]
    use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};
    #[cfg(target_os = "macos")]
    use objc2_core_video::{
        CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
        CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferIOSurfacePropertiesKey,
        kCVPixelBufferMetalCompatibilityKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    };
    use tokio::sync::mpsc::UnboundedReceiver;

    /// 一个测试：参数是存快照的目录（没设就不存）。
    type Case = fn(&Option<PathBuf>);

    /// 复制（终端有选区时）：macOS ⌘C，Linux Ctrl+Shift+C
    pub(super) const COPY_KEY: &str = if cfg!(target_os = "macos") {
        "cmd-c"
    } else {
        "ctrl-shift-c"
    };

    const W: f32 = 1280.;
    const H: f32 = 820.;

    pub fn run() {
        // 测试里改偏好（字号、本机主机开关）会存盘、记住主机会写 hosts：都写到一个临时的 HOME，
        // 别动跑测试的人自己的 app.json、hosts，也别读到这台真装着的主机服务。还没起别的线程，改环境变量是安全的
        let tmp_home =
            std::env::temp_dir().join(format!("lolidesk-snapshots-{}", std::process::id()));
        std::fs::create_dir_all(&tmp_home).expect("建临时 HOME");
        unsafe {
            std::env::set_var("HOME", &tmp_home);
            std::env::set_var("XDG_CONFIG_HOME", tmp_home.join(".config"));
        }
        // 没有 Metal 设备（比如某些虚拟机）、Linux 上没有 Vulkan / GL 就画不了：跳过，不算失败
        match gpui_kit::platform::current_headless_renderer() {
            Ok(Some(_)) => {}
            Ok(None) => return println!("snapshots: 跳过（没有离屏渲染器）"),
            Err(e) => return println!("snapshots: 跳过（离屏渲染器开不了：{e:#}）"),
        }
        let out = std::env::var_os("LOLIDESK_SNAPSHOTS").map(PathBuf::from);
        if let Some(dir) = &out {
            std::fs::create_dir_all(dir).expect("建快照目录");
        }
        let tests: &[(&str, Case)] = &[
            ("home", home),
            ("home_empty", home_empty),
            ("add_host", add_host),
            ("fingerprint", fingerprint),
            ("terminal", term_cases::terminal),
            ("terminal_alt", term_cases::terminal_alt),
            ("terminal_selection", term_cases::terminal_selection),
            ("terminal_ime", term_cases::terminal_ime),
            ("terminal_keys", term_cases::terminal_keys),
            ("terminal_reconnecting", term_cases::terminal_reconnecting),
            ("terminal_reattach", term_cases::terminal_reattach),
            ("terminal_perf", term_cases::terminal_perf),
            ("desktop", desktop),
            #[cfg(target_os = "macos")]
            ("desktop_menu", desktop_menu),
            ("desktop_states", desktop_states),
            ("new_menu", new_menu),
            ("permissions", permissions),
            ("desktop_permissions", desktop_permissions),
            ("network_not_joined", network_not_joined),
            ("network_joined", network_joined),
            ("network_down", network_down),
            ("invite_phone", invite_phone),
            ("invite_desktop", invite_desktop),
            // 远程画面上的输入：只有 macOS 有画面
            #[cfg(target_os = "macos")]
            ("keyboard_back", keyboard_back),
            #[cfg(target_os = "macos")]
            ("stage_keys", stage_keys),
            #[cfg(target_os = "macos")]
            ("stage_pointer", stage_pointer),
        ];
        // 终端的那些要 libghostty-vt（LOLIDESK_GHOSTTY_VT）；没有就跳过，同 lolidesk-ghostty 自己的测试
        let vt = lolidesk_ghostty::load();
        if let Err(e) = &vt {
            println!("snapshots: 终端的跳过（没有 libghostty-vt：{e:#}）");
        }
        let mut failed = 0;
        for (name, t) in tests {
            if name.starts_with("terminal") && vt.is_err() {
                continue;
            }
            match catch_unwind(AssertUnwindSafe(|| t(&out))) {
                Ok(()) => println!("snapshots::{name} ... ok"),
                Err(_) => {
                    println!("snapshots::{name} ... FAILED");
                    failed += 1;
                }
            }
        }
        if let Some(dir) = &out {
            println!("快照在 {}", dir.display());
        }
        let _ = std::fs::remove_dir_all(&tmp_home);
        if failed > 0 {
            std::process::exit(1);
        }
    }

    // ------------------------------------------------------------ 环境

    fn context() -> HeadlessAppContext {
        let mut cx = HeadlessAppContext::with_platform(
            gpui_kit::platform::current_platform(true).text_system(),
            Arc::new(AppAssets),
            gpui_kit::platform::current_headless_renderer,
        );
        cx.update(init);
        cx
    }

    fn open(
        cx: &mut HeadlessAppContext,
        model: Entity<Model>,
    ) -> (AnyWindowHandle, Entity<Workspace>) {
        let (handle, ws) = cx
            .update(|cx| {
                gpui_kit::open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds {
                            origin: Default::default(),
                            size: size(px(W), px(H)),
                        })),
                        // 窗口要是激活的，GPUI 才派发焦点事件（失焦 → ReleaseAll）
                        focus: true,
                        show: false,
                        ..Default::default()
                    },
                    cx,
                    |window, cx| cx.new(|cx| Workspace::new(model, window, cx)),
                )
            })
            .expect("开窗口");
        cx.update_window(handle, |_, window, _| window.activate_window())
            .unwrap();
        frame(cx, handle);
        (handle, ws)
    }

    fn frame(cx: &mut HeadlessAppContext, handle: AnyWindowHandle) {
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    fn with_window<R>(
        cx: &mut HeadlessAppContext,
        handle: AnyWindowHandle,
        f: impl FnOnce(&mut Window, &mut gpui_kit::App) -> R,
    ) -> R {
        cx.update_window(handle, |_, window, cx| f(window, cx))
            .unwrap()
    }

    /// 截图：存 PNG（设了目录时），并检查不是一片黑。
    fn shot(
        cx: &mut HeadlessAppContext,
        handle: AnyWindowHandle,
        out: &Option<PathBuf>,
        name: &str,
    ) {
        frame(cx, handle);
        let img = cx.capture_screenshot(handle).expect("离屏渲染");
        let lit = img.pixels().filter(|p| p.0[0] > 40 || p.0[1] > 40).count();
        assert!(lit > 500, "{name}：画面几乎是黑的（{lit} 个亮点）");
        if let Some(dir) = out {
            img.save(dir.join(format!("{name}.png"))).expect("存 PNG");
        }
    }

    // ------------------------------------------------------------ 假数据

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    const FP: &str = "3f9a1c0be2d47a8890f1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5d";

    fn session(id: u32, command: &str, title: &str, fg: &str, cwd: &str) -> SessionInfo {
        SessionInfo {
            id,
            title: title.into(),
            command: command.into(),
            foreground: fg.into(),
            cwd: cwd.into(),
            cols: 120,
            rows: 36,
            viewers: 1,
            started: 0,
            restored: false,
            exited: None,
        }
    }

    fn display(id: u32, name: &str, w: u32, h: u32, primary: bool) -> DisplayInfo {
        DisplayInfo {
            id,
            width: w,
            height: h,
            refresh: 120,
            primary,
            name: name.into(),
        }
    }

    fn fake_model(cx: &mut HeadlessAppContext) -> Entity<Model> {
        cx.new(|_| {
            let mut m = Model::empty(Prefs::default());
            let mut desk = HostState::new(addr("10.188.1.2:10190"), "desktop", FP, "t");
            let mut s3 = session(
                3,
                "claude",
                "✳ 修远程桌面的快捷键",
                "claude",
                "/Users/me/code/lolinet",
            );
            s3.viewers = 2;
            desk.set_online_for_test(
                Welcome {
                    host: "desktop".into(),
                    os: "macos".into(),
                    version: "0.2.0".into(),
                    desktop: true,
                    persistent: true,
                    allow: Default::default(),
                    role: lolidesk_proto::Role::Device("g-test".into()),
                },
                3.2,
                vec![
                    session(1, "", "desktop", "zsh", "/Users/me/code/lolinet"),
                    s3,
                    {
                        let mut s = session(5, "codex", "", "codex", "/Users/me/code/web");
                        s.exited = Some(0);
                        s
                    },
                ],
                vec![
                    display(1, "内建显示器", 3024, 1964, true),
                    display(4, "显示器 4", 2560, 1440, false),
                ],
                vec![
                    Launcher {
                        id: "shell".into(),
                        name: "终端".into(),
                        command: String::new(),
                    },
                    Launcher {
                        id: "claude".into(),
                        name: "Claude Code".into(),
                        command: "claude".into(),
                    },
                    Launcher {
                        id: "codex".into(),
                        name: "Codex".into(),
                        command: "codex".into(),
                    },
                ],
            );
            m.hosts.push(desk);

            let mut mini = HostState::new(addr("10.188.1.7:10190"), "mini", FP, "t");
            mini.set_online_for_test(
                Welcome {
                    host: "mini".into(),
                    os: "macos".into(),
                    version: "0.2.0".into(),
                    desktop: true,
                    persistent: false,
                    allow: Default::default(),
                    role: lolidesk_proto::Role::Device("g-test".into()),
                },
                11.0,
                vec![session(2, "", "", "htop", "/")],
                vec![],
                vec![],
            );
            mini.displays_error =
                Some("列不出显示器：没有屏幕录制权限（系统设置 → 隐私与安全性 → 屏幕录制）".into());
            m.hosts.push(mini);

            let mut server = HostState::new(addr("10.188.1.3:10190"), "server", FP, "t");
            server.set_failed_for_test("连 10.188.1.3:10190 超时");
            server.expanded = false;
            m.hosts.push(server);
            m
        })
    }

    /// Linux：没有解码器，画面是个空壳（远程桌面那一页只说明看不了）。
    #[cfg(not(target_os = "macos"))]
    fn fake_frame(_: usize, _: usize) -> Frame {
        Frame
    }

    /// 一帧假的远程画面：NV12 全范围、IOSurface 做底（GPUI 的 surface 只收这种）。
    #[cfg(target_os = "macos")]
    fn fake_frame(w: usize, h: usize) -> Frame {
        unsafe {
            let iosurface = CFDictionary::<CFString, CFType>::empty();
            let yes: &CFType = CFBoolean::new(true);
            let attrs = CFDictionary::<CFString, CFType>::from_slices(
                &[
                    kCVPixelBufferIOSurfacePropertiesKey,
                    kCVPixelBufferMetalCompatibilityKey,
                ],
                &[&iosurface, yes],
            );
            let mut out: *mut CVPixelBuffer = ptr::null_mut();
            let st = CVPixelBufferCreate(
                None,
                w,
                h,
                kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
                Some(attrs.as_opaque()),
                NonNull::from(&mut out),
            );
            assert_eq!(st, 0, "CVPixelBufferCreate");
            let pb = CFRetained::from_raw(NonNull::new(out).unwrap());
            CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags(0));
            // 亮度：深色桌面上的渐变，几块浅色「窗口」
            let y = CVPixelBufferGetBaseAddressOfPlane(&pb, 0) as *mut u8;
            let ys = CVPixelBufferGetBytesPerRowOfPlane(&pb, 0);
            let wins = [
                (0.08, 0.10, 0.45, 0.55, 225u8),
                (0.50, 0.22, 0.92, 0.80, 200u8),
                (0.12, 0.62, 0.40, 0.90, 170u8),
            ];
            for row in 0..h {
                for col in 0..w {
                    let (fx, fy) = (col as f32 / w as f32, row as f32 / h as f32);
                    let mut v = (30.0 + 40.0 * (fx + fy) / 2.0) as u8;
                    for (x0, y0, x1, y1, lum) in wins {
                        if fx >= x0 && fx < x1 && fy >= y0 && fy < y1 {
                            v = if fy < y0 + 0.035 { lum - 40 } else { lum };
                        }
                    }
                    if fy > 0.965 {
                        v = 60; // 底部的 Dock
                    }
                    *y.add(row * ys + col) = v;
                }
            }
            // 色度：大部分中性，一条猫眼绿、一条偏蓝
            let uv = CVPixelBufferGetBaseAddressOfPlane(&pb, 1) as *mut u8;
            let uvs = CVPixelBufferGetBytesPerRowOfPlane(&pb, 1);
            for row in 0..h / 2 {
                for col in 0..w / 2 {
                    let (fx, fy) = (col as f32 / (w / 2) as f32, row as f32 / (h / 2) as f32);
                    let (mut cb, mut cr) = (128u8, 128u8);
                    if (0.10..0.43).contains(&fx) && (0.16..0.20).contains(&fy) {
                        (cb, cr) = (40, 110);
                    }
                    if (0.52..0.90).contains(&fx) && (0.28..0.34).contains(&fy) {
                        (cb, cr) = (190, 100);
                    }
                    *uv.add(row * uvs + col * 2) = cb;
                    *uv.add(row * uvs + col * 2 + 1) = cr;
                }
            }
            CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags(0));
            Frame(pb)
        }
    }

    /// 在窗口里开一个假的远程桌面（不连主机），输入发到返回的接收端。
    fn open_desk(
        cx: &mut HeadlessAppContext,
        handle: AnyWindowHandle,
        model: &Entity<Model>,
    ) -> (Entity<DeskPane>, UnboundedReceiver<DeskReq>) {
        let frame_img = fake_frame(1920, 1246);
        let (sender, rx) = DeskSender::detached();
        let pane = with_window(cx, handle, |window, cx| {
            let d = model.read(cx).hosts[0].displays[0].clone();
            let pane = cx.new(|cx| {
                DeskPane::new(
                    model.downgrade(),
                    addr("10.188.1.2:10190"),
                    "desktop".into(),
                    d,
                    None,
                    window,
                    cx,
                )
            });
            pane.update(cx, |p, cx| {
                p.set_playing_for_test(
                    Encoding {
                        width: 1920,
                        height: 1246,
                        fps: 60,
                        bitrate: 25_800_000,
                    },
                    StatsSnapshot {
                        fps: 59.0,
                        mbps: 9.4,
                        decode_ms: 1.8,
                        dropped: 0,
                        keyframes: 1,
                        rtt_ms: 3.0,
                    },
                    Some(frame_img),
                    cx,
                );
                p.stage.update(cx, |s, _| s.set_sender(sender));
            });
            model.update(cx, |m, cx| {
                m.desk = Some(pane.clone());
                m.selection = Selection::Desk;
                cx.notify();
            });
            pane
        });
        frame(cx, handle);
        (pane, rx)
    }

    fn drain(rx: &mut UnboundedReceiver<DeskReq>) -> Vec<DeskInput> {
        let mut v = Vec::new();
        while let Ok(r) = rx.try_recv() {
            if let DeskReq::Input(i) = r {
                v.push(i);
            }
        }
        v
    }

    fn keys_of(inputs: &[DeskInput]) -> Vec<(String, bool, u8)> {
        inputs
            .iter()
            .filter_map(|i| match i {
                DeskInput::Key { code, down, mods } => Some((code.clone(), *down, mods.0)),
                _ => None,
            })
            .collect()
    }

    // ------------------------------------------------------------ 快照

    fn home(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        shot(&mut cx, handle, out, "home");
        // 出错的通知
        with_window(&mut cx, handle, |_, cx| {
            model.update(cx, |m, cx| {
                m.toast(
                    lolidesk_app::model::ToastKind::Error,
                    "连 10.188.1.3:10190 超时",
                    cx,
                )
            })
        });
        cx.advance_clock(std::time::Duration::from_millis(600));
        shot(&mut cx, handle, out, "toast");
        // 主机的右键菜单
        with_window(&mut cx, handle, |window, cx| {
            window.pointer_move(point(px(110.), px(109.)), None, cx);
            window.pointer_down(point(px(110.), px(109.)), MouseButton::Right, cx);
            window.pointer_up(point(px(110.), px(109.)), MouseButton::Right, cx);
        });
        cx.advance_clock(std::time::Duration::from_secs(1));
        shot(&mut cx, handle, out, "host-menu");
    }

    fn home_empty(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = cx.new(|_| Model::empty(Prefs::default()));
        let (handle, _) = open(&mut cx, model);
        shot(&mut cx, handle, out, "home-empty");
    }

    fn add_host(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        with_window(&mut cx, handle, |window, cx| {
            lolidesk_app::ui::dialogs::open_add_host(model.clone(), window, cx)
        });
        // 对话框有进场动画：等它走完
        cx.advance_clock(std::time::Duration::from_secs(1));
        shot(&mut cx, handle, out, "add-host");
    }

    fn fingerprint(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        with_window(&mut cx, handle, |window, cx| {
            let form = cx.new(|cx| AddHostForm::new(model.clone(), window, cx));
            let mut fp = [0u8; 32];
            hex_decode(FP, &mut fp);
            form.update(cx, |f, cx| {
                f.show_fingerprint(addr("10.188.1.9:10190"), fp, cx)
            });
            open_form(form, window, cx);
        });
        cx.advance_clock(std::time::Duration::from_secs(1));
        shot(&mut cx, handle, out, "fingerprint");
    }

    fn hex_decode(s: &str, out: &mut [u8; 32]) {
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
        }
    }

    fn desktop(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        let (pane, _rx) = open_desk(&mut cx, handle, &model);
        // 只有 macOS 解得了画面；别的平台不开流，只说明看不了
        let unsupported = pane.read_with(&cx, |p, _| p.unsupported);
        assert_eq!(unsupported, !cfg!(target_os = "macos"));
        shot(&mut cx, handle, out, "desktop");
        // 点画面：键盘交给远端（顶栏换成「键盘在远端」）
        let stage = pane.read_with(&cx, |p, _| p.stage.clone());
        with_window(&mut cx, handle, |window, cx| {
            stage.read(cx).focus.clone().focus(window, cx)
        });
        shot(&mut cx, handle, out, "desktop-focused");
    }

    #[cfg(target_os = "macos")]
    fn desktop_menu(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        let _ = open_desk(&mut cx, handle, &model);
        with_window(&mut cx, handle, |window, cx| window.click("tb-res", cx));
        cx.advance_clock(std::time::Duration::from_secs(1));
        shot(&mut cx, handle, out, "desktop-menu");
    }

    fn desktop_states(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        // 还没收到 Ready：转圈 +「正在打开远程桌面…」
        let pane = with_window(&mut cx, handle, |window, cx| {
            let d = model.read(cx).hosts[0].displays[1].clone();
            let pane = cx.new(|cx| {
                DeskPane::new(
                    model.downgrade(),
                    addr("10.188.1.2:10190"),
                    "desktop".into(),
                    d,
                    None,
                    window,
                    cx,
                )
            });
            model.update(cx, |m, cx| {
                m.desk = Some(pane.clone());
                m.selection = Selection::Desk;
                cx.notify();
            });
            pane
        });
        shot(&mut cx, handle, out, "desktop-opening");
        // 主机停了：「远程桌面已结束」+ 重新连接
        with_window(&mut cx, handle, |_, cx| {
            pane.update(cx, |p, cx| {
                p.error = Some("主机上的显示器拔掉了".into());
                p.stop(cx);
            })
        });
        shot(&mut cx, handle, out, "desktop-ended");
    }

    fn new_menu(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model);
        with_window(&mut cx, handle, |window, cx| {
            window.click(("new", 0usize), cx)
        });
        cx.advance_clock(std::time::Duration::from_secs(1));
        shot(&mut cx, handle, out, "new-menu");
    }

    // ------------------------------------------------------------ 权限引导

    /// 侧栏里 mini 下面的卡片：两个都没开；屏幕录制开了（变绿，要重启服务）、辅助功能还没开。
    fn permissions(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        let mini = addr("10.188.1.7:10190");
        with_window(&mut cx, handle, |_, cx| {
            model.update(cx, |m, cx| {
                let h = m.host_mut(mini).unwrap();
                h.set_perms(Perms {
                    screen: false,
                    access: false,
                });
                assert_eq!(h.guide_steps(), 2);
                cx.notify();
            })
        });
        shot(&mut cx, handle, out, "perm-missing");
        with_window(&mut cx, handle, |_, cx| {
            model.update(cx, |m, cx| {
                let h = m.host_mut(mini).unwrap();
                assert!(h.set_perms(Perms {
                    screen: true,
                    access: false,
                }));
                // 屏幕录制开了还差重启，辅助功能还没开
                assert_eq!(h.guide_steps(), 2);
                assert!(h.guide_offers_restart());
                cx.notify();
            })
        });
        shot(&mut cx, handle, out, "perm-one-granted");
        // 都开了、重启过（新连接）：卡片不见了
        with_window(&mut cx, handle, |_, cx| {
            model.update(cx, |m, cx| {
                let h = m.host_mut(mini).unwrap();
                h.guide = Default::default();
                h.set_perms(Perms {
                    screen: true,
                    access: true,
                });
                assert_eq!(h.guide_steps(), 0);
                cx.notify();
            })
        });
        frame(&mut cx, handle);
    }

    /// 远程桌面顶上的宽卡片：屏幕录制有了，辅助功能还没开（能看不能控）。
    fn desktop_permissions(out: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        with_window(&mut cx, handle, |_, cx| {
            model.update(cx, |m, _| {
                m.hosts[0].set_perms(Perms {
                    screen: true,
                    access: false,
                });
            })
        });
        let _ = open_desk(&mut cx, handle, &model);
        shot(&mut cx, handle, out, "desktop-permissions");
    }

    // ------------------------------------------------------------ 网络页

    /// 假的本机 lolinet：状态和邀请是给定的，要管理员的事一概不做。
    struct FakeNet {
        status: NetStatus,
    }

    impl Lolinet for FakeNet {
        fn status(&self) -> NetStatus {
            self.status.clone()
        }
        fn invite(&self, kind: InviteKind) -> anyhow::Result<Invite> {
            let code = "K7QX-M3PA-9TWE";
            let command = match kind {
                InviteKind::Phone => format!("https://join.example.com/m#{code}"),
                InviteKind::Userspace => format!(
                    "curl -fsSL https://join.example.com/i | sudo LOLINET_USERSPACE=1 sh -s {code}"
                ),
                InviteKind::Desktop => {
                    format!("curl -fsSL https://join.example.com/i | sudo sh -s {code}")
                }
            };
            Ok(Invite {
                code: code.into(),
                command,
                name: format!("{}-3f2a", kind.arg()),
                ip: "10.188.48.7".into(),
                kind: kind.arg().into(),
                expires_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
                    + 900,
            })
        }
        fn join(&self, _: &JoinTarget) -> anyhow::Result<String> {
            anyhow::bail!("测试里不入网")
        }
        fn set_exit(&self, _: bool) -> anyhow::Result<String> {
            anyhow::bail!("测试里不开出口节点")
        }
        fn install_cli(&self) -> anyhow::Result<String> {
            anyhow::bail!("测试里不装命令行")
        }
    }

    fn shell(lolinet: bool, lolidesk: bool) -> Shell {
        Shell {
            lolinet,
            lolidesk: lolidesk.then(|| "/usr/local/lib/lolidesk/lolidesk".into()),
            helper: Some("/Applications/Lolidesk.app/Contents/Helpers/lolidesk".into()),
            menu: None,
        }
    }

    fn peer(name: &str, ip: &str, direct: bool, proto: &str, lat: f64) -> Peer {
        Peer {
            name: name.into(),
            ip: ip.into(),
            direct,
            proto: proto.into(),
            lat_ms: Some(lat),
            relay: ip.starts_with("10.188.0."),
        }
    }

    fn joined_status() -> NetStatus {
        NetStatus {
            kernel: Kernel::Up {
                node: Node {
                    name: "studio".into(),
                    ip: "10.188.16.10".into(),
                    userspace: false,
                    version: "2.7.0".into(),
                },
                peers: vec![
                    peer("relay-hk", "10.188.0.1", true, "udp", 38.0),
                    peer("relay-sg", "10.188.0.2", false, "", 71.4),
                    peer("desktop", "10.188.1.2", true, "tcp", 3.2),
                    peer("mini", "10.188.1.7", true, "udp", 11.0),
                    peer("nas", "10.188.32.4", false, "", 46.8),
                ],
            },
            exit: Some(10180),
            shell: shell(true, false),
        }
    }

    /// 窗口里切到网络页，读一次假的本机 lolinet。
    fn open_network(
        cx: &mut HeadlessAppContext,
        status: NetStatus,
    ) -> (AnyWindowHandle, Entity<Workspace>, Entity<Model>) {
        let model = fake_model(cx);
        model.update(cx, |m, _| m.set_lolinet(Arc::new(FakeNet { status })));
        let (handle, ws) = open(cx, model.clone());
        with_window(cx, handle, |_, cx| {
            model.update(cx, |m, cx| m.select(Selection::Network, cx))
        });
        frame(cx, handle);
        assert!(
            model.read_with(cx, |m, _| m.net.status.is_some()),
            "网络页没读到状态"
        );
        (handle, ws, model)
    }

    fn network_not_joined(out: &Option<PathBuf>) {
        let mut cx = context();
        let status = NetStatus {
            kernel: Kernel::NotJoined,
            exit: None,
            shell: shell(true, false),
        };
        let (handle, ws, _) = open_network(&mut cx, status);
        shot(&mut cx, handle, out, "network-join");
        // 粘贴一条配对命令、勾上无 TUN：下面出现要跑的命令
        with_window(&mut cx, handle, |window, cx| {
            let page = ws.read(cx).network.clone();
            let line = page.read(cx).line.clone();
            line.update(cx, |i, cx| {
                i.set_value(
                    "curl -fsSL https://join.example.com/i | sudo sh -s k7qx-m3pa-9twe",
                    window,
                    cx,
                )
            });
            page.update(cx, |p, cx| p.set_userspace(true, cx));
        });
        shot(&mut cx, handle, out, "network-join-filled");
        // 坏的命令：只认校验过的几段
        with_window(&mut cx, handle, |window, cx| {
            let page = ws.read(cx).network.clone();
            let line = page.read(cx).line.clone();
            line.update(cx, |i, cx| {
                i.set_value(
                    "curl -fsSL https://join.example.com/i | sudo sh -s K7QX-M3PA-9TWE; rm -rf ~",
                    window,
                    cx,
                )
            });
            page.update(cx, |p, cx| p.join(cx));
        });
        shot(&mut cx, handle, out, "network-join-bad");
    }

    fn network_joined(out: &Option<PathBuf>) {
        let mut cx = context();
        let (handle, _, _) = open_network(&mut cx, joined_status());
        shot(&mut cx, handle, out, "network-joined");
    }

    fn network_down(out: &Option<PathBuf>) {
        let mut cx = context();
        let status = NetStatus {
            kernel: Kernel::Down("dial tcp 127.0.0.1:15888: connection refused".into()),
            exit: None,
            shell: shell(true, true),
        };
        let (handle, _, _) = open_network(&mut cx, status);
        shot(&mut cx, handle, out, "network-down");
    }

    fn open_invite_for(out: &Option<PathBuf>, kind: InviteKind, name: &str) {
        let mut cx = context();
        let (handle, _, model) = open_network(&mut cx, joined_status());
        let form = with_window(&mut cx, handle, |window, cx| {
            lolidesk_app::ui::network::open_invite(model.clone(), window, cx)
        });
        with_window(&mut cx, handle, |_, cx| {
            form.update(cx, |f, cx| {
                f.set_kind(kind, cx);
                f.generate(cx);
            })
        });
        cx.advance_clock(std::time::Duration::from_secs(1));
        frame(&mut cx, handle);
        assert!(
            form.read_with(&cx, |f, _| matches!(f.state, InviteState::Done(_))),
            "{name}：没拿到配对码"
        );
        shot(&mut cx, handle, out, name);
    }

    fn invite_phone(out: &Option<PathBuf>) {
        open_invite_for(out, InviteKind::Phone, "invite-phone");
    }

    fn invite_desktop(out: &Option<PathBuf>) {
        open_invite_for(out, InviteKind::Desktop, "invite-desktop");
    }

    /// 点侧栏把键盘从远程画面拿回来：画面失焦、远端松开所有键、焦点回到窗口。
    #[cfg(target_os = "macos")]
    fn keyboard_back(_: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, ws) = open(&mut cx, model.clone());
        let (pane, mut rx) = open_desk(&mut cx, handle, &model);
        let stage = pane.read_with(&cx, |p, _| p.stage.clone());
        with_window(&mut cx, handle, |window, cx| {
            stage.read(cx).focus.clone().focus(window, cx);
            window.render_frame(cx);
        });
        drain(&mut rx);
        with_window(&mut cx, handle, |window, cx| {
            window.pointer_down(point(px(130.), px(760.)), MouseButton::Left, cx);
            window.pointer_up(point(px(130.), px(760.)), MouseButton::Left, cx);
        });
        frame(&mut cx, handle);
        with_window(&mut cx, handle, |window, cx| {
            use gpui_kit::Focusable as _;
            assert!(!stage.read(cx).focus.is_focused(window), "画面还拿着键盘");
            assert!(
                ws.read(cx).focus_handle(cx).is_focused(window),
                "焦点没回到窗口"
            );
        });
        let got = drain(&mut rx);
        assert!(
            got.iter().any(|i| matches!(i, DeskInput::ReleaseAll)),
            "没发 ReleaseAll：{got:?}"
        );
    }

    // ------------------------------------------------------------ 按键和指针

    #[cfg(target_os = "macos")]
    fn stage_keys(_: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        let (pane, mut rx) = open_desk(&mut cx, handle, &model);
        let stage = pane.read_with(&cx, |p, _| p.stage.clone());
        with_window(&mut cx, handle, |window, cx| {
            stage.read(cx).focus.clone().focus(window, cx);
            window.render_frame(cx);
        });
        drain(&mut rx);

        with_window(&mut cx, handle, |window, cx| {
            window.key_down("tab", false, cx);
            window.key_up("tab", cx);
            window.key_down("shift-tab", false, cx);
            window.key_down("cmd-c", false, cx);
            window.key_down("cmd-h", false, cx);
            window.key_down("up", false, cx);
            window.key_down("f5", false, cx);
            // 焦点没被 Tab 挪走
            assert!(stage.read(cx).focus.is_focused(window), "Tab 把焦点挪走了");
        });
        let got = keys_of(&drain(&mut rx));
        let want: Vec<(String, bool, u8)> = vec![
            ("Tab".into(), true, 0),
            ("Tab".into(), false, 0),
            ("Tab".into(), true, 1),
            ("KeyC".into(), true, 8),
            ("KeyC".into(), false, 8),
            ("KeyH".into(), true, 8),
            ("KeyH".into(), false, 8),
            ("ArrowUp".into(), true, 0),
            ("F5".into(), true, 0),
        ];
        assert_eq!(got, want);

        // 修饰键单独按下、松开
        with_window(&mut cx, handle, |window, cx| {
            window.change_modifiers(
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
                cx,
            );
            window.change_modifiers(Modifiers::default(), cx);
        });
        let got = keys_of(&drain(&mut rx));
        assert_eq!(
            got,
            vec![
                ("ShiftLeft".into(), true, 1),
                ("ShiftLeft".into(), false, 0)
            ]
        );

        // 失焦：远端松开所有键
        with_window(&mut cx, handle, |window, cx| {
            window.blur(cx);
            window.render_frame(cx);
        });
        cx.run_until_parked();
        let got = drain(&mut rx);
        assert!(
            got.iter().any(|i| matches!(i, DeskInput::ReleaseAll)),
            "失焦没发 ReleaseAll：{got:?}"
        );
    }

    #[cfg(target_os = "macos")]
    fn stage_pointer(_: &Option<PathBuf>) {
        let mut cx = context();
        let model = fake_model(&mut cx);
        let (handle, _) = open(&mut cx, model.clone());
        let (pane, mut rx) = open_desk(&mut cx, handle, &model);
        let stage = pane.read_with(&cx, |p, _| p.stage.clone());
        // 画面区域：侧栏右边、顶栏下面；画面等比放在里面
        let (sx, sy, sw, sh) = (260., 52., W - 260., H - 52.);
        let pic =
            lolidesk_app::labels::fit(lolidesk_app::labels::Rect::new(sx, sy, sw, sh), 1920, 1246);
        let at = |fx: f32, fy: f32| point(px(pic.x + pic.w * fx), px(pic.y + pic.h * fy));

        with_window(&mut cx, handle, |window, cx| {
            window.pointer_move(at(0.25, 0.25), None, cx);
            window.pointer_down(at(0.5, 0.5), MouseButton::Left, cx);
            window.pointer_up(at(0.5, 0.5), MouseButton::Left, cx);
            assert!(stage.read(cx).focus.is_focused(window), "点画面没拿到焦点");
            // 滚动：向下滚 50 px（内容往上走，负值）
            window.dispatch_event(
                ScrollEvent {
                    position: at(0.5, 0.5),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-50.))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
        let got = drain(&mut rx);
        let close = |a: f32, b: f32| (a - b).abs() < 0.01;
        assert!(
            matches!(got.first(), Some(DeskInput::Move { x, y }) if close(*x, 0.25) && close(*y, 0.25)),
            "{got:?}"
        );
        assert!(
            got.iter().any(|i| matches!(i, DeskInput::Button { x, y, button: PButton::Left, down: true } if close(*x, 0.5) && close(*y, 0.5))),
            "{got:?}"
        );
        assert!(
            got.iter().any(|i| matches!(
                i,
                DeskInput::Button {
                    button: PButton::Left,
                    down: false,
                    ..
                }
            )),
            "{got:?}"
        );
        assert!(
            got.iter().any(|i| matches!(i, DeskInput::Scroll { dy, dx, .. } if close(*dy, -50.0 / pic.h) && *dx == 0.0)),
            "{got:?}"
        );

        // 工具条上的指针不发给远端
        let toolbar_y = sy + sh - 16. - 21.;
        with_window(&mut cx, handle, |window, cx| {
            window.pointer_move(point(px(sx + sw / 2.), px(toolbar_y)), None, cx);
        });
        let got = drain(&mut rx);
        assert!(
            !got.iter().any(|i| matches!(i, DeskInput::Move { .. })),
            "工具条上的指针发给了远端：{got:?}"
        );
    }
}
