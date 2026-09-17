use std::sync::Arc;

use tokio::time::timeout_at;

use super::error::{deadline_elapsed, sanitize_task_failure};
use super::{LifecycleTarget, RuntimeLifecyclePort, TransitionContext};

pub(super) async fn invoke(
    participant: &Arc<dyn RuntimeLifecyclePort>,
    target: LifecycleTarget,
    context: &TransitionContext,
) -> anyhow::Result<()> {
    let participant = Arc::clone(participant);
    let context = context.clone();
    let deadline = context.deadline();
    // 独立任务隔离参与者 panic；负责人只接收不含 panic payload 的稳定失败并继续其他收尾。
    tokio::spawn(async move {
        let invocation = async {
            match target {
                LifecycleTarget::Active => participant.resume(&context).await,
                LifecycleTarget::Suspended => participant.suspend(&context).await,
            }
        };
        match deadline {
            Some(deadline) => timeout_at(deadline, invocation)
                .await
                .map_err(|_| deadline_elapsed())?,
            None => invocation.await,
        }
    })
    .await
    .map_err(sanitize_task_failure)?
}
