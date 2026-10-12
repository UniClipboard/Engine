use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use uc_core::error_class::ErrorClass;
use uc_core::membership::{BaseMembershipHistoryPosition, VersionedMembershipHistory};
use uc_observability_contract::error_source::io_error_kind;

use super::{GroupUpdateApplyCoordinator, RecoverGroupUpdatesPort};
use crate::space::membership::{GroupUpdateRecoveryRecord, MembershipLedgerError, MembershipOwner};
use crate::space::membership::{MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger};
use uc_observability_contract::uc_warn;

#[derive(Deserialize)]
struct UpdateTarget {
    group_epoch: u64,
}

impl MembershipOwner {
    pub(crate) async fn request_group_update_recovery(
        &self,
        payload: &[u8],
    ) -> Result<bool, MembershipLedgerError> {
        let target: UpdateTarget =
            serde_json::from_slice(payload).map_err(MembershipLedgerError::corrupt_from)?;
        let digest = Sha256::digest(payload).into();
        let committed = self
            .commit(|draft| {
                let base_position = draft
                    .require_space()?
                    .history()
                    .current_position()
                    .map_err(MembershipLedgerError::corrupt_from)?;
                let current = draft.group_update_recovery_mut()?;
                let changed = current
                    .as_ref()
                    .is_none_or(|current| current.target_epoch < target.group_epoch);
                if changed {
                    *current = Some(GroupUpdateRecoveryRecord {
                        base_position,
                        target_epoch: target.group_epoch,
                        payload_digest: digest,
                    });
                }
                Ok(changed)
            })
            .await?;
        // 首次需求与重复需求合并；后续退避由原 worker 安排。
        if committed.output {
            self.schedule_worker_at(self.now_ms());
        }
        Ok(committed.output)
    }
}

/// 无进展时只推进有界 deadline；相同通知不能持续重置重试次数。
#[derive(Default)]
pub(crate) struct GroupUpdateRecoverySchedule {
    pub(crate) deadline_ms: Option<i64>,
    attempts: usize,
    progress_depth: Option<u64>,
    contact_wake_used: bool,
}
impl GroupUpdateRecoverySchedule {
    pub(crate) fn ready(&mut self, now_ms: i64, wake: bool) -> bool {
        if self.deadline_ms.is_none_or(|deadline| deadline <= now_ms) {
            self.contact_wake_used = false;
            return true;
        }
        if wake && !self.contact_wake_used {
            self.contact_wake_used = true;
            return true;
        }
        false
    }
    pub(crate) fn deferred(&mut self, now_ms: i64, progress_depth: u64) -> i64 {
        if self
            .progress_depth
            .is_some_and(|last| progress_depth > last)
        {
            self.attempts = 0;
        }
        self.progress_depth = Some(progress_depth);
        let seconds = [2, 4, 8, 16, 30][self.attempts.min(4)];
        self.attempts = self.attempts.saturating_add(1);
        let deadline = now_ms.saturating_add(seconds * 1000);
        self.deadline_ms = Some(deadline);
        deadline
    }
}

#[async_trait]
impl RecoverGroupUpdatesPort for GroupUpdateApplyCoordinator {
    async fn recover(
        &self,
        trigger: &MembershipMaintenanceTrigger,
        verified_contact: bool,
    ) -> MembershipMaintenanceStepOutcome {
        match self.recover_inner(trigger, verified_contact).await {
            Ok(outcome) => outcome,
            Err(source) => {
                uc_warn!(
                    error_kind = "group_update_recovery",
                    io_error_kind = io_error_kind(source.as_ref()),
                    "组更新主动恢复延后"
                );
                MembershipMaintenanceStepOutcome::Deferred
            }
        }
    }
}
impl GroupUpdateApplyCoordinator {
    async fn recover_inner(
        &self,
        trigger: &MembershipMaintenanceTrigger,
        verified_contact: bool,
    ) -> Result<MembershipMaintenanceStepOutcome, anyhow::Error> {
        let view = self.owner.load().await?;
        let Some(space) = view.space() else {
            *self.schedule.lock().await = Default::default();
            return Ok(MembershipMaintenanceStepOutcome::Completed);
        };
        let Some(recovery) = space.group_update_recovery().cloned() else {
            *self.schedule.lock().await = Default::default();
            return Ok(MembershipMaintenanceStepOutcome::Completed);
        };
        let mut schedule = self.schedule.lock().await;
        let now = self.owner.now_ms();
        let resumed = matches!(
            trigger,
            MembershipMaintenanceTrigger::Startup | MembershipMaintenanceTrigger::Resume
        );
        let depth = space.history().current_position()?.depth;
        let progressed = schedule
            .progress_depth
            .is_some_and(|previous| depth > previous);
        let new_request = self.recovery_wake.swap(false, Ordering::AcqRel);
        if !new_request && !progressed && !resumed && !schedule.ready(now, verified_contact) {
            if let Some(deadline) = schedule.deadline_ms {
                self.owner.schedule_worker_at(deadline);
            }
            return Ok(MembershipMaintenanceStepOutcome::Deferred);
        }
        if !recovery_follows_branch(space.history(), &recovery.base_position) {
            self.owner
                .commit(|draft| {
                    if draft.require_space()?.group_update_recovery() == Some(&recovery) {
                        *draft.group_update_recovery_mut()? = None;
                    }
                    Ok(())
                })
                .await?;
            *schedule = Default::default();
            return Ok(MembershipMaintenanceStepOutcome::Completed);
        }
        let peers = space
            .history()
            .effective_members()
            .into_iter()
            .filter_map(|member| space.history().admission_facts_for(member))
            .map(|facts| facts.device_id)
            .filter(|device| {
                device != space.local_device_id() && space.ledger().admits_inbound_peer(device)
            })
            .take(8)
            .collect();
        // 沿用原成员历史负责人、签名/祖先验证与逐页 /4 交换。
        if let Err(source) = self.history.synchronize(peers).await {
            let deadline = schedule.deferred(self.owner.now_ms(), depth);
            self.owner.schedule_worker_at(deadline);
            return Err(anyhow::Error::new(source));
        }
        let current = self.owner.load().await?;
        let Some(space) = current.space() else {
            return Ok(MembershipMaintenanceStepOutcome::Deferred);
        };
        let mut head = space.history().current_head();
        let mut updates = Vec::new();
        while let Some(id) = head {
            let Some(event) = space.history().event(id) else {
                // 激活基线不是普通事件；从此处开始没有更早 MLS 更新。
                break;
            };
            if !event.security_update_payload.is_empty() {
                let target: UpdateTarget = serde_json::from_slice(&event.security_update_payload)?;
                if target.group_epoch <= recovery.target_epoch
                    && target.group_epoch >= recovery.target_epoch.saturating_sub(64)
                {
                    updates.push((target.group_epoch, event.security_update_payload.clone()));
                }
            }
            head = event.parent_event_id;
        }
        // 未验证请求的 digest 只用于合并诊断，不能授权历史选择。
        // 从可信祖先开始逐个应用前置更新；正确历史不必与最初的坏包相等。
        updates.reverse();
        let _apply = self.applying.lock().await;
        let mut applied_target = false;
        for (epoch, payload) in updates {
            let proof = self
                .proof(&payload)
                .await?
                .ok_or(MembershipLedgerError::RecoveryRequired)?;
            match self.inbound.apply(&payload, Some(&proof), None).await {
                Ok(_) => applied_target |= epoch == recovery.target_epoch,
                Err(source) => {
                    uc_warn!(
                        error_kind = "group_update_recovery_apply",
                        error_class = source.class(),
                        "已验证历史的安全效果仍待应用"
                    );
                    break;
                }
            }
        }
        if applied_target {
            self.owner
                .commit(|draft| {
                    if draft.require_space()?.group_update_recovery() == Some(&recovery) {
                        *draft.group_update_recovery_mut()? = None;
                    }
                    Ok(())
                })
                .await?;
            *schedule = Default::default();
            return Ok(MembershipMaintenanceStepOutcome::Completed);
        }
        let depth = space.history().current_position()?.depth;
        let deadline = schedule.deferred(self.owner.now_ms(), depth);
        self.owner.schedule_worker_at(deadline);
        Ok(MembershipMaintenanceStepOutcome::Deferred)
    }
}

/// 只比较当前可信 parent 链；另一分支仍在事件表中不代表它是当前祖先。
fn recovery_follows_branch(
    history: &VersionedMembershipHistory,
    base: &BaseMembershipHistoryPosition,
) -> bool {
    let Some(base_id) = base.event_id else {
        return history.current_head().is_none();
    };
    let mut head = history.current_head();
    while let Some(id) = head {
        if id == base_id {
            return true;
        }
        let Some(event) = history.event(id) else {
            break;
        };
        head = event.parent_event_id;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::GroupUpdateRecoverySchedule;

    #[test]
    fn no_progress_uses_bounded_backoff_and_coalesces_contact_wakes() {
        let mut schedule = GroupUpdateRecoverySchedule::default();
        assert!(schedule.ready(0, false));
        assert_eq!(schedule.deferred(0, 3), 2000);
        assert!(schedule.ready(1, true));
        assert_eq!(schedule.deferred(1, 3), 4001);
        assert!(!schedule.ready(2, true));
        assert!(!schedule.ready(4000, false));
        assert!(schedule.ready(4001, false));
        assert_eq!(schedule.deferred(4001, 3), 12001);
        assert_eq!(schedule.deferred(12001, 3), 28001);
        assert_eq!(schedule.deferred(28001, 3), 58001);
        assert_eq!(schedule.deferred(58001, 3), 88001);
        assert_eq!(schedule.deferred(88001, 4), 90001);
    }
}
