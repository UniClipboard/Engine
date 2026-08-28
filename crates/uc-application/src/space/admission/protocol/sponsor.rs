use std::sync::Arc;

use super::{PrepareSponsorCandidatePort, SponsorJoinRequestStatePort};

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
