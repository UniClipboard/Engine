use std::sync::Arc;

use async_trait::async_trait;

use super::model::TransitionContext;

/// 完整能力必须可重复暂停和恢复；成功暂停表示实际停止完成。
#[async_trait]
pub trait RuntimeLifecyclePort: Send + Sync {
    async fn suspend(&self, context: &TransitionContext) -> anyhow::Result<()>;
    async fn resume(&self, context: &TransitionContext) -> anyhow::Result<()>;
}

/// 三类依赖显式命名，组装时不能靠相同类型参数的位置区分责任。
pub(crate) struct RuntimeLifecycleParticipants {
    pub session_work: Arc<dyn RuntimeLifecyclePort>,
    pub local_work: Arc<dyn RuntimeLifecyclePort>,
    pub local_resources: Arc<dyn RuntimeLifecyclePort>,
}
