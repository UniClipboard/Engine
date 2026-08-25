use std::sync::Arc;

use async_trait::async_trait;
use uc_core::membership::{
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort, GroupBootstrapPort,
    GroupBootstrapResult, MembershipInitializationError, SpaceMembershipInitializerPort,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort};

use crate::space::current_member_signing::CurrentMemberSignaturePort;

use super::MembershipLedger;

pub(crate) struct InitializeSpaceMembershipUseCase {
    ledger: Arc<MembershipLedger>,
    membership_identity: Arc<dyn CurrentMembershipIdentityPort>,
    announcement: Arc<dyn CurrentMembershipAnnouncementPort>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    group_bootstrap: Arc<dyn GroupBootstrapPort>,
    clock: Arc<dyn ClockPort>,
}

impl InitializeSpaceMembershipUseCase {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        membership_identity: Arc<dyn CurrentMembershipIdentityPort>,
        announcement: Arc<dyn CurrentMembershipAnnouncementPort>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        group_bootstrap: Arc<dyn GroupBootstrapPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            ledger,
            membership_identity,
            announcement,
            signatures,
            device_identity,
            group_bootstrap,
            clock,
        }
    }

    async fn execute(&self) -> Result<(), MembershipInitializationError> {
        let local_device_id = self.device_identity.current_device_id();
        let bootstrap = self
            .group_bootstrap
            .bootstrap_legacy_space(&local_device_id, &[], self.clock.now_ms())
            .await
            .map_err(|_| MembershipInitializationError::Unavailable)?;
        if !matches!(bootstrap, GroupBootstrapResult::Complete { .. }) {
            return Err(MembershipInitializationError::Inconsistent);
        }
        let identity = self
            .membership_identity
            .current_membership_identity()
            .await
            .map_err(|_| MembershipInitializationError::Unavailable)?;
        let material = self
            .announcement
            .current_announcement_material()
            .await
            .map_err(|_| MembershipInitializationError::Unavailable)?;
        if material.device_id != local_device_id {
            return Err(MembershipInitializationError::Inconsistent);
        }
        let member_instance = self
            .signatures
            .current_member_instance(&local_device_id)
            .await
            .map_err(|_| MembershipInitializationError::Unavailable)?;
        let credential = self
            .signatures
            .current_membership_credential(&local_device_id)
            .await
            .map_err(|_| MembershipInitializationError::Unavailable)?;
        let mut facts = uc_core::membership::AdmissionChangeFacts {
            member_instance,
            device_id: material.device_id,
            device_name: material.device_name,
            identity_fingerprint: material.identity_fingerprint,
            transport_public_key: material.transport_public_key,
            transport_address_blob: material.transport_address_blob,
            identity_signature: Vec::new(),
        };
        facts.identity_signature = self
            .signatures
            .sign_current_member_payload(&facts.signing_payload())
            .await
            .map_err(|_| MembershipInitializationError::Unavailable)?;
        self.ledger
            .initialize_current_space(identity.space_id.as_ref().to_owned(), facts, credential)
            .await
            .map_err(|_| MembershipInitializationError::Inconsistent)
    }
}

#[async_trait]
impl SpaceMembershipInitializerPort for InitializeSpaceMembershipUseCase {
    async fn initialize(&self) -> Result<(), MembershipInitializationError> {
        self.execute().await
    }
}
