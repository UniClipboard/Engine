mod joiner;
mod protocol;
mod recovery;
mod sponsor;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub(crate) use joiner::JoinerAdmissionService;
pub use joiner::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState, PrepareJoinerAppliedError,
    PrepareJoinerAppliedPort, PrepareJoinerCandidateError, PrepareJoinerCandidatePort,
    PreparedJoinerAppliedMaterial, PreparedJoinerCandidateMaterial, SpaceAdmissionCommitToken,
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
    LoadedSponsorAdmission, PrepareSponsorCandidateError, PrepareSponsorCandidatePort,
    PrepareSponsorCommitError, PrepareSponsorCommitPort, PreparedSponsorCandidate,
    PreparedSponsorCommit, SpaceAdmissionMessageReply, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionState, SponsorAdmissionStateError,
    SponsorAdmissionStatePort,
};
