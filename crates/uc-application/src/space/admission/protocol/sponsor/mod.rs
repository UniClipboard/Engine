use std::sync::Arc;

mod handle_authenticated_message;
mod handle_join_request;
mod handle_prepared;
mod state;

pub use handle_authenticated_message::{
    HandleAuthenticatedSpaceAdmissionMessageError, HandleAuthenticatedSpaceAdmissionMessagePort,
};
pub use handle_join_request::{
    AuthenticatedSpaceAdmissionMessage, PrepareSponsorCandidateError, PrepareSponsorCandidatePort,
    PreparedSponsorCandidate, SpaceAdmissionMessageReply,
};
pub use handle_prepared::{
    PrepareSponsorCommitError, PrepareSponsorCommitPort, PreparedSponsorCommit,
};
pub use state::{
    CommittedSponsorAdmission, LoadedSponsorAdmission, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionState, SponsorAdmissionStateError,
    SponsorAdmissionStatePort,
};

pub(crate) struct SponsorAdmissionService {
    pub(super) state: Arc<dyn SponsorAdmissionStatePort>,
    pub(super) prepare_candidate: Arc<dyn PrepareSponsorCandidatePort>,
    pub(super) prepare_commit: Arc<dyn PrepareSponsorCommitPort>,
}

impl SponsorAdmissionService {
    pub(crate) fn new(
        state: Arc<dyn SponsorAdmissionStatePort>,
        prepare_candidate: Arc<dyn PrepareSponsorCandidatePort>,
        prepare_commit: Arc<dyn PrepareSponsorCommitPort>,
    ) -> Self {
        Self {
            state,
            prepare_candidate,
            prepare_commit,
        }
    }
}
