use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{AdmissionChangeFacts, VersionedMembershipHistory};

use super::{
    LoadedMembershipLedger, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    MembershipMaintenanceStepOutcome, ReconcileMembershipProjectionPort,
};

/// 已验证的当前成员资料计划；历史记录不直接拥有投影删除资格。
#[derive(Clone)]
pub struct MembershipProjectionPlan {
    pub expected_revision: u64,
    pub expected_history_digest: [u8; 32],
    pub local_device_id: DeviceId,
    pub members: Vec<AdmissionChangeFacts>,
    pub trusted_device_ids: BTreeSet<DeviceId>,
}

impl MembershipProjectionPlan {
    pub fn from_verified_history(
        record: &LoadedMembershipLedger,
        history: &VersionedMembershipHistory,
        expected_history_digest: [u8; 32],
    ) -> Result<Self, MembershipLedgerError> {
        let local_device_id = record
            .local_device_id
            .clone()
            .ok_or(MembershipLedgerError::Corrupt)?;
        let mut required = BTreeSet::from([local_device_id.clone()]);
        let mut trusted_device_ids = BTreeSet::new();
        for member in history.active_members() {
            let facts = history
                .admission_facts_for(member)
                .ok_or(MembershipLedgerError::Corrupt)?;
            required.insert(facts.device_id.clone());
            if facts.device_id != local_device_id {
                trusted_device_ids.insert(facts.device_id.clone());
            }
        }
        required.extend(
            record
                .peer_reconciliation
                .iter()
                .filter(|(_, peer)| !peer.restricted_delivery.is_empty())
                .map(|(id, _)| id.clone()),
        );
        required.extend(
            record
                .current_effects(history)
                .into_iter()
                .filter(|(_, effect)| effect.phase < MembershipEffectPhase::Activated)
                .flat_map(|(_, effect)| effect.affected_device_ids.iter().cloned()),
        );
        let ids: Vec<_> = required.into_iter().collect();
        let members = ids
            .iter()
            .map(|id| {
                let member = history
                    .member_for_device(id, &ids)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                history
                    .admission_facts_for(member)
                    .cloned()
                    .ok_or(MembershipLedgerError::Corrupt)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            expected_revision: record.revision,
            expected_history_digest,
            local_device_id,
            members,
            trusted_device_ids,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyMembershipProjectionError {
    #[error("membership projection plan is outdated")]
    Conflict,
    #[error("membership projection storage failed")]
    Dependency {
        #[source]
        source: anyhow::Error,
    },
}

/// 在同一事务中验证 ledger 版本并完整维护成员、可信身份和地址。
#[async_trait]
pub trait ApplyMembershipProjectionPort: Send + Sync {
    async fn apply_membership_projection(
        &self,
        plan: MembershipProjectionPlan,
    ) -> Result<(), ApplyMembershipProjectionError>;
}

pub(crate) struct ReconcileMembershipProjectionUseCase {
    ledger: Arc<MembershipLedger>,
    projection: Arc<dyn ApplyMembershipProjectionPort>,
}

impl ReconcileMembershipProjectionUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        projection: Arc<dyn ApplyMembershipProjectionPort>,
    ) -> Self {
        Self { ledger, projection }
    }
}

#[async_trait]
impl ReconcileMembershipProjectionPort for ReconcileMembershipProjectionUseCase {
    async fn reconcile_membership_projection(&self) -> MembershipMaintenanceStepOutcome {
        let snapshot = match self.ledger.load_verified().await {
            Ok(snapshot) => snapshot,
            Err(_) => return MembershipMaintenanceStepOutcome::Deferred,
        };
        let Some(history) = snapshot.history() else {
            return MembershipMaintenanceStepOutcome::Completed;
        };
        let Some(digest) = snapshot.history_digest() else {
            return MembershipMaintenanceStepOutcome::Corrupt;
        };
        let plan = match MembershipProjectionPlan::from_verified_history(
            snapshot.record(),
            history,
            digest,
        ) {
            Ok(plan) => plan,
            Err(_) => return MembershipMaintenanceStepOutcome::Corrupt,
        };
        match self.projection.apply_membership_projection(plan).await {
            Ok(()) => MembershipMaintenanceStepOutcome::Completed,
            Err(_) => MembershipMaintenanceStepOutcome::Deferred,
        }
    }
}
