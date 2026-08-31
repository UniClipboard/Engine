use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use uc_core::ids::DeviceId;
use uc_core::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipBranchId,
    MembershipBranchRecoveryPackageV1, MembershipConflictChoice, MembershipConflictId,
    MembershipDecisionV2, MembershipHistoryAckV3, MembershipHistoryRelationship,
    MembershipHistorySuffixPageV3,
};

const MAX_RECOVERY_STATE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipConflictStatus {
    Unresolved,
    Selected,
    Transitioning,
    Completed,
    RePairingRequired,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipConflictRecord {
    pub conflict_id: MembershipConflictId,
    pub local_branch_id: MembershipBranchId,
    pub remote_branch_id: MembershipBranchId,
    pub local_choice: MembershipConflictChoice,
    pub remote_choice: MembershipConflictChoice,
    pub evidence_peer_device_ids: std::collections::BTreeSet<DeviceId>,
    pub detected_at_revision: u64,
    pub status: MembershipConflictStatus,
    pub selected_branch_id: Option<MembershipBranchId>,
    pub transition_id: Option<[u8; 32]>,
}

impl MembershipConflictRecord {
    pub(crate) fn choice_for(
        &self,
        branch_id: MembershipBranchId,
    ) -> Option<MembershipConflictChoice> {
        if branch_id == self.local_branch_id {
            Some(self.local_choice)
        } else if branch_id == self.remote_branch_id {
            Some(self.remote_choice)
        } else {
            None
        }
    }
}

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
pub struct InitiatedMembershipRemovalEffect {
    pub event: uc_core::membership::MembershipEventV2,
    pub retained_device_ids: Vec<DeviceId>,
}

impl PendingMembershipEffect {
    pub fn membership_event(&self) -> Option<uc_core::membership::MembershipEventV2> {
        postcard::from_bytes(&self.payload).ok().or_else(|| {
            postcard::from_bytes::<InitiatedMembershipRemovalEffect>(&self.payload)
                .ok()
                .map(|effect| effect.event)
        })
    }

    pub fn initiated_removal(&self) -> Option<InitiatedMembershipRemovalEffect> {
        postcard::from_bytes(&self.payload).ok()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipBranchRecoverySessionState {
    RecipientPrepared {
        external_commit: Vec<u8>,
        recipient_staged_mls_state: Vec<u8>,
    },
    RecipientCompleted {
        recipient_staged_mls_state: Vec<u8>,
        recovery_package: MembershipBranchRecoveryPackageV1,
    },
    TargetPrepared {
        external_commit_digest: [u8; 32],
        target_staged_space_material: Vec<u8>,
        recovery_package: MembershipBranchRecoveryPackageV1,
    },
    TargetCommitted {
        external_commit_digest: [u8; 32],
        recovery_package: MembershipBranchRecoveryPackageV1,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipBranchRecoverySession {
    transition_id: [u8; 32],
    conflict_id: MembershipConflictId,
    target_branch_id: MembershipBranchId,
    recipient_member: MemberInstanceId,
    state: MembershipBranchRecoverySessionState,
}

impl MembershipBranchRecoverySession {
    pub fn new_recipient_prepared(
        transition_id: [u8; 32],
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        external_commit: Vec<u8>,
        recipient_staged_mls_state: Vec<u8>,
    ) -> Option<Self> {
        let session = Self {
            transition_id,
            conflict_id,
            target_branch_id,
            recipient_member,
            state: MembershipBranchRecoverySessionState::RecipientPrepared {
                external_commit,
                recipient_staged_mls_state,
            },
        };
        session.validate().then_some(session)
    }

    pub fn new_target_prepared(
        transition_id: [u8; 32],
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        external_commit_digest: [u8; 32],
        target_staged_space_material: Vec<u8>,
        recovery_package: MembershipBranchRecoveryPackageV1,
    ) -> Option<Self> {
        let session = Self {
            transition_id,
            conflict_id,
            target_branch_id,
            recipient_member,
            state: MembershipBranchRecoverySessionState::TargetPrepared {
                external_commit_digest,
                target_staged_space_material,
                recovery_package,
            },
        };
        session.validate().then_some(session)
    }

    pub const fn transition_id(&self) -> &[u8; 32] {
        &self.transition_id
    }

    pub fn recipient_preparation(&self) -> Option<(&[u8], &[u8])> {
        match &self.state {
            MembershipBranchRecoverySessionState::RecipientPrepared {
                external_commit,
                recipient_staged_mls_state,
            } => Some((external_commit, recipient_staged_mls_state)),
            _ => None,
        }
    }

    pub fn recipient_completion(&self) -> Option<(&[u8], &MembershipBranchRecoveryPackageV1)> {
        match &self.state {
            MembershipBranchRecoverySessionState::RecipientCompleted {
                recipient_staged_mls_state,
                recovery_package,
            } => Some((recipient_staged_mls_state, recovery_package)),
            _ => None,
        }
    }

    pub fn target_preparation(
        &self,
    ) -> Option<([u8; 32], &[u8], &MembershipBranchRecoveryPackageV1)> {
        match &self.state {
            MembershipBranchRecoverySessionState::TargetPrepared {
                external_commit_digest,
                target_staged_space_material,
                recovery_package,
            } => Some((
                *external_commit_digest,
                target_staged_space_material,
                recovery_package,
            )),
            _ => None,
        }
    }

    pub fn target_completion(&self) -> Option<([u8; 32], &MembershipBranchRecoveryPackageV1)> {
        match &self.state {
            MembershipBranchRecoverySessionState::TargetCommitted {
                external_commit_digest,
                recovery_package,
            } => Some((*external_commit_digest, recovery_package)),
            _ => None,
        }
    }

    pub fn complete_recipient(
        &mut self,
        recovery_package: MembershipBranchRecoveryPackageV1,
    ) -> bool {
        if !self.package_matches(&recovery_package) {
            return false;
        }
        match &self.state {
            MembershipBranchRecoverySessionState::RecipientPrepared {
                recipient_staged_mls_state,
                ..
            } => {
                self.state = MembershipBranchRecoverySessionState::RecipientCompleted {
                    recipient_staged_mls_state: recipient_staged_mls_state.clone(),
                    recovery_package,
                };
                true
            }
            MembershipBranchRecoverySessionState::RecipientCompleted {
                recovery_package: existing,
                ..
            } => existing == &recovery_package,
            _ => false,
        }
    }

    pub fn commit_target(&mut self) -> bool {
        match &self.state {
            MembershipBranchRecoverySessionState::TargetPrepared {
                external_commit_digest,
                recovery_package,
                ..
            } => {
                self.state = MembershipBranchRecoverySessionState::TargetCommitted {
                    external_commit_digest: *external_commit_digest,
                    recovery_package: recovery_package.clone(),
                };
                true
            }
            MembershipBranchRecoverySessionState::TargetCommitted { .. } => true,
            _ => false,
        }
    }

    pub(crate) fn validate(&self) -> bool {
        if self.transition_id == [0; 32] {
            return false;
        }
        match &self.state {
            MembershipBranchRecoverySessionState::RecipientPrepared {
                external_commit,
                recipient_staged_mls_state,
            } => bounded(external_commit) && bounded(recipient_staged_mls_state),
            MembershipBranchRecoverySessionState::RecipientCompleted {
                recipient_staged_mls_state,
                recovery_package,
            } => bounded(recipient_staged_mls_state) && self.package_matches(recovery_package),
            MembershipBranchRecoverySessionState::TargetPrepared {
                external_commit_digest,
                target_staged_space_material,
                recovery_package,
            } => {
                *external_commit_digest != [0; 32]
                    && bounded(target_staged_space_material)
                    && self.package_matches(recovery_package)
            }
            MembershipBranchRecoverySessionState::TargetCommitted {
                external_commit_digest,
                recovery_package,
            } => *external_commit_digest != [0; 32] && self.package_matches(recovery_package),
        }
    }

    fn package_matches(&self, package: &MembershipBranchRecoveryPackageV1) -> bool {
        package.conflict_id() == self.conflict_id
            && package.target_branch_id() == self.target_branch_id
            && package.recipient_member() == self.recipient_member
    }
}

fn bounded(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= MAX_RECOVERY_STATE_BYTES
}

#[cfg(test)]
mod recovery_session_tests {
    use super::*;

    fn package(
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
    ) -> MembershipBranchRecoveryPackageV1 {
        MembershipBranchRecoveryPackageV1::new_unsigned(
            conflict_id,
            target_branch_id,
            recipient_member,
            recipient_member,
            1_000,
            [0x44; 32],
            vec![1],
            vec![2],
            vec![3],
        )
        .unwrap()
        .with_authorization_signature(vec![4])
    }

    #[test]
    fn recipient_completion_is_bound_and_idempotent() {
        let conflict_id = MembershipConflictId::from_bytes([0x11; 32]);
        let target_branch_id = MembershipBranchId::from_bytes([0x12; 32]);
        let recipient_member = MemberInstanceId::from_bytes([0x13; 32]);
        let recovery_package = package(conflict_id, target_branch_id, recipient_member);
        let mut session = MembershipBranchRecoverySession::new_recipient_prepared(
            [0x14; 32],
            conflict_id,
            target_branch_id,
            recipient_member,
            vec![5],
            vec![6],
        )
        .unwrap();

        assert!(session.complete_recipient(recovery_package.clone()));
        assert!(session.complete_recipient(recovery_package));
        assert!(session.validate());
    }

    #[test]
    fn target_commit_is_monotonic_and_idempotent() {
        let conflict_id = MembershipConflictId::from_bytes([0x21; 32]);
        let target_branch_id = MembershipBranchId::from_bytes([0x22; 32]);
        let recipient_member = MemberInstanceId::from_bytes([0x23; 32]);
        let mut session = MembershipBranchRecoverySession::new_target_prepared(
            [0x24; 32],
            conflict_id,
            target_branch_id,
            recipient_member,
            [0x25; 32],
            vec![7],
            package(conflict_id, target_branch_id, recipient_member),
        )
        .unwrap();

        assert!(session.commit_target());
        assert!(session.commit_target());
        assert!(session.validate());
    }
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
    /// 冲突、证据来源和用户选择随 ledger 整体加密，并与关系状态共用 CAS revision。
    #[serde(default)]
    pub membership_conflicts: BTreeMap<MembershipConflictId, MembershipConflictRecord>,
    /// 同 lineage 分支切换状态随 ledger 加密；每一步只能由 Core 状态机向前推进。
    #[serde(default)]
    pub membership_branch_transitions:
        BTreeMap<[u8; 32], uc_core::membership::MembershipBranchTransitionV1>,
    /// 已接受恢复包的 nonce 随 ledger 整体加密；value 绑定首次消费它的 conflict。
    #[serde(default)]
    pub consumed_membership_recovery_nonces: BTreeMap<[u8; 32], MembershipConflictId>,
    /// 两阶段恢复的私有 staged state 与幂等响应随 ledger 整体 AEAD 加密。
    #[serde(default)]
    pub membership_branch_recovery_sessions: BTreeMap<[u8; 32], MembershipBranchRecoverySession>,
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
            membership_conflicts: BTreeMap::new(),
            membership_branch_transitions: BTreeMap::new(),
            consumed_membership_recovery_nonces: BTreeMap::new(),
            membership_branch_recovery_sessions: BTreeMap::new(),
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

impl std::fmt::Debug for MembershipConflictRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipConflictRecord")
            .field("conflict_id", &"[REDACTED]")
            .field("branch_ids", &"[REDACTED]")
            .field("local_choice", &self.local_choice)
            .field("remote_choice", &self.remote_choice)
            .field("evidence_peer_count", &self.evidence_peer_device_ids.len())
            .field("detected_at_revision", &self.detected_at_revision)
            .field("status", &self.status)
            .field("has_selected_branch", &self.selected_branch_id.is_some())
            .field("has_transition", &self.transition_id.is_some())
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

impl std::fmt::Debug for MembershipBranchRecoverySessionState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::RecipientPrepared { .. } => "RecipientPrepared([REDACTED])",
            Self::RecipientCompleted { .. } => "RecipientCompleted([REDACTED])",
            Self::TargetPrepared { .. } => "TargetPrepared([REDACTED])",
            Self::TargetCommitted { .. } => "TargetCommitted([REDACTED])",
        })
    }
}

impl std::fmt::Debug for MembershipBranchRecoverySession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipBranchRecoverySession")
            .field("bindings", &"[REDACTED]")
            .field("state", &self.state)
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
            .field(
                "membership_conflict_count",
                &self.membership_conflicts.len(),
            )
            .field(
                "membership_branch_transition_count",
                &self.membership_branch_transitions.len(),
            )
            .field(
                "consumed_membership_recovery_nonce_count",
                &self.consumed_membership_recovery_nonces.len(),
            )
            .field(
                "membership_branch_recovery_session_count",
                &self.membership_branch_recovery_sessions.len(),
            )
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
