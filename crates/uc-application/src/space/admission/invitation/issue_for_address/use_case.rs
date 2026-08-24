use std::net::IpAddr;
use std::sync::Arc;

use tracing::instrument;
use uc_core::ports::pairing_invitation::{IssuedInvitation, PairingInvitationByAddressPort};

use crate::facade::space_setup::{IssuePairingInvitationError, IssuePairingInvitationResult};
use crate::space::admission::invitation::issuer::map_invitation_error;
use crate::space::admission::invitation::PairingInvitationIssuer;

/// 开发工具专用：使用调用方明确选择的本机地址签发邀请。
pub(crate) struct IssuePairingInvitationForAddressUseCase {
    invitation: Arc<dyn PairingInvitationByAddressPort>,
    issuer: Arc<PairingInvitationIssuer>,
}

impl IssuePairingInvitationForAddressUseCase {
    pub(crate) fn new(
        invitation: Arc<dyn PairingInvitationByAddressPort>,
        issuer: Arc<PairingInvitationIssuer>,
    ) -> Self {
        Self { invitation, issuer }
    }

    #[instrument(skip_all, fields(selected_ip = %selected_ip))]
    pub(crate) async fn execute(
        &self,
        selected_ip: IpAddr,
    ) -> Result<IssuePairingInvitationResult, IssuePairingInvitationError> {
        let admission_generation = self.issuer.begin().await?;
        let issued: IssuedInvitation = self
            .invitation
            .issue_invitation_for_address(selected_ip)
            .await
            .map_err(map_invitation_error)?;
        self.issuer.finish(issued, admission_generation).await
    }
}
