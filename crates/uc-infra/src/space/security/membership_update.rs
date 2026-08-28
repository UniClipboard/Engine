use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::CurrentMemberSignaturePort;
use uc_core::membership::{
    GroupRevocationPort, MembershipSecurityState, MembershipSecurityUpdateError,
    MembershipSecurityUpdatePort,
};

use super::session::InMemorySession;

pub struct DefaultMembershipSecurityUpdateAdapter {
    session: Arc<InMemorySession>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    group_updates: Arc<dyn GroupRevocationPort>,
}

impl DefaultMembershipSecurityUpdateAdapter {
    pub fn new(
        session: Arc<InMemorySession>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        group_updates: Arc<dyn GroupRevocationPort>,
    ) -> Self {
        Self {
            session,
            signatures,
            group_updates,
        }
    }
}

#[async_trait]
impl MembershipSecurityUpdatePort for DefaultMembershipSecurityUpdateAdapter {
    async fn current_state(
        &self,
    ) -> Result<MembershipSecurityState, MembershipSecurityUpdateError> {
        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| MembershipSecurityUpdateError::Unavailable)?;
        let group_epoch = self
            .signatures
            .current_member_epoch()
            .await
            .map_err(|error| MembershipSecurityUpdateError::Repository(error.to_string()))?;
        Ok(MembershipSecurityState {
            space_id,
            group_epoch,
        })
    }

    async fn apply_group_epoch_update(
        &self,
        payload: &[u8],
    ) -> Result<u64, MembershipSecurityUpdateError> {
        self.group_updates
            .apply_group_epoch_update(payload)
            .await
            .map(|epoch| epoch.value())
            .map_err(|error| MembershipSecurityUpdateError::Repository(error.to_string()))
    }
}
