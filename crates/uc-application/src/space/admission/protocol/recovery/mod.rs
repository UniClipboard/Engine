use std::sync::Arc;

use uc_core::membership::JoinerAdmissionTransition;

mod recover_pending;

pub use recover_pending::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, LoadedPendingAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};

pub(crate) struct AdmissionRecoveryService {
    pub(super) state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub(super) transport: Arc<dyn SpaceAdmissionTransportPort>,
}

impl AdmissionRecoveryService {
    pub(crate) fn new(
        state: Arc<dyn PendingAdmissionRecoveryStatePort>,
        transport: Arc<dyn SpaceAdmissionTransportPort>,
    ) -> Self {
        Self { state, transport }
    }

    pub(super) async fn commit_recovery(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let started = std::time::Instant::now();
        let result = self.state.commit(token, transition).await;
        super::record_performance_phase("joiner_state_commit", started, result.is_ok());
        result
    }

    pub(super) fn record_state_error(
        &self,
        report: &mut AdmissionRecoveryReport,
        error: PendingAdmissionRecoveryStateError,
    ) {
        match error {
            PendingAdmissionRecoveryStateError::RecoveryRequired => {
                report.recovery_required_count += 1;
            }
            PendingAdmissionRecoveryStateError::Locked
            | PendingAdmissionRecoveryStateError::Unavailable
            | PendingAdmissionRecoveryStateError::StateChanged => report.deferred_count += 1,
        }
    }
}
