use std::future::Future;
use std::sync::Arc;

use super::{AdmissionRecoveryService, JoinerAdmissionService, SponsorAdmissionService};
use super::{AdmissionRecoveryTrigger, PendingAdmissionRecoveryStateError};
use crate::space::membership::{
    AcquireSpaceWorkPermitPort, QuerySpaceWorkModeError, SpaceWorkMode, SpaceWorkPermit,
};
use tokio::sync::Mutex;
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

pub(crate) struct SpaceAdmissionProtocol {
    pub(super) joiner: JoinerAdmissionService,
    pub(super) sponsor: SponsorAdmissionService,
    pub(super) recovery: AdmissionRecoveryService,
    execution_lock: Arc<Mutex<()>>,
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
            execution_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(super) async fn execute_exclusively<T>(&self, action: impl Future<Output = T>) -> T {
        let waiting = LocalWorkObservation::begin(LocalWorkStep::ProtocolLock);
        let _guard = self.execution_lock.lock().await;
        waiting.finish(LocalWorkOutcome::Ok);
        action.await
    }
}

#[async_trait::async_trait]
impl AcquireSpaceWorkPermitPort for SpaceAdmissionProtocol {
    async fn acquire_space_work_permit(&self) -> Result<SpaceWorkPermit, QuerySpaceWorkModeError> {
        let guard = Arc::clone(&self.execution_lock).lock_owned().await;
        let loaded = self
            .recovery
            .state
            .load(
                AdmissionRecoveryTrigger::StateChanged,
                self.recovery.clock.now_ms(),
            )
            .await
            .map_err(|error| match error {
                PendingAdmissionRecoveryStateError::RecoveryRequired => {
                    QuerySpaceWorkModeError::NeedsAttention
                }
                PendingAdmissionRecoveryStateError::Locked
                | PendingAdmissionRecoveryStateError::Unavailable
                | PendingAdmissionRecoveryStateError::StateChanged => {
                    QuerySpaceWorkModeError::Unavailable
                }
            })?;
        let mode = if loaded.pairing_in_progress() {
            SpaceWorkMode::Pairing
        } else {
            SpaceWorkMode::Active
        };
        Ok(SpaceWorkPermit::guarded(mode, guard))
    }
}
