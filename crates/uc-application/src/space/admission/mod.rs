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
mod handle_space_admission_message;
mod invitation;
mod join_space;
mod model;
mod outbox;
mod protocol;
mod query_pending_space_transition;
mod recover_space_admissions;
mod security_transition;
mod space_transition;

pub use cancel_space_join::CancelSpaceJoinError;
pub use complete_pending_space_transition::CompletePendingSpaceTransitionError;
pub(crate) use handle_space_admission_message::{
    AcceptAdmissionError, ConsumedInvitation, InboundAdmissionStatePort, LoadMemberAdmissionError,
    LoadedMemberAdmissionActivation, MemberAdmissionCommitToken, PreparedMemberAdmissionActivation,
};
pub use invitation::{
    CancelInvitationError, PairingInvitationAddressCandidate, QueryPairingInvitationAddressesError,
};
pub use join_space::{JoinSpaceError, JoinSpaceInput, JoinSpaceResult};
pub use model::{CurrentJoinStatus, JoinedSpace, PendingInboundMember};
pub use outbox::{
    AdmissionOutboxDeliveryError, AdmissionOutboxDeliveryPort, AdmissionOutboxDeliveryResult,
    AdmissionOutboxDeliveryRoute, InvitationConsumeDeliveryResult,
};
pub use protocol::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    HandleAuthenticatedSpaceAdmissionMessageError, HandleAuthenticatedSpaceAdmissionMessagePort,
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState, LoadedPendingAdmission,
    LoadedSponsorJoinRequest, PendingAdmissionRecoveryStateError,
    PendingAdmissionRecoveryStatePort, PrepareJoinerCandidateError, PrepareJoinerCandidatePort,
    PrepareSponsorCandidateError, PrepareSponsorCandidatePort, PreparedJoinerCandidateMaterial,
    PreparedSponsorCandidate, SpaceAdmissionCommitToken, SpaceAdmissionMessageReply,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort, SponsorAdmissionMutation,
    SponsorJoinRequestCommitToken, SponsorJoinRequestState, SponsorJoinRequestStateError,
    SponsorJoinRequestStatePort,
};
pub use query_pending_space_transition::QueryPendingSpaceTransitionError;
pub use security_transition::{
    ActivateCompletionHelperAdmissionSecurityPort,
    ActivateCompletionHelperAdmissionSecurityRequest, ActivateSponsorAdmissionSecurityPort,
    ActivateSponsorAdmissionSecurityRequest, AdmissionSecurityTransitionError,
    AdmissionSecurityTransitionInput, AdmissionSecurityTransitionPort,
    JoinerStagedSecurityTransition, PrepareSponsorAdmissionSecurityPort,
    SponsorAdmissionSecurityDelivery, SponsorAdmissionSecurityRecipient,
    SponsorAdmissionSecurityRequest, SponsorPreparedAdmissionSecurity,
    SponsorPreparedSecurityTransition,
};
pub use space_transition::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    DeviceManagementResetDataPort,
};

pub(super) use cancel_space_join::CancelSpaceJoinUseCase;
pub(super) use complete_pending_space_transition::CompletePendingSpaceTransitionUseCase;
pub(super) use invitation::{
    CancelPairingInvitationUseCase, InMemoryPairingInvitationHolder,
    IssuePairingInvitationForAddressUseCase, IssuePairingInvitationUseCase,
    PairingInvitationIssuer, QueryPairingInvitationAddressesUseCase,
};
pub(super) use protocol::SpaceAdmissionProtocol;
pub(super) use query_pending_space_transition::QueryPendingSpaceTransitionUseCase;
