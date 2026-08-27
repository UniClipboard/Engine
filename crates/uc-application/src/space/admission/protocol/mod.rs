mod handle_authenticated_message;
mod protocol;
mod recover_pending;
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
pub(crate) use protocol::SpaceAdmissionProtocol;
pub use recover_pending::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, LoadedPendingAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerCandidateError, PrepareJoinerCandidatePort, PreparedJoinerCandidateMaterial,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
pub use start_join::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState, SpaceAdmissionCommitToken,
};
