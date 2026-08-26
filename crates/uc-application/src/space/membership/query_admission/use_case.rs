use std::sync::Arc;

use uc_core::membership::{MembershipAdmissionDecision, MembershipHistoryRelationship};

use crate::space::membership::{MembershipEffectPhase, MembershipLedger, MembershipLedgerError};

use super::{MembershipAdmissionSnapshot, QueryMembershipAdmissionError};

#[async_trait::async_trait]
pub trait QueryMembershipAdmissionPort: Send + Sync {
    async fn query_membership_admission(
        &self,
        invitation_generation: Option<u64>,
    ) -> Result<MembershipAdmissionSnapshot, QueryMembershipAdmissionError>;
}

pub(crate) struct QueryMembershipAdmissionUseCase {
    ledger: Arc<MembershipLedger>,
}

impl QueryMembershipAdmissionUseCase {
    pub(crate) fn new(ledger: Arc<MembershipLedger>) -> Self {
        Self { ledger }
    }
}

#[async_trait::async_trait]
impl QueryMembershipAdmissionPort for QueryMembershipAdmissionUseCase {
    async fn query_membership_admission(
        &self,
        invitation_generation: Option<u64>,
    ) -> Result<MembershipAdmissionSnapshot, QueryMembershipAdmissionError> {
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        let current_generation = snapshot.record().revision;
        let history = snapshot
            .history()
            .ok_or(QueryMembershipAdmissionError::RecoveryRequired)?;
        let local_member = snapshot
            .record()
            .local_member_instance
            .ok_or(QueryMembershipAdmissionError::RecoveryRequired)?;
        let active_peer_device_ids = history
            .active_members()
            .iter()
            .filter(|member| **member != local_member)
            .filter_map(|member| history.admission_facts_for(*member))
            .map(|facts| facts.device_id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let decision =
            if invitation_generation.is_some_and(|generation| generation != current_generation) {
                MembershipAdmissionDecision::SupersededInvitation
            } else if !snapshot.record().local_join_active
                || !history.active_members().contains(&local_member)
            {
                MembershipAdmissionDecision::RecoveryRequired
            } else if snapshot.record().peer_reconciliation.values().any(|peer| {
                active_peer_device_ids.contains(&peer.peer_device_id)
                    && !matches!(peer.relationship, MembershipHistoryRelationship::Consistent)
            }) || snapshot
                .record()
                .pending_effects
                .values()
                .any(|effect| effect.phase < MembershipEffectPhase::Activated)
            {
                MembershipAdmissionDecision::AwaitingConvergence
            } else {
                MembershipAdmissionDecision::Allowed
            };
        Ok(MembershipAdmissionSnapshot {
            current_generation,
            decision,
        })
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> QueryMembershipAdmissionError {
    match error {
        MembershipLedgerError::Locked => QueryMembershipAdmissionError::Locked,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            QueryMembershipAdmissionError::RecoveryRequired
        }
        MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable => {
            QueryMembershipAdmissionError::Unavailable
        }
    }
}
