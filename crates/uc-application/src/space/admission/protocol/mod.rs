mod joiner;
mod protocol;
mod recovery;
mod sponsor;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

fn record_performance_phase(phase: &'static str, started: std::time::Instant, success: bool) {
    tracing::info!(
        target: "admission.performance",
        phase,
        elapsed_ms = started.elapsed().as_millis() as u64,
        outcome = if success { "ok" } else { "error" },
        "pairing subphase completed"
    );
}

pub(crate) use joiner::JoinerAdmissionService;
pub use joiner::{
    CompletedJoinerActivation, CurrentJoinAdmissionStatePort, ExecuteJoinerActivationError,
    ExecuteJoinerActivationPort, JoinerActivationCommitToken, JoinerActivationMutation,
    JoinerActivationOutcome, JoinerActivationStateError, JoinerActivationStatePort,
    JoinerCancellationCommitToken, JoinerCancellationMaterial, JoinerCancellationMaterialError,
    JoinerCancellationMutation, JoinerCancellationStateError, JoinerStartMaterial,
    JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation, JoinerStartStateError,
    JoinerStartStatePort, LoadedCurrentJoin, LoadedJoinerActivation, LoadedJoinerStartState,
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PrepareJoinerAppliedError,
    PrepareJoinerAppliedPort, PrepareJoinerCancellationPort, PrepareJoinerCandidateError,
    PrepareJoinerCandidatePort, PrepareJoinerInvitationError, PrepareJoinerInvitationPort,
    PreparedJoinerActivation, PreparedJoinerAppliedMaterial, PreparedJoinerCandidateMaterial,
    PreparedJoinerInvitation, ResolveJoinerInvitationError, ResolveJoinerInvitationPort,
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
    ActivateSponsorAdmissionError, ActivateSponsorAdmissionPort,
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
