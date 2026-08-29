use async_trait::async_trait;
use rand::RngCore;
use uc_application::deps::{
    PrepareSponsorSettledError, PrepareSponsorSettledPort, PreparedSponsorSettled,
};
use uc_core::membership::{
    AdmissionRole, AdmissionSettledV1, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId,
};

use crate::space::admission::digest::{complete_ack_digest, completion_digest};

pub struct DefaultSponsorSettledPreparation;

#[async_trait]
impl PrepareSponsorSettledPort for DefaultSponsorSettledPreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: uc_core::membership::SponsorSettlementPreparation<'_>,
        complete_ack: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorSettled, PrepareSponsorSettledError> {
        let completion = match preparation.complete_reply().body() {
            SpaceAdmissionBodyV1::Complete(complete) => complete.completion(),
            _ => return Err(invalid("the saved Sponsor reply is not Complete")),
        };
        let acknowledgment = match complete_ack.body() {
            SpaceAdmissionBodyV1::CompleteAck(acknowledgment) => acknowledgment,
            _ => return Err(invalid("the Sponsor settlement input is not CompleteAck")),
        };
        if complete_ack.header().admission_id() != admission_id
            || complete_ack.header().predecessor_message_id()
                != Some(preparation.complete_reply().header().message_id())
            || acknowledgment.completion_digest() != &completion_digest(completion)
        {
            return Err(invalid(
                "the CompleteAck is not bound to the exact Complete",
            ));
        }
        let settled = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Sponsor,
            3,
            mint_message_id(),
            Some(complete_ack.header().message_id()),
            SpaceAdmissionBodyV1::Settled(
                AdmissionSettledV1::new(complete_ack_digest(acknowledgment))
                    .ok_or_else(|| invalid("the CompleteAck digest is invalid"))?,
            ),
        )
        .map_err(|error| PrepareSponsorSettledError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedSponsorSettled::new(settled))
    }
}

fn invalid(message: &'static str) -> PrepareSponsorSettledError {
    PrepareSponsorSettledError::invalid(anyhow::anyhow!(message))
}

fn mint_message_id() -> uc_core::membership::AdmissionMessageId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = uc_core::membership::AdmissionMessageId::from_bytes(bytes) {
            return id;
        }
    }
}
