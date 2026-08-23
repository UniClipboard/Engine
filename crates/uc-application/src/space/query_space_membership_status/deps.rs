use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionAttemptRepositoryPort, CurrentMemberSignaturePort,
    HistoricalMembershipSignatureVerifier, MemberRepositoryPort,
};
use uc_core::ports::{ClockPort, PresencePort};

use crate::space::membership_state::SpaceMembershipStateRepositoryPort;

pub(crate) struct QuerySpaceMembershipStatusDeps {
    pub(crate) admission_attempts: Arc<dyn AdmissionAttemptRepositoryPort>,
    pub(crate) own_device: DeviceId,
    pub(crate) clock: Arc<dyn ClockPort>,
}

#[derive(Clone)]
pub(crate) struct ActiveSpaceMembershipStatusDeps {
    pub(crate) state_repository: Arc<dyn SpaceMembershipStateRepositoryPort>,
    pub(crate) historical_signatures: Arc<dyn HistoricalMembershipSignatureVerifier>,
    pub(crate) member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub(crate) member_repo: Arc<dyn MemberRepositoryPort>,
    pub(crate) presence: Arc<dyn PresencePort>,
}
