use std::sync::Arc;

use async_trait::async_trait;
use uc_core::membership::{MembershipDecisionV2, VersionedMembershipHistory};

use crate::space::membership::{
    MembershipMaintenanceReport, MembershipMaintenanceStepOutcome, RecoverMembershipEffectsPort,
};

use super::{
    MembershipEffectPhase, MembershipLedger, MembershipLedgerError, PendingMembershipEffect,
};

#[derive(Debug, thiserror::Error)]
pub enum MembershipEffectExecutionError {
    #[error("membership effect is temporarily unavailable")]
    Deferred,
    #[error("membership effect state is corrupt")]
    Corrupt,
    #[error("membership effect dependency failed")]
    Dependency {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait ApplyMembershipMemberFactsPort: Send + Sync {
    async fn apply_member_facts(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError>;
}

#[async_trait]
pub trait ApplyMembershipSecurityPort: Send + Sync {
    async fn apply_membership_security(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError>;
}

#[async_trait]
pub trait ActivateMembershipEffectPort: Send + Sync {
    async fn activate_membership_effect(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError>;
}

pub(crate) struct RePairingAwareMembershipActivation {
    inner: Arc<dyn ActivateMembershipEffectPort>,
    re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
}

impl RePairingAwareMembershipActivation {
    pub(crate) fn new(
        inner: Arc<dyn ActivateMembershipEffectPort>,
        re_pairing: Arc<dyn crate::space::membership::ResolveRePairingPort>,
    ) -> Self {
        Self { inner, re_pairing }
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for RePairingAwareMembershipActivation {
    async fn activate_membership_effect(
        &self,
        effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.inner.activate_membership_effect(effect).await?;
        if effect.kind == super::MembershipEffectKind::AddDevice {
            self.re_pairing
                .resolve_after_successful_pairing()
                .await
                .map_err(|_| MembershipEffectExecutionError::Deferred)?;
        }
        Ok(())
    }
}

pub(crate) struct RecoverMembershipEffectsUseCase {
    ledger: Arc<MembershipLedger>,
    member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    security: Arc<dyn ApplyMembershipSecurityPort>,
    activation: Arc<dyn ActivateMembershipEffectPort>,
}

impl RecoverMembershipEffectsUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
        security: Arc<dyn ApplyMembershipSecurityPort>,
        activation: Arc<dyn ActivateMembershipEffectPort>,
    ) -> Self {
        Self {
            ledger,
            member_facts,
            security,
            activation,
        }
    }

    pub(crate) async fn execute(&self) -> MembershipMaintenanceReport {
        let mut report = MembershipMaintenanceReport::default();
        let event_ids = match self.ledger.load_verified().await {
            Ok(snapshot) => {
                let Some(history) = snapshot.history() else {
                    report.corrupt_count = 1;
                    tracing::warn!("成员 effect 恢复缺少已验证历史");
                    return report;
                };
                let mut ordered = Vec::with_capacity(snapshot.record().pending_effects.len());
                for (event_id, effect) in &snapshot.record().pending_effects {
                    let Some(depth) = effect_history_depth(effect, history) else {
                        report.corrupt_count = 1;
                        tracing::warn!("成员 effect 无法关联到有效历史负载");
                        return report;
                    };
                    ordered.push((depth, *event_id));
                }
                ordered.sort_unstable();
                tracing::debug!(effect_count = ordered.len(), "成员 effect 已按因果深度排序");
                ordered
                    .into_iter()
                    .map(|(_, event_id)| event_id)
                    .collect::<Vec<_>>()
            }
            Err(_) => {
                report.corrupt_count = 1;
                tracing::warn!("成员 effect 恢复无法读取已验证 ledger");
                return report;
            }
        };
        for event_id in event_ids {
            loop {
                let effect = match self.ledger.load_verified().await {
                    Ok(snapshot) => snapshot.record().pending_effects.get(&event_id).cloned(),
                    Err(_) => {
                        report.deferred_count += 1;
                        break;
                    }
                };
                let Some(effect) = effect else {
                    report.corrupt_count += 1;
                    tracing::warn!("成员 effect 在恢复期间消失");
                    break;
                };
                tracing::debug!(
                    kind = ?effect.kind,
                    phase = ?effect.phase,
                    affected_device_count = effect.affected_device_ids.len(),
                    "开始执行成员 effect 阶段"
                );
                let (next_phase, result) = match effect.phase {
                    MembershipEffectPhase::Prepared => (
                        MembershipEffectPhase::MemberFactsApplied,
                        self.member_facts.apply_member_facts(&effect).await,
                    ),
                    MembershipEffectPhase::MemberFactsApplied => (
                        MembershipEffectPhase::SecurityApplied,
                        self.security.apply_membership_security(&effect).await,
                    ),
                    MembershipEffectPhase::SecurityApplied => (
                        MembershipEffectPhase::Activated,
                        self.activation.activate_membership_effect(&effect).await,
                    ),
                    MembershipEffectPhase::Activated => {
                        report.completed_count += 1;
                        tracing::debug!(kind = ?effect.kind, "成员 effect 已完成");
                        break;
                    }
                };
                match result {
                    Ok(()) => {
                        if self
                            .ledger
                            .advance_membership_effect_phase(event_id, effect.phase, next_phase)
                            .await
                            .is_err()
                        {
                            report.deferred_count += 1;
                            tracing::debug!("成员 effect 阶段提交延后");
                            break;
                        }
                        tracing::debug!(next_phase = ?next_phase, "成员 effect 阶段已提交");
                    }
                    Err(MembershipEffectExecutionError::Deferred) => {
                        report.deferred_count += 1;
                        tracing::debug!("成员 effect 执行延后");
                        break;
                    }
                    Err(MembershipEffectExecutionError::Corrupt) => {
                        report.corrupt_count += 1;
                        tracing::warn!("成员 effect 内容损坏");
                        break;
                    }
                    Err(MembershipEffectExecutionError::Dependency { .. }) => {
                        report.deferred_count += 1;
                        tracing::debug!("成员 effect 依赖暂不可用");
                        break;
                    }
                }
            }
        }
        tracing::debug!(
            completed_count = report.completed_count,
            deferred_count = report.deferred_count,
            corrupt_count = report.corrupt_count,
            "成员 effect 恢复轮次结束"
        );
        report
    }
}

/// effect 负载在写入 ledger 前已随历史完成验签；恢复时再次绑定标识与因果深度，
/// 避免持久化 Map 的哈希顺序泄漏到安全组和成员事实的执行顺序。
fn effect_history_depth(
    effect: &PendingMembershipEffect,
    history: &VersionedMembershipHistory,
) -> Option<u64> {
    if let Some(event) = effect.membership_event() {
        return (event.event_id().as_bytes() == &effect.event_id).then_some(event.parent_depth);
    }
    let decision = postcard::from_bytes::<MembershipDecisionV2>(&effect.payload).ok()?;
    (decision.removal_event_id.as_bytes() == &effect.event_id)
        .then(|| history.depth(decision.removal_event_id))
        .flatten()
}

#[async_trait]
impl RecoverMembershipEffectsPort for RecoverMembershipEffectsUseCase {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        let report = self.execute().await;
        if report.corrupt_count > 0 {
            MembershipMaintenanceStepOutcome::Corrupt
        } else if report.deferred_count > 0 {
            MembershipMaintenanceStepOutcome::Deferred
        } else {
            MembershipMaintenanceStepOutcome::Completed
        }
    }
}

impl MembershipLedger {
    async fn advance_membership_effect_phase(
        &self,
        event_id: [u8; 32],
        expected_phase: MembershipEffectPhase,
        next_phase: MembershipEffectPhase,
    ) -> Result<(), MembershipLedgerError> {
        if next_phase <= expected_phase {
            return Err(MembershipLedgerError::Corrupt);
        }
        self.compare_and_commit(move |record| {
            let effect = record
                .pending_effects
                .get_mut(&event_id)
                .ok_or(MembershipLedgerError::Conflict)?;
            if effect.phase != expected_phase {
                return Err(MembershipLedgerError::Conflict);
            }
            effect.phase = next_phase;
            Ok(())
        })
        .await?;
        Ok(())
    }
}
