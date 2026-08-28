mod handle_authenticated_message;
mod joiner;
mod protocol;
mod recover_pending;
mod recovery;
mod sponsor;
mod start_join;
#[cfg(test)]
mod test_support;

pub use handle_authenticated_message::{
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    HandleAuthenticatedSpaceAdmissionMessageError, HandleAuthenticatedSpaceAdmissionMessagePort,
    LoadedSponsorJoinRequest, PrepareSponsorCandidateError, PrepareSponsorCandidatePort,
    PreparedSponsorCandidate, SpaceAdmissionMessageReply, SponsorAdmissionMutation,
    SponsorJoinRequestCommitToken, SponsorJoinRequestState, SponsorJoinRequestStateError,
    SponsorJoinRequestStatePort,
};
pub(crate) use joiner::JoinerAdmissionService;
pub(crate) use protocol::SpaceAdmissionProtocol;
pub use recover_pending::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, LoadedPendingAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerCandidateError, PrepareJoinerCandidatePort, PreparedJoinerCandidateMaterial,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
pub(crate) use recovery::AdmissionRecoveryService;
pub(crate) use sponsor::SponsorAdmissionService;
pub use start_join::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState, SpaceAdmissionCommitToken,
};
