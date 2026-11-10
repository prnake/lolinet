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
    ui::{desktop::STAGE_CONTEXT, dialogs, home, sidebar::Sidebar},
};

actions!(
    lolidesk,
    [Quit, Hide, HideOthers, Minimize, AddHost, ToggleFullScreen]
);

/// 窗口根视图的按键上下文。
pub const WORKSPACE_CONTEXT: &str = "Workspace";

/// 窗口里的快捷键：画面（DeskStage）拿走所有键，所以这些在那一层都用 NoAction 挡掉，
/// 连同 gpui-kit 的 Root 绑的 tab / shift-tab / cmd-c。⌘Q 不挡（全局绑定，留在本地）。
const WINDOW_KEYS: [&str; 5] = ["cmd-h", "cmd-alt-h", "cmd-m", "cmd-n", "cmd-ctrl-f"];
const ROOT_KEYS: [&str; 3] = ["tab", "shift-tab", "cmd-c"];

/// 初始化 gpui-kit、主题、按键（窗口和测试都用）。
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    theme::install(cx);
    let mut keys = vec![
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new(WINDOW_KEYS[0], Hide, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[1], HideOthers, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[2], Minimize, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[3], AddHost, Some(WORKSPACE_CONTEXT)),
        KeyBinding::new(WINDOW_KEYS[4], ToggleFullScreen, Some(WORKSPACE_CONTEXT)),
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
        ..Default::default()
    }
}

/// 开应用：一个窗口，连上记住的主机。
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lolidesk_app=info,lolidesk_core=info".into()),
        )
        .try_init();
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
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
            if let Err(e) = opened {
                tracing::error!("开窗口：{e:#}");
                cx.quit();
                return;
            }
            model.update(cx, |m, cx| m.start(cx));
            cx.activate(true);
        });
}

/// 窗口的根：侧栏 + 右边。
pub struct Workspace {
    pub model: Entity<Model>,
    sidebar: Entity<Sidebar>,
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
            // 窗口回到前台：等着重连的主机马上试
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.model.update(cx, |m, cx| m.retry_now(cx));
                }
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
            Selection::Home => {}
        }
        home::render(&self.model, cx)
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let right = self.right(cx);
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
    }
}
