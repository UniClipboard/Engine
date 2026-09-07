use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uc_application::deps::{
    InboundMembershipTransfer, LoadedMembershipLedger, MembershipBranchRecoverySession,
    MembershipConflictRecord, MembershipLedgerError, PeerHistorySyncState,
    PeerReconciliationRecord, PendingMembershipEffect,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipBranchTransitionV1,
    MembershipConflictId, MembershipHistoryAckV3, MembershipHistoryRelationship,
};

const MEMBERSHIP_LEDGER_FORMAT_V1: u16 = 1;
const MEMBERSHIP_LEDGER_FORMAT_V2: u16 = 2;
const MEMBERSHIP_LEDGER_FORMAT_V3: u16 = 3;

#[derive(Serialize, Deserialize)]
struct PersistedMembershipLedgerV1 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LegacyLoadedMembershipLedgerV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedMembershipLedgerV2 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LegacyLoadedMembershipLedgerV2,
}

#[derive(Serialize, Deserialize)]
struct PersistedMembershipLedgerV3 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LoadedMembershipLedger,
}

#[derive(Serialize, Deserialize)]
struct LegacyLoadedMembershipLedgerV2 {
    revision: u64,
    lineage_id: Option<String>,
    membership_history: Option<Vec<u8>>,
    local_device_id: Option<DeviceId>,
    local_member_instance: Option<MemberInstanceId>,
    local_join_active: bool,
    peer_reconciliation: BTreeMap<DeviceId, PeerReconciliationRecord>,
    history_sync_cursor: Option<DeviceId>,
    inbound_transfers: BTreeMap<DeviceId, InboundMembershipTransfer>,
    completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryAckV3>,
    pending_effects: BTreeMap<[u8; 32], PendingMembershipEffect>,
    membership_conflicts: BTreeMap<MembershipConflictId, MembershipConflictRecord>,
    membership_branch_transitions: BTreeMap<[u8; 32], MembershipBranchTransitionV1>,
    consumed_membership_recovery_nonces: BTreeMap<[u8; 32], MembershipConflictId>,
    membership_branch_recovery_sessions: BTreeMap<[u8; 32], MembershipBranchRecoverySession>,
}

#[derive(Serialize, Deserialize)]
struct LegacyPeerReconciliationRecordV1 {
    peer_device_id: DeviceId,
    relationship: MembershipHistoryRelationship,
    confirmed_position: Option<BaseMembershipHistoryPosition>,
    restricted_delivery: Vec<uc_application::deps::RestrictedMembershipDelivery>,
    updated_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct LegacyLoadedMembershipLedgerV1 {
    revision: u64,
    lineage_id: Option<String>,
    membership_history: Option<Vec<u8>>,
    local_device_id: Option<DeviceId>,
    local_member_instance: Option<MemberInstanceId>,
    local_join_active: bool,
    peer_reconciliation: BTreeMap<DeviceId, LegacyPeerReconciliationRecordV1>,
    inbound_transfers: BTreeMap<DeviceId, uc_application::deps::InboundMembershipTransfer>,
    completed_inbound_transfers:
        BTreeMap<(DeviceId, [u8; 32]), uc_core::membership::MembershipHistoryV2Ack>,
    pending_effects: BTreeMap<[u8; 32], uc_application::deps::PendingMembershipEffect>,
}

pub(super) fn decode(
    bytes: &[u8],
    generation: [u8; 16],
) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
    let (version, _) =
        postcard::take_from_bytes::<u16>(bytes).map_err(|_| MembershipLedgerError::Corrupt)?;
    let (profile_generation, ledger) = match version {
        MEMBERSHIP_LEDGER_FORMAT_V3 => {
            let value: PersistedMembershipLedgerV3 = parse(bytes)?;
            (value.profile_generation, value.ledger)
        }
        MEMBERSHIP_LEDGER_FORMAT_V2 => {
            let value: PersistedMembershipLedgerV2 = parse(bytes)?;
            (value.profile_generation, migrate_v2_ledger(value.ledger))
        }
        MEMBERSHIP_LEDGER_FORMAT_V1 => {
            let value: PersistedMembershipLedgerV1 = parse(bytes)?;
            (value.profile_generation, migrate_v1_ledger(value.ledger))
        }
        _ => return Err(MembershipLedgerError::Corrupt),
    };
    if profile_generation != generation {
        return Err(MembershipLedgerError::Corrupt);
    }
    Ok(ledger)
}

pub(super) fn encode(
    ledger: &LoadedMembershipLedger,
    generation: [u8; 16],
) -> Result<Vec<u8>, MembershipLedgerError> {
    postcard::to_stdvec(&PersistedMembershipLedgerV3 {
        format_version: MEMBERSHIP_LEDGER_FORMAT_V3,
        profile_generation: generation,
        ledger: ledger.clone(),
    })
    .map_err(|_| MembershipLedgerError::Corrupt)
}

fn parse<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, MembershipLedgerError> {
    let (value, tail) =
        postcard::take_from_bytes(bytes).map_err(|_| MembershipLedgerError::Corrupt)?;
    if !tail.is_empty() {
        return Err(MembershipLedgerError::Corrupt);
    }
    Ok(value)
}

fn migrate_v2_ledger(legacy: LegacyLoadedMembershipLedgerV2) -> LoadedMembershipLedger {
    LoadedMembershipLedger {
        revision: legacy.revision,
        lineage_id: legacy.lineage_id,
        membership_history: legacy.membership_history,
        local_device_id: legacy.local_device_id,
        local_member_instance: legacy.local_member_instance,
        local_join_active: legacy.local_join_active,
        peer_reconciliation: legacy.peer_reconciliation,
        history_sync_cursor: legacy.history_sync_cursor,
        inbound_transfers: legacy.inbound_transfers,
        completed_inbound_transfers: legacy.completed_inbound_transfers,
        pending_effects: legacy.pending_effects,
        membership_conflicts: legacy.membership_conflicts,
        membership_branch_transitions: legacy.membership_branch_transitions,
        consumed_membership_recovery_nonces: legacy.consumed_membership_recovery_nonces,
        membership_branch_recovery_sessions: legacy.membership_branch_recovery_sessions,
        membership_conflict_presentations: BTreeMap::new(),
    }
}

fn migrate_v1_ledger(legacy: LegacyLoadedMembershipLedgerV1) -> LoadedMembershipLedger {
    let pending_revision = legacy.revision.saturating_add(1);
    LoadedMembershipLedger {
        revision: legacy.revision,
        lineage_id: legacy.lineage_id,
        membership_history: legacy.membership_history,
        local_device_id: legacy.local_device_id,
        local_member_instance: legacy.local_member_instance,
        local_join_active: legacy.local_join_active,
        peer_reconciliation: legacy
            .peer_reconciliation
            .into_iter()
            .map(|(device_id, peer)| {
                (
                    device_id,
                    PeerReconciliationRecord {
                        peer_device_id: peer.peer_device_id,
                        relationship: peer.relationship,
                        // V1 水位可能由 Sponsor 本地推断，升级时必须重新取得认证 ACK。
                        confirmed_position: None,
                        sync_state: PeerHistorySyncState {
                            pending_since_revision: Some(pending_revision),
                            ..Default::default()
                        },
                        restricted_delivery: peer.restricted_delivery,
                        updated_at_ms: peer.updated_at_ms,
                    },
                )
            })
            .collect(),
        history_sync_cursor: None,
        // V2 的半成品传输不能被 V3 续传；历史本体保留，传输会由持久欠账重试。
        inbound_transfers: BTreeMap::new(),
        completed_inbound_transfers: BTreeMap::new(),
        pending_effects: legacy.pending_effects,
        membership_conflicts: BTreeMap::new(),
        membership_branch_transitions: BTreeMap::new(),
        consumed_membership_recovery_nonces: BTreeMap::new(),
        membership_branch_recovery_sessions: BTreeMap::new(),
        membership_conflict_presentations: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_records_preserve_decisions_and_reject_wrong_generation_or_trailing_bytes() {
        let id = MembershipConflictId::from_bytes([0x91; 32]);
        let branch = uc_core::membership::MembershipBranchId::from_bytes([0x92; 32]);
        let record = MembershipConflictRecord {
            conflict_id: id,
            local_branch_id: branch,
            remote_branch_id: uc_core::membership::MembershipBranchId::from_bytes([0x93; 32]),
            local_choice: uc_core::membership::MembershipConflictChoice::ActiveMemberRecovery,
            remote_choice: uc_core::membership::MembershipConflictChoice::RePairingRequired,
            evidence_peer_device_ids: [DeviceId::new("peer")].into(),
            detected_at_revision: 2,
            status: uc_application::deps::MembershipConflictStatus::Completed,
            selected_branch_id: Some(branch),
            transition_id: None,
        };
        let legacy = LegacyLoadedMembershipLedgerV2 {
            revision: 9,
            lineage_id: Some("lineage".to_owned()),
            membership_history: Some(vec![1, 2, 3]),
            local_device_id: Some(DeviceId::new("local")),
            local_member_instance: Some(MemberInstanceId::from_bytes([0x94; 32])),
            local_join_active: true,
            peer_reconciliation: BTreeMap::new(),
            history_sync_cursor: Some(DeviceId::new("cursor")),
            inbound_transfers: BTreeMap::new(),
            completed_inbound_transfers: BTreeMap::new(),
            pending_effects: BTreeMap::new(),
            membership_conflicts: BTreeMap::from([(id, record.clone())]),
            membership_branch_transitions: BTreeMap::new(),
            consumed_membership_recovery_nonces: BTreeMap::from([([0x95; 32], id)]),
            membership_branch_recovery_sessions: BTreeMap::new(),
        };
        let mut bytes = postcard::to_stdvec(&PersistedMembershipLedgerV2 {
            format_version: 2,
            profile_generation: [4; 16],
            ledger: legacy,
        })
        .unwrap();
        let loaded = decode(&bytes, [4; 16]).unwrap();
        assert_eq!(loaded.membership_conflicts[&id], record);
        assert_eq!(loaded.membership_history, Some(vec![1, 2, 3]));
        assert_eq!(loaded.history_sync_cursor, Some(DeviceId::new("cursor")));
        assert_eq!(loaded.consumed_membership_recovery_nonces[&[0x95; 32]], id);
        assert!(loaded.membership_conflict_presentations.is_empty());
        assert_eq!(
            decode(&encode(&loaded, [4; 16]).unwrap(), [4; 16]).unwrap(),
            loaded
        );
        assert_eq!(decode(&bytes, [5; 16]), Err(MembershipLedgerError::Corrupt));
        bytes.push(0);
        assert_eq!(decode(&bytes, [4; 16]), Err(MembershipLedgerError::Corrupt));
    }

    #[test]
    fn v1_migration_drops_untrusted_peer_watermarks_and_creates_debt() {
        let peer_id = DeviceId::new("peer-b");
        let legacy = LegacyLoadedMembershipLedgerV1 {
            revision: 7,
            lineage_id: Some("space-a".to_owned()),
            membership_history: None,
            local_device_id: Some(DeviceId::new("device-a")),
            local_member_instance: None,
            local_join_active: true,
            peer_reconciliation: BTreeMap::from([(
                peer_id.clone(),
                LegacyPeerReconciliationRecordV1 {
                    peer_device_id: peer_id.clone(),
                    relationship: MembershipHistoryRelationship::Consistent,
                    confirmed_position: Some(BaseMembershipHistoryPosition {
                        event_id: None,
                        depth: 3,
                        history_digest: [9; 32],
                    }),
                    restricted_delivery: Vec::new(),
                    updated_at_ms: 4,
                },
            )]),
            inbound_transfers: BTreeMap::new(),
            completed_inbound_transfers: BTreeMap::new(),
            pending_effects: BTreeMap::new(),
        };

        let migrated = migrate_v1_ledger(legacy);
        let peer = migrated.peer_reconciliation.get(&peer_id).unwrap();

        assert_eq!(peer.confirmed_position, None);
        assert_eq!(peer.sync_state.pending_since_revision, Some(8));
        assert_eq!(migrated.history_sync_cursor, None);
    }
}
