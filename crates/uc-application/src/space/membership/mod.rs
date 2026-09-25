mod access;
mod decide_device_trust_change;
mod group_update_delivery;
mod handle_history_message;
mod initializer;
mod maintenance;
mod owner;
mod ports;
mod projection;
mod query_admission;
mod query_device_group_choices;
mod query_device_trust;
mod query_diagnostics;
mod query_member_roster;
mod query_readiness;
mod re_pairing;
mod reconcile_history_evidence;
mod record;
mod recover_conflict;
mod remove_space_member;
mod resolve_conflict;
mod scope;
mod signing;
#[cfg(test)]
mod testing;
mod worker;

pub use access::{KnownPeerIdentity, PeerAccess, PeerIdentityDirectoryPort};
pub use decide_device_trust_change::{
    DecideDeviceTrustChange, DecideDeviceTrustChangeError, DecideDeviceTrustChangeResult,
    DeviceTrustChangeChoice,
};
pub(crate) use maintenance::PreparedSpaceMembershipMaintenanceRuntime;
pub use maintenance::{
    AcquireSpaceWorkPermitPort, AdmissionMaintenanceOutcome, DeliverPendingGroupUpdatesPort,
    KnownPeerContact, MembershipNetworkActivityPort, QuerySpaceWorkModeError,
    RecoverMembershipConflictsPort, RecoverMembershipEffectsPort, RecoverSpaceAdmissionsPort,
    SpaceWorkMode, SpaceWorkPermit,
};
pub use ports::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    MembershipEffectExecutionError, MembershipLedgerError, MembershipRecordCommit,
    MembershipRecordStorePort, RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, StagedMembershipRecord,
};
pub use projection::MembershipProjectionPlan;
pub(crate) use query_device_group_choices::QueryDeviceGroupChoicesUseCase;
pub use query_device_group_choices::{DeviceGroupChoicesView, QueryDeviceGroupChoicesError};
pub use query_device_trust::{
    AdmissionDisplayStatus, DeviceTrustDevice, DeviceTrustImpact, DeviceTrustMembership,
    DeviceTrustObservation, DeviceTrustRelationship, DeviceTrustStatus, DeviceTrustSyncState,
    LoadCurrentJoinStatusPort, LoadDeviceTrustObservationsPort, PairingConfirmationObservation,
    PairingConfirmationStatus, PairingConfirmationTarget, PendingDeviceTrustChange,
    QueryDeviceTrustError, SpaceDeviceUpdatePhase, SpaceDeviceUpdateProblem,
    SpaceDeviceUpdateRecovery, SpaceDeviceUpdateStatus,
};
pub(super) use query_diagnostics::QueryMembershipDiagnosticsUseCase;
pub use query_diagnostics::{MembershipDiagnosticsView, QueryMembershipDiagnosticsError};
pub use query_member_roster::RosterEntry;
pub(crate) use query_member_roster::{QueryMemberRosterError, QueryMemberRosterUseCase};
pub(crate) use query_readiness::QueryMembershipReadinessUseCase;
pub use query_readiness::{MembershipReadiness, QueryMembershipReadinessError};
pub use re_pairing::{RePairingStateError, RePairingStateStorePort};
pub(crate) use reconcile_history_evidence::ReconcileMembershipEvidenceUseCase;
pub use record::{
    InboundMembershipTransfer, MembershipBranchRecoveryRecord, MembershipBranchRecoverySession,
    MembershipBranchRecoverySessionState, MembershipConflictMember, MembershipConflictPresentation,
    MembershipConflictRecord, MembershipConflictStatus, MembershipHistoryExchangeRecord,
    MembershipRecord, SpaceMembershipRecord,
};
pub use recover_conflict::{
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort, BeginMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryError, IssueMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryPort, MembershipBranchRecoveryChannelError,
    MembershipBranchRecoveryChannelPort, MembershipBranchRecoveryCommit,
    MembershipBranchRecoveryRequest, PrepareMembershipBranchRecoveryMaterialError,
    PrepareMembershipBranchRecoveryMaterialInput, PrepareMembershipBranchRecoveryMaterialPort,
    PrepareMembershipBranchRecoveryRecipientError, PrepareMembershipBranchRecoveryRecipientPort,
    PrepareMembershipBranchTransitionError, PrepareMembershipBranchTransitionInput,
    PrepareMembershipBranchTransitionPort, PreparedMembershipBranchRecoveryMaterial,
    PreparedMembershipBranchRecoveryRecipient,
};
pub use remove_space_member::{
    AdmissionAbandonmentRevocationTarget, AdmissionRevocationPort, AdmissionRevocationResult,
    AdmissionRevocationTarget, MembershipCommitReceipt, RemoveSpaceMemberError,
    RemoveSpaceMemberResult,
};
pub use resolve_conflict::{
    DeviceGroupChoiceImpact, MembershipConflictBranchView, MembershipConflictView,
    MembershipConflictsView, QueryMembershipConflictsError, ResolveMembershipConflictError,
    ResolveMembershipConflictInput, ResolveMembershipConflictResult,
};
pub use scope::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    PausedSpaceMember, SpaceMemberPauseReason,
};
pub use signing::{CurrentMemberSignatureError, CurrentMemberSignaturePort};
#[cfg(test)]
pub(crate) use testing::{
    append_active_peer_to_history, member_facts, started_record, MemoryMembershipRecords,
    OwnerFixture,
};
pub use worker::RefreshVerifiedPeerAddressPort;

pub(super) use decide_device_trust_change::DecideDeviceTrustChangeUseCase;
pub(super) use group_update_delivery::DeliverPendingGroupUpdatesUseCase;
pub(crate) use group_update_delivery::RetainedGroupUpdateRecipientsPort;
pub(super) use handle_history_message::HandleMembershipHistoryMessageUseCase;
pub(super) use initializer::InitializeSpaceMembershipUseCase;
pub use maintenance::MembershipMaintenanceStepOutcome;
pub(super) use maintenance::{
    MaintainSpaceMembershipDeps, MaintainSpaceMembershipUseCase, MembershipMaintenanceReport,
    MembershipMaintenanceTrigger, RunMembershipWorkPort, SpaceMembershipMaintenanceActivity,
    SpaceMembershipMaintenanceRuntime, WakeSpaceMembershipMaintenancePort,
};
pub(crate) use owner::{ledger_error, pause_reason, MembershipOwner, MembershipView};
#[cfg(test)]
pub(super) use query_admission::{MembershipAdmissionSnapshot, QueryMembershipAdmissionError};
pub(super) use query_admission::{QueryMembershipAdmissionPort, QueryMembershipAdmissionUseCase};
pub(crate) use query_device_trust::LoadSecurityDeviceUpdateStatusPort;
pub(super) use query_device_trust::QueryDeviceTrustUseCase;
pub(super) use re_pairing::{RePairingState, ResolveRePairingPort};
pub(super) use recover_conflict::IssueMembershipBranchRecoveryUseCase;
pub(super) use recover_conflict::RecoverMembershipConflictUseCase;
pub(super) use remove_space_member::RemoveSpaceMemberUseCase;
pub(crate) use resolve_conflict::QueryMembershipConflictStatusPort;
pub(super) use resolve_conflict::ResolveMembershipConflictUseCase;
pub(crate) use worker::{
    MembershipWorker, MembershipWorkerDeps, RePairingAwareMembershipActivation,
};
