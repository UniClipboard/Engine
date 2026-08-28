mod joiner;
mod protocol;
mod recovery;
mod sponsor;
#[cfg(test)]
mod test_support;

pub(crate) use joiner::JoinerAdmissionService;
pub use joiner::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState,
    PrepareJoinerCandidateError, PrepareJoinerCandidatePort, PreparedJoinerCandidateMaterial,
    SpaceAdmissionCommitToken,
};
pub(crate) use protocol::SpaceAdmissionProtocol;
pub(crate) use recovery::AdmissionRecoveryService;
pub use recovery::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryTrigger,
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply, LoadedPendingAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
pub(crate) use sponsor::SponsorAdmissionService;
pub use sponsor::{
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    HandleAuthenticatedSpaceAdmissionMessageError, HandleAuthenticatedSpaceAdmissionMessagePort,
    LoadedSponsorJoinRequest, PrepareSponsorCandidateError, PrepareSponsorCandidatePort,
    PreparedSponsorCandidate, SpaceAdmissionMessageReply, SponsorAdmissionMutation,
    SponsorJoinRequestCommitToken, SponsorJoinRequestState, SponsorJoinRequestStateError,
    SponsorJoinRequestStatePort,
};
