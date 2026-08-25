use std::sync::Arc;

use uc_core::membership::{
    AdmissionSpaceTransitionV2, AdmissionTerminalResult, JoinerAdmissionStage,
    JoinerAdmissionState, SpaceJoinRecord, SpaceJoinRecordId, SpaceJoinRoleState,
};

use super::CompletePendingSpaceTransitionError;
use crate::deps::{AdmissionSpaceTransitionPort, AdmissionSpaceTransitionStepV2};
use crate::space::admission::CurrentJoinStatus;
use crate::space::membership_ledger::MembershipLedger;
use crate::space::query_device_trust::project_current_join;

pub(crate) struct CompletePendingSpaceTransitionUseCase {
    ledger: Arc<MembershipLedger>,
    space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
}

impl CompletePendingSpaceTransitionUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
    ) -> Self {
        Self {
            ledger,
            space_transition,
        }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<CurrentJoinStatus, CompletePendingSpaceTransitionError> {
        let records = self
            .ledger
            .recoverable_admission_records()
            .await
            .map_err(state)?;
        for record in records.into_iter().filter(is_pending_space_transition) {
            self.complete_transition(record).await?;
        }
        let snapshot = self.ledger.load_verified().await.map_err(state)?;
        let status = project_current_join(snapshot.record())
            .map_err(state)?
            .ok_or(CompletePendingSpaceTransitionError::JoinNotActive)?;
        if !matches!(status, CurrentJoinStatus::Active { .. }) {
            return Err(CompletePendingSpaceTransitionError::JoinNotActive);
        }
        Ok(status)
    }

    async fn complete_transition(
        &self,
        mut record: SpaceJoinRecord,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        if !record.is_joiner() || record.completion.is_none() {
            return Err(state("Space transition started before join completion"));
        }
        loop {
            let transition = AdmissionSpaceTransitionV2::decode(
                record
                    .space_transition
                    .as_deref()
                    .ok_or_else(|| state("pending Space transition is missing"))?,
            )
            .ok_or_else(|| state("pending Space transition is invalid"))?;
            match self
                .space_transition
                .advance(&transition)
                .await
                .map_err(state)?
            {
                AdmissionSpaceTransitionStepV2::Advanced(next) => {
                    if !transition.can_advance_to(&next) {
                        return Err(state("Space transition skipped or replaced a phase"));
                    }
                    record.space_transition = Some(
                        next.encode()
                            .ok_or_else(|| state("advanced Space transition is invalid"))?,
                    );
                    self.persist(record).await?;
                    record = self.load_required(transition.attempt_id()).await?;
                }
                AdmissionSpaceTransitionStepV2::Finished(result) => {
                    if !result.matches_cleanup_pending(&transition) {
                        return Err(state(
                            "Space transition result does not match cleanup state",
                        ));
                    }
                    let history = record
                        .verified_membership_history
                        .clone()
                        .ok_or_else(|| state("Space transition verified history is missing"))?;
                    record.space_transition_result = Some(
                        result
                            .encode()
                            .ok_or_else(|| state("Space transition result cannot be encoded"))?,
                    );
                    record.terminal_result = Some(AdmissionTerminalResult::Active);
                    record.role_state = SpaceJoinRoleState::Joiner(JoinerAdmissionState {
                        stage: JoinerAdmissionStage::Completed,
                    });
                    self.persist_with_history(record, history).await?;
                    return Ok(());
                }
            }
        }
    }

    async fn persist(
        &self,
        mut record: SpaceJoinRecord,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        let expected_version = record.record_version;
        record.record_version = expected_version
            .checked_add(1)
            .ok_or_else(|| state("admission record version overflow"))?;
        self.ledger
            .advance_admission_record(record.record_id, expected_version, record)
            .await
            .map_err(state)?;
        Ok(())
    }

    async fn persist_with_history(
        &self,
        mut record: SpaceJoinRecord,
        history: Vec<u8>,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        let expected_version = record.record_version;
        record.record_version = expected_version
            .checked_add(1)
            .ok_or_else(|| state("admission record version overflow"))?;
        self.ledger
            .advance_admission_record_with_history(
                record.record_id,
                expected_version,
                record,
                history.clone(),
                history,
            )
            .await
            .map_err(state)?;
        Ok(())
    }

    async fn load_required(
        &self,
        record_id: SpaceJoinRecordId,
    ) -> Result<SpaceJoinRecord, CompletePendingSpaceTransitionError> {
        self.ledger
            .load_admission_record(record_id)
            .await
            .map_err(state)?
            .ok_or_else(|| state("admission attempt was not found"))
    }
}

fn is_pending_space_transition(record: &SpaceJoinRecord) -> bool {
    record.is_joiner()
        && record.completion.is_some()
        && record.space_transition.is_some()
        && record.space_transition_result.is_none()
}

fn state(error: impl std::fmt::Display) -> CompletePendingSpaceTransitionError {
    CompletePendingSpaceTransitionError::State(error.to_string())
}
