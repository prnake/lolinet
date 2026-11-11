//! 应用和窗口：初始化（主题、按键、菜单）、窗口的根视图 [`Workspace`]。

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyBinding, Menu, MenuItem, NoAction, ParentElement, Render, StyleRefinement, Styled,
    Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, point, px,
    size,
};

use crate::{
    assets::AppAssets,
    model::{Model, ModelEvent, Selection, ToastKind},
    theme::{self, *},
    ui::{
        access, desktop::STAGE_CONTEXT, dialogs, home, network::NetworkPage, pair, sidebar::Sidebar,
    },
};

actions!(
    lolidesk,
    [Quit, Hide, HideOthers, Minimize, AddHost, ToggleFullScreen]
);

/// 窗口根视图的按键上下文。
pub const WORKSPACE_CONTEXT: &str = "Workspace";

/// 窗口里的快捷键：画面（DeskStage）拿走所有键，所以这些在那一层都用 NoAction 挡掉，
/// 连同 gpui-kit 的 Root 绑的 tab / shift-tab / cmd-c。⌘Q 不挡（全局绑定，留在本地）。
#[cfg(target_os = "macos")]
const WINDOW_KEYS: [&str; 5] = ["cmd-h", "cmd-alt-h", "cmd-m", "cmd-n", "cmd-ctrl-f"];
#[cfg(target_os = "macos")]
const ROOT_KEYS: [&str; 3] = ["tab", "shift-tab", "cmd-c"];
#[cfg(target_os = "macos")]
const QUIT_KEY: &str = "cmd-q";

/// Linux：Ctrl 组合键是终端的键，应用自己的用 Ctrl+Shift（和 GNOME 终端一样），全屏是 F11。
/// 没有隐藏、最小化的快捷键（窗口管理器管）。gpui-kit 的 Root 在这里绑的是 ctrl-c。
#[cfg(not(target_os = "macos"))]
const WINDOW_KEYS: [&str; 2] = ["ctrl-shift-n", "f11"];
#[cfg(not(target_os = "macos"))]
const ROOT_KEYS: [&str; 3] = ["tab", "shift-tab", "ctrl-c"];
#[cfg(not(target_os = "macos"))]
const QUIT_KEY: &str = "ctrl-shift-q";

/// 初始化 gpui-kit、主题、按键（窗口和测试都用）。
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    theme::install(cx);
    #[cfg(target_os = "macos")]
    let mut keys = vec![
        KeyBinding::new(QUIT_KEY, Quit, None),
        KeyBinding::new(WINDOW_KEYS[0], Hide, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[1], HideOthers, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[2], Minimize, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[3], AddHost, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[4], ToggleFullScreen, Some(WORKSPACE_CONTEXT)),
    ];
    #[cfg(not(target_os = "macos"))]
    let mut keys = vec![
        KeyBinding::new(QUIT_KEY, Quit, None),
        KeyBinding::new(WINDOW_KEYS[0], AddHost, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[1], ToggleFullScreen, Some(WORKSPACE_CONTEXT)),
    ];
    for k in ROOT_KEYS.iter().chain(WINDOW_KEYS.iter()) {
        keys.push(KeyBinding::new(k, NoAction, Some(STAGE_CONTEXT)));
    }
    cx.bind_keys(keys);
    crate::ui::grid::bind_keys(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
}

pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("lolidesk".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(18.), px(18.))),
        }),
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(820.), px(520.))),
        window_bounds: Some(WindowBounds::centered(size(px(1280.), px(820.)), cx)),
        app_id: cfg!(target_os = "linux").then(|| crate::APP_ID.to_string()),
        ..Default::default()
    }
}

/// 系统交给 app 的 `lolidesk://` 链接（分享链接）。
fn is_link(s: &str) -> bool {
    s.starts_with("lolidesk://")
}

/// 开应用：一个窗口，本机主机和记住的主机。
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lolidesk_app=info,lolidesk_core=info".into()),
        )
        .try_init();
    // 分享链接：macOS 由系统交来（Info.plist 里登记了 lolidesk://），Linux 由桌面文件的 %u 当参数给
    let (links, mut link_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    for a in std::env::args().skip(1).filter(|a| is_link(a)) {
        let _ = links.send(a);
    }
    let app = gpui_kit::application().with_assets(AppAssets);
    app.on_open_urls(move |urls| {
        for u in urls.into_iter().filter(|u| is_link(u)) {
            let _ = links.send(u);
        }
    });
    app.run(move |cx: &mut App| {
        init(cx);
        // 终端库早点载入：缺了的话终端面板里说清楚
        if let Err(e) = lolidesk_ghostty::load() {
            tracing::warn!("libghostty-vt：{e:#}");
        }
        cx.set_menus([
            Menu::new("lolidesk").items([
                MenuItem::action("添加主机…", AddHost),
                MenuItem::separator(),
                MenuItem::action("隐藏 lolidesk", Hide),
                MenuItem::action("隐藏其他", HideOthers),
                MenuItem::separator(),
                MenuItem::action("退出 lolidesk", Quit),
            ]),
            Menu::new("窗口").items([
                MenuItem::action("最小化", Minimize),
                MenuItem::action("全屏", ToggleFullScreen),
            ]),
        ]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let model = cx.new(|_| Model::load());
        let opened = gpui_kit::open_window(window_options(cx), cx, |window, cx| {
            let model = model.clone();
            cx.new(|cx| Workspace::new(model, window, cx))
        });
        let handle = match opened {
            Ok((handle, _)) => handle,
            Err(e) => {
                tracing::error!("开窗口：{e:#}");
                cx.quit();
                return;
            }
        };
        model.update(cx, |m, cx| m.start(cx));
        cx.activate(true);
        // 分享链接：打开「导入分享链接」，先填好，等人点「配对」
        cx.spawn(async move |cx| {
            while let Some(url) = link_rx.recv().await {
                let model = model.clone();
                let _ = cx.update_window(handle, |_, window, cx| {
                    window.activate_window();
                    pair::open_import(model, Some(url), window, cx);
                });
            }
        })
        .detach();
    });
}

/// 窗口的根：侧栏 + 右边。
pub struct Workspace {
    pub model: Entity<Model>,
    sidebar: Entity<Sidebar>,
    /// 网络页（留着：输入框里填了一半的东西切走再回来还在）
    pub network: Entity<NetworkPage>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(model.clone(), cx));
        let network = cx.new(|cx| NetworkPage::new(model.clone(), window, cx));
        let subs = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &model,
                window,
                |_, model, ev: &ModelEvent, window, cx| match ev {
                    ModelEvent::Toast(kind, text) => {
                        let note = match kind {
                            ToastKind::Info => Notification::info(text.clone()),
                            ToastKind::Success => Notification::success(text.clone()),
                            ToastKind::Error => Notification::error(text.clone()),
                        };
                        window.push_notification(note, cx);
                    }
                    ModelEvent::FocusTerm(id) => {
                        // 切到终端：键盘给它
                        let focus = model
                            .read(cx)
                            .terms
                            .iter()
                            .find(|t| t.entity_id() == *id)
                            .map(|t| t.read(cx).grid.read(cx).focus.clone());
                        if let Some(f) = focus {
                            f.focus(window, cx);
                        }
                    }
                },
            ),
            // 窗口回到前台：等着重连的主机马上试；不在前台时配对请求另发系统通知
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                let active = window.is_window_active();
                this.model
                    .update(cx, |m, cx| m.set_window_active(active, cx));
            }),
        ];
        let focus = cx.focus_handle();
        // 窗口的动作（菜单、快捷键）要有焦点路径经过这里；谁都没焦点时拿回来
        focus.focus(window, cx);
        let mut subs = subs;
        subs.push(cx.on_focus_lost(window, |this: &mut Self, window, cx| {
            this.focus.focus(window, cx)
        }));
        Self {
            model,
            sidebar,
            network,
            focus,
            _subs: subs,
        }
    }

    fn right(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let m = self.model.read(cx);
        match m.selection {
            Selection::Term(id) => {
                if let Some(t) = m.terms.iter().find(|t| t.entity_id() == id) {
                    return t.clone().into_any_element();
                }
            }
            Selection::Desk => {
                if let Some(d) = &m.desk {
                    return d.clone().into_any_element();
                }
            }
            Selection::Network => return self.network.clone().into_any_element(),
            Selection::Access(addr) => {
                if m.host(addr).is_some() {
                    return access::render(&self.model, addr, cx);
                }
            }
            Selection::Home => {}
        }
        home::render(&self.model, cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let right = self.right(cx);
        let consent = pair::consent_overlay(&self.model, window, cx);
        div()
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &AddHost, window, cx| {
                dialogs::open_add_host(this.model.clone(), window, cx)
            }))
            .on_action(|_: &Minimize, window, _| window.minimize_window())
            .on_action(|_: &ToggleFullScreen, window, _| window.toggle_fullscreen())
            .size_full()
            .flex()
            .bg(void())
            .text_color(fg())
            .font_family(SANS)
            .text_size(T_SM)
            .child(
                self.sidebar
                    .clone()
                    .cached(StyleRefinement::default().w(SIDEBAR_W).h_full().flex_none()),
            )
            .child(div().flex_1().min_w_0().h_full().child(right))
            .children(consent)
    }
}
