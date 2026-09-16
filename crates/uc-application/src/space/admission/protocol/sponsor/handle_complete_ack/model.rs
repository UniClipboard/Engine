use uc_core::membership::SpaceAdmissionEnvelopeV1;

pub struct PreparedSponsorSettled {
    settled_reply: SpaceAdmissionEnvelopeV1,
}

impl PreparedSponsorSettled {
    pub fn new(settled_reply: SpaceAdmissionEnvelopeV1) -> Self {
        Self { settled_reply }
    }

    pub(crate) fn into_reply(self) -> SpaceAdmissionEnvelopeV1 {
        self.settled_reply
    }
}
