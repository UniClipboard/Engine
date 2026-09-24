use std::sync::Arc;

use uc_core::membership::{
    LedgerMemberStatus, MembershipAdmissionDecision, PeerLink, PeerRelation,
};

use crate::space::membership::{MembershipLedgerError, MembershipOwner};

use super::{MembershipAdmissionSnapshot, QueryMembershipAdmissionError};

#[async_trait::async_trait]
pub trait QueryMembershipAdmissionPort: Send + Sync {
    async fn query_membership_admission(
        &self,
        invitation_generation: Option<u64>,
    ) -> Result<MembershipAdmissionSnapshot, QueryMembershipAdmissionError>;
}

pub(crate) struct QueryMembershipAdmissionUseCase {
    owner: Arc<MembershipOwner>,
}

impl QueryMembershipAdmissionUseCase {
    pub(crate) fn new(owner: Arc<MembershipOwner>) -> Self {
        Self { owner }
    }
}

#[async_trait::async_trait]
impl QueryMembershipAdmissionPort for QueryMembershipAdmissionUseCase {
    async fn query_membership_admission(
        &self,
        invitation_generation: Option<u64>,
    ) -> Result<MembershipAdmissionSnapshot, QueryMembershipAdmissionError> {
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let current_generation = view.revision();
        let space = view.require_space().map_err(map_ledger_error)?;
        let ledger = space.ledger();
        let history = space.history();
        let local_member = space.local_member();
        let active_peer_device_ids = history
            .active_members()
            .iter()
            .filter(|member| **member != local_member)
            .filter_map(|member| history.admission_facts_for(*member))
            .map(|facts| facts.device_id)
            .collect::<std::collections::BTreeSet<_>>();
        // 只看当前已激活成员的关系与未完成效果；已不在当前历史中的旧关系不阻塞邀请。
        let decision =
            if invitation_generation.is_some_and(|generation| generation != current_generation) {
                MembershipAdmissionDecision::SupersededInvitation
            } else if ledger.local_status() != LedgerMemberStatus::Active {
                MembershipAdmissionDecision::RecoveryRequired
            } else if ledger.peers().any(|(device_id, link)| {
                active_peer_device_ids.contains(device_id)
                    && !matches!(link, PeerLink::Member(member) if member.relation() == PeerRelation::Consistent)
            }) || ledger.unfinished_effects().next().is_some()
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
