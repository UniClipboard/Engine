//! V4 与 V5 共用的冻结布局。字段与变体顺序即字节布局，修改任何一处都会改变已保存资料的含义。
//! V4 的这些部分原由 Application 类型直接序列化，这里以相同顺序独立声明，使解码不再依赖上层模型。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uc_application::deps::{
    InboundMembershipTransfer, MembershipBranchRecoveryRecord, MembershipBranchRecoverySession,
    MembershipBranchRecoverySessionState, MembershipConflictMember, MembershipConflictPresentation,
    MembershipConflictRecord, MembershipConflictStatus, MembershipLedgerError,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipBranchId,
    MembershipBranchRecoveryPackageV1, MembershipBranchTransitionV1, MembershipChangeFact,
    MembershipChangeKind, MembershipChangeSide, MembershipConflictChoice, MembershipConflictDevice,
    MembershipConflictExplanation, MembershipConflictId, MembershipConflictReason,
    MembershipDecisionFact, MembershipEventId, MembershipHistorySuffixPageV4, RemovalDecision,
};

#[derive(Serialize, Deserialize)]
pub(super) struct PositionDto {
    event_id: Option<MembershipEventId>,
    depth: u64,
    history_digest: [u8; 32],
}

impl PositionDto {
    pub(super) fn from_position(position: &BaseMembershipHistoryPosition) -> Self {
        Self {
            event_id: position.event_id,
            depth: position.depth,
            history_digest: position.history_digest,
        }
    }

    pub(super) fn into_position(self) -> BaseMembershipHistoryPosition {
        BaseMembershipHistoryPosition {
            event_id: self.event_id,
            depth: self.depth,
            history_digest: self.history_digest,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct InboundTransferDto {
    source_device_id: DeviceId,
    transfer_id: [u8; 32],
    page_count: u32,
    pages: BTreeMap<u32, MembershipHistorySuffixPageV4>,
    total_bytes: usize,
}

impl InboundTransferDto {
    pub(super) fn from_transfer(transfer: &InboundMembershipTransfer) -> Self {
        Self {
            source_device_id: transfer.source_device_id,
            transfer_id: transfer.transfer_id,
            page_count: transfer.page_count,
            pages: transfer.pages.clone(),
            total_bytes: transfer.total_bytes,
        }
    }

    pub(super) fn into_transfer(self) -> InboundMembershipTransfer {
        InboundMembershipTransfer {
            source_device_id: self.source_device_id,
            transfer_id: self.transfer_id,
            page_count: self.page_count,
            pages: self.pages,
            total_bytes: self.total_bytes,
        }
    }
}

#[derive(Serialize, Deserialize)]
enum ConflictStatusDto {
    Unresolved,
    Selected,
    Transitioning,
    Completed,
    RePairingRequired,
}

#[derive(Serialize, Deserialize)]
enum ConflictChoiceDto {
    ActiveMemberRecovery,
    RePairingRequired,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ConflictRecordDto {
    conflict_id: MembershipConflictId,
    local_branch_id: MembershipBranchId,
    remote_branch_id: MembershipBranchId,
    local_choice: ConflictChoiceDto,
    remote_choice: ConflictChoiceDto,
    evidence_peer_device_ids: BTreeSet<DeviceId>,
    detected_at_revision: u64,
    status: ConflictStatusDto,
    selected_branch_id: Option<MembershipBranchId>,
    transition_id: Option<[u8; 32]>,
}

#[derive(Serialize, Deserialize)]
struct ConflictDeviceDto {
    device_id: DeviceId,
    display_name: String,
}

#[derive(Serialize, Deserialize)]
struct ConflictMemberDto {
    device: ConflictDeviceDto,
    active: bool,
}

#[derive(Serialize, Deserialize)]
enum ConflictReasonDto {
    Unknown,
    PendingRemoval,
    DifferentRemovals,
    RemovalDecisionDisagreement,
    DivergedHistory,
}

#[derive(Serialize, Deserialize)]
enum ChangeSideDto {
    Local,
    Remote,
}

#[derive(Serialize, Deserialize)]
enum ChangeKindDto {
    AddedDevice,
    RemovedDevice,
}

#[derive(Serialize, Deserialize)]
enum RemovalDecisionDto {
    Accept,
    Reject,
}

#[derive(Serialize, Deserialize)]
struct ChangeFactDto {
    side: ChangeSideDto,
    kind: ChangeKindDto,
    actor: ConflictDeviceDto,
    target: ConflictDeviceDto,
}

#[derive(Serialize, Deserialize)]
struct DecisionFactDto {
    device: ConflictDeviceDto,
    decision: RemovalDecisionDto,
    target: ConflictDeviceDto,
}

#[derive(Serialize, Deserialize)]
struct ConflictExplanationDto {
    reason: ConflictReasonDto,
    changes: Vec<ChangeFactDto>,
    decisions: Vec<DecisionFactDto>,
    details_complete: bool,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ConflictPresentationDto {
    local_branch_id: MembershipBranchId,
    remote_branch_id: MembershipBranchId,
    local_members: Vec<ConflictMemberDto>,
    remote_members: Vec<ConflictMemberDto>,
    explanation: ConflictExplanationDto,
}

#[derive(Serialize, Deserialize)]
enum RecoverySessionStateDto {
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

#[derive(Serialize, Deserialize)]
pub(super) struct RecoverySessionDto {
    transition_id: [u8; 32],
    conflict_id: MembershipConflictId,
    target_branch_id: MembershipBranchId,
    recipient_member: MemberInstanceId,
    state: RecoverySessionStateDto,
}

/// 分叉恢复资料的冻结布局；V4 以五个并列字段保存，V5 以本结构保存。
#[derive(Serialize, Deserialize, Default)]
pub(super) struct BranchRecoveryDto {
    pub(super) conflicts: BTreeMap<MembershipConflictId, ConflictRecordDto>,
    pub(super) branch_transitions: BTreeMap<[u8; 32], MembershipBranchTransitionV1>,
    pub(super) consumed_recovery_nonces: BTreeMap<[u8; 32], MembershipConflictId>,
    pub(super) recovery_sessions: BTreeMap<[u8; 32], RecoverySessionDto>,
    pub(super) conflict_presentations: BTreeMap<MembershipConflictId, ConflictPresentationDto>,
}

impl BranchRecoveryDto {
    pub(super) fn from_record(record: &MembershipBranchRecoveryRecord) -> Self {
        Self {
            conflicts: record
                .conflicts
                .iter()
                .map(|(id, conflict)| (*id, ConflictRecordDto::from_record(conflict)))
                .collect(),
            branch_transitions: record.branch_transitions.clone(),
            consumed_recovery_nonces: record.consumed_recovery_nonces.clone(),
            recovery_sessions: record
                .recovery_sessions
                .iter()
                .map(|(id, session)| (*id, RecoverySessionDto::from_session(session)))
                .collect(),
            conflict_presentations: record
                .conflict_presentations
                .iter()
                .map(|(id, presentation)| {
                    (
                        *id,
                        ConflictPresentationDto::from_presentation(presentation),
                    )
                })
                .collect(),
        }
    }

    pub(super) fn into_record(
        self,
    ) -> Result<MembershipBranchRecoveryRecord, MembershipLedgerError> {
        Ok(MembershipBranchRecoveryRecord {
            conflicts: self
                .conflicts
                .into_iter()
                .map(|(id, conflict)| (id, conflict.into_record()))
                .collect(),
            conflict_presentations: self
                .conflict_presentations
                .into_iter()
                .map(|(id, presentation)| (id, presentation.into_presentation()))
                .collect(),
            branch_transitions: self.branch_transitions,
            consumed_recovery_nonces: self.consumed_recovery_nonces,
            recovery_sessions: self
                .recovery_sessions
                .into_iter()
                .map(|(id, session)| {
                    let session = session.into_session()?;
                    if *session.transition_id() != id {
                        return Err(MembershipLedgerError::corrupt());
                    }
                    Ok((id, session))
                })
                .collect::<Result<_, MembershipLedgerError>>()?,
        })
    }
}

impl ConflictRecordDto {
    fn from_record(record: &MembershipConflictRecord) -> Self {
        Self {
            conflict_id: record.conflict_id,
            local_branch_id: record.local_branch_id,
            remote_branch_id: record.remote_branch_id,
            local_choice: ConflictChoiceDto::from_choice(record.local_choice),
            remote_choice: ConflictChoiceDto::from_choice(record.remote_choice),
            evidence_peer_device_ids: record.evidence_peer_device_ids.clone(),
            detected_at_revision: record.detected_at_revision,
            status: ConflictStatusDto::from_status(record.status),
            selected_branch_id: record.selected_branch_id,
            transition_id: record.transition_id,
        }
    }

    fn into_record(self) -> MembershipConflictRecord {
        MembershipConflictRecord {
            conflict_id: self.conflict_id,
            local_branch_id: self.local_branch_id,
            remote_branch_id: self.remote_branch_id,
            local_choice: self.local_choice.into_choice(),
            remote_choice: self.remote_choice.into_choice(),
            evidence_peer_device_ids: self.evidence_peer_device_ids,
            detected_at_revision: self.detected_at_revision,
            status: self.status.into_status(),
            selected_branch_id: self.selected_branch_id,
            transition_id: self.transition_id,
        }
    }
}

impl ConflictStatusDto {
    fn from_status(status: MembershipConflictStatus) -> Self {
        match status {
            MembershipConflictStatus::Unresolved => Self::Unresolved,
            MembershipConflictStatus::Selected => Self::Selected,
            MembershipConflictStatus::Transitioning => Self::Transitioning,
            MembershipConflictStatus::Completed => Self::Completed,
            MembershipConflictStatus::RePairingRequired => Self::RePairingRequired,
        }
    }

    fn into_status(self) -> MembershipConflictStatus {
        match self {
            Self::Unresolved => MembershipConflictStatus::Unresolved,
            Self::Selected => MembershipConflictStatus::Selected,
            Self::Transitioning => MembershipConflictStatus::Transitioning,
            Self::Completed => MembershipConflictStatus::Completed,
            Self::RePairingRequired => MembershipConflictStatus::RePairingRequired,
        }
    }
}

impl ConflictChoiceDto {
    fn from_choice(choice: MembershipConflictChoice) -> Self {
        match choice {
            MembershipConflictChoice::ActiveMemberRecovery => Self::ActiveMemberRecovery,
            MembershipConflictChoice::RePairingRequired => Self::RePairingRequired,
        }
    }

    fn into_choice(self) -> MembershipConflictChoice {
        match self {
            Self::ActiveMemberRecovery => MembershipConflictChoice::ActiveMemberRecovery,
            Self::RePairingRequired => MembershipConflictChoice::RePairingRequired,
        }
    }
}

impl ConflictDeviceDto {
    fn from_device(device: &MembershipConflictDevice) -> Self {
        Self {
            device_id: device.device_id,
            display_name: device.display_name.clone(),
        }
    }

    fn into_device(self) -> MembershipConflictDevice {
        MembershipConflictDevice {
            device_id: self.device_id,
            display_name: self.display_name,
        }
    }
}

impl ConflictMemberDto {
    fn from_member(member: &MembershipConflictMember) -> Self {
        Self {
            device: ConflictDeviceDto::from_device(&member.device),
            active: member.active,
        }
    }

    fn into_member(self) -> MembershipConflictMember {
        MembershipConflictMember {
            device: self.device.into_device(),
            active: self.active,
        }
    }
}

impl ConflictPresentationDto {
    fn from_presentation(presentation: &MembershipConflictPresentation) -> Self {
        let explanation = &presentation.explanation;
        Self {
            local_branch_id: presentation.local_branch_id,
            remote_branch_id: presentation.remote_branch_id,
            local_members: presentation
                .local_members
                .iter()
                .map(ConflictMemberDto::from_member)
                .collect(),
            remote_members: presentation
                .remote_members
                .iter()
                .map(ConflictMemberDto::from_member)
                .collect(),
            explanation: ConflictExplanationDto {
                reason: match explanation.reason {
                    MembershipConflictReason::Unknown => ConflictReasonDto::Unknown,
                    MembershipConflictReason::PendingRemoval => ConflictReasonDto::PendingRemoval,
                    MembershipConflictReason::DifferentRemovals => {
                        ConflictReasonDto::DifferentRemovals
                    }
                    MembershipConflictReason::RemovalDecisionDisagreement => {
                        ConflictReasonDto::RemovalDecisionDisagreement
                    }
                    MembershipConflictReason::DivergedHistory => ConflictReasonDto::DivergedHistory,
                },
                changes: explanation
                    .changes
                    .iter()
                    .map(|change| ChangeFactDto {
                        side: match change.side {
                            MembershipChangeSide::Local => ChangeSideDto::Local,
                            MembershipChangeSide::Remote => ChangeSideDto::Remote,
                        },
                        kind: match change.kind {
                            MembershipChangeKind::AddedDevice => ChangeKindDto::AddedDevice,
                            MembershipChangeKind::RemovedDevice => ChangeKindDto::RemovedDevice,
                        },
                        actor: ConflictDeviceDto::from_device(&change.actor),
                        target: ConflictDeviceDto::from_device(&change.target),
                    })
                    .collect(),
                decisions: explanation
                    .decisions
                    .iter()
                    .map(|decision| DecisionFactDto {
                        device: ConflictDeviceDto::from_device(&decision.device),
                        decision: match decision.decision {
                            RemovalDecision::Accept => RemovalDecisionDto::Accept,
                            RemovalDecision::Reject => RemovalDecisionDto::Reject,
                        },
                        target: ConflictDeviceDto::from_device(&decision.target),
                    })
                    .collect(),
                details_complete: explanation.details_complete,
            },
        }
    }

    fn into_presentation(self) -> MembershipConflictPresentation {
        let explanation = self.explanation;
        MembershipConflictPresentation {
            local_branch_id: self.local_branch_id,
            remote_branch_id: self.remote_branch_id,
            local_members: self
                .local_members
                .into_iter()
                .map(ConflictMemberDto::into_member)
                .collect(),
            remote_members: self
                .remote_members
                .into_iter()
                .map(ConflictMemberDto::into_member)
                .collect(),
            explanation: MembershipConflictExplanation {
                reason: match explanation.reason {
                    ConflictReasonDto::Unknown => MembershipConflictReason::Unknown,
                    ConflictReasonDto::PendingRemoval => MembershipConflictReason::PendingRemoval,
                    ConflictReasonDto::DifferentRemovals => {
                        MembershipConflictReason::DifferentRemovals
                    }
                    ConflictReasonDto::RemovalDecisionDisagreement => {
                        MembershipConflictReason::RemovalDecisionDisagreement
                    }
                    ConflictReasonDto::DivergedHistory => MembershipConflictReason::DivergedHistory,
                },
                changes: explanation
                    .changes
                    .into_iter()
                    .map(|change| MembershipChangeFact {
                        side: match change.side {
                            ChangeSideDto::Local => MembershipChangeSide::Local,
                            ChangeSideDto::Remote => MembershipChangeSide::Remote,
                        },
                        kind: match change.kind {
                            ChangeKindDto::AddedDevice => MembershipChangeKind::AddedDevice,
                            ChangeKindDto::RemovedDevice => MembershipChangeKind::RemovedDevice,
                        },
                        actor: change.actor.into_device(),
                        target: change.target.into_device(),
                    })
                    .collect(),
                decisions: explanation
                    .decisions
                    .into_iter()
                    .map(|decision| MembershipDecisionFact {
                        device: decision.device.into_device(),
                        decision: match decision.decision {
                            RemovalDecisionDto::Accept => RemovalDecision::Accept,
                            RemovalDecisionDto::Reject => RemovalDecision::Reject,
                        },
                        target: decision.target.into_device(),
                    })
                    .collect(),
                details_complete: explanation.details_complete,
            },
        }
    }
}

impl RecoverySessionDto {
    fn from_session(session: &MembershipBranchRecoverySession) -> Self {
        Self {
            transition_id: *session.transition_id(),
            conflict_id: session.conflict_id(),
            target_branch_id: session.target_branch_id(),
            recipient_member: session.recipient_member(),
            state: match session.state().clone() {
                MembershipBranchRecoverySessionState::RecipientPrepared {
                    external_commit,
                    recipient_staged_mls_state,
                } => RecoverySessionStateDto::RecipientPrepared {
                    external_commit,
                    recipient_staged_mls_state,
                },
                MembershipBranchRecoverySessionState::RecipientCompleted {
                    recipient_staged_mls_state,
                    recovery_package,
                } => RecoverySessionStateDto::RecipientCompleted {
                    recipient_staged_mls_state,
                    recovery_package,
                },
                MembershipBranchRecoverySessionState::TargetPrepared {
                    external_commit_digest,
                    target_staged_space_material,
                    recovery_package,
                } => RecoverySessionStateDto::TargetPrepared {
                    external_commit_digest,
                    target_staged_space_material,
                    recovery_package,
                },
                MembershipBranchRecoverySessionState::TargetCommitted {
                    external_commit_digest,
                    recovery_package,
                } => RecoverySessionStateDto::TargetCommitted {
                    external_commit_digest,
                    recovery_package,
                },
            },
        }
    }

    fn into_session(self) -> Result<MembershipBranchRecoverySession, MembershipLedgerError> {
        let state = match self.state {
            RecoverySessionStateDto::RecipientPrepared {
                external_commit,
                recipient_staged_mls_state,
            } => MembershipBranchRecoverySessionState::RecipientPrepared {
                external_commit,
                recipient_staged_mls_state,
            },
            RecoverySessionStateDto::RecipientCompleted {
                recipient_staged_mls_state,
                recovery_package,
            } => MembershipBranchRecoverySessionState::RecipientCompleted {
                recipient_staged_mls_state,
                recovery_package,
            },
            RecoverySessionStateDto::TargetPrepared {
                external_commit_digest,
                target_staged_space_material,
                recovery_package,
            } => MembershipBranchRecoverySessionState::TargetPrepared {
                external_commit_digest,
                target_staged_space_material,
                recovery_package,
            },
            RecoverySessionStateDto::TargetCommitted {
                external_commit_digest,
                recovery_package,
            } => MembershipBranchRecoverySessionState::TargetCommitted {
                external_commit_digest,
                recovery_package,
            },
        };
        MembershipBranchRecoverySession::restore(
            self.transition_id,
            self.conflict_id,
            self.target_branch_id,
            self.recipient_member,
            state,
        )
        .ok_or_else(MembershipLedgerError::corrupt)
    }
}
