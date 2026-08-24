use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::CurrentMemberSignaturePort;
use uc_core::ports::ClockPort;

use crate::space::membership_history::MembershipHistoryStore;
use crate::space::membership_runtime::MembershipRecoveryRequests;
use crate::space::membership_state::SpaceMembershipStateEvents;
use crate::space::membership_state::SpaceMembershipStateRepositoryPort;
use crate::space::query_space_membership_status::QuerySpaceMembershipStatusUseCase;

/// 决定一项待处理成员移除所需的内部能力。
pub(crate) struct DecidePendingMembershipRemovalDeps {
    pub(crate) membership_history: Arc<MembershipHistoryStore>,
    pub(crate) state_repository: Arc<dyn SpaceMembershipStateRepositoryPort>,
    pub(crate) member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub(crate) own_device: DeviceId,
    pub(crate) clock: Arc<dyn ClockPort>,
    pub(crate) state_write_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) state_events: SpaceMembershipStateEvents,
    pub(crate) recovery_requests: MembershipRecoveryRequests,
    pub(crate) membership_status_query: Arc<QuerySpaceMembershipStatusUseCase>,
}
