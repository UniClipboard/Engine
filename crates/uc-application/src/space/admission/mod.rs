//! Workspace admission channel (ADR-017): the private internal communication
//! implementation of workspace admission, plus the pairing use cases.
//!
//! This module owns Space invitation commands, the complete join command,
//! durable admission progression, and restart recovery. Membership rules and
//! accepted member history are committed through the membership ledger.
//!
//! Sessions and invitations exist only in memory here; process interruption
//! discards them and recovery relies solely on the owner's encrypted saved
//! member changes and admission records.
//!
//! Invitation issuance (B1), redemption (B2), and the complete join command
//! live in this subdomain as well. The join use case owns device-name
//! persistence and the best-effort network preparation before redemption.

mod cancel_space_join;
mod complete_pending_space_transition;
mod invitation;
mod join_space;
mod model;
mod protocol;
mod query_pending_space_transition;
mod security_transition;
mod space_transition;

pub use cancel_space_join::CancelSpaceJoinError;
pub use complete_pending_space_transition::CompletePendingSpaceTransitionError;
pub use invitation::{
    CancelInvitationError, PairingInvitationAddressCandidate, QueryPairingInvitationAddressesError,
};
pub use join_space::{JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
pub use model::{CurrentJoinStatus, JoinedSpace, PendingInboundMember};
pub use protocol::{
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
pub use query_pending_space_transition::QueryPendingSpaceTransitionError;
pub use security_transition::{
    ActivateCompletionHelperAdmissionSecurityPort,
    ActivateCompletionHelperAdmissionSecurityRequest, ActivateSponsorAdmissionSecurityPort,
    ActivateSponsorAdmissionSecurityRequest, AdmissionSecurityTransitionError,
    AdmissionSecurityTransitionInput, AdmissionSecurityTransitionPort,
    JoinerStagedSecurityTransition, PrepareSponsorAdmissionSecurityPort,
    PreparedMemberSecurityDelivery, SponsorAdmissionSecurityRecipient,
    SponsorAdmissionSecurityRequest, SponsorPreparedAdmissionSecurity,
    SponsorPreparedSecurityTransition,
};
pub use space_transition::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    DeviceManagementResetDataPort,
};

pub(super) use invitation::{
    CancelPairingInvitationUseCase, InMemoryPairingInvitationHolder,
    IssuePairingInvitationForAddressUseCase, IssuePairingInvitationUseCase,
    PairingInvitationIssuer, QueryPairingInvitationAddressesUseCase,
};
pub(super) use protocol::{
    AdmissionRecoveryService, JoinerAdmissionService, SpaceAdmissionProtocol,
    SponsorAdmissionService,
};
