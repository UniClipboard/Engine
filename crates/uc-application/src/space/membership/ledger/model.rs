use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use uc_core::ids::DeviceId;
use uc_core::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipDecisionV2, MembershipHistoryAckV3,
    MembershipHistoryRelationship, MembershipHistorySuffixPageV3,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerReconciliationRecord {
    pub peer_device_id: DeviceId,
    pub relationship: MembershipHistoryRelationship,
    pub confirmed_position: Option<BaseMembershipHistoryPosition>,
    #[serde(default)]
    pub sync_state: PeerHistorySyncState,
    pub restricted_delivery: Vec<RestrictedMembershipDelivery>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PeerHistorySyncOutcome {
    #[default]
    Never,
    Deferred,
    Acked,
    StableRejected,
}

/// 对端历史确认后的持久化调度状态；它与关系判断正交，并随 ledger 整体加密。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PeerHistorySyncState {
    pub pending_since_revision: Option<u64>,
    pub retry_attempt: u32,
    pub next_attempt_at_ms: i64,
    pub last_attempt_outcome: PeerHistorySyncOutcome,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RestrictedMembershipDelivery {
    Event(uc_core::membership::MembershipEventV2),
    Decision(MembershipDecisionV2),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboundMembershipTransfer {
    pub source_device_id: DeviceId,
    pub transfer_id: [u8; 32],
    pub page_count: u32,
    pub pages: BTreeMap<u32, MembershipHistorySuffixPageV3>,
    pub total_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipEffectKind {
    AddDevice,
    RemoveDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MembershipEffectPhase {
    Prepared,
    MemberFactsApplied,
    SecurityApplied,
    Activated,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMembershipEffect {
    pub event_id: [u8; 32],
    pub kind: MembershipEffectKind,
    pub phase: MembershipEffectPhase,
    pub affected_device_ids: Vec<DeviceId>,
    pub payload: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadedMembershipLedger {
    pub revision: u64,
    pub lineage_id: Option<String>,
    pub membership_history: Option<Vec<u8>>,
    pub local_device_id: Option<DeviceId>,
    pub local_member_instance: Option<MemberInstanceId>,
    pub local_join_active: bool,
    pub peer_reconciliation: BTreeMap<DeviceId, PeerReconciliationRecord>,
    /// 公平游标只保存最后选中的 peer；下一轮从其后继续，避免排序尾部饥饿。
    #[serde(default)]
    pub history_sync_cursor: Option<DeviceId>,
    pub inbound_transfers: BTreeMap<DeviceId, InboundMembershipTransfer>,
    pub completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryAckV3>,
    pub pending_effects: BTreeMap<[u8; 32], PendingMembershipEffect>,
}

impl LoadedMembershipLedger {
    pub fn no_current_space() -> Self {
        Self {
            revision: 0,
            lineage_id: None,
            membership_history: None,
            local_device_id: None,
            local_member_instance: None,
            local_join_active: false,
            peer_reconciliation: BTreeMap::new(),
            history_sync_cursor: None,
            inbound_transfers: BTreeMap::new(),
            completed_inbound_transfers: BTreeMap::new(),
            pending_effects: BTreeMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MembershipLedgerMutation {
    pub expected_revision: u64,
    pub expected_history_digest: Option<[u8; 32]>,
    pub replacement: LoadedMembershipLedger,
}

impl std::fmt::Debug for PeerReconciliationRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PeerReconciliationRecord")
            .field("peer_device_id", &"[REDACTED]")
            .field("relationship", &self.relationship)
            .field("restricted_delivery_count", &self.restricted_delivery.len())
            .field("sync_state", &self.sync_state)
            .field("updated_at_ms", &self.updated_at_ms)
            .finish()
    }
}

impl std::fmt::Debug for RestrictedMembershipDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Event(_) => "RestrictedMembershipDelivery::Event([REDACTED])",
            Self::Decision(_) => "RestrictedMembershipDelivery::Decision([REDACTED])",
        })
    }
}

impl std::fmt::Debug for InboundMembershipTransfer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InboundMembershipTransfer")
            .field("source_device_id", &"[REDACTED]")
            .field("transfer_id", &"[REDACTED]")
            .field("page_count", &self.page_count)
            .field("saved_page_count", &self.pages.len())
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

impl std::fmt::Debug for PendingMembershipEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingMembershipEffect")
            .field("event_id", &"[REDACTED]")
            .field("kind", &self.kind)
            .field("phase", &self.phase)
            .field("affected_device_count", &self.affected_device_ids.len())
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl std::fmt::Debug for LoadedMembershipLedger {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoadedMembershipLedger")
            .field("revision", &self.revision)
            .field("has_current_space", &self.lineage_id.is_some())
            .field(
                "has_membership_history_v2",
                &self.membership_history.is_some(),
            )
            .field("local_join_active", &self.local_join_active)
            .field("peer_count", &self.peer_reconciliation.len())
            .field("inbound_transfer_count", &self.inbound_transfers.len())
            .field("pending_effect_count", &self.pending_effects.len())
            .finish()
    }
}

impl std::fmt::Debug for MembershipLedgerMutation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipLedgerMutation")
            .field("expected_revision", &self.expected_revision)
            .field("expected_history_digest", &"[REDACTED]")
            .field("replacement", &self.replacement)
            .finish()
    }
}
