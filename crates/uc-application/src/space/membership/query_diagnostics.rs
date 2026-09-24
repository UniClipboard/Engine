use std::sync::Arc;

use thiserror::Error;
use uc_core::membership::{
    MembershipBranchId, MembershipBranchTransitionPhaseV1, MembershipConflictPolicy,
    MembershipEventId,
};

use uc_core::membership::{PeerLink, PeerRelation};

use super::{CurrentMemberSignaturePort, MembershipConflictStatus, MembershipOwner};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipDiagnosticsView {
    pub revision: u64,
    pub branch_id: MembershipBranchId,
    pub head_event_id: MembershipEventId,
    pub group_epoch: u64,
    pub effective_member_count: usize,
    pub pending_conflict_count: usize,
    pub pending_confirmation_count: usize,
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
    owner: Arc<MembershipOwner>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
}

impl QueryMembershipDiagnosticsUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        signer: Arc<dyn CurrentMemberSignaturePort>,
    ) -> Self {
        Self { owner, signer }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<MembershipDiagnosticsView, QueryMembershipDiagnosticsError> {
        let view =
            self.owner
                .load()
                .await
                .map_err(|error| QueryMembershipDiagnosticsError::Ledger {
                    source: anyhow::Error::new(error),
                })?;
        let space = view
            .space()
            .ok_or_else(|| QueryMembershipDiagnosticsError::InvalidState {
                source: anyhow::anyhow!("current membership history is unavailable"),
            })?;
        let history = space.history();
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
        let record = space.branch_recovery();
        Ok(MembershipDiagnosticsView {
            revision: view.revision(),
            branch_id,
            head_event_id,
            group_epoch,
            effective_member_count: history.active_members().len(),
            pending_conflict_count: record
                .conflicts
                .values()
                .filter(|conflict| conflict.status != MembershipConflictStatus::Completed)
                .count(),
            pending_confirmation_count: space
                .ledger()
                .peers()
                // 仍在等待确认的对端：一致但尚未确认本机当前位置，或一方仍在决定一项移除。
                .filter(|(_, link)| {
                    matches!(link, PeerLink::Member(member)
                    if (member.relation() == PeerRelation::Consistent
                        && member.confirmed_position() != Some(&position))
                        || matches!(
                            member.relation(),
                            PeerRelation::AwaitingLocalDecision
                                | PeerRelation::AwaitingPeerDecision
                        ))
                })
                .count(),
            pending_effect_count: space.ledger().unfinished_effects().count(),
            transition_phases: record
                .branch_transitions
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
