//! 会话排空等待已开始的分支恢复签发实际结束。
use async_trait::async_trait;
use tokio::runtime::Handle;
use uc_application::deps::{
    BranchRecoveryLocalWorkPort, BranchRecoveryResponse, BranchRecoveryWork,
};
use uc_observability_contract::diagnostics::{connectivity::LocalWorkContext, ObservationContext};

use super::super::session_generation::begin_session_local_work;

pub struct BranchRecoveryLocalWork;

#[async_trait]
impl BranchRecoveryLocalWorkPort for BranchRecoveryLocalWork {
    async fn execute(
        &self,
        work: BranchRecoveryWork,
    ) -> Result<Result<BranchRecoveryResponse, anyhow::Error>, anyhow::Error> {
        let local = begin_session_local_work()
            .ok_or_else(|| anyhow::anyhow!("recovery session is draining"))?;
        let runtime = Handle::current();
        let context = LocalWorkContext::capture();
        let observation = ObservationContext::capture();
        Ok(tokio::task::spawn_blocking(move || {
            let _local = local;
            context.run(|| runtime.block_on(observation.scope(work)))
        })
        .await?)
    }
}
