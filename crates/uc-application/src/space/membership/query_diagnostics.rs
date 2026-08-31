use std::sync::Arc;

use thiserror::Error;
use uc_core::membership::{
    MembershipBranchId, MembershipBranchTransitionPhaseV1, MembershipConflictPolicy,
    MembershipEventId,
};

use super::{CurrentMemberSignaturePort, MembershipConflictStatus, MembershipLedger};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipDiagnosticsView {
    pub revision: u64,
    pub branch_id: MembershipBranchId,
    pub head_event_id: MembershipEventId,
    pub group_epoch: u64,
    pub effective_member_count: usize,
    pub pending_conflict_count: usize,
    pub pending_effect_count: usize,
    pub transition_phases: Vec<MembershipBranchTransitionPhaseV1>,
}

#[derive(Debug, Error)]
pub enum QueryMembershipDiagnosticsError {
    #[error("成员账本诊断读取失败")]
    Ledger {
        #[source]
        source: anyhow::Error,
    },
    #[error("成员安全状态诊断读取失败")]
    Security {
        #[source]
        source: anyhow::Error,
    },
    #[error("成员诊断状态无效")]
    InvalidState {
        #[source]
        source: anyhow::Error,
    },
}

pub(crate) struct QueryMembershipDiagnosticsUseCase {
    ledger: Arc<MembershipLedger>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
}

impl QueryMembershipDiagnosticsUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        signer: Arc<dyn CurrentMemberSignaturePort>,
    ) -> Self {
        Self { ledger, signer }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<MembershipDiagnosticsView, QueryMembershipDiagnosticsError> {
        let snapshot = self.ledger.load_verified().await.map_err(|error| {
            QueryMembershipDiagnosticsError::Ledger {
                source: anyhow::Error::new(error),
            }
        })?;
        let history =
            snapshot
                .history()
                .ok_or_else(|| QueryMembershipDiagnosticsError::InvalidState {
                    source: anyhow::anyhow!("current membership history is unavailable"),
                })?;
        let position = history.current_position().map_err(|error| {
            QueryMembershipDiagnosticsError::InvalidState {
                source: anyhow::Error::new(error),
            }
        })?;
        let head_event_id =
            position
                .event_id
                .ok_or_else(|| QueryMembershipDiagnosticsError::InvalidState {
                    source: anyhow::anyhow!("current membership head is unavailable"),
                })?;
        let branch_id = MembershipConflictPolicy::branch_id(history).map_err(|error| {
            QueryMembershipDiagnosticsError::InvalidState {
                source: anyhow::Error::new(error),
            }
        })?;
        let group_epoch = self.signer.current_member_epoch().await.map_err(|error| {
            QueryMembershipDiagnosticsError::Security {
                source: anyhow::Error::new(error),
            }
        })?;
        let record = snapshot.record();
        Ok(MembershipDiagnosticsView {
            revision: record.revision,
            branch_id,
            head_event_id,
            group_epoch,
            effective_member_count: history.active_members().len(),
            pending_conflict_count: record
                .membership_conflicts
                .values()
                .filter(|conflict| conflict.status != MembershipConflictStatus::Completed)
                .count(),
            pending_effect_count: record.pending_effects.len(),
            transition_phases: record
                .membership_branch_transitions
                .values()
                .map(|transition| transition.phase())
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::QueryMembershipDiagnosticsError;

    #[test]
    fn dependency_failure_keeps_stable_classification_and_source() {
        let error = QueryMembershipDiagnosticsError::Ledger {
            source: anyhow::anyhow!("ledger unavailable"),
        };

        assert!(matches!(
            error,
            QueryMembershipDiagnosticsError::Ledger { .. }
        ));
        assert!(error.source().is_some());
    }
}
