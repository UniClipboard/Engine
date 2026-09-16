use uc_core::membership::{AdmissionActivatedSecurityState, SpaceAdmissionEnvelopeV1};

pub struct PreparedSponsorComplete {
    activated_security: AdmissionActivatedSecurityState,
    complete_reply: SpaceAdmissionEnvelopeV1,
}

impl PreparedSponsorComplete {
    pub fn new(
        activated_security: AdmissionActivatedSecurityState,
        complete_reply: SpaceAdmissionEnvelopeV1,
    ) -> Self {
        Self {
            activated_security,
            complete_reply,
        }
    }

    pub(crate) fn into_parts(self) -> (AdmissionActivatedSecurityState, SpaceAdmissionEnvelopeV1) {
        (self.activated_security, self.complete_reply)
    }
}
