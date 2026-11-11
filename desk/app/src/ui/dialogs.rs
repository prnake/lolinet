//! 对话框：添加主机（地址 + token → 核对证书指纹 → 信任并连接）、自定义命令。
//! 用 gpui-kit 的 Dialog 画外壳（遮罩、Esc、焦点圈），里面是自己的视图。

use std::net::SocketAddr;

use gpui_kit::component::{
    WindowExt as _,
    dialog::Dialog,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px,
};

use crate::{
    labels::group_fingerprint,
    model::Model,
    runtime::error_text,
    term::TermStart,
    theme::*,
    ui::widgets::{caption, pill, pill_busy},
};

/// 对话框的外壳：默认 440 宽、圆角 16、深一层的底。
fn shell(dialog: Dialog, width: f32) -> Dialog {
    dialog
        .w(px(width))
        .p(px(24.))
        .bg(ink2())
        .border_color(line2())
        .rounded(R_LG)
        .close_button(false)
        .centered()
}

pub(crate) fn header(title: &'static str, sub: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(T_LG)
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(div().text_size(T_XS).text_color(fg2()).child(sub.into()))
}

pub(crate) fn field(label: &'static str, input: &Entity<InputState>) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(caption(label))
        .child(
            Input::new(input)
                .h(px(36.))
                .font_family(MONO)
                .text_size(T_SM)
                .bg(ink1())
                .border_color(line2())
                .rounded(R_SM),
        )
}

pub(crate) fn error_line(e: &Option<String>) -> Option<impl IntoElement> {
    e.as_ref()
        .map(|e| div().text_size(T_XS).text_color(bad()).child(e.clone()))
}

// ---------------------------------------------------------------- 添加主机

enum Step {
    /// 填地址和 token
    Enter,
    /// 新主机的证书指纹，等用户核对
    Verify { addr: SocketAddr, fp: [u8; 32] },
}

pub struct AddHostForm {
    model: Entity<Model>,
    pub addr: Entity<InputState>,
    pub token: Entity<InputState>,
    step: Step,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl Focusable for AddHostForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AddHostForm {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let addr = cx.new(|cx| InputState::new(window, cx).placeholder("10.188.1.2 或主机名"));
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("32 位十六进制")
                .masked(true)
        });
        let subs = [&addr, &token]
            .into_iter()
            .map(|i| {
                cx.subscribe_in(i, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.next_step(window, cx);
                    }
                })
            })
            .collect();
        Self {
            model,
            addr,
            token,
            step: Step::Enter,
            busy: false,
            error: None,
            focus: cx.focus_handle(),
            _subs: subs,
        }
    }

    /// 快照用：直接到核对指纹那一步。
    pub fn show_fingerprint(&mut self, addr: SocketAddr, fp: [u8; 32], cx: &mut Context<Self>) {
        self.step = Step::Verify { addr, fp };
        cx.notify();
    }

    /// 下一步 / 信任并连接。
    fn next_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match self.step {
            Step::Enter => {
                let host = self.addr.read(cx).value().trim().to_string();
                if host.is_empty() {
                    self.error = Some("填主机的 overlay 地址或主机名".into());
                    cx.notify();
                    return;
                }
                self.busy = true;
                self.error = None;
                let task = Model::probe_fingerprint(host, cx);
                cx.spawn_in(window, async move |this, cx| {
                    let r = task.await;
                    let _ = this.update(cx, |f, cx| {
                        f.busy = false;
                        match r {
                            Ok((addr, fp)) => f.step = Step::Verify { addr, fp },
                            Err(e) => f.error = Some(error_text(&e)),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
            Step::Verify { addr, fp } => {
                let token = self.token.read(cx).value().trim().to_string();
                self.busy = true;
                self.error = None;
                let task = Model::trust_and_connect(addr, token.clone(), fp, cx);
                let model = self.model.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let r = task.await;
                    let _ = this.update_in(cx, |f, window, cx| {
                        f.busy = false;
                        match r {
                            Ok(client) => {
                                model.update(cx, |m, cx| m.adopt(addr, client, fp, &token, cx));
                                window.close_dialog(cx);
                            }
                            Err(e) => f.error = Some(error_text(&e)),
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.step {
            Step::Enter => window.close_dialog(cx),
            Step::Verify { .. } => {
                self.step = Step::Enter;
                self.error = None;
                cx.notify();
            }
        }
    }
}

impl Render for AddHostForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy;
        let footer = |primary: &'static str| {
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(
                    pill("dlg-cancel", "取消", false)
                        .on_click(cx.listener(|this, _, window, cx| this.back(window, cx))),
                )
                .child(if busy {
                    pill_busy("dlg-ok", "连接中…", true).into_any_element()
                } else {
                    pill("dlg-ok", primary, true)
                        .on_click(cx.listener(|this, _, window, cx| this.next_step(window, cx)))
                        .into_any_element()
                })
        };
        let body = div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(18.));
        match self.step {
            Step::Enter => body
                .child(header(
                    "添加主机",
                    "在要连的电脑上先跑 lolidesk host install；token 和指纹用 lolidesk host token 看。",
                ))
                .child(field("地址", &self.addr))
                .child(field("token", &self.token))
                .children(error_line(&self.error))
                .child(footer("下一步")),
            Step::Verify { fp, .. } => body
                .child(header(
                    "核对证书指纹",
                    "和主机上 lolidesk host token 打出的「指纹」对一下，一样再信任。",
                ))
                .child(
                    div()
                        .id("fp")
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(R_SM)
                        .bg(ink1())
                        .border_1()
                        .border_color(line2())
                        .font_family(MONO)
                        .text_size(T_SM)
                        .line_height(px(21.))
                        .text_color(eye())
                        .child(group_fingerprint(&hex::encode(fp))),
                )
                .children(error_line(&self.error))
                .child(footer("信任并连接")),
        }
    }
}

/// 打开「添加主机」。
pub fn open_add_host(model: Entity<Model>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| AddHostForm::new(model, window, cx));
    open_form(form.clone(), window, cx);
    let input = form.read(cx).addr.clone();
    input.update(cx, |i, cx| i.focus(window, cx));
}

/// 对话框里的表单：回车（焦点不在输入框里时，对话框的「确定」）也提交。
pub trait Form: Render {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>);
}

impl Form for AddHostForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.next_step(window, cx)
    }
}

impl Form for CommandForm {
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx)
    }
}

/// 把一个表单放进对话框外壳里打开。
pub fn open_form<V: Form>(view: Entity<V>, window: &mut Window, cx: &mut App) {
    open_form_sized(view, 440., window, cx)
}

/// 同 [`open_form`]，自己定宽度。
pub fn open_form_sized<V: Form>(view: Entity<V>, width: f32, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, _| {
        let form = view.clone();
        shell(dialog, width)
            .on_ok(move |_, window, cx| {
                form.update(cx, |f, cx| f.submit(window, cx));
                false
            })
            .child(view.clone())
    });
}

// ---------------------------------------------------------------- 自定义命令

pub struct CommandForm {
    model: Entity<Model>,
    addr: SocketAddr,
    host_title: String,
    pub command: Entity<InputState>,
    pub cwd: Entity<InputState>,
    _subs: Vec<Subscription>,
}

impl CommandForm {
    pub fn new(
        model: Entity<Model>,
        addr: SocketAddr,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let host_title = model
            .read(cx)
            .host(addr)
            .map(|h| h.title())
            .unwrap_or_else(|| addr.to_string());
        let command = cx.new(|cx| InputState::new(window, cx).placeholder("claude --resume"));
        let cwd = cx.new(|cx| InputState::new(window, cx).placeholder("~/code/project"));
        let subs = [&command, &cwd]
            .into_iter()
            .map(|i| {
                cx.subscribe_in(i, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.run(window, cx);
                    }
                })
            })
            .collect();
        Self {
            model,
            addr,
            host_title,
            command,
            cwd,
            _subs: subs,
        }
    }

    fn run(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let command = self.command.read(cx).value().trim().to_string();
        let cwd = self.cwd.read(cx).value().trim().to_string();
        let start = TermStart::Open {
            command: (!command.is_empty()).then_some(command),
            cwd: (!cwd.is_empty()).then_some(cwd),
        };
        let addr = self.addr;
        self.model.update(cx, |m, cx| m.open_term(addr, start, cx));
        window.close_dialog(cx);
    }
}

impl Render for CommandForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(header(
                "自定义命令",
                format!(
                    "在 {} 的登录 shell 里跑，退出后留在 shell 里。",
                    self.host_title
                ),
            ))
            .child(field("命令", &self.command))
            .child(field("目录（可以不填）", &self.cwd))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        pill("cmd-cancel", "取消", false)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        pill("cmd-run", "开始", true)
                            .on_click(cx.listener(|this, _, window, cx| this.run(window, cx))),
                    ),
            )
    }
}

/// 打开「自定义命令」。
pub fn open_custom_command(
    model: Entity<Model>,
    addr: SocketAddr,
    window: &mut Window,
    cx: &mut App,
) {
    let form = cx.new(|cx| CommandForm::new(model, addr, window, cx));
    open_form(form.clone(), window, cx);
    let input = form.read(cx).command.clone();
    input.update(cx, |i, cx| i.focus(window, cx));
}
