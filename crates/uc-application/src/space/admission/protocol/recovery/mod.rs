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
    host_events: Arc<crate::facade::HostEventBus>,
    pub(super) execution_lock: tokio::sync::Mutex<()>,
}

impl AdmissionRecoveryService {
    pub(crate) fn new(
        state: Arc<dyn PendingAdmissionRecoveryStatePort>,
        transport: Arc<dyn SpaceAdmissionTransportPort>,
        host_events: Arc<crate::facade::HostEventBus>,
    ) -> Self {
        Self {
            state,
            transport,
            host_events,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(super) async fn commit_recovery(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        self.state.commit(token, transition).await
    }

    pub(super) async fn commit_recovery_and_notify(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let loaded = self.commit_recovery(token, transition).await?;
        self.host_events
            .emit_or_warn(uc_core::ports::HostEvent::Membership(
                uc_core::ports::MembershipHostEvent::AdmissionChanged,
            ));
        Ok(loaded)
    }

    pub(super) async fn commit_recovery_with_optional_notification(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
        notify: bool,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let loaded = self.commit_recovery(token, transition).await?;
        if notify {
            self.host_events
                .emit_or_warn(uc_core::ports::HostEvent::Membership(
                    uc_core::ports::MembershipHostEvent::AdmissionChanged,
                ));
        }
        Ok(loaded)
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
