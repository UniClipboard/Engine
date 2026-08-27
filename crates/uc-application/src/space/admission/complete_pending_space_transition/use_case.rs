use std::sync::Arc;

use uc_core::membership::{AdmissionSpaceTransitionV2, SpaceJoinRecord, SpaceJoinRecordId};

use super::CompletePendingSpaceTransitionError;
use crate::deps::{AdmissionSpaceTransitionPort, AdmissionSpaceTransitionStepV2};
use crate::space::admission::CurrentJoinStatus;
use crate::space::membership::project_current_join;
use crate::space::membership::MembershipLedger;

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
            .recoverable_join_records()
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
                    record = record
                        .advanced_space_transition(&transition, &next)
                        .map_err(state)?;
                    self.persist(record).await?;
                    record = self.load_required(transition.attempt_id()).await?;
                }
                AdmissionSpaceTransitionStepV2::Finished(result) => {
                    let (record, history) = record
                        .completed_space_transition(&transition, &result)
                        .map_err(state)?;
                    self.persist_with_history(record, history).await?;
                    return Ok(());
                }
            }
        }
    }

    async fn persist(
        &self,
        record: SpaceJoinRecord,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        self.ledger
            .save_join_record_progress(record)
            .await
            .map_err(state)?;
        Ok(())
    }

    async fn persist_with_history(
        &self,
        record: SpaceJoinRecord,
        history: Vec<u8>,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        self.ledger
            .activate_joined_space(record, history.clone(), history)
            .await
            .map_err(state)?;
        Ok(())
    }

    async fn load_required(
        &self,
        record_id: SpaceJoinRecordId,
    ) -> Result<SpaceJoinRecord, CompletePendingSpaceTransitionError> {
        self.ledger
            .load_join_record(record_id)
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
