use serde::{Deserialize, Serialize};

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1,
    MembershipConflictChoice, MembershipConflictId,
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

    /// 从持久资料恢复会话；绑定或状态不合法时返回 `None`。
    pub fn restore(
        transition_id: [u8; 32],
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        state: MembershipBranchRecoverySessionState,
    ) -> Option<Self> {
        let session = Self {
            transition_id,
            conflict_id,
            target_branch_id,
            recipient_member,
            state,
        };
        session.validate().then_some(session)
    }

    pub const fn transition_id(&self) -> &[u8; 32] {
        &self.transition_id
    }

    pub const fn conflict_id(&self) -> MembershipConflictId {
        self.conflict_id
    }

    pub const fn target_branch_id(&self) -> MembershipBranchId {
        self.target_branch_id
    }

    pub const fn recipient_member(&self) -> MemberInstanceId {
        self.recipient_member
    }

    pub const fn state(&self) -> &MembershipBranchRecoverySessionState {
        &self.state
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
