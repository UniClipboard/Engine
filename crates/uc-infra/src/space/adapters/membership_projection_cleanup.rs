use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    CleanupLegacyMembershipDataPort, LoadMembershipLedgerPort, MembershipEffectKind,
    MembershipEffectPhase, MembershipMaintenanceStepOutcome,
};
use uc_core::membership::MemberRepositoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

/// 在受限交付完成后清理已移除成员仅供认证和寻址使用的临时投影。
pub struct MembershipProjectionCleanupAdapter {
    ledger: Arc<dyn LoadMembershipLedgerPort>,
    members: Arc<dyn MemberRepositoryPort>,
    peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
}

impl MembershipProjectionCleanupAdapter {
    pub fn new(
        ledger: Arc<dyn LoadMembershipLedgerPort>,
        members: Arc<dyn MemberRepositoryPort>,
        peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            ledger,
            members,
            peer_addresses,
        }
    }
}

#[async_trait]
impl CleanupLegacyMembershipDataPort for MembershipProjectionCleanupAdapter {
    async fn cleanup_legacy_membership_data(&self) -> MembershipMaintenanceStepOutcome {
        let Ok(loaded) = self.ledger.load().await else {
            return MembershipMaintenanceStepOutcome::Deferred;
        };
        let restricted_peers = loaded
            .peer_reconciliation
            .iter()
            .filter(|(_, record)| !record.restricted_delivery.is_empty())
            .map(|(device_id, _)| device_id.clone())
            .collect::<BTreeSet<_>>();
        let removable = loaded
            .pending_effects
            .values()
            .filter(|effect| {
                effect.kind == MembershipEffectKind::RemoveDevice
                    && effect.phase == MembershipEffectPhase::Activated
            })
            .flat_map(|effect| effect.affected_device_ids.iter())
            .filter(|device_id| !restricted_peers.contains(*device_id))
            .cloned()
            .collect::<BTreeSet<_>>();
        for device_id in removable {
            if self.members.remove(&device_id).await.is_err()
                || self.peer_addresses.remove(&device_id).await.is_err()
            {
                return MembershipMaintenanceStepOutcome::Deferred;
            }
        }
        MembershipMaintenanceStepOutcome::Completed
    }
}
