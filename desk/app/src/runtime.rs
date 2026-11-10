//! 网络跑在一个 tokio 运行时上；GPUI 的任务 await 它的 JoinHandle（不挑执行器）。

use std::{future::Future, sync::OnceLock};

use tokio::{runtime::Runtime, task::JoinHandle};

pub fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lolidesk-net")
            .enable_all()
            .build()
            .expect("建 tokio 运行时")
    })
}

/// 在网络运行时上跑一个任务。
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    rt().spawn(fut)
}

/// 给 GPUI 那边 await：tokio 任务的结果，任务被取消 / panic 时当成错误。
pub async fn run<T: Send + 'static>(
    fut: impl Future<Output = anyhow::Result<T>> + Send + 'static,
) -> anyhow::Result<T> {
    match spawn(fut).await {
        Ok(r) => r,
        Err(e) => Err(anyhow::anyhow!("后台任务没了：{e}")),
    }
}

/// 报给用户的错误：anyhow 的整条原因链，去掉重复。
pub fn error_text(e: &anyhow::Error) -> String {
    let mut parts: Vec<String> = Vec::new();
    for cause in e.chain() {
        let s = cause.to_string();
        if !parts.iter().any(|p| p.contains(&s)) {
            parts.push(s);
        }
    }
    parts.join("：")
}
