use std::sync::Arc;

use sha2::{Digest, Sha256};
use uc_core::membership::MembershipConflictChoice;

use crate::space::membership::{MembershipConflictStatus, MembershipLedger, MembershipLedgerError};

use super::{
    QueryMembershipConflictStatusPort, ResolveMembershipConflictError,
    ResolveMembershipConflictInput, ResolveMembershipConflictResult,
};

pub(crate) struct ResolveMembershipConflictUseCase {
    ledger: Arc<MembershipLedger>,
    query: Arc<dyn QueryMembershipConflictStatusPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl ResolveMembershipConflictUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        query: Arc<dyn QueryMembershipConflictStatusPort>,
    ) -> Self {
        Self {
            ledger,
            query,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        input: ResolveMembershipConflictInput,
    ) -> Result<ResolveMembershipConflictResult, ResolveMembershipConflictError> {
        let _guard = self.execution_lock.lock().await;
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let Some(conflict) = snapshot
            .record()
            .membership_conflicts
            .get(&input.conflict_id)
        else {
            return Ok(ResolveMembershipConflictResult::StateChanged {
                current_conflict_id: current_conflict_id(snapshot.record()),
            });
        };
        let Some(choice) = conflict.choice_for(input.target_branch_id) else {
            return Err(ResolveMembershipConflictError::InvalidChoice);
        };
        if let Some(selected) = conflict.selected_branch_id {
            if selected != input.target_branch_id {
                return Ok(ResolveMembershipConflictResult::StateChanged {
                    current_conflict_id: current_conflict_id(snapshot.record()),
                });
            }
            return self.result_for_persisted_choice(conflict.status).await;
        }

        let target_is_local = input.target_branch_id == conflict.local_branch_id;
        let next_status = if target_is_local {
            MembershipConflictStatus::Completed
        } else if choice == MembershipConflictChoice::RePairingRequired {
            MembershipConflictStatus::RePairingRequired
        } else {
            MembershipConflictStatus::Selected
        };
        let transition_id = (next_status == MembershipConflictStatus::Selected)
            .then(|| transition_id(input.conflict_id, input.target_branch_id));
        let commit = self
            .ledger
            .compare_and_commit(|record| {
                let current = record
                    .membership_conflicts
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
            let latest = self
                .ledger
                .load_verified()
                .await
                .map_err(map_ledger_error)?;
            return Ok(ResolveMembershipConflictResult::StateChanged {
                current_conflict_id: current_conflict_id(latest.record()),
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
                let snapshot = self
                    .ledger
                    .load_verified()
                    .await
                    .map_err(map_ledger_error)?;
                Ok(ResolveMembershipConflictResult::RePairingRequired {
                    conflict_id: current_conflict_id(snapshot.record())
                        .ok_or_else(recovery_required)?,
                })
            }
            MembershipConflictStatus::Selected | MembershipConflictStatus::Transitioning => {
                let snapshot = self
                    .ledger
                    .load_verified()
                    .await
                    .map_err(map_ledger_error)?;
                Ok(ResolveMembershipConflictResult::Pending {
                    conflict_id: current_conflict_id(snapshot.record())
                        .ok_or_else(recovery_required)?,
                })
            }
            MembershipConflictStatus::Unresolved => Err(recovery_required()),
        }
    }
}

fn current_conflict_id(
    record: &crate::space::membership::LoadedMembershipLedger,
) -> Option<uc_core::membership::MembershipConflictId> {
    record
        .membership_conflicts
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
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            ResolveMembershipConflictError::RecoveryRequired {
                source: anyhow::Error::new(error),
            }
        }
        MembershipLedgerError::Unavailable => ResolveMembershipConflictError::TargetUnavailable {
            source: anyhow::Error::new(error),
        },
    }
}

fn recovery_required() -> ResolveMembershipConflictError {
    ResolveMembershipConflictError::RecoveryRequired {
        source: anyhow::Error::new(MembershipLedgerError::RecoveryRequired),
    }
}

fn transition_id(
    conflict_id: uc_core::membership::MembershipConflictId,
    target_branch_id: uc_core::membership::MembershipBranchId,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-branch-transition/v1\0");
    hasher.update(conflict_id.as_bytes());
    hasher.update(target_branch_id.as_bytes());
    hasher.finalize().into()
}
