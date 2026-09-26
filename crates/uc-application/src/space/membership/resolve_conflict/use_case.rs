use std::sync::Arc;

use uc_core::membership::{
    MembershipBranchTransitionV1, MembershipConflictChoice, MembershipConflictPolicy,
};

use crate::space::membership::{
    MembershipBranchRecoveryRecord, MembershipConflictStatus, MembershipLedgerError,
    MembershipOwner, MembershipView,
};

use super::{
    MembershipConflictView, MembershipConflictsView, QueryMembershipConflictStatusPort,
    QueryMembershipConflictsError, ResolveMembershipConflictError, ResolveMembershipConflictInput,
    ResolveMembershipConflictResult,
};

pub(crate) struct ResolveMembershipConflictUseCase {
    owner: Arc<MembershipOwner>,
    query: Arc<dyn QueryMembershipConflictStatusPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl ResolveMembershipConflictUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        query: Arc<dyn QueryMembershipConflictStatusPort>,
    ) -> Self {
        Self {
            owner,
            query,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        input: ResolveMembershipConflictInput,
    ) -> Result<ResolveMembershipConflictResult, ResolveMembershipConflictError> {
        let _guard = self.execution_lock.lock().await;
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let space = view.require_space().map_err(map_ledger_error)?;
        let Some(conflict) = space.branch_recovery().conflicts.get(&input.conflict_id) else {
            return Ok(ResolveMembershipConflictResult::StateChanged {
                current_conflict_id: current_conflict_id(space.branch_recovery()),
            });
        };
        let Some(choice) = conflict.choice_for(input.target_branch_id) else {
            return Err(ResolveMembershipConflictError::InvalidChoice);
        };
        if let Some(selected) = conflict.selected_branch_id {
            if selected != input.target_branch_id {
                return Ok(ResolveMembershipConflictResult::StateChanged {
                    current_conflict_id: current_conflict_id(space.branch_recovery()),
                });
            }
            return self.result_for_persisted_choice(conflict.status).await;
        }

        if MembershipConflictPolicy::branch_id(space.history()).ok()
            != Some(conflict.local_branch_id)
        {
            return Ok(ResolveMembershipConflictResult::StateChanged {
                current_conflict_id: None,
            });
        }

        let target_is_local = input.target_branch_id == conflict.local_branch_id;
        let next_status = if target_is_local {
            MembershipConflictStatus::Completed
        } else if choice == MembershipConflictChoice::RePairingRequired {
            MembershipConflictStatus::RePairingRequired
        } else {
            MembershipConflictStatus::Selected
        };
        let transition_id = (next_status == MembershipConflictStatus::Selected).then(|| {
            MembershipBranchTransitionV1::derive_id(input.conflict_id, input.target_branch_id)
        });
        let commit = self
            .owner
            .commit(|draft| {
                let current = draft
                    .branch_recovery_mut()?
                    .conflicts
                    .get_mut(&input.conflict_id)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if current.selected_branch_id.is_some() {
                    return Err(MembershipLedgerError::Conflict);
                }
                current.selected_branch_id = Some(input.target_branch_id);
                current.status = next_status;
                current.transition_id = transition_id;
                Ok(())
            })
            .await;
        if matches!(commit, Err(MembershipLedgerError::Conflict)) {
            let latest = self.owner.load().await.map_err(map_ledger_error)?;
            return Ok(ResolveMembershipConflictResult::StateChanged {
                current_conflict_id: latest
                    .space()
                    .and_then(|space| current_conflict_id(space.branch_recovery())),
            });
        }
        commit.map_err(map_ledger_error)?;

        if next_status == MembershipConflictStatus::Completed {
            let status = self.query.query_status().await.map_err(|error| {
                ResolveMembershipConflictError::CommittedButPending {
                    source: anyhow::Error::new(error),
                }
            })?;
            Ok(ResolveMembershipConflictResult::Completed { status })
        } else {
            self.result_for_persisted_choice(next_status).await
        }
    }

    pub(crate) async fn query(
        &self,
    ) -> Result<MembershipConflictsView, QueryMembershipConflictsError> {
        let view = self.owner.load().await.map_err(|error| match error {
            MembershipLedgerError::Locked => QueryMembershipConflictsError::Locked {
                source: anyhow::Error::new(error),
            },
            MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
                QueryMembershipConflictsError::RecoveryRequired {
                    source: anyhow::Error::new(error),
                }
            }
            MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable { .. } => {
                QueryMembershipConflictsError::Unavailable {
                    source: anyhow::Error::new(error),
                }
            }
        })?;
        self.query_view(&view)
    }

    pub(crate) fn query_view(
        &self,
        view: &MembershipView,
    ) -> Result<MembershipConflictsView, QueryMembershipConflictsError> {
        let Some(space) = view
            .space()
            .filter(|space| !space.branch_recovery().conflicts.is_empty())
        else {
            return Ok(MembershipConflictsView {
                revision: view.revision(),
                conflicts: Vec::new(),
            });
        };
        let record = space.branch_recovery();
        let history = space.history();
        let scope = view.current_scope().map_err(|source| {
            QueryMembershipConflictsError::RecoveryRequired {
                source: anyhow::Error::new(source),
            }
        })?;
        let current_branch = MembershipConflictPolicy::branch_id(history).ok();
        let conflicts = record
            .conflicts
            .values()
            // 已提交恢复继续按原授权推进；过期的未选择快照等待新证据，不再提供旧选项。
            .filter(|conflict| {
                Some(conflict.local_branch_id) == current_branch
                    || conflict.selected_branch_id.is_some()
            })
            .map(|conflict| {
                let presentation = record.conflict_presentations.get(&conflict.conflict_id);
                Ok(MembershipConflictView {
                    conflict_id: conflict.conflict_id,
                    status: conflict.status,
                    selected_branch_id: conflict.selected_branch_id,
                    transition_phase: conflict
                        .transition_id
                        .and_then(|transition_id| record.branch_transitions.get(&transition_id))
                        .map(|transition| transition.phase()),
                    detected_at_revision: conflict.detected_at_revision,
                    evidence_peer_count: conflict.evidence_peer_device_ids.len(),
                    branches: [
                        super::presentation::branch_view(
                            conflict,
                            true,
                            presentation,
                            space.local_device_id(),
                            history,
                            &scope,
                        )?,
                        super::presentation::branch_view(
                            conflict,
                            false,
                            presentation,
                            space.local_device_id(),
                            history,
                            &scope,
                        )?,
                    ],
                    local_resolution_completed: conflict.status
                        == MembershipConflictStatus::Completed,
                    explanation: presentation
                        .map(|data| data.explanation.clone())
                        .unwrap_or_else(
                            uc_core::membership::MembershipConflictExplanation::unknown,
                        ),
                })
            })
            .collect::<Result<Vec<_>, MembershipLedgerError>>()
            .map_err(|source| QueryMembershipConflictsError::RecoveryRequired {
                source: anyhow::Error::new(source),
            })?;
        Ok(MembershipConflictsView {
            revision: view.revision(),
            conflicts,
        })
    }

    async fn result_for_persisted_choice(
        &self,
        status: MembershipConflictStatus,
    ) -> Result<ResolveMembershipConflictResult, ResolveMembershipConflictError> {
        match status {
            MembershipConflictStatus::Completed => {
                let status = self.query.query_status().await.map_err(|error| {
                    ResolveMembershipConflictError::CommittedButPending {
                        source: anyhow::Error::new(error),
                    }
                })?;
                Ok(ResolveMembershipConflictResult::AlreadyCompleted { status })
            }
            MembershipConflictStatus::RePairingRequired => {
                Ok(ResolveMembershipConflictResult::RePairingRequired {
                    conflict_id: self.current_conflict().await?,
                })
            }
            MembershipConflictStatus::Selected | MembershipConflictStatus::Transitioning => {
                Ok(ResolveMembershipConflictResult::Pending {
                    conflict_id: self.current_conflict().await?,
                })
            }
            MembershipConflictStatus::Unresolved => Err(recovery_required()),
        }
    }
}

impl ResolveMembershipConflictUseCase {
    async fn current_conflict(
        &self,
    ) -> Result<uc_core::membership::MembershipConflictId, ResolveMembershipConflictError> {
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        view.space()
            .and_then(|space| current_conflict_id(space.branch_recovery()))
            .ok_or_else(recovery_required)
    }
}

fn current_conflict_id(
    record: &MembershipBranchRecoveryRecord,
) -> Option<uc_core::membership::MembershipConflictId> {
    record
        .conflicts
        .values()
        .find(|conflict| conflict.status != MembershipConflictStatus::Completed)
        .map(|conflict| conflict.conflict_id)
}

fn map_ledger_error(error: MembershipLedgerError) -> ResolveMembershipConflictError {
    match error {
        MembershipLedgerError::Locked => ResolveMembershipConflictError::Locked {
            source: anyhow::Error::new(error),
        },
        MembershipLedgerError::Conflict => ResolveMembershipConflictError::TargetUnavailable {
            source: anyhow::Error::new(error),
        },
        MembershipLedgerError::Corrupt { .. } | MembershipLedgerError::RecoveryRequired => {
            ResolveMembershipConflictError::RecoveryRequired {
                source: anyhow::Error::new(error),
            }
        }
        MembershipLedgerError::Unavailable { .. } => {
            ResolveMembershipConflictError::TargetUnavailable {
                source: anyhow::Error::new(error),
            }
        }
    }
}

fn recovery_required() -> ResolveMembershipConflictError {
    ResolveMembershipConflictError::RecoveryRequired {
        source: anyhow::Error::new(MembershipLedgerError::RecoveryRequired),
    }
}
