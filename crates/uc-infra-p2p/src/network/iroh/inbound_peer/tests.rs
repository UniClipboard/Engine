//! 统一入站身份门的判定表（切片 S2 暂存验收；落地路径即本文件在仓库中的相对路径）。
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MemberSyncPreferences, MembershipError, SpaceMember,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;

use super::{InboundPeerRejection, PeerIdentityResolver};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

const KEY: [u8; 32] = [0x11; 32];
const OTHER_KEY: [u8; 32] = [0x22; 32];

struct Members(Result<Vec<SpaceMember>, ()>);

crate::network::iroh::inbound_peer::member_table_identity_directory!(Members);
#[async_trait]
impl MemberRepositoryPort for Members {
    async fn get(&self, _: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        panic!("the gate resolves identities only through list()")
    }
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        self.0
            .clone()
            .map_err(|()| MembershipError::Repository("fixture".into()))
    }
    async fn save(&self, _: &SpaceMember) -> Result<(), MembershipError> {
        panic!("read-only fixture")
    }
    async fn remove(&self, _: &DeviceId) -> Result<bool, MembershipError> {
        panic!("read-only fixture")
    }
}

struct BrokenFingerprints;

impl IdentityFingerprintFactoryPort for BrokenFingerprints {
    fn from_public_key(&self, _: &[u8]) -> anyhow::Result<IdentityFingerprint> {
        Err(anyhow::anyhow!("fixture"))
    }
}

fn member(device: &str, key: &[u8; 32]) -> SpaceMember {
    SpaceMember {
        device_id: DeviceId::new(device),
        device_name: device.to_owned(),
        identity_fingerprint: Sha256IdentityFingerprintFactory
            .from_public_key(key)
            .unwrap(),
        joined_at: chrono::Utc::now(),
        sync_preferences: MemberSyncPreferences::default(),
    }
}

fn resolver(members: Result<Vec<SpaceMember>, ()>) -> PeerIdentityResolver {
    PeerIdentityResolver::new(
        Arc::new(Members(members)),
        Arc::new(Sha256IdentityFingerprintFactory),
    )
}

async fn identify(resolver: &PeerIdentityResolver) -> Result<DeviceId, InboundPeerRejection> {
    resolver
        .resolve(&KEY)
        .await
        .map_err(|error| error.rejection())
}

#[tokio::test]
async fn a_unique_fingerprint_identifies_its_device() {
    let found = identify(&resolver(Ok(vec![
        member("a", &KEY),
        member("b", &OTHER_KEY),
    ])))
    .await;
    assert_eq!(found, Ok(DeviceId::new("a")));
}

#[tokio::test]
async fn an_unknown_fingerprint_is_unresolved() {
    let found = identify(&resolver(Ok(vec![member("b", &OTHER_KEY)]))).await;
    assert_eq!(found, Err(InboundPeerRejection::IdentityUnresolved));
}

#[tokio::test]
async fn a_fingerprint_shared_by_two_devices_is_ambiguous_in_any_order() {
    for members in [
        vec![member("stale", &KEY), member("active", &KEY)],
        vec![member("active", &KEY), member("stale", &KEY)],
    ] {
        assert_eq!(
            identify(&resolver(Ok(members))).await,
            Err(InboundPeerRejection::IdentityAmbiguous)
        );
    }
}

#[tokio::test]
async fn a_member_read_failure_is_not_reported_as_unknown() {
    assert_eq!(
        identify(&resolver(Err(()))).await,
        Err(InboundPeerRejection::MemberReadFailed)
    );
}

#[tokio::test]
async fn a_fingerprint_derivation_failure_is_its_own_reason() {
    let resolver = PeerIdentityResolver::new(
        Arc::new(Members(Ok(vec![member("a", &KEY)]))),
        Arc::new(BrokenFingerprints),
    );
    assert_eq!(
        identify(&resolver).await,
        Err(InboundPeerRejection::FingerprintUnavailable)
    );
}

#[tokio::test]
async fn resolution_failures_keep_their_typed_source() {
    let read = resolver(Err(())).resolve(&KEY).await.unwrap_err();
    assert_eq!(read.rejection(), InboundPeerRejection::MemberReadFailed);
    // 身份目录（049 S5）把读取失败包成 `MembershipLedgerError`，原始失败保留在其来源链中。
    let mut source = std::error::Error::source(&read);
    let mut original = None;
    while let Some(error) = source {
        original = original.or(error.downcast_ref::<MembershipError>());
        source = error.source();
    }
    assert!(
        matches!(original, Some(MembershipError::Repository(_))),
        "member read failure keeps its source"
    );

    let derivation = PeerIdentityResolver::new(
        Arc::new(Members(Ok(vec![member("a", &KEY)]))),
        Arc::new(BrokenFingerprints),
    )
    .resolve(&KEY)
    .await
    .unwrap_err();
    assert_eq!(
        derivation.rejection(),
        InboundPeerRejection::FingerprintUnavailable
    );
    assert_eq!(
        std::error::Error::source(&derivation).map(ToString::to_string),
        Some("fixture".to_owned())
    );
}
