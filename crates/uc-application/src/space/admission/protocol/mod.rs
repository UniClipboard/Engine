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
    CompletedJoinerActivation, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationStateError,
    JoinerActivationStatePort, JoinerStartMaterial, JoinerStartMaterialError,
    JoinerStartMaterialPort, JoinerStartMutation, JoinerStartStateError, JoinerStartStatePort,
    LoadedJoinerActivation, LoadedJoinerStartState, PrepareJoinerActivationError,
    PrepareJoinerActivationPort, PrepareJoinerAppliedError, PrepareJoinerAppliedPort,
    PrepareJoinerCandidateError, PrepareJoinerCandidatePort, PreparedJoinerActivation,
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
    PrepareSponsorCommitError, PrepareSponsorCommitPort, PrepareSponsorCompleteError,
    PrepareSponsorCompletePort, PrepareSponsorSettledError, PrepareSponsorSettledPort,
    PreparedSponsorCandidate, PreparedSponsorCommit, PreparedSponsorComplete,
    PreparedSponsorSettled, SpaceAdmissionMessageReply, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionState, SponsorAdmissionStateError,
    SponsorAdmissionStatePort,
};
