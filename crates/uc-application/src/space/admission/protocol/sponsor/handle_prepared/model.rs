use uc_core::membership::{
    AdmissionSealedSecurityState, AdmissionSignedMembershipHistory, SpaceAdmissionEnvelopeV1,
};

pub struct PreparedSponsorCommit {
    committed_history: AdmissionSignedMembershipHistory,
    sealed_security: AdmissionSealedSecurityState,
    commit_reply: SpaceAdmissionEnvelopeV1,
}

impl PreparedSponsorCommit {
    pub fn new(
        committed_history: AdmissionSignedMembershipHistory,
        sealed_security: AdmissionSealedSecurityState,
        commit_reply: SpaceAdmissionEnvelopeV1,
    ) -> Self {
        Self {
            committed_history,
            sealed_security,
            commit_reply,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        AdmissionSignedMembershipHistory,
        AdmissionSealedSecurityState,
        SpaceAdmissionEnvelopeV1,
    ) {
        (
            self.committed_history,
            self.sealed_security,
            self.commit_reply,
        )
    }
}
