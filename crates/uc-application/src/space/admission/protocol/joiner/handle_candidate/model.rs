use uc_core::membership::{
    AdmissionSignedMembershipHistory, AdmissionStagedTarget, AdmissionStagedTargetInput,
    PendingAdmissionExchange,
};

pub struct PreparedJoinerCandidateMaterial {
    staged_target_input: AdmissionStagedTargetInput,
    verified_history: AdmissionSignedMembershipHistory,
    staged_target: AdmissionStagedTarget,
    prepared_exchange: PendingAdmissionExchange,
}

impl PreparedJoinerCandidateMaterial {
    pub fn new(
        staged_target_input: AdmissionStagedTargetInput,
        verified_history: AdmissionSignedMembershipHistory,
        staged_target: AdmissionStagedTarget,
        prepared_exchange: PendingAdmissionExchange,
    ) -> Self {
        Self {
            staged_target_input,
            verified_history,
            staged_target,
            prepared_exchange,
        }
    }

    pub fn into_parts(
        self,
    ) -> (
        AdmissionStagedTargetInput,
        AdmissionSignedMembershipHistory,
        AdmissionStagedTarget,
        PendingAdmissionExchange,
    ) {
        (
            self.staged_target_input,
            self.verified_history,
            self.staged_target,
            self.prepared_exchange,
        )
    }
}
