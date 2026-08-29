use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    ApplyMembershipSecurityPort, CurrentMemberSignaturePort, MembershipEffectExecutionError,
    MembershipEffectKind, PendingMembershipEffect,
};
use uc_core::membership::{
    GroupRevocationPort, MembershipOperationV2, MembershipSecurityState,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
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

#[async_trait]
impl ApplyMembershipSecurityPort for DefaultMembershipSecurityUpdateAdapter {
    async fn apply_membership_security(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        if let Ok(event) =
            postcard::from_bytes::<uc_core::membership::MembershipEventV2>(&effect.payload)
        {
            if event.event_id().as_bytes() != &effect.event_id
                || !operation_matches(effect.kind, &event.operation)
            {
                return Err(MembershipEffectExecutionError::Corrupt);
            }
            if !event.security_update_payload.is_empty() {
                self.group_updates
                    .apply_group_epoch_update(&event.security_update_payload)
                    .await
                    .map_err(|error| MembershipEffectExecutionError::Dependency {
                        source: anyhow::Error::new(error),
                    })?;
            }
            return Ok(());
        }

        if effect.kind == MembershipEffectKind::RemoveDevice {
            let decision =
                postcard::from_bytes::<uc_core::membership::MembershipDecisionV2>(&effect.payload)
                    .map_err(|_| MembershipEffectExecutionError::Corrupt)?;
            if decision.removal_event_id.as_bytes() == &effect.event_id {
                return Ok(());
            }
        }
        Err(MembershipEffectExecutionError::Corrupt)
    }
}

fn operation_matches(kind: MembershipEffectKind, operation: &MembershipOperationV2) -> bool {
    matches!(
        (kind, operation),
        (
            MembershipEffectKind::AddDevice,
            MembershipOperationV2::AddDevice { .. }
        ) | (
            MembershipEffectKind::RemoveDevice,
            MembershipOperationV2::RemoveDevice { .. }
        )
    )
}
