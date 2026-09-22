use crate::space::admission::{AdmissionRecoveryTrigger, CurrentJoinStatus, JoinSpaceInput};
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};

use super::super::super::test_support::SpaceAdmissionProtocolTestPair;

pub(in crate::space::admission::protocol::tests) struct PairingScenarioFixture {
    pair: SpaceAdmissionProtocolTestPair,
}

pub(in crate::space::admission::protocol::tests) struct PairingScenarioSnapshot {
    status: CurrentJoinStatus,
    final_confirmation_complete: bool,
}

impl PairingScenarioSnapshot {
    pub(in crate::space::admission::protocol::tests) fn is_active(&self) -> bool {
        matches!(self.status, CurrentJoinStatus::Active { .. })
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
            || admission.should_continue()
        {
            return Err(PairingScenarioFailure::AdmissionMaintenance);
        }

        let status = self
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
            status,
            final_confirmation_complete: true,
        })
    }
}
