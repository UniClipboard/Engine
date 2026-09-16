mod aggregate;
mod capability;
mod helper;
mod joiner;
mod persistence;
mod replay;
mod sponsor;
mod terminal;
mod transition;
mod view;

use super::super::{
    AdmissionActivationReceipt, MemberInstanceId, MembershipEventId, MembershipOperationV2,
};
use super::artifact::{
    AdmissionActivatedSecurityState, AdmissionBaseSnapshot, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionHelperNonce, AdmissionHelperSecurityState,
    AdmissionInvitationClaim, AdmissionJoinerPrivateState, AdmissionJoinerStartContext,
    AdmissionPeerBinding, AdmissionSealedSecurityState, AdmissionShortInvitationCode,
    AdmissionSignedMembershipHistory, AdmissionSourceSnapshot, AdmissionSpaceTransition,
    AdmissionSpaceTransitionResult, AdmissionStagedSecurityState, AdmissionStagedTarget,
    AdmissionStagedTargetInput, SpaceAdmissionRoute,
};
use super::attempt::{
    AdmissionAttemptContractV2, AdmissionAttemptTimeline, AdmissionMemberBindingV2,
};
use super::exchange::{
    AdmissionErrorCategory, AdmissionExchangeBlockReason, AdmissionInboundDecision,
    AdmissionInboundExpectation, AdmissionMessageEvidence, AdmissionRetryState,
    PendingAdmissionExchange, SavedAdmissionReply,
};
use super::id::{AdmissionMessageId, JoinId, SpaceAdmissionId};
use super::message::{
    AdmissionAbandonedV2, AdmissionAbandonmentReasonV2, AdmissionAbandonmentV2, AdmissionRole,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionProtocolVersion, SpaceAdmissionRejectionReason,
};
use crate::ids::SpaceId;
use crate::pairing::invitation::FullInvitation;

pub use aggregate::{
    AdmissionEffect, AdmissionRecoveryCategory, AdmissionTransition, SpaceAdmissionAggregate,
    SpaceAdmissionAggregateError, SpaceAdmissionRecordState, SpaceAdmissionTerminalState,
    SPACE_ADMISSION_RECORD_FORMAT_V1, SPACE_ADMISSION_RECORD_FORMAT_V2,
};
pub use capability::{
    AdmissionRecordPersistence, JoinerAdmission, JoinerAdmissionTransition, SponsorAdmission,
    SponsorAdmissionTransition, StartedJoinerInvitationResolution,
};
pub use helper::{
    SpaceAdmissionCompletionHelperApplied, SpaceAdmissionCompletionHelperChallenged,
    SpaceAdmissionCompletionHelperState,
};
pub use joiner::{
    SpaceAdmissionInvitationResolutionState, SpaceAdmissionJoinerActivating,
    SpaceAdmissionJoinerApplied, SpaceAdmissionJoinerCancelling, SpaceAdmissionJoinerCandidate,
    SpaceAdmissionJoinerChannelState, SpaceAdmissionJoinerCommitted, SpaceAdmissionJoinerInitiated,
    SpaceAdmissionJoinerPrepared, SpaceAdmissionJoinerResolvedInvitation,
    SpaceAdmissionJoinerResolvingInvitation, SpaceAdmissionJoinerState,
};
pub use persistence::SpaceAdmissionPersistenceError;
pub(crate) use persistence::{decode_envelope_v1, encode_envelope_v1};
pub use sponsor::{
    SpaceAdmissionSponsorAccepted, SpaceAdmissionSponsorApplied, SpaceAdmissionSponsorCandidate,
    SpaceAdmissionSponsorCommitted, SpaceAdmissionSponsorState, SponsorPairingConfirmationStatus,
    SponsorPairingConfirmationSummary,
};
pub use terminal::{
    AdmissionCleanupObligation, AdmissionCommitKnowledge, SpaceAdmissionActivePendingSettlement,
    SpaceAdmissionActiveSettled, SpaceAdmissionActiveState, SpaceAdmissionCompletedTerminal,
    SpaceAdmissionJoinerRejected, SpaceAdmissionLocalJoinerRejected,
    SpaceAdmissionLocalJoinerTerminated, SpaceAdmissionRecoveryRequiredTerminal,
    SpaceAdmissionRejectedState, SpaceAdmissionSponsorExpired, SpaceAdmissionSponsorRejected,
    SpaceAdmissionSupersededState, SpaceAdmissionSupersededTerminal,
    SpaceAdmissionTerminationReason, SponsorAbandonmentCleanup,
};
pub use view::{
    AdmissionPendingRecovery, JoinerActivationPreparation, JoinerAppliedPreparation,
    JoinerCandidatePreparation, JoinerCompletePreparation, JoinerInvitationResolution,
    SponsorCandidatePreparation, SponsorCommitPreparation, SponsorCompletePreparation,
    SponsorSettlementPreparation,
};
