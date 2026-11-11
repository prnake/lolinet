//! computer use：给内网里的 agent 截图、做键鼠（客户端是 `lolidesk mcp`，一个 stdio 的 MCP 服务）。
//!
//! 用的是远程桌面那份屏幕录制、辅助功能授权：能给人看桌面，就能给 agent 截图，不用再授权一次。
//! 主机愿不愿意给看 `lolidesk host allow`（allow.json 的 cua）。
//! 人优先：有人刚用过这台电脑（这台前面的人，或者远程桌面里的人），agent 的输入不做，回 UserActive。

use std::{collections::HashMap, time::Duration};

use anyhow::Result;
use lolidesk_proto::{self as proto, CuaReq, CuaResp, DeskInput};
use quinn::{RecvStream, SendStream};

use crate::{desktop, input};

/// 一次 Input 最多这么多个事件（一段文字是一个事件）
const MAX_EVENTS: usize = 256;

/// `allowed`：现在还给不给 computer use（主机开关 ∧ 这台设备的限制，每个请求现算）；`why`：不给时的原因
pub async fn serve(
    mut send: SendStream,
    mut recv: RecvStream,
    allowed: impl Fn() -> bool,
    why: String,
) -> Result<()> {
    let mut injectors: HashMap<u32, input::Injector> = HashMap::new();
    while let Some(req) = proto::recv::<CuaReq>(&mut recv).await? {
        let resp = if !allowed() {
            CuaResp::Denied(why.clone())
        } else {
            handle(req, &mut injectors).await
        };
        proto::send(&mut send, &resp).await?;
    }
    for inj in injectors.values() {
        inj.apply(DeskInput::ReleaseAll);
    }
    Ok(())
}

async fn handle(req: CuaReq, injectors: &mut HashMap<u32, input::Injector>) -> CuaResp {
    match req {
        CuaReq::Screenshot {
            display,
            max_edge,
            quality,
        } => match tokio::task::spawn_blocking(move || {
            desktop::screenshot(display, max_edge, quality)
        })
        .await
        {
            Ok(Ok(s)) => CuaResp::Screenshot {
                jpeg: s.jpeg,
                width: s.width,
                height: s.height,
                native_width: s.native_width,
                native_height: s.native_height,
            },
            Ok(Err(e)) => CuaResp::Error(format!("{e:#}")),
            Err(e) => CuaResp::Error(format!("截图任务：{e}")),
        },
        CuaReq::Input {
            display,
            events,
            gap_ms,
        } => {
            if !desktop::supported() {
                return CuaResp::Error("这个平台还不支持键鼠注入".into());
            }
            if events.len() > MAX_EVENTS {
                return CuaResp::Error(format!("一次最多 {MAX_EVENTS} 个输入"));
            }
            if let Some(idle_ms) = input::person_active() {
                return CuaResp::UserActive { idle_ms };
            }
            let inj = injectors
                .entry(display)
                .or_insert_with(|| input::Injector::new(display));
            let gap = Duration::from_millis(gap_ms.min(2000) as u64);
            for (i, ev) in events.into_iter().enumerate() {
                if i > 0 && !gap.is_zero() {
                    inj.flush().await;
                    tokio::time::sleep(gap).await;
                }
                input::AGENT_LAST.store(input::now_ms(), std::sync::atomic::Ordering::Relaxed);
                inj.apply(ev);
            }
            inj.apply(DeskInput::ReleaseAll);
            inj.flush().await;
            input::AGENT_LAST.store(input::now_ms(), std::sync::atomic::Ordering::Relaxed);
            CuaResp::Done
        }
        CuaReq::Cursor => match input::cursor() {
            Some((display, x, y)) => CuaResp::Cursor { display, x, y },
            None => CuaResp::Error("拿不到鼠标位置".into()),
        },
    }
}
