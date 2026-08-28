use async_trait::async_trait;
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
        match message.envelope().kind() {
            SpaceAdmissionMessageKind::JoinRequest => self.handle_join_request(message).await,
            SpaceAdmissionMessageKind::Prepared => self.handle_prepared(message).await,
            _ => Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("the Sponsor cannot handle this admission message kind"),
            )),
        }
    }
}
