use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2, MembershipCredential,
    MembershipEventId, MembershipHistoryAckV3, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, MembershipHistoryRelationship,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, CurrentSpaceMemberScope, CurrentSpaceMemberScopeError,
    CurrentSpaceMemberScopePort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedger, MembershipLedgerError, MembershipLedgerMutation, PausedSpaceMember,
    PeerReconciliationRecord, SpaceMemberPauseReason, SynchronizeMembershipMaintenancePort,
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

struct FixedScope(Vec<DeviceId>);

struct FixedClock;

impl uc_core::ports::ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        10_000
    }
}

struct ClockAt(i64);

impl uc_core::ports::ClockPort for ClockAt {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

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

#[async_trait]
impl CurrentSpaceMemberScopePort for FixedScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 5,
            local_member_active: true,
            usable_peer_device_ids: self.0.clone(),
            paused_peer_devices: Vec::new(),
        })
    }
}

struct RecordingTransport {
    recipients: Mutex<Vec<DeviceId>>,
}

struct SwitchableTransport {
    offline: AtomicBool,
    recipients: Mutex<Vec<DeviceId>>,
}

struct ConcurrentTransport {
    active: AtomicUsize,
    max_active: AtomicUsize,
}

#[async_trait]
impl MembershipHistoryExchangePort for ConcurrentTransport {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        match message {
            MembershipHistoryMessage::SummaryV3(summary) => Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Confirmed {
                    transfer_id: summary.transfer_id,
                    confirmed_position: summary.current_position,
                },
            )),
            _ => Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            )),
        }
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for SwitchableTransport {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.recipients.lock().unwrap().push(recipient.clone());
        if self.offline.load(Ordering::SeqCst) {
            return Err(MembershipHistoryExchangeError::Offline);
        }
        match message {
            MembershipHistoryMessage::SummaryV3(summary) => Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Confirmed {
                    transfer_id: summary.transfer_id,
                    confirmed_position: summary.current_position,
                },
            )),
            _ => Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Invalid,
            )),
        }
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for RecordingTransport {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        if matches!(&message, MembershipHistoryMessage::SummaryV3(_)) {
            self.recipients.lock().unwrap().push(recipient.clone());
        }
        if recipient == &DeviceId::new("device-c") {
            return Err(MembershipHistoryExchangeError::Offline);
        }
        if let MembershipHistoryMessage::SummaryV3(summary) = message {
            return Ok(MembershipHistoryMessage::AckV3(
                MembershipHistoryAckV3::Confirmed {
                    transfer_id: summary.transfer_id,
                    confirmed_position: summary.current_position,
                },
            ));
        }
        Ok(MembershipHistoryMessage::AckV3(
            MembershipHistoryAckV3::Invalid,
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
                sync_state: Default::default(),
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
        Arc::new(FixedClock),
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
        Arc::new(FixedClock),
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
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(EmptyScope),
        transport.clone(),
        Arc::new(FixedClock),
    );

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

#[tokio::test]
async fn periodic_round_is_required_until_every_peer_confirms_the_current_position() {
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(active_ledger())));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(UnsortedScope),
        Arc::new(RecordingTransport {
            recipients: Mutex::new(Vec::new()),
        }),
        Arc::new(FixedClock),
    );

    assert!(synchronize
        .periodic_synchronization_required()
        .await
        .expect("周期反熵判断应可用"));
}

#[tokio::test]
async fn persistent_cursor_eventually_selects_two_hundred_pending_peers() {
    let mut loaded = active_ledger();
    let peers = (0..200)
        .map(|index| DeviceId::new(format!("peer-{index:03}")))
        .collect::<Vec<_>>();
    for peer in &peers {
        loaded.peer_reconciliation.insert(
            peer.clone(),
            PeerReconciliationRecord {
                peer_device_id: peer.clone(),
                relationship: MembershipHistoryRelationship::Consistent,
                confirmed_position: None,
                sync_state: Default::default(),
                restricted_delivery: Vec::new(),
                updated_at_ms: 0,
            },
        );
    }
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(EmptyScope),
        Arc::new(RecordingTransport {
            recipients: Mutex::new(Vec::new()),
        }),
        Arc::new(FixedClock),
    );
    let mut selected = std::collections::BTreeSet::new();

    for _ in 0..25 {
        selected.extend(synchronize.select_due_peers(peers.clone()).await.unwrap());
    }

    assert_eq!(selected.len(), 200);
}

#[tokio::test]
async fn clock_rollback_makes_persisted_retry_due_immediately() {
    let mut loaded = active_ledger();
    let peer = DeviceId::new("device-b");
    let peer_state = &mut loaded
        .peer_reconciliation
        .get_mut(&peer)
        .unwrap()
        .sync_state;
    peer_state.retry_attempt = 1;
    peer_state.next_attempt_at_ms = 11_000;
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(EmptyScope),
        Arc::new(RecordingTransport {
            recipients: Mutex::new(Vec::new()),
        }),
        Arc::new(ClockAt(9_000)),
    );

    let selected = synchronize
        .select_due_peers(vec![peer.clone()])
        .await
        .unwrap();

    assert_eq!(selected, vec![peer]);
}

#[tokio::test]
async fn deferred_attempt_survives_restart_and_retries_when_due() {
    let peer = DeviceId::new("device-c");
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(active_ledger())));
    let transport = Arc::new(SwitchableTransport {
        offline: AtomicBool::new(true),
        recipients: Mutex::new(Vec::new()),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(FixedScope(vec![peer.clone()])),
        transport.clone(),
        Arc::new(ClockAt(10_000)),
    );

    let first = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();
    assert_eq!(first.deferred_peer_count, 1);
    let persisted = repository.load().await.unwrap();
    let persisted_peer = persisted.peer_reconciliation.get(&peer).unwrap();
    assert_eq!(persisted_peer.sync_state.retry_attempt, 1);
    assert_eq!(persisted_peer.sync_state.next_attempt_at_ms, 11_000);
    assert!(persisted_peer.confirmed_position.is_none());
    drop(synchronize);

    transport.offline.store(false, Ordering::SeqCst);
    let restarted_ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let restarted = SynchronizeMembershipHistoryUseCase::new(
        restarted_ledger,
        Arc::new(FixedScope(vec![peer.clone()])),
        transport.clone(),
        Arc::new(ClockAt(11_000)),
    );

    let second = restarted
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();
    assert_eq!(second.completed_peer_count, 1);
    let completed = repository.load().await.unwrap();
    let completed_peer = completed.peer_reconciliation.get(&peer).unwrap();
    assert!(completed_peer.confirmed_position.is_some());
    assert_eq!(completed_peer.sync_state.retry_attempt, 0);
    assert_eq!(completed_peer.sync_state.next_attempt_at_ms, 0);
    assert_eq!(
        transport.recipients.lock().unwrap().as_slice(),
        &[peer.clone(), peer]
    );
}

#[tokio::test]
async fn retry_counter_overflow_fails_closed_without_committing_partial_state() {
    let peer = DeviceId::new("device-c");
    let mut loaded = active_ledger();
    loaded
        .peer_reconciliation
        .get_mut(&peer)
        .unwrap()
        .sync_state
        .retry_attempt = u32::MAX;
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(FixedScope(vec![peer.clone()])),
        Arc::new(SwitchableTransport {
            offline: AtomicBool::new(true),
            recipients: Mutex::new(Vec::new()),
        }),
        Arc::new(ClockAt(10_000)),
    );

    let error = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        SynchronizeMembershipHistoryError::RecoveryRequired
    ));
    let persisted = repository.load().await.unwrap();
    let persisted_peer = persisted.peer_reconciliation.get(&peer).unwrap();
    assert_eq!(persisted_peer.sync_state.retry_attempt, u32::MAX);
    assert_eq!(persisted_peer.sync_state.next_attempt_at_ms, 0);
    assert!(persisted_peer.confirmed_position.is_none());
}

#[tokio::test]
async fn round_uses_the_fixed_concurrency_bound() {
    let peers = (0..8)
        .map(|index| DeviceId::new(format!("peer-{index:02}")))
        .collect::<Vec<_>>();
    let mut loaded = active_ledger();
    for peer in &peers {
        loaded.peer_reconciliation.insert(
            peer.clone(),
            PeerReconciliationRecord {
                peer_device_id: peer.clone(),
                relationship: MembershipHistoryRelationship::Consistent,
                confirmed_position: None,
                sync_state: Default::default(),
                restricted_delivery: Vec::new(),
                updated_at_ms: 0,
            },
        );
    }
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let transport = Arc::new(ConcurrentTransport {
        active: AtomicUsize::new(0),
        max_active: AtomicUsize::new(0),
    });
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(FixedScope(peers)),
        transport.clone(),
        Arc::new(FixedClock),
    );

    let report = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();

    assert_eq!(report.completed_peer_count, 8);
    assert_eq!(transport.max_active.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn equal_summary_completes_without_sending_history_pages() {
    let peer = DeviceId::new("device-b");
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(active_ledger())));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let transport = Arc::new(SwitchableTransport {
        offline: AtomicBool::new(false),
        recipients: Mutex::new(Vec::new()),
    });
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(FixedScope(vec![peer.clone()])),
        transport.clone(),
        Arc::new(FixedClock),
    );

    let report = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();

    assert_eq!(report.completed_peer_count, 1);
    assert_eq!(transport.recipients.lock().unwrap().as_slice(), &[peer]);
}

#[tokio::test]
async fn round_batch_limit_preserves_unselected_peer_debt_for_the_next_round() {
    let peers = (0..9)
        .map(|index| DeviceId::new(format!("peer-{index:02}")))
        .collect::<Vec<_>>();
    let mut loaded = active_ledger();
    for peer in &peers {
        loaded.peer_reconciliation.insert(
            peer.clone(),
            PeerReconciliationRecord {
                peer_device_id: peer.clone(),
                relationship: MembershipHistoryRelationship::Consistent,
                confirmed_position: None,
                sync_state: Default::default(),
                restricted_delivery: Vec::new(),
                updated_at_ms: 0,
            },
        );
    }
    let repository = Arc::new(MemoryLedgerRepository(Mutex::new(loaded)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let transport = Arc::new(SwitchableTransport {
        offline: AtomicBool::new(false),
        recipients: Mutex::new(Vec::new()),
    });
    let synchronize = SynchronizeMembershipHistoryUseCase::new(
        ledger,
        Arc::new(FixedScope(peers.clone())),
        transport,
        Arc::new(FixedClock),
    );

    let first = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();
    assert_eq!(first.completed_peer_count, 8);
    assert_eq!(
        repository
            .load()
            .await
            .unwrap()
            .peer_reconciliation
            .values()
            .filter(|record| record.confirmed_position.is_none())
            .count(),
        3,
        "两个 fixture peer 与一个本轮未选择 peer 仍未确认"
    );

    let second = synchronize
        .execute(MembershipSyncTarget::AllCurrentPeers)
        .await
        .unwrap();
    assert_eq!(second.completed_peer_count, 1);
    let persisted = repository.load().await.unwrap();
    assert!(peers.iter().all(|peer| persisted.peer_reconciliation[peer]
        .confirmed_position
        .is_some()));
}
