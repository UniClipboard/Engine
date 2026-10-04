//! V1–V4 旧布局，只用于一次性迁移。V1–V3 先升级到 V4 形状，规则与原 V4 读取完全相同：
//! V1 水位不可信，清除确认位置并登记同步欠账；V1–V3 的半成品传输无法续传，丢弃后由同步重试。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uc_application::deps::MembershipLedgerError;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, BaseMembershipHistoryPosition,
    MemberInstanceId, MembershipBranchTransitionV1, MembershipConflictId, MembershipDecisionV2,
    MembershipEventId, MembershipEventV2, MembershipHistoryAckV3, MembershipHistoryV2Ack,
};

use super::common::{
    BranchRecoveryDto, ConflictPresentationDto, ConflictRecordDto, InboundTransferDto, PositionDto,
    RecoverySessionDto,
};
use super::parse;

pub(super) const FORMAT_V1: u16 = 1;
pub(super) const FORMAT_V2: u16 = 2;
pub(super) const FORMAT_V3: u16 = 3;
pub(super) const FORMAT_V4: u16 = 4;

/// V4 布局，也是 V1–V3 升级后的统一形状。
#[derive(Serialize, Deserialize)]
pub(super) struct LegacyLedger {
    pub(super) revision: u64,
    pub(super) lineage_id: Option<String>,
    pub(super) membership_history: Option<Vec<u8>>,
    pub(super) local_device_id: Option<DeviceId>,
    pub(super) local_member_instance: Option<MemberInstanceId>,
    pub(super) local_join_active: bool,
    pub(super) peer_reconciliation: BTreeMap<DeviceId, LegacyPeer>,
    pub(super) history_sync_cursor: Option<DeviceId>,
    pub(super) inbound_transfers: BTreeMap<DeviceId, InboundTransferDto>,
    pub(super) completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryAckV3>,
    pub(super) effect_journal: BTreeMap<MembershipEventId, LegacyEffect>,
    pub(super) branch_recovery: BranchRecoveryDto,
}

#[derive(Serialize, Deserialize)]
pub(super) struct LegacyPeer {
    pub(super) peer_device_id: DeviceId,
    pub(super) relationship: LegacyRelationship,
    pub(super) confirmed_position: Option<PositionDto>,
    pub(super) sync_state: LegacySyncState,
    pub(super) restricted_delivery: Vec<LegacyDelivery>,
    pub(super) updated_at_ms: i64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum LegacyRelationship {
    Unknown,
    Consistent,
    UpgradeRequired,
    PendingRemovalDecision,
    Diverged,
    Invalid,
}

#[derive(Serialize, Deserialize, Default)]
pub(super) struct LegacySyncState {
    pub(super) pending_since_revision: Option<u64>,
    pub(super) retry_attempt: u32,
    pub(super) next_attempt_at_ms: i64,
    pub(super) last_attempt_outcome: LegacySyncOutcome,
}

#[derive(Clone, Copy, Serialize, Deserialize, Default)]
pub(super) enum LegacySyncOutcome {
    #[default]
    Never,
    Deferred,
    Acked,
    StableRejected,
}

#[derive(Serialize, Deserialize)]
pub(super) enum LegacyDelivery {
    Event(MembershipEventV2),
    Decision(MembershipDecisionV2),
}

#[derive(Serialize, Deserialize)]
pub(super) struct LegacyEffect {
    pub(super) event_id: MembershipEventId,
    pub(super) kind: LegacyEffectKind,
    pub(super) phase: LegacyEffectPhase,
    pub(super) affected_device_ids: Vec<DeviceId>,
    pub(super) payload: Vec<u8>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub(super) enum LegacyEffectKind {
    AddDevice,
    RemoveDevice,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum LegacyEffectPhase {
    Prepared,
    MemberFactsApplied,
    SecurityApplied,
    Activated,
}

/// 本机发起移除时效果载荷的布局。
#[derive(Serialize, Deserialize)]
pub(super) struct LegacyInitiatedRemoval {
    pub(super) event: MembershipEventV2,
    pub(super) retained_device_ids: Vec<DeviceId>,
}

#[derive(Serialize, Deserialize)]
struct PersistedV4 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LegacyLedger,
}

#[derive(Serialize, Deserialize)]
struct PersistedV3 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LedgerV3,
}

// Postcard 顺序编码中，V3 是 V2 字段后追加展示资料；不增加嵌套长度前缀。
#[derive(Serialize, Deserialize)]
struct LedgerV3 {
    common: LedgerV2,
    presentations: BTreeMap<MembershipConflictId, ConflictPresentationDto>,
}

#[derive(Serialize, Deserialize)]
struct PersistedV2 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LedgerV2,
}

#[derive(Serialize, Deserialize)]
struct LedgerV2 {
    revision: u64,
    lineage_id: Option<String>,
    membership_history: Option<Vec<u8>>,
    local_device_id: Option<DeviceId>,
    local_member_instance: Option<MemberInstanceId>,
    local_join_active: bool,
    peer_reconciliation: BTreeMap<DeviceId, LegacyPeer>,
    history_sync_cursor: Option<DeviceId>,
    inbound_transfers: BTreeMap<DeviceId, FrameTransfer>,
    completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryAckV3>,
    effect_journal: BTreeMap<MembershipEventId, LegacyEffect>,
    conflicts: BTreeMap<MembershipConflictId, ConflictRecordDto>,
    branch_transitions: BTreeMap<[u8; 32], MembershipBranchTransitionV1>,
    consumed_recovery_nonces: BTreeMap<[u8; 32], MembershipConflictId>,
    recovery_sessions: BTreeMap<[u8; 32], RecoverySessionDto>,
}

#[derive(Serialize, Deserialize)]
struct PersistedV1 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LedgerV1,
}

#[derive(Serialize, Deserialize)]
struct LedgerV1 {
    revision: u64,
    lineage_id: Option<String>,
    membership_history: Option<Vec<u8>>,
    local_device_id: Option<DeviceId>,
    local_member_instance: Option<MemberInstanceId>,
    local_join_active: bool,
    peer_reconciliation: BTreeMap<DeviceId, PeerV1>,
    inbound_transfers: BTreeMap<DeviceId, FrameTransfer>,
    completed_inbound_transfers: BTreeMap<(DeviceId, [u8; 32]), MembershipHistoryV2Ack>,
    effect_journal: BTreeMap<MembershipEventId, LegacyEffect>,
}

#[derive(Serialize, Deserialize)]
struct PeerV1 {
    peer_device_id: DeviceId,
    relationship: LegacyRelationship,
    confirmed_position: Option<PositionDto>,
    restricted_delivery: Vec<LegacyDelivery>,
    updated_at_ms: i64,
}

/// V1–V3 的在途帧，只为跳过其字节；迁移时丢弃。
#[derive(Serialize, Deserialize)]
struct FrameTransfer {
    source_device_id: DeviceId,
    transfer_id: [u8; 32],
    page_count: u32,
    pages: BTreeMap<u32, FramePage>,
    total_bytes: usize,
}

#[derive(Serialize, Deserialize)]
struct FramePage {
    format_version: u16,
    transfer_id: [u8; 32],
    page_index: u32,
    page_count: u32,
    lineage_id: String,
    base_position: BaseMembershipHistoryPosition,
    target_position: BaseMembershipHistoryPosition,
    sender_admission: AdmissionChangeFacts,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    decisions: Vec<MembershipDecisionV2>,
}

/// 按版本读取旧布局，返回写入时的 profile generation 与升级到 V4 形状的内容。
pub(super) fn decode(
    version: u16,
    bytes: &[u8],
) -> Result<([u8; 16], LegacyLedger), MembershipLedgerError> {
    match version {
        FORMAT_V4 => {
            let value: PersistedV4 = parse(bytes)?;
            Ok((value.profile_generation, value.ledger))
        }
        FORMAT_V3 => {
            let value: PersistedV3 = parse(bytes)?;
            let mut ledger = upgrade_v2(value.ledger.common);
            ledger.branch_recovery.conflict_presentations = value.ledger.presentations;
            Ok((value.profile_generation, ledger))
        }
        FORMAT_V2 => {
            let value: PersistedV2 = parse(bytes)?;
            Ok((value.profile_generation, upgrade_v2(value.ledger)))
        }
        FORMAT_V1 => {
            let value: PersistedV1 = parse(bytes)?;
            Ok((value.profile_generation, upgrade_v1(value.ledger)))
        }
        _ => Err(MembershipLedgerError::corrupt()),
    }
}

fn upgrade_v2(legacy: LedgerV2) -> LegacyLedger {
    LegacyLedger {
        revision: legacy.revision,
        lineage_id: legacy.lineage_id,
        membership_history: legacy.membership_history,
        local_device_id: legacy.local_device_id,
        local_member_instance: legacy.local_member_instance,
        local_join_active: legacy.local_join_active,
        peer_reconciliation: legacy.peer_reconciliation,
        history_sync_cursor: legacy.history_sync_cursor,
        inbound_transfers: BTreeMap::new(),
        completed_inbound_transfers: BTreeMap::new(),
        effect_journal: legacy.effect_journal,
        branch_recovery: BranchRecoveryDto {
            conflicts: legacy.conflicts,
            branch_transitions: legacy.branch_transitions,
            consumed_recovery_nonces: legacy.consumed_recovery_nonces,
            recovery_sessions: legacy.recovery_sessions,
            conflict_presentations: BTreeMap::new(),
        },
    }
}

fn upgrade_v1(legacy: LedgerV1) -> LegacyLedger {
    let pending_revision = legacy.revision.saturating_add(1);
    LegacyLedger {
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
                    LegacyPeer {
                        peer_device_id: peer.peer_device_id,
                        relationship: peer.relationship,
                        // V1 水位可能由 Sponsor 本地推断，升级时必须重新取得认证 ACK。
                        confirmed_position: None,
                        sync_state: LegacySyncState {
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
        inbound_transfers: BTreeMap::new(),
        completed_inbound_transfers: BTreeMap::new(),
        effect_journal: legacy.effect_journal,
        branch_recovery: BranchRecoveryDto::default(),
    }
}
