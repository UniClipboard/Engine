use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uc_application::deps::{
    ApplyMembershipMemberFactsPort, MembershipEffectExecutionError, MembershipEffectKind,
    PendingMembershipEffect,
};
use uc_core::membership::{MemberRepositoryPort, MembershipOperationV2};
use uc_core::ports::{ClockPort, DeviceIdentityPort, PeerAddressRecord, PeerAddressRepositoryPort};
use uc_core::trusted_peer::{TrustedPeer, TrustedPeerRepositoryPort};
use uc_core::{MemberSyncPreferences, SpaceMember};

/// 将签名成员事件投影为本机 roster、可信身份和传输地址。
///
/// 投影可重复执行，但不能授予成员资格；成员资格始终由 Application 的账本决定。
pub struct MembershipMemberFactsAdapter {
    members: Arc<dyn MemberRepositoryPort>,
    trusted_peers: Arc<dyn TrustedPeerRepositoryPort>,
    peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    clock: Arc<dyn ClockPort>,
}

impl MembershipMemberFactsAdapter {
    pub fn new(
        members: Arc<dyn MemberRepositoryPort>,
        trusted_peers: Arc<dyn TrustedPeerRepositoryPort>,
        peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            members,
            trusted_peers,
            peer_addresses,
            device_identity,
            clock,
        }
    }

    async fn apply_add(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        let event: uc_core::membership::MembershipEventV2 =
            postcard::from_bytes(&effect.payload)
                .map_err(|_| MembershipEffectExecutionError::Corrupt)?;
        if event.event_id().as_bytes() != &effect.event_id {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        let MembershipOperationV2::AddDevice { admission } = event.operation else {
            return Err(MembershipEffectExecutionError::Corrupt);
        };
        let facts = admission.facts;
        if effect.affected_device_ids.as_slice() != std::slice::from_ref(&facts.device_id) {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        let joined_at = DateTime::<Utc>::from_timestamp_millis(self.clock.now_ms())
            .ok_or(MembershipEffectExecutionError::Corrupt)?;
        let sync_preferences = self
            .members
            .get(&facts.device_id)
            .await
            .map_err(dependency)?
            .map_or_else(MemberSyncPreferences::default, |member| {
                member.sync_preferences
            });
        self.members
            .save(&SpaceMember {
                device_id: facts.device_id.clone(),
                device_name: facts.device_name,
                identity_fingerprint: facts.identity_fingerprint.clone(),
                joined_at,
                sync_preferences,
            })
            .await
            .map_err(dependency)?;
        self.trusted_peers
            .save(&TrustedPeer {
                local_device_id: self.device_identity.current_device_id(),
                peer_device_id: facts.device_id.clone(),
                peer_fingerprint: facts.identity_fingerprint,
                trusted_at: joined_at,
            })
            .await
            .map_err(dependency)?;
        if !facts.transport_address_blob.is_empty() {
            self.peer_addresses
                .upsert(&PeerAddressRecord {
                    device_id: facts.device_id,
                    addr_blob: facts.transport_address_blob,
                    observed_at: joined_at,
                })
                .await
                .map_err(dependency)?;
        }
        Ok(())
    }

    async fn apply_remove(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        if effect.affected_device_ids.is_empty() {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        for device_id in &effect.affected_device_ids {
            self.trusted_peers
                .remove(device_id)
                .await
                .map_err(dependency)?;
        }
        Ok(())
    }
}

#[async_trait]
impl ApplyMembershipMemberFactsPort for MembershipMemberFactsAdapter {
    async fn apply_member_facts(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        match effect.kind {
            MembershipEffectKind::AddDevice => self.apply_add(effect).await,
            MembershipEffectKind::RemoveDevice => self.apply_remove(effect).await,
        }
    }
}

fn dependency(
    error: impl std::error::Error + Send + Sync + 'static,
) -> MembershipEffectExecutionError {
    MembershipEffectExecutionError::Dependency {
        source: anyhow::Error::new(error),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use uc_core::ids::DeviceId;
    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::ports::PeerAddressError;
    use uc_core::trusted_peer::TrustedPeerError;

    use super::*;

    struct PassiveMembers;

    #[async_trait]
    impl MemberRepositoryPort for PassiveMembers {
        async fn get(&self, _device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(None)
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(Vec::new())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    struct FailingTrustedPeers;

    #[async_trait]
    impl TrustedPeerRepositoryPort for FailingTrustedPeers {
        async fn get(
            &self,
            _peer_device_id: &DeviceId,
        ) -> Result<Option<TrustedPeer>, TrustedPeerError> {
            Ok(None)
        }

        async fn list(&self) -> Result<Vec<TrustedPeer>, TrustedPeerError> {
            Ok(Vec::new())
        }

        async fn save(&self, _trusted_peer: &TrustedPeer) -> Result<(), TrustedPeerError> {
            Ok(())
        }

        async fn remove(&self, _peer_device_id: &DeviceId) -> Result<bool, TrustedPeerError> {
            Err(TrustedPeerError::Repository("test failure".to_owned()))
        }
    }

    struct PassivePeerAddresses;

    #[async_trait]
    impl PeerAddressRepositoryPort for PassivePeerAddresses {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(None)
        }

        async fn upsert(&self, _record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            Ok(())
        }

        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(Vec::new())
        }

        async fn remove(&self, _device: &DeviceId) -> Result<(), PeerAddressError> {
            Ok(())
        }
    }

    struct FixedDeviceIdentity;

    impl DeviceIdentityPort for FixedDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            DeviceId::new("local")
        }
    }

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            0
        }
    }

    #[tokio::test]
    async fn repository_failure_keeps_classification_and_source() {
        let adapter = MembershipMemberFactsAdapter::new(
            Arc::new(PassiveMembers),
            Arc::new(FailingTrustedPeers),
            Arc::new(PassivePeerAddresses),
            Arc::new(FixedDeviceIdentity),
            Arc::new(FixedClock),
        );
        let error = adapter
            .apply_member_facts(&PendingMembershipEffect {
                event_id: [1; 32],
                kind: MembershipEffectKind::RemoveDevice,
                phase: uc_application::deps::MembershipEffectPhase::Prepared,
                affected_device_ids: vec![DeviceId::new("remote")],
                payload: Vec::new(),
            })
            .await
            .expect_err("repository failure must surface");

        assert!(matches!(
            error,
            MembershipEffectExecutionError::Dependency { .. }
        ));
        assert!(error.source().is_some());
    }
}
