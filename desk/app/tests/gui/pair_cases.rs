//! 配对的快照和测试：本机在侧栏最上面（有会话）、本机开不起来、有设备请求连这台（一个、两个排队）、「访问」页
//! （主人看全部、设备看自己、限制的开关）、新配对码（码、链接、二维码）、网里的设备和「正在请 X 同意…」、
//! 导入分享链接、添加主机时填配对码。
//! 再用假的服务测：本机主机没跑过时替用户装一次服务、开发版只说明不装、
//! 配对请求在窗口不在前台时每个只发一次系统通知、答过的不再弹。都不起进程、不连网。

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui_kit::{
    AppContext, Entity, HeadlessAppContext,
    test::{TestEventExt, TestWindowExt},
};
use lolidesk_app::{
    model::{HostState, Link, LocalState, Model, NewCode, Selection},
    service::{Found, Probe, Service},
    ui::{access, dialogs::AddHostForm, pair},
};
use lolidesk_core::{local::LocalHost, net::Welcome};
use lolidesk_proto::{
    AccessInfo, Allow, CodeInfo, DeviceInfo, GrantInfo, Launcher, PairLink, PendingInfo, Role,
};

use super::{
    FP, addr, context, display, fake_model, frame, hex_decode, open, session, shot, with_window,
};

const LOCAL: &str = "127.0.0.1:10190";
/// 快照里的「现在」：时间都按它算（「最近 2 分钟前」）
fn now() -> u64 {
    lolidesk_app::pairing::now()
}

fn fp() -> [u8; 32] {
    let mut b = [0u8; 32];
    hex_decode(FP, &mut b);
    b
}

// ------------------------------------------------------------ 假的服务

#[derive(Default)]
struct Calls {
    installs: u32,
    uninstalls: u32,
    /// 装、撤时用的程序
    programs: Vec<PathBuf>,
    notes: Vec<String>,
}

/// 本机主机、通知、探设备都是假的：记下调了什么，不起进程、不连网。
struct FakeService {
    found: Option<LocalHost>,
    helper: Option<PathBuf>,
    /// 已经装着的服务跑的程序：装了就是装的那个，撤了就没有
    installed: Mutex<Option<PathBuf>>,
    calls: Arc<Mutex<Calls>>,
}

const APP_HELPER: &str = "/Applications/Lolidesk.app/Contents/Helpers/lolidesk";

impl Service for FakeService {
    fn find(&self) -> Option<LocalHost> {
        self.found.clone()
    }
    fn helper(&self) -> Option<PathBuf> {
        self.helper.clone()
    }
    fn installed(&self) -> Option<PathBuf> {
        self.installed.lock().unwrap().clone()
    }
    fn install(&self, program: &Path) -> anyhow::Result<String> {
        let mut c = self.calls.lock().unwrap();
        c.installs += 1;
        c.programs.push(program.into());
        *self.installed.lock().unwrap() = Some(program.into());
        Ok("已装（假的）".into())
    }
    fn uninstall(&self, program: &Path) -> anyhow::Result<String> {
        let mut c = self.calls.lock().unwrap();
        c.uninstalls += 1;
        c.programs.push(program.into());
        *self.installed.lock().unwrap() = None;
        Ok("已撤（假的）".into())
    }
    fn notify(&self, text: &str) {
        self.calls.lock().unwrap().notes.push(text.into());
    }
    fn probe(&self, _: SocketAddr) -> Probe {
        Box::pin(async { None })
    }
}

fn fake_service(helper: bool) -> (Arc<FakeService>, Arc<Mutex<Calls>>) {
    fake_service_with(helper, None, None)
}

fn fake_service_with(
    helper: bool,
    found: Option<LocalHost>,
    installed: Option<PathBuf>,
) -> (Arc<FakeService>, Arc<Mutex<Calls>>) {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let svc = Arc::new(FakeService {
        found,
        helper: helper.then(|| APP_HELPER.into()),
        installed: Mutex::new(installed),
        calls: calls.clone(),
    });
    (svc, calls)
}

/// 「命令行装的 lolidesk」（像 /usr/local/lib/lolidesk/lolidesk）：一个真的文件，app 看得到它还在
fn system_lolidesk() -> PathBuf {
    let p = std::env::temp_dir().join(format!("lolidesk-system-{}", std::process::id()));
    std::fs::write(&p, b"").expect("写假的 lolidesk");
    p
}

// ------------------------------------------------------------ 假数据

fn dev(name: &str, os: &str, kind: &str) -> DeviceInfo {
    DeviceInfo {
        name: name.into(),
        os: os.into(),
        kind: kind.into(),
        version: "0.2.0".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn grant(
    id: &str,
    d: DeviceInfo,
    code: Option<&str>,
    ago: u64,
    ip: &str,
    online: u32,
    allow: Option<Allow>,
) -> GrantInfo {
    GrantInfo {
        id: id.into(),
        device: d,
        code: code.map(Into::into),
        created: now() - 30 * 86400,
        created_addr: format!("{ip}:52011"),
        last_seen: now() - ago,
        last_addr: format!("{ip}:52011"),
        allow,
        online,
    }
}

fn code(
    id: &str,
    label: &str,
    by: Option<&str>,
    expires: Option<u64>,
    max: Option<u32>,
    uses: u32,
    allow: Option<Allow>,
) -> CodeInfo {
    CodeInfo {
        id: id.into(),
        label: label.into(),
        created: now() - 86400,
        created_by: by.map(Into::into),
        expires: expires.map(|d| now() + d),
        max_uses: max,
        uses,
        allow,
    }
}

fn pending(id: u64, name: &str, ip: &str, ago: u64) -> PendingInfo {
    PendingInfo {
        id,
        device: dev(name, "macos", "app"),
        addr: format!("{ip}:60512"),
        since: now() - ago,
    }
}

/// 本机：经主人同意的两台（一台限制了）、主人发的码（一台用了）、laptop 发的分享链接（nas 用了）、
/// 一个还没人用的码、一个等着同意的请求。
fn owner_access() -> AccessInfo {
    let no_cua = Allow {
        cua: false,
        share: false,
        ..Allow::default()
    };
    AccessInfo {
        owner: true,
        me: None,
        grants: vec![
            grant(
                "g-7c1a0e",
                dev("laptop", "macos", "app"),
                None,
                120,
                "10.188.16.11",
                1,
                None,
            ),
            grant(
                "g-2d9e44",
                dev("mini", "macos", "cli"),
                None,
                3 * 3600,
                "10.188.1.7",
                0,
                Some(Allow::view_only()),
            ),
            grant(
                "g-41f0b2",
                dev("air", "macos", "app"),
                Some("c-8b2071"),
                2 * 86400,
                "10.188.16.21",
                0,
                None,
            ),
            grant(
                "g-93aa5c",
                dev("nas", "linux", "cli"),
                Some("c-5e17d3"),
                600,
                "10.188.32.4",
                1,
                Some(no_cua),
            ),
        ],
        codes: vec![
            code(
                "c-8b2071",
                "给 MacBook Air",
                None,
                Some(6 * 86400 + 3600),
                Some(5),
                1,
                None,
            ),
            code(
                "c-5e17d3",
                "nas",
                Some("g-7c1a0e"),
                None,
                Some(1),
                1,
                Some(Allow::view_only()),
            ),
            code("c-0f3c9a", "", None, Some(3500), None, 0, None),
        ],
        pending: vec![pending(3, "pixel-slate", "10.188.16.40", 20)],
    }
}

/// 我在 desktop 上：我自己和我发的分享链接（一台用了它）。
fn device_access() -> AccessInfo {
    AccessInfo {
        owner: false,
        me: Some("g-test".into()),
        grants: vec![
            grant(
                "g-test",
                dev("studio", "macos", "app"),
                None,
                0,
                "10.188.16.10",
                1,
                None,
            ),
            grant(
                "g-61b7aa",
                dev("ipad-mini", "linux", "cli"),
                Some("c-77e1f0"),
                5 * 3600,
                "10.188.48.7",
                0,
                None,
            ),
        ],
        codes: vec![code(
            "c-77e1f0",
            "给家里的 Linux",
            Some("g-test"),
            Some(86400),
            Some(1),
            1,
            None,
        )],
        pending: vec![],
    }
}

fn owner_welcome() -> Welcome {
    Welcome {
        host: "studio".into(),
        os: "macos".into(),
        version: "0.2.0".into(),
        desktop: true,
        persistent: true,
        allow: Allow::default(),
        role: Role::Owner,
    }
}

fn launchers() -> Vec<Launcher> {
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
    ]
}

/// 假的主机列表 + 最上面的本机（`online` 时有会话、显示器和访问记录）。
fn with_local(cx: &mut HeadlessAppContext, state: LocalState, online: bool) -> Entity<Model> {
    let model = fake_model(cx);
    model.update(cx, |m, _| {
        let mut h = HostState::new_local(addr(LOCAL), FP, "t");
        // 快照里不带这台机器真的主机名
        h.name = "studio".into();
        if online {
            h.set_online_for_test(
                owner_welcome(),
                0.3,
                vec![
                    session(
                        1,
                        "claude",
                        "✳ 本机配对的界面",
                        "claude",
                        "/Users/me/code/lolinet",
                    ),
                    session(2, "", "studio", "zsh", "/Users/me"),
                ],
                vec![display(1, "内建显示器", 3024, 1964, true)],
                launchers(),
            );
            h.access = Some(owner_access());
        } else {
            h.link = Link::Reconnecting;
        }
        m.set_local_for_test(h, state);
        // 别的主机收起来，侧栏放得下
        for h in m.hosts.iter_mut().filter(|h| !h.local) {
            h.expanded = false;
        }
    });
    model
}

// ------------------------------------------------------------ 快照

/// 本机在最上面：会话、访问（4 台设备、1 个请求）、新建、显示器；首页第一张卡片也是本机。
pub fn local_top(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    let (handle, _) = open(&mut cx, model.clone());
    let first = model.read_with(&cx, |m, _| m.hosts[0].title());
    assert_eq!(first, "本机");
    shot(&mut cx, handle, out, "local");
    // 本机的右键菜单：访问、新配对码、让网里的设备能连这台
    with_window(&mut cx, handle, |window, cx| {
        use gpui_kit::{MouseButton, point, px};
        window.pointer_move(point(px(110.), px(145.)), None, cx);
        window.pointer_down(point(px(110.), px(145.)), MouseButton::Right, cx);
        window.pointer_up(point(px(110.), px(145.)), MouseButton::Right, cx);
    });
    cx.advance_clock(Duration::from_secs(1));
    shot(&mut cx, handle, out, "local-menu");
}

/// 本机正在开、开不起来、开发版没带 lolidesk。
pub fn local_states(out: &Option<PathBuf>) {
    for (name, state) in [
        ("local-starting", LocalState::Starting),
        (
            "local-failed",
            LocalState::Failed("开不了本机主机：launchctl bootstrap 失败".into()),
        ),
        (
            "local-dev",
            LocalState::NotRunning("本机主机没在跑：lolidesk host install".into()),
        ),
        ("local-off", LocalState::Off),
    ] {
        let mut cx = context();
        let model = with_local(&mut cx, state, false);
        let (handle, _) = open(&mut cx, model);
        shot(&mut cx, handle, out, name);
    }
}

/// 有设备请求连这台：整个窗口上的框；两个请求时排着队。
pub fn consent(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    let (handle, _) = open(&mut cx, model.clone());
    with_window(&mut cx, handle, |_, cx| {
        model.update(cx, |m, cx| {
            m.update_consent(vec![pending(3, "pixel-slate", "10.188.16.40", 20)], cx)
        })
    });
    shot(&mut cx, handle, out, "consent");
    with_window(&mut cx, handle, |_, cx| {
        model.update(cx, |m, cx| {
            m.update_consent(
                vec![
                    pending(3, "pixel-slate", "10.188.16.40", 25),
                    pending(4, "build-box", "10.188.32.9", 5),
                ],
                cx,
            )
        })
    });
    assert_eq!(model.read_with(&cx, |m, _| m.consent.len()), 2);
    shot(&mut cx, handle, out, "consent-queue");
    // 答了第一个：框换成第二个（这里没有连接：只看队列）
    with_window(&mut cx, handle, |_, cx| {
        model.update(cx, |m, cx| m.decide(3, true, cx))
    });
    frame(&mut cx, handle);
    let left: Vec<u64> = model.read_with(&cx, |m, _| m.consent.iter().map(|p| p.id).collect());
    assert_eq!(left, vec![4]);
}

/// 「访问」页：主人看全部（请求、经主人同意、两个码和一个空码、限制）；限制的开关；我在别的主机上的配对。
pub fn access_page(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    let (handle, _) = open(&mut cx, model.clone());
    with_window(&mut cx, handle, |_, cx| {
        model.update(cx, |m, cx| m.select(Selection::Access(addr(LOCAL)), cx))
    });
    shot(&mut cx, handle, out, "access");
    // 往下滚：码和用它们配对的设备
    with_window(&mut cx, handle, |window, cx| {
        use gpui_kit::{Modifiers, ScrollDelta, ScrollEvent, TouchPhase, point, px};
        window.dispatch_event(
            gpui_kit::InputEvent::to_platform_input(ScrollEvent {
                position: point(px(700.), px(500.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(-560.))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
                ..Default::default()
            }),
            cx,
        );
    });
    shot(&mut cx, handle, out, "access-codes");
    // 限制 mini：五个开关
    with_window(&mut cx, handle, |window, cx| {
        model.update(cx, |m, cx| {
            m.select(Selection::Home, cx);
            m.select(Selection::Access(addr(LOCAL)), cx)
        });
        window.render_frame(cx);
    });
    frame(&mut cx, handle);
    with_window(&mut cx, handle, |window, cx| {
        window.click("acc-limt-g-2d9e44", cx)
    });
    cx.advance_clock(Duration::from_secs(1));
    shot(&mut cx, handle, out, "access-limit");

    // 我在 desktop 上的配对（我是配对过的设备）
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    let desktop = addr("10.188.1.2:10190");
    model.update(&mut cx, |m, _| {
        let h = m.host_mut(desktop).unwrap();
        h.access = Some(device_access());
        h.expanded = true;
    });
    let (handle, _) = open(&mut cx, model.clone());
    with_window(&mut cx, handle, |_, cx| {
        model.update(cx, |m, cx| m.select(Selection::Access(desktop), cx))
    });
    shot(&mut cx, handle, out, "access-device");
}

/// 新配对码：填的样子，发好以后的码、链接、二维码。
pub fn new_code(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    let (handle, _) = open(&mut cx, model.clone());
    let form = with_window(&mut cx, handle, |window, cx| {
        access::open_new_code(model.clone(), addr(LOCAL), window, cx)
    });
    with_window(&mut cx, handle, |window, cx| {
        form.update(cx, |f, cx| {
            f.label
                .update(cx, |i, cx| i.set_value("给 NAS", window, cx));
            f.expires = 3;
            f.uses = 1;
            cx.notify();
        })
    });
    cx.advance_clock(Duration::from_secs(1));
    shot(&mut cx, handle, out, "new-code");
    let secret = "K7QM-4XRT-P2HD-W9BN".to_string();
    let done = NewCode {
        info: CodeInfo {
            id: "c-a71e02".into(),
            label: "给 NAS".into(),
            created: now(),
            created_by: None,
            expires: Some(now() + 7 * 86400),
            max_uses: Some(1),
            uses: 0,
            allow: None,
        },
        link: PairLink {
            addr: "10.188.16.10:10190".into(),
            fingerprint: FP.into(),
            code: secret.clone(),
            name: "studio".into(),
        },
        secret,
    };
    with_window(&mut cx, handle, |_, cx| {
        form.update(cx, |f, cx| f.show_code(done, cx))
    });
    shot(&mut cx, handle, out, "new-code-done");
}

/// 网里的设备（侧栏最下面），点「连接」后的「正在请 X 同意…」。
pub fn discovery(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    model.update(&mut cx, |m, _| {
        m.hosts[0].expanded = false;
        m.found = vec![
            Found {
                name: "air".into(),
                addr: addr("10.188.16.21:10190"),
                fingerprint: fp(),
            },
            Found {
                name: "build-box".into(),
                addr: addr("10.188.32.9:10190"),
                fingerprint: fp(),
            },
            Found {
                name: "nas".into(),
                addr: addr("10.188.32.4:10190"),
                fingerprint: fp(),
            },
        ];
    });
    let (handle, _) = open(&mut cx, model.clone());
    shot(&mut cx, handle, out, "found");
    with_window(&mut cx, handle, |window, cx| {
        pair::open_pairing_for_test(
            model.clone(),
            addr("10.188.16.21:10190"),
            fp(),
            "air".into(),
            window,
            cx,
        );
    });
    cx.advance_clock(Duration::from_secs(1));
    shot(&mut cx, handle, out, "pair-waiting");
}

/// 导入分享链接：粘贴了一条，下面认出是哪台。
pub fn import(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = with_local(&mut cx, LocalState::Normal, true);
    let (handle, _) = open(&mut cx, model.clone());
    let url = PairLink {
        addr: "10.188.16.10:10190".into(),
        fingerprint: FP.into(),
        code: "K7QM-4XRT-P2HD-W9BN".into(),
        name: "studio".into(),
    }
    .to_url();
    with_window(&mut cx, handle, |window, cx| {
        pair::open_import(model.clone(), Some(url), window, cx);
    });
    cx.advance_clock(Duration::from_secs(1));
    shot(&mut cx, handle, out, "import");
}

/// 添加主机：展开「我有配对码 / token」，填了配对码。
pub fn add_host_code(out: &Option<PathBuf>) {
    let mut cx = context();
    let model = fake_model(&mut cx);
    let (handle, _) = open(&mut cx, model.clone());
    with_window(&mut cx, handle, |window, cx| {
        let form = cx.new(|cx| AddHostForm::new(model.clone(), window, cx));
        form.update(cx, |f, cx| {
            f.addr
                .update(cx, |i, cx| i.set_value("10.188.16.21", window, cx));
            f.token
                .update(cx, |i, cx| i.set_value("K7QM-4XRT-P2HD-W9BN", window, cx));
            f.set_has_secret(true, cx);
        });
        lolidesk_app::ui::dialogs::open_form(form, window, cx);
    });
    cx.advance_clock(Duration::from_secs(1));
    shot(&mut cx, handle, out, "add-host-code");
}

// ------------------------------------------------------------ 本机主机、通知（假的服务）

/// 本机主机从来没跑过：从 app 里跑的就替用户装一次服务，开发版只说明怎么开；关掉「让网里的设备能连这台」就撤服务。
pub fn local_lifecycle(_: &Option<PathBuf>) {
    let mut cx = context();
    let (svc, calls) = fake_service(true);
    let model = cx.new(|_| Model::empty(Default::default()));
    model.update(&mut cx, |m, cx| {
        m.set_service(svc);
        m.start(cx);
    });
    cx.run_until_parked();
    let (title, local, state) = model.read_with(&cx, |m, _| {
        (
            m.hosts[0].title(),
            m.hosts[0].local,
            m.local.as_ref().map(|l| l.state.clone()),
        )
    });
    assert_eq!((title.as_str(), local), ("本机", true));
    assert_eq!(state, Some(LocalState::Starting));
    assert_eq!(calls.lock().unwrap().installs, 1, "没替用户装服务");
    // 装好了要重连（一秒后），不再装第二次
    let link = model.read_with(&cx, |m, _| m.hosts[0].link);
    assert_eq!(link, Link::Reconnecting);
    model.update(&mut cx, |m, cx| m.retry_now(cx));
    cx.run_until_parked();
    assert_eq!(calls.lock().unwrap().installs, 1, "装了两次");
    // 关掉：撤服务、断开、说明
    model.update(&mut cx, |m, cx| {
        m.prefs.local_host = true;
        m.set_local_host(false, cx)
    });
    cx.run_until_parked();
    assert_eq!(calls.lock().unwrap().uninstalls, 1);
    let state = model.read_with(&cx, |m, _| m.local.as_ref().map(|l| l.state.clone()));
    assert_eq!(state, Some(LocalState::Off));

    // 开发版（没有 app 带的 lolidesk）：不装，说明怎么开
    let mut cx = context();
    let (svc, calls) = fake_service(false);
    let model = cx.new(|_| Model::empty(Default::default()));
    model.update(&mut cx, |m, cx| {
        m.set_service(svc);
        m.start(cx);
    });
    cx.run_until_parked();
    let state = model.read_with(&cx, |m, _| m.local.as_ref().map(|l| l.state.clone()));
    assert_eq!(
        state,
        Some(LocalState::NotRunning(
            "本机主机没在跑：lolidesk host install".into()
        ))
    );
    assert_eq!(calls.lock().unwrap().installs, 0);
}

/// 已经装着服务（命令行从 /usr/local/lib/lolidesk 装的）：app 不把它换成自己带的程序（授权跟着程序走）。
/// 只是连不上（重启、升级）就不动服务、过一阵说明；点「重试」才重装，还是那个程序；关了再开也装回它。
pub fn local_keeps_service(_: &Option<PathBuf>) {
    let system = system_lolidesk();
    let local_state = |model: &Entity<Model>, cx: &HeadlessAppContext| {
        model.read_with(cx, |m, _| m.local.as_ref().map(|l| l.state.clone()))
    };

    // 主机目录里没东西（主机从没跑过）：马上装，但用的是装着的服务的程序
    let mut cx = context();
    let (svc, calls) = fake_service_with(true, None, Some(system.clone()));
    let model = cx.new(|_| Model::empty(Default::default()));
    model.update(&mut cx, |m, cx| {
        m.set_service(svc);
        m.start(cx);
    });
    cx.run_until_parked();
    assert_eq!(
        calls.lock().unwrap().programs,
        std::slice::from_ref(&system)
    );

    // 跑过、现在连不上：一直重连，不装
    let mut cx = context();
    let (svc, calls) = fake_service_with(true, None, Some(system.clone()));
    let model = cx.new(|_| Model::empty(Default::default()));
    model.update(&mut cx, |m, _| {
        let mut h = HostState::new_local(addr(LOCAL), FP, "t");
        h.wanted = true;
        h.link = Link::Reconnecting;
        m.set_local_for_test(h, LocalState::Normal);
        m.set_service(svc);
    });
    for _ in 0..2 {
        model.update(&mut cx, |m, cx| m.retry_now(cx));
        cx.run_until_parked();
    }
    assert_eq!(local_state(&model, &cx), Some(LocalState::Normal));
    for _ in 0..3 {
        model.update(&mut cx, |m, cx| m.retry_now(cx));
        cx.run_until_parked();
    }
    assert_eq!(calls.lock().unwrap().installs, 0, "替用户动了装着的服务");
    let state = local_state(&model, &cx);
    assert!(
        matches!(&state, Some(LocalState::NotRunning(t)) if t.starts_with("本机主机服务装着，但连不上")),
        "{state:?}"
    );
    let link = model.read_with(&cx, |m, _| m.hosts[0].link);
    assert_eq!(link, Link::Reconnecting, "不再重连了");

    // 点「重试」：重装，还是那个程序
    model.update(&mut cx, |m, cx| m.retry_local(cx));
    cx.run_until_parked();
    assert_eq!(
        calls.lock().unwrap().programs,
        std::slice::from_ref(&system)
    );
    assert_eq!(local_state(&model, &cx), Some(LocalState::Starting));

    // 关掉再开：撤的、装回的都是那个程序，不是 app 带的
    model.update(&mut cx, |m, cx| m.set_local_host(false, cx));
    cx.run_until_parked();
    model.update(&mut cx, |m, cx| m.set_local_host(true, cx));
    cx.run_until_parked();
    let c = calls.lock().unwrap();
    assert_eq!((c.installs, c.uninstalls), (2, 1));
    assert_eq!(c.programs, [system.clone(), system.clone(), system.clone()]);
    drop(c);
    let _ = std::fs::remove_file(&system);
}

/// 配对请求：窗口不在前台时每个请求只发一次系统通知；在前台时不发；答过的不再弹。
pub fn consent_notify(_: &Option<PathBuf>) {
    let mut cx = context();
    let (svc, calls) = fake_service(true);
    let model = with_local(&mut cx, LocalState::Normal, true);
    model.update(&mut cx, |m, cx| {
        m.set_service(svc);
        m.window_active = false;
        let a = pending(1, "laptop", "10.188.16.11", 3);
        m.update_consent(vec![a.clone()], cx);
        m.update_consent(vec![a], cx);
    });
    cx.run_until_parked();
    {
        let c = calls.lock().unwrap();
        assert_eq!(c.notes.len(), 1, "{:?}", c.notes);
        assert!(c.notes[0].contains("「laptop」（10.188.16.11）想连这台电脑"));
    }
    model.update(&mut cx, |m, cx| {
        m.update_consent(
            vec![
                pending(1, "laptop", "10.188.16.11", 4),
                pending(2, "nas", "10.188.32.4", 1),
            ],
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(calls.lock().unwrap().notes.len(), 2);
    // 窗口在前台：只弹框，不发通知
    model.update(&mut cx, |m, cx| {
        m.window_active = true;
        m.update_consent(
            vec![
                pending(1, "laptop", "10.188.16.11", 5),
                pending(2, "nas", "10.188.32.4", 2),
                pending(3, "air", "10.188.16.21", 0),
            ],
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(calls.lock().unwrap().notes.len(), 2);
    // 答了 1：主机列表里还有它时也不再弹；主机上没了的从队列里去掉
    model.update(&mut cx, |m, cx| {
        m.decide(1, true, cx);
        m.update_consent(
            vec![
                pending(1, "laptop", "10.188.16.11", 6),
                pending(3, "air", "10.188.16.21", 1),
            ],
            cx,
        );
    });
    cx.run_until_parked();
    let ids: Vec<u64> = model.read_with(&cx, |m, _| m.consent.iter().map(|p| p.id).collect());
    assert_eq!(ids, vec![3]);
}
