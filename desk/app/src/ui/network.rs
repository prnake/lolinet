//! 网络页（侧栏最上面的「网络」）：本机在 lolinet 里的样子（本机、别的节点、出口节点）、
//! 没入网时的「加入网络」、邀请别的设备（配对码、命令、手机的二维码）、把命令行装上。
//!
//! 数据都来自本机的 lolinet 命令行（[`crate::lolinet`]）：模型在后台线程里读，页面开着时每 3 秒一次。
//! 要管理员密码的事（入网、出口节点、装命令行）交给模型跑（[`NetJob`]），系统弹密码框。

use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::{
    WindowExt as _,
    input::{InputEvent, InputState},
    spinner::Spinner,
    tooltip::Tooltip,
};
use gpui_kit::{
    AnyElement, App, AppContext, Bounds, ClipboardItem, Context, Div, Entity, FocusHandle,
    Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, fill,
    point, prelude::FluentBuilder, px, size,
};

use crate::{
    assets::ic,
    lolinet::{self, Invite, InviteKind, JoinTarget, Kernel, Node, Peer, Status},
    model::{Model, NetJob, ToastKind},
    theme::*,
    ui::{
        dialogs::{self, Form, error_line, field, header},
        widgets::{
            badge, caption, checkbox, dot, icon, icon_button, mono, pill, pill_busy, pill_sm,
            switch,
        },
    },
};

/// 侧栏「网络」那一行右边的字和点。
pub fn summary(status: Option<&Status>) -> (String, Hsla) {
    match status.map(|s| &s.kernel) {
        None => (String::new(), fg4()),
        Some(Kernel::Missing) => ("没装".into(), fg4()),
        Some(Kernel::NotJoined) => ("未入网".into(), fg4()),
        Some(Kernel::Down(_)) => ("没在运行".into(), warn()),
        Some(Kernel::Up { peers, .. }) => (format!("{} 台在线", peers.len()), eye()),
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------- 页面

pub struct NetworkPage {
    model: Entity<Model>,
    /// 粘贴的整条配对命令
    pub line: Entity<InputState>,
    /// 或者分开填：准入站、配对码
    pub base: Entity<InputState>,
    pub code: Entity<InputState>,
    /// 无 TUN 入网
    pub userspace: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl NetworkPage {
    pub fn new(model: Entity<Model>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let line = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("curl -fsSL https://join.example.com/i | sudo sh -s XXXX-XXXX-XXXX")
        });
        let base = cx.new(|cx| InputState::new(window, cx).placeholder("https://join.example.com"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("XXXX-XXXX-XXXX"));
        let mut subs: Vec<Subscription> = [&line, &base, &code]
            .into_iter()
            .map(|i| {
                cx.subscribe_in(i, window, |this, _, ev: &InputEvent, _, cx| match ev {
                    // 下面的「将以管理员身份运行」跟着改
                    InputEvent::Change => {
                        this.error = None;
                        cx.notify()
                    }
                    InputEvent::PressEnter { .. } => this.join(cx),
                    _ => {}
                })
            })
            .collect();
        subs.push(cx.observe(&model, |_, _, cx| cx.notify()));
        Self {
            model,
            line,
            base,
            code,
            userspace: false,
            error: None,
            _subs: subs,
        }
    }

    /// 从输入里校验出入网目标（不跑）。没填是 None。
    fn target(&self, cx: &App) -> Option<Result<JoinTarget, String>> {
        let line = self.line.read(cx).value().trim().to_string();
        if !line.is_empty() {
            return Some(lolinet::parse_join_line(&line).map(|mut t| {
                t.userspace |= self.userspace;
                t
            }));
        }
        let base = self.base.read(cx).value().trim().to_string();
        let code = self.code.read(cx).value().trim().to_string();
        if base.is_empty() && code.is_empty() {
            return None;
        }
        Some((|| {
            Ok(JoinTarget {
                base: lolinet::parse_base(&base)?,
                code: lolinet::parse_code(&code)?,
                userspace: self.userspace,
            })
        })())
    }

    /// 「加入」：校验输入，对了交给模型跑（系统弹管理员密码框）。
    pub fn join(&mut self, cx: &mut Context<Self>) {
        match self.target(cx) {
            None => self.error = Some("粘贴配对命令，或者填准入站和配对码".into()),
            Some(Err(e)) => self.error = Some(e),
            Some(Ok(t)) => {
                self.error = None;
                self.model
                    .update(cx, |m, cx| m.run_net_job(NetJob::Join(t), cx));
            }
        }
        cx.notify();
    }

    /// 快照用：勾上无 TUN。
    pub fn set_userspace(&mut self, on: bool, cx: &mut Context<Self>) {
        self.userspace = on;
        cx.notify();
    }
}

impl Render for NetworkPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.model.read(cx);
        let status = m.net.status.clone();
        let job = m.net.job.clone();
        let output = m.net.output.clone();
        let model = self.model.clone();

        let up = status.as_ref().and_then(|s| s.node().cloned());
        let invite = up.as_ref().map(|_| {
            let model = model.clone();
            pill("net-invite", "邀请", true).on_click(move |_, window, cx| {
                open_invite(model.clone(), window, cx);
            })
        });
        let bar = super::top_bar(
            "net-bar",
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon(ic::NETWORK, 15., fg2()))
                .child(
                    div()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child("网络"),
                )
                .when_some(up.as_ref(), |d, n| {
                    d.child(mono(format!("{} · {}", n.name, n.ip), T_XS, fg3()))
                }),
            div().children(invite),
        );

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .max_w(px(880.))
            .p(px(24.));
        match &status {
            None => {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(Spinner::new().color(fg3()))
                        .child(
                            div()
                                .text_size(T_SM)
                                .text_color(fg3())
                                .child("正在读本机的 lolinet…"),
                        ),
                )
            }
            Some(st) => {
                match &st.kernel {
                    Kernel::Up { node, peers } => {
                        body = body
                            .child(section(
                                "本机",
                                None,
                                node_card(node, st, &job, &output, &model),
                            ))
                            .child(peers_section(peers));
                    }
                    Kernel::Down(e) => body = body.child(down_card(e)),
                    Kernel::Missing | Kernel::NotJoined => {
                        body =
                            body.child(section("加入网络", None, self.join_card(&job, &output, cx)))
                    }
                }
                body = body.child(cli_section(st, &job, &output, &model));
            }
        }

        div().size_full().flex().flex_col().child(bar).child(
            div()
                .id("net-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
    }
}

impl NetworkPage {
    fn join_card(
        &self,
        job: &Option<NetJob>,
        output: &Option<(NetJob, bool, String)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let joining = matches!(job, Some(NetJob::Join(_)));
        let preview = match self.target(cx) {
            Some(Ok(t)) => Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(R_SM)
                    .bg(ink1())
                    .border_1()
                    .border_color(line1())
                    .child(caption("将以管理员身份运行"))
                    .child(mono(t.display(), T_XS, fg2())),
            ),
            _ => None,
        };
        let go = if joining {
            pill_busy("net-join", "正在加入…", true).into_any_element()
        } else {
            pill("net-join", "加入", true)
                .tooltip(|window, cx| {
                    Tooltip::new("要管理员密码；lolinet 和 lolidesk 装进 /usr/local/bin")
                        .build(window, cx)
                })
                .on_click(cx.listener(|this, _, _, cx| this.join(cx)))
                .into_any_element()
        };
        let out = output
            .as_ref()
            .filter(|(j, _, _)| matches!(j, NetJob::Join(_)))
            .map(|(_, ok, text)| output_box(*ok, text));
        card()
            .gap(px(18.))
            .p(px(22.))
            .child(field("配对命令", &self.line))
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(field("准入站", &self.base).flex_1().min_w_0())
                    .child(field("配对码", &self.code).w(px(200.)).flex_none()),
            )
            .child(
                div().flex().child(
                    checkbox("net-userspace", self.userspace, "无 TUN")
                        .tooltip(|window, cx| {
                            Tooltip::new("不建网卡：本机程序经 127.0.0.1:18080 的 SOCKS5 进网（容器里、或者 TUN 已被占着时用）")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.userspace = !this.userspace;
                            cx.notify()
                        })),
                ),
            )
            .children(preview)
            .children(error_line(&self.error))
            .child(div().flex().justify_end().child(go))
            .children(out)
    }
}

/// 页面最上面两行大字（和首页一样的口气）。
/// 一节：小标题（右边可以有一行等宽字）+ 内容。
fn section(title: &'static str, right: Option<String>, body: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .px(px(4.))
                .child(caption(title).flex_1())
                .children(right.map(|r| mono(r, T_CAP, fg3()))),
        )
        .child(body)
}

/// 卡片：深一层的底、细边、圆角 16。
fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(R_LG)
        .bg(ink2())
        .border_1()
        .border_color(line1())
}

/// 卡片里的一行（第一行上面没有分隔线）。
fn card_row(first: bool) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(18.))
        .py(px(14.))
        .when(!first, |d| d.border_t_1().border_color(line1()))
}

/// 36×36 的图标方块。
fn tile(path: &'static str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(36.))
        .rounded(R_MD)
        .bg(ink3())
        .child(icon(path, 18., fg2()))
}

/// 两行字：上面正文，下面一行小字。
fn two_lines(top: impl IntoElement, sub: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(3.))
        .child(top)
        .child(sub)
}

fn node_card(
    node: &Node,
    st: &Status,
    job: &Option<NetJob>,
    output: &Option<(NetJob, bool, String)>,
    model: &Entity<Model>,
) -> Div {
    let exit = st.exit;
    let busy = matches!(job, Some(NetJob::Exit(_)));
    let on = exit.is_some();
    let exit_note: SharedString = match exit {
        Some(port) => format!(
            "网里别的机器可以经这台上网：socks5://{}:{port}，只对内网开",
            node.ip
        ),
        None => format!(
            "开了以后，网里别的机器可以经这台上网（SOCKS5，端口 {}，只对内网开）",
            lolinet::EXIT_PORT
        ),
    }
    .into();
    let toggle = {
        let model = model.clone();
        switch("net-exit", on, busy).when(!busy, |s| {
            s.on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.run_net_job(NetJob::Exit(!on), cx))
            })
        })
    };
    let failed = output
        .as_ref()
        .filter(|(j, ok, _)| matches!(j, NetJob::Exit(_) | NetJob::Mihomo(_)) && !ok)
        .map(|(_, _, text)| card_row(false).child(output_box(false, text).flex_1()));
    // 分流：mihomo 当系统服务跑，规则（直连、拒绝、从哪台出去）由管理面的「分流」页下发
    let mihomo_busy = matches!(job, Some(NetJob::Mihomo(_)));
    let mihomo_on = st.mihomo.is_some();
    let mihomo_note: SharedString = match &st.mihomo {
        Some(mode) => format!(
            "这台的流量按管理面「分流」页的规则走（mihomo，{}），规则每 10 分钟经内网取",
            if mode == "userspace" { "无 TUN 模式" } else { "TUN 模式" }
        ),
        None => "开了以后这台的流量按管理面「分流」页走：网内走 lolinet，有的直连、有的拒绝、有的从指定的机器出去".into(),
    }
    .into();
    let mihomo_toggle = {
        let model = model.clone();
        switch("net-mihomo", mihomo_on, mihomo_busy).when(!mihomo_busy && !busy, |s| {
            s.on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.run_net_job(NetJob::Mihomo(!mihomo_on), cx))
            })
        })
    };
    card()
        .child(
            card_row(true)
                .child(tile(ic::COMPUTER))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(MONO)
                        .text_size(T_MD)
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(node.name.clone()),
                )
                .child(mono(node.ip.clone(), T_XS, fg2()))
                .child(badge(
                    if node.userspace { "无 TUN" } else { "TUN" },
                    fg2(),
                    ink3(),
                ))
                .child(dot(eye(), 7.)),
        )
        .child(
            card_row(false)
                .id("net-exit-row")
                .tooltip(move |window, cx| Tooltip::new(exit_note.clone()).build(window, cx))
                .child(tile(ic::GLOBE))
                .child(
                    div()
                        .flex_1()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child("出口节点"),
                )
                .when(busy, |r| r.child(Spinner::new().color(fg3())))
                .child(toggle),
        )
        .child(
            card_row(false)
                .id("net-mihomo-row")
                .tooltip(move |window, cx| Tooltip::new(mihomo_note.clone()).build(window, cx))
                .child(tile(ic::SPLIT))
                .child(
                    div()
                        .flex_1()
                        .text_size(T_SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child("分流"),
                )
                .when(mihomo_busy, |r| r.child(Spinner::new().color(fg3())))
                .child(mihomo_toggle),
        )
        .children(failed)
}

fn peers_section(peers: &[Peer]) -> Div {
    let direct = peers.iter().filter(|p| p.direct).count();
    let right = (!peers.is_empty()).then(|| format!("{} 台在线 · 直连 {direct}", peers.len()));
    let body = if peers.is_empty() {
        card().child(
            card_row(true).child(
                div()
                    .text_size(T_SM)
                    .text_color(fg3())
                    .child("没有别的节点"),
            ),
        )
    } else {
        card().children(peers.iter().enumerate().map(|(i, p)| peer_row(i == 0, p)))
    };
    section("节点", right, body)
}

fn peer_row(first: bool, p: &Peer) -> Div {
    let link = if p.direct {
        if p.proto.is_empty() {
            "直连".to_string()
        } else {
            format!("直连 · {}", p.proto)
        }
    } else {
        "中转".to_string()
    };
    let lat = p
        .lat_ms
        .map(|l| format!("{l:.0} ms"))
        .unwrap_or_else(|| "–".into());
    card_row(first)
        .h(px(46.))
        .py(px(0.))
        .child(dot(if p.direct { eye() } else { warn() }, 6.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(MONO)
                .text_size(T_SM)
                .truncate()
                .child(p.name.clone()),
        )
        .child(
            div()
                .flex_none()
                .w(px(64.))
                .flex()
                .when(p.relay, |d| d.child(badge("接入点", fg3(), ink3()))),
        )
        .child(mono(p.ip.clone(), T_CAP, fg3()).flex_none().w(px(104.)))
        .child(
            div()
                .flex_none()
                .w(px(84.))
                .text_size(T_XS)
                .text_color(if p.direct { fg2() } else { warn() })
                .child(link),
        )
        .child(
            mono(lat, T_XS, fg2())
                .flex_none()
                .w(px(56.))
                .flex()
                .justify_end(),
        )
}

fn down_card(error: &str) -> Div {
    card().border_color(warn_line()).child(
        card_row(true)
            .items_start()
            .child(icon(ic::WARNING, 18., warn()))
            .child(two_lines(
                div()
                    .text_size(T_SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child("入了网，但本机的 lolinet 服务没在运行"),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(mono(error.to_string(), T_CAP, fg3()))
                    .child(
                        div()
                            .text_size(T_XS)
                            .text_color(fg2())
                            .child("在终端里跑 sudo lolinet restart；日志用 lolinet logs 看。"),
                    ),
            )),
    )
}

fn cli_section(
    st: &Status,
    job: &Option<NetJob>,
    output: &Option<(NetJob, bool, String)>,
    model: &Entity<Model>,
) -> Div {
    let sh = &st.shell;
    let installed = |text: &'static str| badge(text, eye(), eye_wash());
    let lolinet_state = if sh.lolinet {
        installed("已装").into_any_element()
    } else {
        div()
            .text_size(T_XS)
            .text_color(fg3())
            .child("入网时装")
            .into_any_element()
    };
    let desk_state = match (&sh.lolidesk, &sh.helper) {
        (Some(_), _) => installed("已装").into_any_element(),
        (None, Some(_)) if matches!(job, Some(NetJob::InstallCli)) => {
            pill_busy("net-cli", "正在装…", false)
                .h(px(26.))
                .px(px(12.))
                .text_size(T_XS)
                .into_any_element()
        }
        (None, Some(_)) => {
            let model = model.clone();
            pill_sm("net-cli", "安装到命令行", false)
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| m.run_net_job(NetJob::InstallCli, cx))
                })
                .into_any_element()
        }
        (None, None) => div()
            .text_size(T_XS)
            .text_color(fg3())
            .child(if cfg!(target_os = "macos") {
                "从 Lolidesk.app 里打开才能装"
            } else {
                "从解开的 lolidesk 包里打开才能装"
            })
            .into_any_element(),
    };
    // Linux：应用菜单（不要管理员）
    let menu_state = sh.menu.map(|added| {
        if added {
            installed("已加").into_any_element()
        } else if matches!(job, Some(NetJob::InstallMenu)) {
            pill_busy("net-menu", "正在加…", false)
                .h(px(26.))
                .px(px(12.))
                .text_size(T_XS)
                .into_any_element()
        } else {
            let model = model.clone();
            pill_sm("net-menu", "加到应用菜单", false)
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| m.run_net_job(NetJob::InstallMenu, cx))
                })
                .into_any_element()
        }
    });
    let menu_path = lolinet::menu_entry_path()
        .map(|p| home_short(&p))
        .unwrap_or_default();
    let desk_path = match &sh.lolidesk {
        Some(t) if t != lolinet::DESK_BIN => format!("{} → {t}", lolinet::DESK_BIN),
        _ => lolinet::DESK_BIN.to_string(),
    };
    let row = |first: bool, name: &'static str, path: String, state: AnyElement| {
        card_row(first)
            .py(px(12.))
            .child(
                div()
                    .flex_none()
                    .w(px(72.))
                    .font_family(MONO)
                    .text_size(T_SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child(name),
            )
            .child(mono(path, T_CAP, fg3()).flex_1().min_w_0().truncate())
            .child(state)
    };
    let failed = output
        .as_ref()
        .filter(|(j, ok, _)| matches!(j, NetJob::InstallCli | NetJob::InstallMenu) && !ok)
        .map(|(_, _, text)| card_row(false).child(output_box(false, text).flex_1()));
    section(
        "命令行",
        None,
        div().flex().flex_col().gap(px(8.)).child(
            card()
                .child(row(
                    true,
                    "lolinet",
                    lolinet::BIN.to_string(),
                    lolinet_state,
                ))
                .child(row(false, "lolidesk", desk_path, desk_state))
                .children(menu_state.map(|st| row(false, "菜单", menu_path, st)))
                .children(failed),
        ),
    )
}

/// 家目录下的路径写成 ~/…
fn home_short(p: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// 一件事的输出：等宽小字，成功是平常的灰，失败是红边。
fn output_box(ok: bool, text: &str) -> Div {
    let text = if text.trim().is_empty() {
        if ok { "完成了" } else { "失败了" }.to_string()
    } else {
        text.to_string()
    };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(12.))
        .py(px(10.))
        .rounded(R_SM)
        .bg(ink1())
        .border_1()
        .border_color(if ok { line1() } else { bad_line() })
        .child(caption(if ok { "输出" } else { "没成功" }))
        .child(
            div()
                .font_family(MONO)
                .text_size(T_CAP)
                .line_height(px(17.))
                .text_color(if ok { fg2() } else { bad() })
                .children(
                    text.lines()
                        .rev()
                        .take(40)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .map(|l| div().child(l.to_string())),
                ),
        )
}

// ---------------------------------------------------------------- 邀请

pub enum InviteState {
    Idle,
    Loading,
    Done(Invite),
    Failed(String),
}

/// 「邀请」对话框：选种类，问管理面要配对码（`lolinet invite --json`），显示码、命令、手机的二维码。
pub struct InviteForm {
    model: Entity<Model>,
    pub kind: InviteKind,
    pub state: InviteState,
    focus: FocusHandle,
}

impl Focusable for InviteForm {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl InviteForm {
    pub fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        Self {
            model,
            kind: InviteKind::Desktop,
            state: InviteState::Idle,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_kind(&mut self, kind: InviteKind, cx: &mut Context<Self>) {
        if self.kind != kind {
            self.kind = kind;
            if !matches!(self.state, InviteState::Loading) {
                self.state = InviteState::Idle;
            }
            cx.notify();
        }
    }

    /// 在后台跑 `lolinet invite --json --kind …`。
    pub fn generate(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, InviteState::Loading) {
            return;
        }
        let Some(backend) = self.model.read(cx).lolinet() else {
            self.state = InviteState::Failed("这台电脑上找不到 lolinet".into());
            cx.notify();
            return;
        };
        self.state = InviteState::Loading;
        let kind = self.kind;
        let task = cx.background_spawn(async move { backend.invite(kind) });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |f, cx| {
                // 等的时候换了种类：这个码不是现在要的
                f.state = match r {
                    Ok(inv) if f.kind == kind => InviteState::Done(inv),
                    Ok(_) => InviteState::Idle,
                    Err(e) => InviteState::Failed(crate::runtime::error_text(&e)),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn copy(&self, text: String, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.model
            .update(cx, |m, cx| m.toast(ToastKind::Success, "已复制", cx));
    }
}

impl Form for InviteForm {
    fn submit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.generate(cx)
    }
}

fn kind_icon(k: InviteKind) -> &'static str {
    match k {
        InviteKind::Desktop => ic::COMPUTER,
        InviteKind::Userspace => ic::TERMINAL,
        InviteKind::Phone => ic::PHONE,
    }
}

fn kind_note(k: InviteKind) -> &'static str {
    match k {
        InviteKind::Desktop => "新电脑上用 sudo 跑一条命令就进网，建一张网卡（TUN）。",
        InviteKind::Userspace => {
            "不建网卡的电脑：容器、NAS，或者 mihomo 之类已经占着 TUN。本机程序经 SOCKS5 进网。"
        }
        InviteKind::Phone => {
            "手机用 mihomo 客户端进网：扫码打开链接，在浏览器里解密出配置，导入 mihomo 客户端。"
        }
    }
}

impl Render for InviteForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let seg = div()
            .flex()
            .p(px(3.))
            .gap(px(2.))
            .rounded(R_PILL)
            .bg(ink1())
            .border_1()
            .border_color(line2())
            .children(InviteKind::ALL.iter().enumerate().map(|(i, &k)| {
                let on = k == self.kind;
                div()
                    .id(("inv-kind", i))
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .rounded(R_PILL)
                    .text_size(T_XS)
                    .font_weight(FontWeight::MEDIUM)
                    .when(on, |d| d.bg(ink4()).text_color(fg()))
                    .when(!on, |d| {
                        d.text_color(fg3())
                            .cursor_pointer()
                            .hover(|s| s.text_color(fg2()))
                    })
                    .child(icon(kind_icon(k), 13., if on { fg() } else { fg3() }))
                    .child(k.label())
                    .on_click(cx.listener(move |f, _, _, cx| f.set_kind(k, cx)))
            }));

        let body: AnyElement = match &self.state {
            InviteState::Idle => div()
                .text_size(T_XS)
                .text_color(fg2())
                .child(kind_note(self.kind))
                .into_any_element(),
            InviteState::Loading => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .h(px(40.))
                .child(Spinner::new().color(fg3()))
                .child(
                    div()
                        .text_size(T_XS)
                        .text_color(fg3())
                        .child("正在向内网管理面要配对码…"),
                )
                .into_any_element(),
            InviteState::Failed(e) => div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(div().text_size(T_XS).text_color(bad()).child(e.clone()))
                .child(
                    div()
                        .text_size(T_CAP)
                        .text_color(fg3())
                        .child("lolinet invite 要经内网连到管理面；看看这台是不是在网里、管理面允不允许邀请这一种。"),
                )
                .into_any_element(),
            InviteState::Done(inv) => self.invite_view(inv, cx),
        };

        let footer = {
            let done = matches!(self.state, InviteState::Done(_));
            let go = match &self.state {
                InviteState::Loading => pill_busy("inv-go", "生成中…", true).into_any_element(),
                _ => pill(
                    "inv-go",
                    if done {
                        "再要一个"
                    } else {
                        "生成配对码"
                    },
                    !done,
                )
                .on_click(cx.listener(|f, _, _, cx| f.generate(cx)))
                .into_any_element(),
            };
            let close = pill("inv-close", if done { "完成" } else { "关闭" }, done)
                .on_click(|_, window, cx| window.close_dialog(cx));
            div()
                .flex()
                .justify_end()
                .gap(px(8.))
                .child(go)
                .child(close)
        };

        div()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(18.))
            .child(header("邀请一台设备", ""))
            .child(seg)
            .child(body)
            .child(footer)
    }
}

impl InviteForm {
    fn invite_view(&self, inv: &Invite, cx: &mut Context<Self>) -> AnyElement {
        let minutes = inv.minutes_left(now_unix());
        let who = [inv.name.as_str(), inv.ip.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let code = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(caption("配对码"))
            .child(
                div()
                    .font_family(MONO)
                    .text_size(px(28.))
                    .line_height(px(36.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(eye())
                    .child(inv.code.clone()),
            )
            .when(!who.is_empty(), |d| {
                d.child(mono(format!("新设备 {who}"), T_CAP, fg3()))
            });
        let text = inv.command.clone();
        let copy_box = div()
            .flex()
            .items_start()
            .gap(px(8.))
            .pl(px(12.))
            .pr(px(6.))
            .py(px(6.))
            .rounded(R_SM)
            .bg(ink1())
            .border_1()
            .border_color(line2())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .py(px(4.))
                    .font_family(MONO)
                    .text_size(T_XS)
                    .line_height(px(18.))
                    .child(inv.command.clone()),
            )
            .child(
                icon_button("inv-copy", ic::COPY, "复制")
                    .on_click(cx.listener(move |f, _, _, cx| f.copy(text.clone(), cx))),
            );
        let expiry = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(dot(warn(), 5.))
            .child(
                div()
                    .text_size(T_XS)
                    .text_color(fg2())
                    .child(format!("{minutes} 分钟内有效，只能用一次")),
            );
        if inv.is_phone() {
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .flex()
                        .gap(px(18.))
                        .child(qr(&inv.command, 168.))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .gap(px(14.))
                                .child(code)
                                .child(
                                    div()
                                        .text_size(T_XS)
                                        .text_color(fg2())
                                        .child("用手机相机扫码打开。配置在浏览器里解密，配对码不经过服务器；取到后导入 mihomo 客户端（Clash Meta、FlClash、Stash……）。"),
                                ),
                        ),
                )
                .child(copy_box)
                .child(expiry)
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(code)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(T_XS)
                                .text_color(fg2())
                                .child("在新机器上运行（或者粘贴到它的 Lolidesk.app「网络」页）："),
                        )
                        .child(copy_box),
                )
                .child(expiry)
                .into_any_element()
        }
    }
}

/// 打开「邀请」。
pub fn open_invite(model: Entity<Model>, window: &mut Window, cx: &mut App) -> Entity<InviteForm> {
    let form = cx.new(|cx| InviteForm::new(model, cx));
    dialogs::open_form_sized(form.clone(), 500., window, cx);
    form
}

// ---------------------------------------------------------------- 二维码

/// 二维码：白底黑块（扫码器认这个），模块直接画成方块，同一行连着的并成一块。
/// 每个模块是整数像素，边上留 2 个模块的空白。
pub fn qr(text: &str, max: f32) -> AnyElement {
    use qrcode::{EcLevel, QrCode};
    let Ok(code) = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M) else {
        return div()
            .text_size(T_XS)
            .text_color(bad())
            .child("链接太长，画不出二维码")
            .into_any_element();
    };
    let n = code.width();
    let dark: Vec<bool> = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    const QUIET: usize = 2;
    let total = n + 2 * QUIET;
    let cell = (max / total as f32).floor().max(1.);
    let side = cell * total as f32;
    div()
        .flex_none()
        .size(px(side))
        .rounded(R_SM)
        .bg(solid())
        .child(
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<gpui_kit::Pixels>, _, window, _| {
                    let o = bounds.origin;
                    for y in 0..n {
                        let mut x = 0;
                        while x < n {
                            if !dark[y * n + x] {
                                x += 1;
                                continue;
                            }
                            let start = x;
                            while x < n && dark[y * n + x] {
                                x += 1;
                            }
                            let at = point(
                                o.x + px(cell * (QUIET + start) as f32),
                                o.y + px(cell * (QUIET + y) as f32),
                            );
                            let sz = size(px(cell * (x - start) as f32), px(cell));
                            window.paint_quad(fill(Bounds::new(at, sz), solid_ink()));
                        }
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    #[test]
    fn qr_fits() {
        use qrcode::{EcLevel, QrCode};
        let code = QrCode::with_error_correction_level(
            "https://join.example.com/m#ABCD-EFGH-JK23",
            EcLevel::M,
        )
        .unwrap();
        // 小版本：168 px 里每个模块至少 4 px
        assert!(code.width() + 4 <= 168 / 4, "{}", code.width());
    }
}
