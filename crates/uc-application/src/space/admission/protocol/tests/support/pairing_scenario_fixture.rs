use crate::space::admission::{AdmissionRecoveryTrigger, CurrentJoinStatus, JoinSpaceInput};
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};

use super::super::super::test_support::SpaceAdmissionProtocolTestPair;

pub(in crate::space::admission::protocol::tests) struct PairingScenarioFixture {
    pair: SpaceAdmissionProtocolTestPair,
}

pub(in crate::space::admission::protocol::tests) struct PairingScenarioSnapshot {
    /// 本机激活的结果：最终确认前按设计为 `Processing`。
    activation: CurrentJoinStatus,
    /// 最终确认后加入记录已结算；公开的 `Active` 正是由这一事实投影。
    active_settled: bool,
    final_confirmation_complete: bool,
}

impl PairingScenarioSnapshot {
    /// 已激活到邀请方的 Space，且最终确认后加入已结算，即公开状态为 `Active`。
    pub(in crate::space::admission::protocol::tests) fn is_active(&self) -> bool {
        matches!(self.activation, CurrentJoinStatus::Processing { .. }) && self.active_settled
    }

    pub(in crate::space::admission::protocol::tests) const fn final_confirmation_complete(
        &self,
    ) -> bool {
        self.final_confirmation_complete
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::space::admission::protocol::tests) enum PairingScenarioFailure {
    JoinStart,
    AdmissionMaintenance,
    Activation,
    FinalConfirmation,
}

impl PairingScenarioFailure {
    pub(in crate::space::admission::protocol::tests) const fn condition(self) -> &'static str {
        match self {
            Self::JoinStart => "join-start",
            Self::AdmissionMaintenance => "admission-maintenance",
            Self::Activation => "activation",
            Self::FinalConfirmation => "final-confirmation",
        }
    }
}

impl PairingScenarioFixture {
    pub(in crate::space::admission::protocol::tests) async fn prepare() -> Self {
        Self {
            pair: SpaceAdmissionProtocolTestPair::receiving_complete().await,
        }
    }

    pub(in crate::space::admission::protocol::tests) async fn complete_joiner_pairing(
        &self,
        input: JoinSpaceInput,
    ) -> Result<PairingScenarioSnapshot, PairingScenarioFailure> {
        self.pair
            .joiner()
            .start_join_at(input, 1_000)
            .await
            .map_err(|_| PairingScenarioFailure::JoinStart)?;

        let admission = self
            .pair
            .joiner()
            .recover_space_admissions(&MembershipMaintenanceTrigger::StateChanged)
            .await;
        if admission.step() != MembershipMaintenanceStepOutcome::Completed
            || admission.allows_ordinary_membership()
        {
            return Err(PairingScenarioFailure::AdmissionMaintenance);
        }

        let activation = self
            .pair
            .joiner()
            .complete_pending_space_transition()
            .await
            .map_err(|_| PairingScenarioFailure::Activation)?;

        let confirmation = self
            .pair
            .joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
        if confirmation.recovery_required_count > 0
            || confirmation.peer_upgrade_required_count > 0
            || !self.pair.saved_join().is_active_settled()
        {
            return Err(PairingScenarioFailure::FinalConfirmation);
        }

        Ok(PairingScenarioSnapshot {
            activation,
            active_settled: self.pair.saved_join().is_active_settled(),
            final_confirmation_complete: true,
        })
    }
}
