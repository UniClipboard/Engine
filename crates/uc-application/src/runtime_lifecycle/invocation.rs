use std::sync::Arc;

use super::{LifecycleTarget, RuntimeLifecyclePort, TransitionContext};

pub(super) async fn invoke(
    participant: &Arc<dyn RuntimeLifecyclePort>,
    target: LifecycleTarget,
    context: &TransitionContext,
) -> anyhow::Result<()> {
    let participant = Arc::clone(participant);
    let context = TransitionContext::new(context.generation(), context.deadline());
    // 独立任务把参与者 panic 保留为 JoinError，让完整负责人继续处理其他收尾。
    tokio::spawn(async move {
        match target {
            LifecycleTarget::Active => participant.resume(&context).await,
            LifecycleTarget::Suspended => participant.suspend(&context).await,
        }
    })
    .await?
}
