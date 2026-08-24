use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::CurrentMemberSignaturePort;

use crate::space::membership_history::MembershipHistoryStore;
use crate::space::membership_runtime::MembershipRecoveryRequests;
use crate::space::membership_state::{
    SpaceMembershipStateEvents, SpaceMembershipStateRepositoryPort,
};

/// 发起成员移除所需的内部能力。
pub(crate) struct InitiateSpaceMemberRemovalDeps {
    pub(crate) membership_history: Arc<MembershipHistoryStore>,
    pub(crate) state_repo: Arc<dyn SpaceMembershipStateRepositoryPort>,
    pub(crate) member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub(crate) own_device: DeviceId,
    pub(crate) state_write_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) state_events: SpaceMembershipStateEvents,
    pub(crate) recovery_requests: MembershipRecoveryRequests,
}
