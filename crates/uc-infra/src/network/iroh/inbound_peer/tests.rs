//! 统一入站身份门的判定表（切片 S2 暂存验收；落地路径即本文件在仓库中的相对路径）。
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MemberSyncPreferences, MembershipError, PeerAdmissionError,
    PeerAdmissionPort, SpaceMember,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;
use uc_observability_contract::diagnostics::connectivity::InboundPeerProtocol;

use super::{InboundPeerGate, InboundPeerRejection, PeerIdentityResolver};
use crate::security::Sha256IdentityFingerprintFactory;

const KEY: [u8; 32] = [0x11; 32];
const OTHER_KEY: [u8; 32] = [0x22; 32];

struct Members(Result<Vec<SpaceMember>, ()>);

#[async_trait]
impl MemberRepositoryPort for Members {
    async fn get(&self, _: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        panic!("the gate resolves identities only through list()")
    }
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        self.0
            .clone()
            .map_err(|()| MembershipError::Repository("fixture".to_owned()))
    }
    async fn save(&self, _: &SpaceMember) -> Result<(), MembershipError> {
        panic!("read-only fixture")
    }
    async fn remove(&self, _: &DeviceId) -> Result<bool, MembershipError> {
        panic!("read-only fixture")
    }
}

struct Admission {
    answer: Result<bool, ()>,
    checked: Mutex<Vec<DeviceId>>,
}

impl Admission {
    fn new(answer: Result<bool, ()>) -> Arc<Self> {
        Arc::new(Self {
            answer,
            checked: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl PeerAdmissionPort for Admission {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        self.checked.lock().unwrap().push(device_id.clone());
        self.answer.map_err(|()| PeerAdmissionError::Unavailable)
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

fn gate(members: Result<Vec<SpaceMember>, ()>, admission: Arc<Admission>) -> InboundPeerGate {
    InboundPeerGate::new(
        InboundPeerProtocol::Presence,
        Arc::new(Members(members)),
        admission,
        Arc::new(Sha256IdentityFingerprintFactory),
    )
}

#[tokio::test]
async fn a_unique_fingerprint_identifies_its_device() {
    let found = resolver(Ok(vec![member("a", &KEY), member("b", &OTHER_KEY)]))
        .identify(&KEY)
        .await;
    assert_eq!(found, Ok(DeviceId::new("a")));
}

#[tokio::test]
async fn an_unknown_fingerprint_is_unresolved() {
    let found = resolver(Ok(vec![member("b", &OTHER_KEY)]))
        .identify(&KEY)
        .await;
    assert_eq!(found, Err(InboundPeerRejection::IdentityUnresolved));
}

#[tokio::test]
async fn a_fingerprint_shared_by_two_devices_is_ambiguous_in_any_order() {
    for members in [
        vec![member("stale", &KEY), member("active", &KEY)],
        vec![member("active", &KEY), member("stale", &KEY)],
    ] {
        assert_eq!(
            resolver(Ok(members)).identify(&KEY).await,
            Err(InboundPeerRejection::IdentityAmbiguous)
        );
    }
}

#[tokio::test]
async fn a_member_read_failure_is_not_reported_as_unknown() {
    assert_eq!(
        resolver(Err(())).identify(&KEY).await,
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
        resolver.identify(&KEY).await,
        Err(InboundPeerRejection::FingerprintUnavailable)
    );
}

#[tokio::test]
async fn resolution_failures_keep_their_typed_source() {
    let read = resolver(Err(())).resolve(&KEY).await.unwrap_err();
    assert_eq!(read.rejection(), InboundPeerRejection::MemberReadFailed);
    let source = std::error::Error::source(&read).expect("member read failure keeps its source");
    assert!(matches!(
        source.downcast_ref::<MembershipError>(),
        Some(MembershipError::Repository(_))
    ));

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

#[tokio::test]
async fn admission_distinguishes_denial_from_unavailability() {
    let denied = Admission::new(Ok(false));
    assert_eq!(
        gate(Ok(vec![member("a", &KEY)]), Arc::clone(&denied))
            .admit(&KEY)
            .await,
        Err(InboundPeerRejection::LedgerDenied)
    );
    let unavailable = Admission::new(Err(()));
    assert_eq!(
        gate(Ok(vec![member("a", &KEY)]), Arc::clone(&unavailable))
            .admit(&KEY)
            .await,
        Err(InboundPeerRejection::LedgerUnavailable)
    );
}

#[tokio::test]
async fn admission_asks_the_ledger_once_and_only_after_identity_succeeds() {
    let admitted = Admission::new(Ok(true));
    assert_eq!(
        gate(Ok(vec![member("a", &KEY)]), Arc::clone(&admitted))
            .admit(&KEY)
            .await,
        Ok(DeviceId::new("a"))
    );
    assert_eq!(*admitted.checked.lock().unwrap(), vec![DeviceId::new("a")]);

    let untouched = Admission::new(Ok(true));
    for members in [
        Ok(vec![member("b", &OTHER_KEY)]),
        Ok(vec![member("x", &KEY), member("y", &KEY)]),
        Err(()),
    ] {
        assert!(gate(members, Arc::clone(&untouched))
            .admit(&KEY)
            .await
            .is_err());
    }
    assert!(untouched.checked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn authorize_uses_the_ledger_for_an_already_identified_device() {
    let admitted = Admission::new(Ok(true));
    let gate = gate(Ok(Vec::new()), Arc::clone(&admitted));
    assert_eq!(gate.authorize(&DeviceId::new("a")).await, Ok(()));
    assert_eq!(*admitted.checked.lock().unwrap(), vec![DeviceId::new("a")]);
}

#[test]
fn every_rejection_has_a_fixed_phase() {
    use InboundPeerRejection::*;
    for (rejection, identity_phase) in [
        (IdentityUnresolved, true),
        (IdentityAmbiguous, true),
        (MemberReadFailed, true),
        (FingerprintUnavailable, true),
        (LedgerDenied, false),
        (LedgerUnavailable, false),
        (NotAccepting, false),
    ] {
        assert_eq!(
            rejection.is_identity_failure(),
            identity_phase,
            "{rejection:?}"
        );
    }
}
