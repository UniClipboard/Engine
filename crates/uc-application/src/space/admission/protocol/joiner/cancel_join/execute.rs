use super::super::JoinerAdmissionService;
use super::{JoinerCancellationMutation, JoinerCancellationStateError};
use crate::space::admission::protocol::SpaceAdmissionProtocol;
use crate::space::admission::{
    CancelSpaceJoinError, CurrentJoinStatus, JoinSpaceTerminationReason,
};
use uc_core::membership::{JoinId, SpaceAdmissionAggregateError};
use uc_observability_contract::diagnostics::SpaceAdmissionObservationOutcome;

impl SpaceAdmissionProtocol {
    pub(crate) async fn cancel_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        let result = self
            .execute_exclusively(self.joiner.cancel_join(join_id))
            .await;
        if result.is_ok() {
            self.recovery.interrupt_current();
        }
        result
    }
}

impl JoinerAdmissionService {
    async fn cancel_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        let join_id = JoinId::from_bytes(join_id).ok_or(CancelSpaceJoinError::NotFound)?;
        // 后台恢复不会持有本机动作锁；版本冲突后必须重新读取并重新判断是否仍可取消。
        const MAX_STATE_CONFLICTS: usize = 3;
        let mut conflicts = 0;
        loop {
            let loaded = self
                .cancellation_state
                .load(join_id)
                .await
                .map_err(CancelSpaceJoinError::state)?
                .ok_or(CancelSpaceJoinError::NotFound)?;
            let (admission, token) = loaded.into_parts();
            let admission_id = admission.admission_id();
            let observation_material = *admission.admission_id().as_bytes();
            let local_transition = admission
                .joiner_activation_preparation()
                .map(|preparation| preparation.space_transition().as_bytes().to_vec());
            let peer_upgrade_required = admission.peer_upgrade_required();
            if admission.can_terminate_locally() {
                let transition = admission
                    .cancel_locally()
                    .map_err(CancelSpaceJoinError::state)?;
                match self
                    .cancellation_state
                    .commit(token, JoinerCancellationMutation::new(transition))
                    .await
                {
                    Ok(()) => {}
                    Err(JoinerCancellationStateError::StateChanged { .. })
                        if conflicts < MAX_STATE_CONFLICTS =>
                    {
                        conflicts += 1;
                        continue;
                    }
                    Err(error) => return Err(CancelSpaceJoinError::state(error)),
                }
                if let Some(local_transition) = local_transition {
                    self.execute_activation
                        .terminate(admission_id, &local_transition)
                        .await
                        .map_err(CancelSpaceJoinError::state)?;
                }
                self.observations.finish(
                    observation_material,
                    SpaceAdmissionObservationOutcome::Cancelled,
                );
                self.maintenance_wake.wake();
                return Ok(CurrentJoinStatus::Terminated {
                    join_id: *join_id.as_bytes(),
                    reason: JoinSpaceTerminationReason::Cancelled,
                });
            }
            let material = self
                .prepare_cancellation
                .prepare()
                .await
                .map_err(CancelSpaceJoinError::state)?;
            let (message_id, retry_state) = material.into_parts();
            let transition = match admission.request_cancel(message_id, retry_state) {
                Ok(transition) => transition,
                Err(SpaceAdmissionAggregateError::TooLateCommitted) => {
                    return Ok(pending_status(join_id, false, peer_upgrade_required));
                }
                Err(error) => return Err(CancelSpaceJoinError::state(error)),
            };
            match self
                .cancellation_state
                .commit(token, JoinerCancellationMutation::new(transition))
                .await
            {
                Ok(()) => {}
                Err(JoinerCancellationStateError::StateChanged { .. })
                    if conflicts < MAX_STATE_CONFLICTS =>
                {
                    conflicts += 1;
                    continue;
                }
                Err(error) => return Err(CancelSpaceJoinError::state(error)),
            }
            self.maintenance_wake.wake();
            return Ok(pending_status(join_id, true, peer_upgrade_required));
        }
    }
}

fn pending_status(
    join_id: JoinId,
    cancel_requested: bool,
    peer_upgrade_required: bool,
) -> CurrentJoinStatus {
    CurrentJoinStatus::Pending {
        join_id: *join_id.as_bytes(),
        target_space_id: None,
        sponsor_device_id: None,
        sponsor_identity_fingerprint: None,
        cancel_requested,
        peer_upgrade_required,
    }
}
