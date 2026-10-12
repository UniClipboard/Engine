use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uc_core::error_class::ErrorClass;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    KeyEpochError, KeyEpochStateIssue, MemberEffectMaterial, UnfinishedMemberEffect,
};
use uc_observability_contract::uc_warn;

use super::{
    ApplyGroupUpdatePort, GroupUpdateApplyError, GroupUpdateReceiveOutcome,
    PrepareMembershipRemovalPort, PreparedRemoval, VerifiedHistoryGroupUpdate,
};
use crate::connectivity::ConnectionLease;
use crate::space::membership::{
    ApplyMembershipSecurityPort, HistorySynchronizer, MembershipEffectExecutionError,
    MembershipOwner,
};

pub(crate) struct GroupUpdateApplyCoordinator {
    pub(super) owner: Arc<MembershipOwner>,
    pub(super) inbound: Arc<dyn ApplyGroupUpdatePort>,
    effects: Arc<dyn ApplyMembershipSecurityPort>,
    pub(super) applying: Arc<Mutex<()>>,
    pub(super) history: Arc<HistorySynchronizer>,
    pub(super) schedule: Mutex<super::GroupUpdateRecoverySchedule>,
    pub(super) recovery_wake: AtomicBool,
}
impl GroupUpdateApplyCoordinator {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        inbound: Arc<dyn ApplyGroupUpdatePort>,
        effects: Arc<dyn ApplyMembershipSecurityPort>,
        history: Arc<HistorySynchronizer>,
    ) -> Self {
        Self {
            owner,
            inbound,
            effects,
            applying: Arc::new(Mutex::new(())),
            history,
            schedule: Mutex::new(Default::default()),
            recovery_wake: AtomicBool::new(false),
        }
    }
    pub(crate) async fn receive(
        &self,
        connection: &ConnectionLease,
        payload: &[u8],
    ) -> Result<GroupUpdateReceiveOutcome, anyhow::Error> {
        let _apply = self.applying.lock().await;
        connection.check_current().await?;
        let proof = self.proof(payload).await?;
        match self
            .inbound
            .apply(payload, proof.as_ref(), Some(connection))
            .await
        {
            Ok(_) => Ok(GroupUpdateReceiveOutcome::Applied),
            Err(GroupUpdateApplyError::Unavailable(source)) => {
                // 暂不可用保留本地来源；恢复需求由同一 owner 保存后再报告。
                uc_warn!(
                    error_kind = "group_update_unavailable",
                    error_class = source.class(),
                    "组更新暂不可用，登记成员恢复需求"
                );
                if matches!(
                    source,
                    KeyEpochError::StateIssue(
                        KeyEpochStateIssue::MissingPredecessor
                            | KeyEpochStateIssue::UnverifiedReplay
                    )
                ) {
                    if self.owner.request_group_update_recovery(payload).await? {
                        self.recovery_wake.store(true, Ordering::Release);
                    }
                }
                Ok(GroupUpdateReceiveOutcome::Unavailable(source))
            }
            Err(GroupUpdateApplyError::Local(source)) => Err(anyhow::Error::new(source)),
            Err(GroupUpdateApplyError::Rejected(source)) => {
                uc_warn!(
                    error_kind = "group_update_rejected",
                    error_class = source.class(),
                    "组更新被安全校验拒绝"
                );
                Ok(GroupUpdateReceiveOutcome::Rejected(source))
            }
        }
    }
    pub(super) async fn proof(
        &self,
        payload: &[u8],
    ) -> Result<Option<VerifiedHistoryGroupUpdate>, anyhow::Error> {
        let view = self.owner.load().await?;
        let Some(space) = view.space() else {
            return Ok(None);
        };
        let history = space.history();
        let mut head = history.current_head();
        let mut catalog_payloads = Vec::new();
        let mut found = false;
        while let Some(id) = head {
            // 激活基线有 head/snapshot，但不是普通事件；可信历史已由 owner 校验。
            let Some(event) = history.event(id) else {
                break;
            };
            if !event.security_update_payload.is_empty() {
                catalog_payloads.push(event.security_update_payload.clone());
                if event.security_update_payload == payload {
                    found = true;
                    break;
                }
            }
            head = event.parent_event_id;
        }
        if found {
            return Ok(Some(VerifiedHistoryGroupUpdate {
                payload: payload.to_vec(),
                space_id: SpaceId::from(history.lineage_id()),
                catalog_payloads,
                prepared_material: space
                    .prepared_group_updates()
                    .get(&<[u8; 32]>::from(Sha256::digest(payload)))
                    .cloned(),
            }));
        }
        Ok(None)
    }
}
#[async_trait]
impl ApplyMembershipSecurityPort for GroupUpdateApplyCoordinator {
    async fn apply_membership_security(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        let _apply = self.applying.lock().await;
        let event = match effect.material() {
            MemberEffectMaterial::Event(event)
            | MemberEffectMaterial::InitiatedRemoval { event, .. } => Some(event),
            MemberEffectMaterial::Decision(_) => None,
        };
        if let Some(event) = event.filter(|event| !event.security_update_payload.is_empty()) {
            let proof = self
                .proof(&event.security_update_payload)
                .await
                .map_err(|source| MembershipEffectExecutionError::Dependency { source })?
                .ok_or(MembershipEffectExecutionError::Corrupt)?;
            self.inbound
                .apply(&event.security_update_payload, Some(&proof), None)
                .await
                .map_err(|source| MembershipEffectExecutionError::Dependency {
                    source: anyhow::Error::new(source),
                })?;
            let digest: [u8; 32] = Sha256::digest(&event.security_update_payload).into();
            self.owner
                .commit(|draft| {
                    draft.prepared_group_updates_mut()?.remove(&digest);
                    Ok(())
                })
                .await
                .map_err(|source| MembershipEffectExecutionError::Dependency {
                    source: source.into(),
                })?;
            Ok(())
        } else {
            self.effects.apply_membership_security(effect).await
        }
    }
}

#[async_trait]
impl PrepareMembershipRemovalPort for GroupUpdateApplyCoordinator {
    async fn prepare(
        &self,
        target: &DeviceId,
        retained: &[DeviceId],
        now_ms: i64,
    ) -> Result<PreparedRemoval, anyhow::Error> {
        let guard = self.applying.clone().lock_owned().await;
        let update = self
            .inbound
            .prepare_removal(target, retained, now_ms)
            .await?;
        Ok(PreparedRemoval {
            update,
            _guard: Some(guard),
        })
    }
}
