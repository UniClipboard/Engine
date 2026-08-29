use super::super::JoinerAdmissionService;
use super::JoinerCancellationMutation;
use crate::space::admission::protocol::SpaceAdmissionProtocol;
use crate::space::admission::{CancelSpaceJoinError, CurrentJoinStatus};
use uc_core::membership::{JoinId, SpaceAdmissionAggregateError};

impl SpaceAdmissionProtocol {
    pub(crate) async fn cancel_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        self.execute_exclusively(self.joiner.cancel_join(join_id))
            .await
    }
}

impl JoinerAdmissionService {
    async fn cancel_join(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        let join_id = JoinId::from_bytes(join_id).ok_or(CancelSpaceJoinError::NotFound)?;
        let loaded = self
            .cancellation_state
            .load(join_id)
            .await
            .map_err(CancelSpaceJoinError::state)?
            .ok_or(CancelSpaceJoinError::NotFound)?;
        let (admission, token) = loaded.into_parts();
        let material = self
            .prepare_cancellation
            .prepare()
            .await
            .map_err(CancelSpaceJoinError::state)?;
        let (message_id, retry_state) = material.into_parts();
        let transition = match admission.request_cancel(message_id, retry_state) {
            Ok(transition) => transition,
            Err(SpaceAdmissionAggregateError::TooLateCommitted) => {
                return Ok(pending_status(join_id, false));
            }
            Err(error) => return Err(CancelSpaceJoinError::state(error)),
        };
        self.cancellation_state
            .commit(token, JoinerCancellationMutation::new(transition))
            .await
            .map_err(CancelSpaceJoinError::state)?;
        self.maintenance_wake.wake();
        Ok(pending_status(join_id, true))
    }
}

fn pending_status(join_id: JoinId, cancel_requested: bool) -> CurrentJoinStatus {
    CurrentJoinStatus::Pending {
        join_id: *join_id.as_bytes(),
        target_space_id: None,
        sponsor_device_id: None,
        sponsor_identity_fingerprint: None,
        cancel_requested,
    }
}
