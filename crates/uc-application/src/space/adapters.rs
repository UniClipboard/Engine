use std::sync::Arc;

use uc_core::membership::{
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort, GroupRevocationPort,
    GroupUpdateDispatchPort, HistoricalMembershipSignatureVerifier, MembershipHistoryExchangePort,
};

use super::admission::{
    ActivateSponsorAdmissionPort, CurrentJoinAdmissionStatePort, ExecuteJoinerActivationPort,
    JoinerActivationStatePort, JoinerStartMaterialPort, JoinerStartStatePort,
    PendingAdmissionRecoveryStatePort, PrepareJoinerActivationPort, PrepareJoinerAppliedPort,
    PrepareJoinerCancellationPort, PrepareJoinerCandidatePort, PrepareJoinerInvitationPort,
    PrepareSponsorCandidatePort, PrepareSponsorCommitPort, PrepareSponsorCompletePort,
    PrepareSponsorSettledPort, ResolveJoinerInvitationPort, SpaceAdmissionTransportPort,
    SponsorAdmissionStatePort,
};
use super::membership::{
    ActivateMembershipEffectPort, AdvanceMembershipBranchTransitionPort,
    ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort, CleanupLegacyMembershipDataPort,
    CommitMembershipLedgerPort, CurrentMemberSignaturePort, LoadCurrentJoinStatusPort,
    LoadDeviceTrustObservationsPort, LoadMembershipLedgerPort, MembershipBranchRecoveryChannelPort,
    MembershipNetworkActivityPort, PrepareMembershipBranchRecoveryMaterialPort,
    PrepareMembershipBranchRecoveryRecipientPort, PrepareMembershipBranchTransitionPort,
    RestrictedMembershipDeliveryPort,
};

/// Engine 一次提交给 Space admission owner 的完整 adapter 集合。
///
/// 该类型只表达 Application 的真实依赖，不选择具体实现，也不包含观测 policy。
pub struct SpaceAdmissionAdapters {
    pub prepare_joiner_invitation: Arc<dyn PrepareJoinerInvitationPort>,
    pub resolve_joiner_invitation: Arc<dyn ResolveJoinerInvitationPort>,
    pub joiner_start_material: Arc<dyn JoinerStartMaterialPort>,
    pub joiner_start_state: Arc<dyn JoinerStartStatePort>,
    pub current_join_admission_state: Arc<dyn CurrentJoinAdmissionStatePort>,
    pub prepare_joiner_cancellation: Arc<dyn PrepareJoinerCancellationPort>,
    pub pending_admission_recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub space_admission_transport: Arc<dyn SpaceAdmissionTransportPort>,
    pub sponsor_admission_state: Arc<dyn SponsorAdmissionStatePort>,
    pub prepare_sponsor_candidate: Arc<dyn PrepareSponsorCandidatePort>,
    pub prepare_sponsor_commit: Arc<dyn PrepareSponsorCommitPort>,
    pub prepare_sponsor_complete: Arc<dyn PrepareSponsorCompletePort>,
    pub activate_sponsor_admission: Arc<dyn ActivateSponsorAdmissionPort>,
    pub prepare_sponsor_settled: Arc<dyn PrepareSponsorSettledPort>,
    pub prepare_joiner_candidate: Arc<dyn PrepareJoinerCandidatePort>,
    pub prepare_joiner_applied: Arc<dyn PrepareJoinerAppliedPort>,
    pub prepare_joiner_activation: Arc<dyn PrepareJoinerActivationPort>,
    pub joiner_activation_state: Arc<dyn JoinerActivationStatePort>,
    pub execute_joiner_activation: Arc<dyn ExecuteJoinerActivationPort>,
    pub current_join_status: Arc<dyn LoadCurrentJoinStatusPort>,
}

/// Engine 一次提交给 Space membership owner 的完整 adapter 集合。
///
/// admission 与 membership 共用但只由一侧持有的能力不得在两个 bundle 中重复。
pub struct SpaceMembershipAdapters {
    pub load_membership_ledger: Arc<dyn LoadMembershipLedgerPort>,
    pub commit_membership_ledger: Arc<dyn CommitMembershipLedgerPort>,
    pub historical_membership_signatures: Arc<dyn HistoricalMembershipSignatureVerifier>,
    pub current_member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub membership_identity: Arc<dyn CurrentMembershipIdentityPort>,
    pub membership_announcement: Arc<dyn CurrentMembershipAnnouncementPort>,
    pub device_trust_observations: Arc<dyn LoadDeviceTrustObservationsPort>,
    pub membership_history_transport: Arc<dyn MembershipHistoryExchangePort>,
    pub membership_branch_recovery_channel: Arc<dyn MembershipBranchRecoveryChannelPort>,
    pub membership_branch_recovery_recipient: Arc<dyn PrepareMembershipBranchRecoveryRecipientPort>,
    pub membership_branch_transition: Arc<dyn PrepareMembershipBranchTransitionPort>,
    pub membership_branch_transition_executor: Arc<dyn AdvanceMembershipBranchTransitionPort>,
    pub membership_branch_recovery_material: Arc<dyn PrepareMembershipBranchRecoveryMaterialPort>,
    pub apply_membership_member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    pub apply_membership_security: Arc<dyn ApplyMembershipSecurityPort>,
    pub activate_membership_effect: Arc<dyn ActivateMembershipEffectPort>,
    pub restricted_membership_delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
    pub group_update_store: Arc<dyn GroupRevocationPort>,
    pub group_update_dispatch: Arc<dyn GroupUpdateDispatchPort>,
    pub cleanup_legacy_membership_data: Arc<dyn CleanupLegacyMembershipDataPort>,
    pub membership_network_activity: Arc<dyn MembershipNetworkActivityPort>,
}

/// Engine 提交给 SpaceApplication 的领域分组 adapter interface。
pub struct SpaceRuntimeAdapters {
    pub admission: SpaceAdmissionAdapters,
    pub membership: SpaceMembershipAdapters,
}
