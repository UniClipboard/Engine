use super::{AdmissionRecoveryService, JoinerAdmissionService, SponsorAdmissionService};

pub(crate) struct SpaceAdmissionProtocol {
    pub(super) joiner: JoinerAdmissionService,
    pub(super) sponsor: SponsorAdmissionService,
    pub(super) recovery: AdmissionRecoveryService,
    execution_lock: tokio::sync::Mutex<()>,
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
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(super) async fn execute_exclusively<T>(
        &self,
        action: impl std::future::Future<Output = T>,
    ) -> T {
        let _guard = self.execution_lock.lock().await;
        action.await
    }
}
