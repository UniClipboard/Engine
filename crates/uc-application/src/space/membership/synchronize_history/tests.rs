use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2, MembershipCredential,
    MembershipEventId, MembershipHistoryExchangeError, MembershipHistoryExchangePort,
    MembershipHistoryMessage, MembershipHistoryRelationship, MembershipHistoryV2Ack,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, CurrentSpaceMemberScope, CurrentSpaceMemberScopeError,
    CurrentSpaceMemberScopePort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedger, MembershipLedgerError, MembershipLedgerMutation, PausedSpaceMember,
    PeerReconciliationRecord, SpaceMemberPauseReason,
};

struct MemoryLedgerRepository(Mutex<LoadedMembershipLedger>);

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedgerRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedgerRepository {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut loaded = self.0.lock().unwrap();
        let digest = loaded
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if loaded.revision != mutation.expected_revision
            || digest != mutation.expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        *loaded = mutation.replacement;
        Ok(loaded.clone())
    }
}

struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

struct UnsortedScope;

struct PausedUnknownScope;

struct EmptyScope;

#[async_trait]
impl CurrentSpaceMemberScopePort for UnsortedScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 5,
            local_member_active: true,
            usable_peer_device_ids: vec![
                DeviceId::new("device-c"),
                DeviceId::new("device-b"),
                DeviceId::new("device-c"),
            ],
            paused_peer_devices: Vec::new(),
        })
    }
}

#[async_trait]
impl CurrentSpaceMemberScopePort for PausedUnknownScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 5,
            local_member_active: true,
            usable_peer_device_ids: Vec::new(),
            paused_peer_devices: vec![PausedSpaceMember {
                device_id: DeviceId::new("device-b"),
                reason: SpaceMemberPauseReason::RelationshipUnconfirmed,
            }],
        })
    }
}

#[async_trait]
impl CurrentSpaceMemberScopePort for EmptyScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 5,
            local_member_active: true,
            usable_peer_device_ids: Vec::new(),
            paused_peer_devices: Vec::new(),
        })
    }
}

struct RecordingTransport {
    recipients: Mutex<Vec<DeviceId>>,
}

#[async_trait]
impl MembershipHistoryExchangePort for RecordingTransport {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        _message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.recipients.lock().unwrap().push(recipient.clone());
        if recipient == &DeviceId::new("device-c") {
            return Err(MembershipHistoryExchangeError::Offline);
        }
        Ok(MembershipHistoryMessage::AckV2(
            MembershipHistoryV2Ack::Consistent,
        ))
    }
}

fn member_facts(device: &str, credential_byte: u8) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![credential_byte; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    (
        AdmissionChangeFacts {
            member_instance,
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        credential,
    )
}

fn active_ledger() -> LoadedMembershipLedger {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer_b, credential_b) = member_facts("device-b", 0x42);
    let (peer_c, credential_c) = member_facts("device-c", 0x43);
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local.clone(), local_credential),
                (peer_b.clone(), credential_b),
                (peer_c.clone(), credential_c),
            ],
        },
    )
    .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 5;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = true;
    for peer in [peer_b.device_id, peer_c.device_id] {
        loaded.peer_reconciliation.insert(
            peer.clone(),
            PeerReconciliationRecord {
                peer_device_id: peer,
                relationship: MembershipHistoryRelationship::Consistent,
                confirmed_position: None,
                restricted_delivery: Vec::new(),
                updated_at_ms: 1,
            },
        );
    }
    loaded
}

#[tokio::test]
async fn all_current_peers_are_sorted_deduplicated_and_independently_deferred() {
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(active_ledger())));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let transport = Arc::new(RecordingTransport {
        recipients: Mutex::new(Vec::new()),
    });
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(UnsortedScope),
        transport.clone(),
    );

    let report = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();

    assert_eq!(report.completed_peer_count, 1);
    assert_eq!(report.deferred_peer_count, 1);
    assert_eq!(report.stable_failure_count, 0);
    assert_eq!(
        transport.recipients.lock().unwrap().as_slice(),
        &[DeviceId::new("device-b"), DeviceId::new("device-c")]
    );
}

#[tokio::test]
async fn unconfirmed_current_member_is_included_in_membership_history_sync() {
    let mut loaded = active_ledger();
    loaded
        .peer_reconciliation
        .remove(&DeviceId::new("device-b"));
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let transport = Arc::new(RecordingTransport {
        recipients: Mutex::new(Vec::new()),
    });
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(PausedUnknownScope),
        transport.clone(),
    );

    let report = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();

    assert_eq!(report.completed_peer_count, 1);
    assert_eq!(
        transport.recipients.lock().unwrap().as_slice(),
        &[DeviceId::new("device-b")]
    );
    assert_eq!(
        repository
            .load()
            .await
            .unwrap()
            .peer_reconciliation
            .get(&DeviceId::new("device-b"))
            .unwrap()
            .relationship,
        MembershipHistoryRelationship::Consistent
    );
}

#[tokio::test]
async fn authenticated_non_member_cannot_receive_full_membership_history() {
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(active_ledger())));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let transport = Arc::new(RecordingTransport {
        recipients: Mutex::new(Vec::new()),
    });
    let synchronize =
        SynchronizeMembershipHistoryUseCase::new(ledger, Arc::new(EmptyScope), transport.clone());

    let error = synchronize
        .execute(MembershipSyncTarget::AuthenticatedPeer(DeviceId::new(
            "removed-device",
        )))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SynchronizeMembershipHistoryError::CurrentScopeUnavailable
    ));
    assert!(transport.recipients.lock().unwrap().is_empty());
}
