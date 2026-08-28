use std::sync::Arc;

mod handle_join_request;

pub use handle_join_request::{
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    HandleAuthenticatedSpaceAdmissionMessageError, HandleAuthenticatedSpaceAdmissionMessagePort,
    LoadedSponsorJoinRequest, PrepareSponsorCandidateError, PrepareSponsorCandidatePort,
    PreparedSponsorCandidate, SpaceAdmissionMessageReply, SponsorAdmissionMutation,
    SponsorJoinRequestCommitToken, SponsorJoinRequestState, SponsorJoinRequestStateError,
    SponsorJoinRequestStatePort,
};

pub(crate) struct SponsorAdmissionService {
    pub(super) join_request_state: Arc<dyn SponsorJoinRequestStatePort>,
    pub(super) prepare_candidate: Arc<dyn PrepareSponsorCandidatePort>,
}

impl SponsorAdmissionService {
    pub(crate) fn new(
        join_request_state: Arc<dyn SponsorJoinRequestStatePort>,
        prepare_candidate: Arc<dyn PrepareSponsorCandidatePort>,
    ) -> Self {
        Self {
            join_request_state,
            prepare_candidate,
        }
    }
}
