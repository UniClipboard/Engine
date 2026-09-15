use std::sync::Arc;

use uc_core::membership::{JoinerAdmissionTransition, SponsorAdmissionTransition};
use uc_core::ports::ClockPort;

use crate::facade::HostEventBus;
use crate::space::membership::AdmissionRevocationPort;

mod recover_pending;

pub use recover_pending::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, LoadedAdmissionRecovery,
    LoadedPendingAdmission, LoadedSponsorAbandonment, LoadedSponsorDeadline,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};

pub(crate) struct AdmissionRecoveryService {
    pub(super) state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub(super) transport: Arc<dyn SpaceAdmissionTransportPort>,
    host_events: Arc<HostEventBus>,
    pub(super) clock: Arc<dyn ClockPort>,
    pub(super) admission_revocation: Arc<dyn AdmissionRevocationPort>,
    pub(super) execution_lock: tokio::sync::Mutex<()>,
    interrupt_generation: tokio::sync::watch::Sender<u64>,
}

impl AdmissionRecoveryService {
    pub(crate) fn new(
        state: Arc<dyn PendingAdmissionRecoveryStatePort>,
        transport: Arc<dyn SpaceAdmissionTransportPort>,
        host_events: Arc<HostEventBus>,
        clock: Arc<dyn ClockPort>,
        admission_revocation: Arc<dyn AdmissionRevocationPort>,
    ) -> Self {
        let (interrupt_generation, _) = tokio::sync::watch::channel(0);
        Self {
            state,
            transport,
            host_events,
            clock,
            admission_revocation,
            execution_lock: tokio::sync::Mutex::new(()),
            interrupt_generation,
        }
    }

    pub(super) fn interrupt_current(&self) {
        self.interrupt_generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
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

    pub(super) async fn commit_sponsor_deadline_and_notify(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: SponsorAdmissionTransition,
    ) -> Result<LoadedSponsorDeadline, PendingAdmissionRecoveryStateError> {
        let loaded = self
            .state
            .commit_sponsor_deadline(token, transition)
            .await?;
        self.host_events
            .emit_or_warn(uc_core::ports::HostEvent::Membership(
                uc_core::ports::MembershipHostEvent::AdmissionChanged,
            ));
        Ok(loaded)
    }

    pub(super) async fn commit_sponsor_abandonment_and_notify(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: SponsorAdmissionTransition,
    ) -> Result<LoadedSponsorAbandonment, PendingAdmissionRecoveryStateError> {
        let loaded = self
            .state
            .commit_sponsor_abandonment(token, transition)
            .await?;
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

    pub(super) fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    pub(super) fn notify_admission_changed(&self) {
        self.host_events
            .emit_or_warn(uc_core::ports::HostEvent::Membership(
                uc_core::ports::MembershipHostEvent::AdmissionChanged,
            ));
    }
}
