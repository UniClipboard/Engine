use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uc_application::deps::{ApplyMembershipMemberFactsPort, MembershipEffectExecutionError};
use uc_core::membership::{
    MemberEffectKind, MemberEffectMaterial, MemberRepositoryPort, MembershipOperationV2,
    UnfinishedMemberEffect,
};
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
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        let MemberEffectMaterial::Event(event) = effect.material() else {
            return Err(MembershipEffectExecutionError::Corrupt);
        };
        if event.event_id() != effect.event_id() {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(MembershipEffectExecutionError::Corrupt);
        };
        let facts = admission.facts.clone();
        if effect.affected_device_ids() != std::slice::from_ref(&facts.device_id) {
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
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        if effect.affected_device_ids().is_empty() {
            return Err(MembershipEffectExecutionError::Corrupt);
        }
        for device_id in effect.affected_device_ids() {
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
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        match effect.kind() {
            MemberEffectKind::AddDevice => self.apply_add(effect).await,
            MemberEffectKind::RemoveDevice => self.apply_remove(effect).await,
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
pub(crate) mod tests {
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
            Err(TrustedPeerError::Repository("test failure".into()))
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

    /// 以一项决定为材料的移除效果；移除资料维护只依赖受影响设备。
    pub(crate) fn removal_effect(target: DeviceId) -> UnfinishedMemberEffect {
        use uc_core::membership::{
            MemberEffectPhase, MemberInstanceId, MembershipCredential, MembershipDecisionV2,
            MembershipEventId, RemovalDecision, ED25519_SIGNATURE_ALGORITHM_V1,
        };
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![7; 32]);
        let removal_event_id: MembershipEventId = postcard::from_bytes(&[1; 32]).unwrap();
        UnfinishedMemberEffect::from_parts(
            removal_event_id,
            MemberEffectKind::RemoveDevice,
            MemberEffectPhase::Prepared,
            vec![target],
            MemberEffectMaterial::Decision(MembershipDecisionV2::new(
                2,
                "lineage".to_owned(),
                removal_event_id,
                MemberInstanceId::from_bytes([2; 32]),
                credential.credential_id,
                credential.signature_algorithm_version,
                RemovalDecision::Accept,
                None,
                [0; 32],
                [0; 16],
                Vec::new(),
            )),
        )
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
            .apply_member_facts(&removal_effect(DeviceId::new("remote")))
            .await
            .expect_err("repository failure must surface");

        assert!(matches!(
            error,
            MembershipEffectExecutionError::Dependency { .. }
        ));
        assert!(error.source().is_some());
    }
}
