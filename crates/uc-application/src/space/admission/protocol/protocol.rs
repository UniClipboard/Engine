use std::future::Future;
use std::sync::Arc;

use super::{AdmissionRecoveryService, JoinerAdmissionService, SponsorAdmissionService};
use super::{AdmissionRecoveryTrigger, PendingAdmissionRecoveryStateError};
use crate::space::membership::{
    AcquireSpaceWorkPermitPort, QuerySpaceWorkModeError, SpaceWorkPermit,
};
use tokio::sync::RwLock;
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

pub(crate) struct SpaceAdmissionProtocol {
    pub(super) joiner: JoinerAdmissionService,
    pub(super) sponsor: SponsorAdmissionService,
    pub(super) recovery: AdmissionRecoveryService,
    /// 准入动作独占执行；普通成员工作许可共享持有，只与准入互斥，彼此之间不互斥。成员历史交换的双方
    /// 可能同时各自持有许可并等待对方的入站处理，许可因此不能互斥。
    execution_lock: Arc<RwLock<()>>,
}

impl SpaceAdmissionProtocol {
    pub(crate) fn new(
        joiner: JoinerAdmissionService,
        sponsor: SponsorAdmissionService,
        recovery: AdmissionRecoveryService,
    ) -> Self {
        Self {
            joiner,
            sponsor,
            recovery,
            execution_lock: Arc::new(RwLock::new(())),
        }
    }

    pub(super) async fn execute_exclusively<T>(&self, action: impl Future<Output = T>) -> T {
        let waiting = LocalWorkObservation::begin(LocalWorkStep::ProtocolLock);
        let _guard = self.execution_lock.write().await;
        waiting.finish(LocalWorkOutcome::Ok);
        action.await
    }
}

#[async_trait::async_trait]
impl AcquireSpaceWorkPermitPort for SpaceAdmissionProtocol {
    async fn acquire_space_work_permit(&self) -> Result<SpaceWorkPermit, QuerySpaceWorkModeError> {
        let guard = Arc::clone(&self.execution_lock).read_owned().await;
        let loaded = self
            .recovery
            .state
            .load(
                AdmissionRecoveryTrigger::StateChanged,
                self.recovery.clock.now_ms(),
            )
            .await
            .map_err(|error| match error {
                PendingAdmissionRecoveryStateError::ReadFailure { .. }
                | PendingAdmissionRecoveryStateError::RecoveryRequired => {
                    QuerySpaceWorkModeError::NeedsAttention
                }
                PendingAdmissionRecoveryStateError::Locked
                | PendingAdmissionRecoveryStateError::Unavailable
                | PendingAdmissionRecoveryStateError::StateChanged => {
                    QuerySpaceWorkModeError::Unavailable
                }
            })?;
        let mode = loaded.work_mode();
        Ok(SpaceWorkPermit::guarded(mode, guard))
    }
}
