use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use uc_application::deps::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, LoadedPendingAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
};
use uc_core::membership::JoinerAdmissionTransition;

#[derive(Clone, Copy)]
enum AdmissionRecoveryStateOperation {
    Load,
    Commit,
}

impl AdmissionRecoveryStateOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Load => "recovery_state_load",
            Self::Commit => "recovery_state_commit",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AdmissionRecoveryObservationPolicy {
    suppress_successful_empty_loads: bool,
}

impl AdmissionRecoveryObservationPolicy {
    pub(crate) const fn suppress_successful_empty_loads() -> Self {
        Self {
            suppress_successful_empty_loads: true,
        }
    }
}

pub(crate) struct ObservedAdmissionRecoveryState {
    inner: Arc<dyn PendingAdmissionRecoveryStatePort>,
    policy: AdmissionRecoveryObservationPolicy,
}

impl ObservedAdmissionRecoveryState {
    pub(crate) fn new(
        inner: Arc<dyn PendingAdmissionRecoveryStatePort>,
        policy: AdmissionRecoveryObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }

    fn record(
        operation: AdmissionRecoveryStateOperation,
        started: Instant,
        success: bool,
        loaded_count: Option<usize>,
    ) {
        tracing::info!(
            target: "admission.performance",
            operation = operation.as_str(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = if success { "ok" } else { "error" },
            loaded_count,
            "admission state operation completed"
        );
    }
}

#[async_trait]
impl PendingAdmissionRecoveryStatePort for ObservedAdmissionRecoveryState {
    async fn load(
        &self,
        trigger: AdmissionRecoveryTrigger,
    ) -> Result<Vec<LoadedPendingAdmission>, PendingAdmissionRecoveryStateError> {
        let started = Instant::now();
        let result = self.inner.load(trigger).await;
        let loaded_count = result.as_ref().ok().map(Vec::len);
        let suppress = self.policy.suppress_successful_empty_loads && loaded_count == Some(0);
        if !suppress {
            Self::record(
                AdmissionRecoveryStateOperation::Load,
                started,
                result.is_ok(),
                loaded_count,
            );
        }
        result
    }

    async fn commit(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let started = Instant::now();
        let result = self.inner.commit(token, transition).await;
        Self::record(
            AdmissionRecoveryStateOperation::Commit,
            started,
            result.is_ok(),
            None,
        );
        result
    }
}
