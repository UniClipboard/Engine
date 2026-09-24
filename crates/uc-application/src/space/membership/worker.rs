//! 成员待办执行器。
//!
//! 执行器不决定做什么：成员账本的 `outstanding_work` 给出全部持久待办及最早执行时间，执行器只取已
//! 到期的项调用传输与安全能力，把结果作为输入交回 Owner，并按剩余项的最早时间安排下次唤醒。分叉
//! 恢复与组密钥投递由各自的持久资料登记，执行器一并调度。同一次运行中每项待办最多尝试一次，未完成的
//! 项留给下一次触发。

mod effects;
mod history_sync;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    LedgerDeliveryKind, LedgerDeliveryResult, LedgerInput, LedgerWork, MemberEffectPhase,
    MembershipEventId, MembershipHistoryExchangePort, ScheduledLedgerWork,
};
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};
use uc_observability_contract::diagnostics::{
    scope_membership_recovery_trigger, MembershipRecoveryTrigger,
};

use super::{
    ledger_error, ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort,
    ApplyMembershipSecurityPort, DeliverPendingGroupUpdatesPort, MembershipEffectExecutionError,
    MembershipLedgerError, MembershipMaintenanceReport, MembershipMaintenanceStepOutcome,
    MembershipMaintenanceTrigger, MembershipOwner, RecoverMembershipConflictsPort,
    RecoverMembershipEffectsPort, RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, RunMembershipWorkPort,
};

pub(crate) use effects::RePairingAwareMembershipActivation;
pub use history_sync::RefreshVerifiedPeerAddressPort;

use effects::MembershipEffectSteps;
use history_sync::{HistorySynchronizer, MAX_PEERS_PER_ROUND};

/// 单次运行最多推进的轮数；每轮至少完成一项新待办，上限只防止异常状态下的无界循环。
const MAX_PASSES_PER_RUN: usize = 256;

pub(crate) struct MembershipWorkerDeps {
    pub(crate) member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    pub(crate) security: Arc<dyn ApplyMembershipSecurityPort>,
    pub(crate) activation: Arc<dyn ActivateMembershipEffectPort>,
    pub(crate) restricted_delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
    pub(crate) history_transport: Arc<dyn MembershipHistoryExchangePort>,
    pub(crate) address_refresh: Arc<dyn RefreshVerifiedPeerAddressPort>,
    pub(crate) conflicts: Arc<dyn RecoverMembershipConflictsPort>,
    pub(crate) group_updates: Arc<dyn DeliverPendingGroupUpdatesPort>,
}

pub(crate) struct MembershipWorker {
    owner: Arc<MembershipOwner>,
    effects: MembershipEffectSteps,
    restricted_delivery: Arc<dyn RestrictedMembershipDeliveryPort>,
    history: HistorySynchronizer,
    conflicts: Arc<dyn RecoverMembershipConflictsPort>,
    group_updates: Arc<dyn DeliverPendingGroupUpdatesPort>,
    ledger_work: tokio::sync::Mutex<()>,
}

/// 同一次运行中已尝试过的待办。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AttemptedWork {
    Effect(MembershipEventId, MemberEffectPhase),
    RemovalNotice(DeviceId),
    Departure(DeviceId),
    Decision(DeviceId),
    HistorySync(DeviceId),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkScope {
    EffectsOnly,
    All,
}

impl MembershipWorker {
    pub(crate) fn new(owner: Arc<MembershipOwner>, deps: MembershipWorkerDeps) -> Self {
        Self {
            effects: MembershipEffectSteps::new(deps.member_facts, deps.security, deps.activation),
            restricted_delivery: deps.restricted_delivery,
            history: HistorySynchronizer::new(
                Arc::clone(&owner),
                deps.history_transport,
                deps.address_refresh,
            ),
            conflicts: deps.conflicts,
            group_updates: deps.group_updates,
            owner,
            ledger_work: tokio::sync::Mutex::new(()),
        }
    }

    async fn run_ledger_work(&self, scope: WorkScope) -> MembershipMaintenanceStepOutcome {
        // 效果各步按事件幂等，账本对重复完成报告返回过期结果，因此用户动作推进效果时不等待网络待办。
        let _guard = match scope {
            WorkScope::All => Some(self.ledger_work.lock().await),
            WorkScope::EffectsOnly => None,
        };
        let mut tally = Tally::default();
        let mut attempted = BTreeSet::new();
        for _ in 0..MAX_PASSES_PER_RUN {
            let due = match self.due_work(scope, &attempted).await {
                Ok(Some(due)) => due,
                Ok(None) => break,
                Err(error) => {
                    tally.record_ledger_error(error);
                    break;
                }
            };
            if let Err(error) = self.execute(due, &mut attempted, &mut tally).await {
                tally.record_ledger_error(error);
                break;
            }
        }
        tally.outcome()
    }

    /// 本轮尚未尝试的已到期待办；没有时安排下次唤醒并返回 `None`。
    async fn due_work(
        &self,
        scope: WorkScope,
        attempted: &BTreeSet<AttemptedWork>,
    ) -> Result<Option<Vec<ScheduledLedgerWork>>, MembershipLedgerError> {
        let view = self.owner.load().await?;
        let Some(space) = view.space() else {
            return Ok(None);
        };
        let now_ms = self.owner.now_ms();
        let work = space
            .ledger()
            .outstanding_work(now_ms)
            .map_err(|_| MembershipLedgerError::Corrupt)?;
        let due: Vec<ScheduledLedgerWork> = work
            .iter()
            .filter(|item| item.due_at_ms <= now_ms)
            .filter(|item| {
                scope == WorkScope::All || matches!(item.work, LedgerWork::AdvanceEffect(_))
            })
            .filter(|item| !attempted.contains(&attempted_work(&item.work)))
            .cloned()
            .collect();
        if due.is_empty() {
            if scope == WorkScope::All {
                if let Some(next) = work
                    .iter()
                    .map(|item| item.due_at_ms)
                    .filter(|due_at_ms| *due_at_ms > now_ms)
                    .min()
                {
                    self.owner.schedule_worker_at(next);
                }
            }
            return Ok(None);
        }
        Ok(Some(due))
    }

    /// 执行一批到期待办。效果按因果顺序每次只推进一步，完成后重新计算待办；其余待办整批执行。
    async fn execute(
        &self,
        due: Vec<ScheduledLedgerWork>,
        attempted: &mut BTreeSet<AttemptedWork>,
        tally: &mut Tally,
    ) -> Result<(), MembershipLedgerError> {
        if let Some(effect) = due.iter().find_map(|item| match &item.work {
            LedgerWork::AdvanceEffect(effect) => Some(effect.clone()),
            _ => None,
        }) {
            attempted.insert(AttemptedWork::Effect(effect.event_id(), effect.phase()));
            match self.effects.run(&effect).await {
                Ok(()) => {
                    self.owner
                        .commit(|draft| {
                            draft
                                .apply(LedgerInput::EffectStepFinished {
                                    event_id: effect.event_id(),
                                    from: effect.phase(),
                                })
                                .map_err(ledger_error)
                        })
                        .await?;
                    tally.completed += 1;
                }
                Err(MembershipEffectExecutionError::Corrupt) => {
                    tracing::warn!("成员效果内容损坏");
                    tally.corrupt += 1;
                }
                Err(
                    MembershipEffectExecutionError::Deferred
                    | MembershipEffectExecutionError::Dependency { .. },
                ) => {
                    tracing::debug!("成员效果执行延后");
                    tally.deferred += 1;
                }
            }
            return Ok(());
        }
        let mut sync_peers = Vec::new();
        for item in due {
            attempted.insert(attempted_work(&item.work));
            match item.work {
                LedgerWork::AdvanceEffect(_) => {}
                LedgerWork::DeliverRemovalNotice { peer, notice } => {
                    self.deliver(
                        peer,
                        LedgerDeliveryKind::RemovalNotice,
                        RestrictedMembershipDelivery::Event(notice),
                        tally,
                    )
                    .await?;
                }
                LedgerWork::DeliverDecision { peer, decision } => {
                    self.deliver(
                        peer,
                        LedgerDeliveryKind::Decision,
                        RestrictedMembershipDelivery::Decision(decision),
                        tally,
                    )
                    .await?;
                }
                LedgerWork::EndDeparture { peer } => {
                    self.owner
                        .commit(|draft| {
                            draft
                                .apply(LedgerInput::DepartureWindowElapsed { peer })
                                .map_err(ledger_error)
                        })
                        .await?;
                    tally.completed += 1;
                }
                LedgerWork::SynchronizeHistory { peer } => {
                    if sync_peers.len() < MAX_PEERS_PER_ROUND {
                        sync_peers.push(peer);
                    }
                }
            }
        }
        if !sync_peers.is_empty() {
            let report = self.history.synchronize(sync_peers).await?;
            tally.completed += report.completed_peer_count;
            tally.deferred += report.deferred_peer_count;
            tally.stable_failure += report.stable_failure_count;
        }
        Ok(())
    }

    /// 受限投递只发送账本中保存的精确资料，不经过普通成员发送路径。
    async fn deliver(
        &self,
        peer: DeviceId,
        kind: LedgerDeliveryKind,
        delivery: RestrictedMembershipDelivery,
        tally: &mut Tally,
    ) -> Result<(), MembershipLedgerError> {
        let result = match self
            .restricted_delivery
            .deliver_restricted_membership(&peer, &delivery)
            .await
        {
            Ok(()) => {
                tally.completed += 1;
                LedgerDeliveryResult::Delivered
            }
            Err(RestrictedMembershipDeliveryError::Deferred) => {
                tally.deferred += 1;
                LedgerDeliveryResult::Deferred
            }
            Err(RestrictedMembershipDeliveryError::Rejected) => {
                tally.stable_failure += 1;
                LedgerDeliveryResult::Rejected
            }
        };
        self.owner
            .commit(|draft| {
                draft
                    .apply(LedgerInput::DeliveryFinished {
                        peer,
                        delivery: kind,
                        result,
                    })
                    .map_err(ledger_error)
            })
            .await?;
        Ok(())
    }
}

#[async_trait]
impl RunMembershipWorkPort for MembershipWorker {
    /// 执行一轮全部已到期待办。
    async fn run_membership_work(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport {
        let mut report = MembershipMaintenanceReport::default();
        let recovery_trigger = match trigger {
            MembershipMaintenanceTrigger::Startup => MembershipRecoveryTrigger::Startup,
            MembershipMaintenanceTrigger::Resume => MembershipRecoveryTrigger::Resume,
            MembershipMaintenanceTrigger::Periodic => MembershipRecoveryTrigger::Retry,
            MembershipMaintenanceTrigger::StateChanged => MembershipRecoveryTrigger::StateChanged,
        };
        let ledger = scope_membership_recovery_trigger(
            recovery_trigger,
            observed(
                LocalWorkStep::MaintenanceEffects,
                self.run_ledger_work(WorkScope::All),
            ),
        )
        .await;
        record_outcome(&mut report, ledger);
        record_outcome(
            &mut report,
            observed(
                LocalWorkStep::MaintenanceConflicts,
                self.conflicts.recover_membership_conflicts(),
            )
            .await,
        );
        record_outcome(
            &mut report,
            observed(
                LocalWorkStep::MaintenanceGroupUpdates,
                self.group_updates.deliver_pending_group_updates(trigger),
            )
            .await,
        );
        report
    }
}

#[async_trait]
impl RecoverMembershipEffectsPort for MembershipWorker {
    /// 立即推进全部已到期的成员效果；仍有未完成效果时报告延后，由后续运行继续。
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        let outcome = self.run_ledger_work(WorkScope::EffectsOnly).await;
        if outcome != MembershipMaintenanceStepOutcome::Completed {
            return outcome;
        }
        match self.owner.load().await {
            Ok(view) => match view.space() {
                Some(space) if space.ledger().unfinished_effects().next().is_some() => {
                    MembershipMaintenanceStepOutcome::Deferred
                }
                Some(_) | None => MembershipMaintenanceStepOutcome::Completed,
            },
            Err(MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired) => {
                MembershipMaintenanceStepOutcome::Corrupt
            }
            Err(_) => MembershipMaintenanceStepOutcome::Deferred,
        }
    }
}

fn attempted_work(work: &LedgerWork) -> AttemptedWork {
    match work {
        LedgerWork::AdvanceEffect(effect) => {
            AttemptedWork::Effect(effect.event_id(), effect.phase())
        }
        LedgerWork::DeliverRemovalNotice { peer, .. } => AttemptedWork::RemovalNotice(*peer),
        LedgerWork::EndDeparture { peer } => AttemptedWork::Departure(*peer),
        LedgerWork::DeliverDecision { peer, .. } => AttemptedWork::Decision(*peer),
        LedgerWork::SynchronizeHistory { peer } => AttemptedWork::HistorySync(*peer),
    }
}

#[derive(Default)]
struct Tally {
    completed: usize,
    deferred: usize,
    stable_failure: usize,
    corrupt: usize,
}

impl Tally {
    fn record_ledger_error(&mut self, error: MembershipLedgerError) {
        match error {
            MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
                tracing::warn!("成员待办无法读取或提交已验证成员状态");
                self.corrupt += 1;
            }
            MembershipLedgerError::Locked
            | MembershipLedgerError::Conflict
            | MembershipLedgerError::Unavailable => self.deferred += 1,
        }
    }

    fn outcome(&self) -> MembershipMaintenanceStepOutcome {
        if self.corrupt > 0 {
            MembershipMaintenanceStepOutcome::Corrupt
        } else if self.stable_failure > 0 {
            MembershipMaintenanceStepOutcome::StableFailure
        } else if self.deferred > 0 {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            MembershipMaintenanceStepOutcome::Completed
        }
    }
}

async fn observed(
    step: LocalWorkStep,
    work: impl Future<Output = MembershipMaintenanceStepOutcome>,
) -> MembershipMaintenanceStepOutcome {
    let observation = LocalWorkObservation::begin(step);
    let outcome = work.await;
    observation.finish(match outcome {
        MembershipMaintenanceStepOutcome::Completed => LocalWorkOutcome::Ok,
        MembershipMaintenanceStepOutcome::Deferred => LocalWorkOutcome::Deferred,
        MembershipMaintenanceStepOutcome::StableFailure => LocalWorkOutcome::Error,
        MembershipMaintenanceStepOutcome::Corrupt => LocalWorkOutcome::Corrupt,
    });
    outcome
}

pub(super) fn record_outcome(
    report: &mut MembershipMaintenanceReport,
    outcome: MembershipMaintenanceStepOutcome,
) {
    match outcome {
        MembershipMaintenanceStepOutcome::Completed => report.completed_count += 1,
        MembershipMaintenanceStepOutcome::Deferred => report.deferred_count += 1,
        MembershipMaintenanceStepOutcome::StableFailure => report.stable_failure_count += 1,
        MembershipMaintenanceStepOutcome::Corrupt => report.corrupt_count += 1,
    }
}
