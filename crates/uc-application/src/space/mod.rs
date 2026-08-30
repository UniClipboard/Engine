//! Space-scoped application workflows.
//!
//! A member roster and every membership transition only exist inside a space.
//! The directory groups the complete space workflows by responsibility:
//!
//! - `lifecycle` — create, unlock, switch, reset and the space session;
//! - `admission` — pairing invitations, joining, transitions and admission
//!   message recovery;
//! - `membership` — the verified ledger, commands, queries, history exchange,
//!   member signing and background recovery;
//! - `connectivity` — network session recovery without membership authority.
//!
//! Everything that belongs to a space stays inside this directory. Child
//! modules are private; this root exports only the contracts used through
//! `crate::facade` and `crate::deps`. See `AGENTS.md` for the code map.

mod admission;
mod application;
mod connectivity;
mod facade;
mod lifecycle;
mod membership;

// Caller-facing facade contract.
pub use admission::{
    CancelInvitationError, CancelSpaceJoinError, CompletePendingSpaceTransitionError,
    CurrentJoinStatus, JoinSpaceError, JoinSpaceInput, JoinSpaceResult, JoinedSpace,
    PairingInvitationAddressCandidate, PendingInboundMember, QueryPairingInvitationAddressesError,
    QueryPendingSpaceTransitionError,
};
pub use connectivity::{
    NetworkRecoveryEvent, NetworkRecoveryFacade, NetworkRecoveryPhase, NetworkRecoveryRequestError,
    NetworkRecoveryStatus, RebuildNetworkSessionError, RebuildNetworkSessionPort,
};
pub use facade::{
    InitializeSpaceInput, InvitationAvailability, IssuePairingInvitationError,
    IssuePairingInvitationResult, RedeemPairingInvitationError, SpaceAdmissionDeps, SpaceFacade,
    SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps, UnlockSpaceInput, UnlockSpaceResult,
};
pub use lifecycle::{CurrentInvitation, QuerySetupStateError, SetupStateView};
pub use lifecycle::{
    InitializeSpaceError, InitializeSpaceResult, LockSpaceSessionError, QuerySpaceAccessStateError,
    RecoverSpaceSessionError, RecoverSpaceSessionResult, ResetSpaceError, SpaceAccessState,
    UnlockSpaceError,
};
pub use membership::{
    DecideDeviceTrustChange, DecideDeviceTrustChangeError, DecideDeviceTrustChangeResult,
    DeviceTrustChangeChoice,
};
pub use membership::{
    DeviceTrustDevice, DeviceTrustMembership, DeviceTrustObservation, DeviceTrustRelationship,
    DeviceTrustStatus, DeviceTrustSyncState, PendingDeviceTrustChange, QueryDeviceTrustError,
};
pub use membership::{MembershipCommitReceipt, RemoveSpaceMemberError, RemoveSpaceMemberResult};
pub use membership::{
    ResolveMembershipConflictError, ResolveMembershipConflictInput, ResolveMembershipConflictResult,
};

// Assembly contract re-exported by `crate::deps`.
pub use admission::{
    ActivateCompletionHelperAdmissionSecurityPort,
    ActivateCompletionHelperAdmissionSecurityRequest, ActivateSponsorAdmissionSecurityPort,
    ActivateSponsorAdmissionSecurityRequest, AdmissionSecurityTransitionError,
    AdmissionSecurityTransitionInput, AdmissionSecurityTransitionPort,
    JoinerStagedSecurityTransition, PrepareSponsorAdmissionSecurityPort,
    PreparedMemberSecurityDelivery, SponsorAdmissionSecurityRecipient,
    SponsorAdmissionSecurityRequest, SponsorPreparedAdmissionSecurity,
    SponsorPreparedSecurityTransition,
};
pub use admission::{
    ActivateSponsorAdmissionError, ActivateSponsorAdmissionPort, AdmissionRecoveryCommitToken,
    AdmissionRecoveryReport, AdmissionRecoveryTrigger, AuthenticatedAdmissionExchangePort,
    AuthenticatedAdmissionReply, AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    CompletedJoinerActivation, CurrentJoinAdmissionStatePort, ExecuteJoinerActivationError,
    ExecuteJoinerActivationPort, HandleAuthenticatedSpaceAdmissionMessageError,
    HandleAuthenticatedSpaceAdmissionMessagePort, JoinerActivationCommitToken,
    JoinerActivationMutation, JoinerActivationOutcome, JoinerActivationStateError,
    JoinerActivationStatePort, JoinerCancellationCommitToken, JoinerCancellationMaterial,
    JoinerCancellationMaterialError, JoinerCancellationMutation, JoinerCancellationStateError,
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedCurrentJoin, LoadedJoinerActivation,
    LoadedJoinerStartState, LoadedPendingAdmission, LoadedSponsorAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PrepareJoinerAppliedError,
    PrepareJoinerAppliedPort, PrepareJoinerCancellationPort, PrepareJoinerCandidateError,
    PrepareJoinerCandidatePort, PrepareJoinerInvitationError, PrepareJoinerInvitationPort,
    PrepareSponsorCandidateError, PrepareSponsorCandidatePort, PrepareSponsorCommitError,
    PrepareSponsorCommitPort, PrepareSponsorCompleteError, PrepareSponsorCompletePort,
    PrepareSponsorSettledError, PrepareSponsorSettledPort, PreparedJoinerActivation,
    PreparedJoinerAppliedMaterial, PreparedJoinerCandidateMaterial, PreparedJoinerInvitation,
    PreparedSponsorCandidate, PreparedSponsorCommit, PreparedSponsorComplete,
    PreparedSponsorSettled, ResolveJoinerInvitationError, ResolveJoinerInvitationPort,
    SpaceAdmissionCommitToken, SpaceAdmissionMessageReply, SpaceAdmissionTransportError,
    SpaceAdmissionTransportPort, SponsorAdmissionCommitToken, SponsorAdmissionMutation,
    SponsorAdmissionState, SponsorAdmissionStateError, SponsorAdmissionStatePort,
};
pub use admission::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    DeviceManagementResetDataPort,
};
pub use application::SpaceApplicationDeps;
pub use lifecycle::UnlockSpacePort;
pub use lifecycle::{
    build_space_session_activity, IsSpaceUnlockedPort, MembershipSessionActivityPort,
    ResumeSpaceSessionPort, SpaceActivityError, SpaceSessionActivityDeps, SpaceSessionActivityPort,
};
pub use lifecycle::{
    CurrentSpaceIdentityError, CurrentSpaceIdentityPort, InitialSpaceActivationPort,
    PortableCurrentSpaceIdentityPort,
};
pub use lifecycle::{InitializeSpacePort, LockSpacePort};
pub use lifecycle::{
    PrepareSpaceAdmissionCredentialsPort, SpaceAdmissionCredentialPreparationError,
};
pub use lifecycle::{
    RebindSpaceSessionPort, SpaceRebuildProgressError, SpaceRebuildProgressPort,
    SpaceSessionRebindError,
};
pub use membership::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    CommitMembershipLedgerPort, CurrentSpaceMemberScope, CurrentSpaceMemberScopeError,
    CurrentSpaceMemberScopePort, InboundMembershipTransfer, LoadMembershipLedgerPort,
    LoadedMembershipLedger, MembershipEffectExecutionError, MembershipEffectKind,
    MembershipEffectPhase, MembershipLedgerError, MembershipLedgerMutation, PausedSpaceMember,
    PeerHistorySyncState, PeerReconciliationRecord, PendingMembershipEffect,
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, SpaceMemberPauseReason,
};
pub use membership::{
    CleanupLegacyMembershipDataPort, DeliverRestrictedMembershipPort,
    MembershipMaintenanceStepOutcome, MembershipNetworkActivityPort, RecoverMembershipEffectsPort,
    RecoverSpaceAdmissionsPort,
};
pub use membership::{CurrentMemberSignatureError, CurrentMemberSignaturePort};
pub use membership::{
    LoadCurrentJoinStatusPort, LoadDeviceTrustObservationsPort, RePairingStateError,
    RePairingStateStorePort,
};

#[cfg(test)]
mod application_tests;
