use async_trait::async_trait;

use super::super::{AuthenticatedSpaceAdmissionMessage, SpaceAdmissionMessageReply};
use super::HandleAuthenticatedSpaceAdmissionMessageError;

#[async_trait]
pub trait HandleAuthenticatedSpaceAdmissionMessagePort: Send + Sync {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError>;
}
