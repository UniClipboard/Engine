use async_trait::async_trait;
use std::time::Instant;
use uc_core::membership::SpaceAdmissionMessageKind;

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    HandleAuthenticatedSpaceAdmissionMessagePort, SpaceAdmissionMessageReply,
};
use crate::space::admission::protocol::{SpaceAdmissionProtocol, SponsorAdmissionService};

#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for SpaceAdmissionProtocol {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        self.execute_exclusively(self.sponsor.handle_authenticated_message(message))
            .await
    }
}

impl SponsorAdmissionService {
    async fn handle_authenticated_message(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let started = Instant::now();
        let kind = message.envelope().kind();
        let result = match kind {
            SpaceAdmissionMessageKind::JoinRequest => self.handle_join_request(message).await,
            SpaceAdmissionMessageKind::Prepared => self.handle_prepared(message).await,
            SpaceAdmissionMessageKind::Applied => self.handle_applied(message).await,
            SpaceAdmissionMessageKind::CompleteAck => self.handle_complete_ack(message).await,
            _ => Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("the Sponsor cannot handle this admission message kind"),
            )),
        };
        tracing::info!(
            target: "admission.performance",
            phase = "sponsor_handle_message",
            message_kind = ?kind,
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = if result.is_ok() { "ok" } else { "error" },
            "pairing phase completed"
        );
        result
    }
}
