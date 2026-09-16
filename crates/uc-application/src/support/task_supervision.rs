use std::future::Future;

use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};

/// 监督没有长期生命周期 owner 的一次性后台工作，固定分类报告 panic。
pub(crate) fn spawn_supervised<F>(
    task_kind: DiagnosticTaskKind,
    future: F,
) -> tokio::task::JoinHandle<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let work = tokio::spawn(future);
    tokio::spawn(async move {
        match work.await {
            Ok(()) => {}
            Err(error) if error.is_cancelled() => {}
            Err(_) => record_task_join_failure(task_kind),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn supervisor_completes_even_when_the_owned_work_panics() {
        let handle = spawn_supervised(DiagnosticTaskKind::ClipboardDeferredDrain, async {
            panic!("PRIVATE_TASK_PANIC");
        });

        handle.await.expect("supervisor remains healthy");
    }
}
