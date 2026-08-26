use std::sync::Arc;

use async_trait::async_trait;

use crate::space::membership::{
    MembershipMaintenanceReport, MembershipMaintenanceStepOutcome, RecoverMembershipEffectsPort,
};

use super::{
    MembershipEffectPhase, MembershipLedger, MembershipLedgerError, PendingMembershipEffect,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MembershipEffectExecutionError {
    #[error("membership effect is temporarily unavailable")]
    Deferred,
    #[error("membership effect state is corrupt")]
    Corrupt,
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
            Ok(snapshot) => snapshot
                .record()
                .pending_effects
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            Err(_) => {
                report.corrupt_count = 1;
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
                    break;
                };
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
                            break;
                        }
                    }
                    Err(MembershipEffectExecutionError::Deferred) => {
                        report.deferred_count += 1;
                        break;
                    }
                    Err(MembershipEffectExecutionError::Corrupt) => {
                        report.corrupt_count += 1;
                        break;
                    }
                }
            }
        }
        report
    }
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
