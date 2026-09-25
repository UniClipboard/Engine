use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{ApplyMembershipSecurityPort, MembershipEffectExecutionError};
use uc_core::membership::{
    GroupRevocationPort, MemberEffectKind, MemberEffectMaterial, MembershipEventV2,
    MembershipOperationV2, UnfinishedMemberEffect,
};
use uc_core::ports::ClockPort;

pub struct DefaultMembershipSecurityUpdateAdapter {
    group_updates: Arc<dyn GroupRevocationPort>,
    clock: Arc<dyn ClockPort>,
}

impl DefaultMembershipSecurityUpdateAdapter {
    pub fn new(group_updates: Arc<dyn GroupRevocationPort>, clock: Arc<dyn ClockPort>) -> Self {
        Self {
            group_updates,
            clock,
        }
    }
}

#[async_trait]
impl ApplyMembershipSecurityPort for DefaultMembershipSecurityUpdateAdapter {
    async fn apply_membership_security(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        let (event, retained_device_ids) = match effect.material() {
            MemberEffectMaterial::Event(event) => (event, None),
            MemberEffectMaterial::InitiatedRemoval {
                event,
                retained_device_ids,
            } => (event, Some(retained_device_ids)),
            // 接受远端移除时安全变化随移除事件本身到达，本机决定不携带安全更新。
            MemberEffectMaterial::Decision(decision) => {
                return (effect.kind() == MemberEffectKind::RemoveDevice
                    && decision.removal_event_id == effect.event_id())
                .then_some(())
                .ok_or(MembershipEffectExecutionError::Corrupt);
            }
        };
        if event.event_id() != effect.event_id() || !operation_matches(effect.kind(), event) {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        if !event.security_update_payload.is_empty() {
            self.group_updates
                .apply_group_epoch_update(&event.security_update_payload)
                .await
                .map_err(|error| MembershipEffectExecutionError::Dependency {
                    source: anyhow::Error::new(error),
                })?;
        } else if let Some(retained_device_ids) = retained_device_ids {
            let target = effect
                .affected_device_ids()
                .first()
                .ok_or(MembershipEffectExecutionError::Corrupt)?;
            self.group_updates
                .revoke_group_member(target, retained_device_ids, self.clock.now_ms())
                .await
                .map_err(|error| MembershipEffectExecutionError::Dependency {
                    source: anyhow::Error::new(error),
                })?;
        }
        Ok(())
    }
}

fn operation_matches(kind: MemberEffectKind, event: &MembershipEventV2) -> bool {
    matches!(
        (kind, &event.operation),
        (
            MemberEffectKind::AddDevice,
            MembershipOperationV2::AddDevice { .. }
        ) | (
            MemberEffectKind::RemoveDevice,
            MembershipOperationV2::RemoveDevice { .. }
        )
    )
}
