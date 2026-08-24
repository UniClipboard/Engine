use std::sync::Arc;

use sha2::Digest as _;
use uc_core::membership::{AdmissionIdentityBindingV1, AdmissionTerminalResultV1};

use super::{PendingJoinerCompleteAck, RecoverSpaceJoinCompletionError};
use crate::space::admission::durable::{self as admission, complete_ack_frame};

pub(crate) struct RecoverSpaceJoinCompletionUseCase {
    admission_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort>,
}

impl RecoverSpaceJoinCompletionUseCase {
    pub(crate) fn new(
        admission_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort>,
    ) -> Self {
        Self { admission_attempts }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<Option<PendingJoinerCompleteAck>, RecoverSpaceJoinCompletionError> {
        let Some(projection) = self
            .admission_attempts
            .project_current_local_join()
            .await
            .map_err(admission::map_repository_error)
            .map_err(map_error)?
        else {
            return Ok(None);
        };
        if projection.terminal_result != Some(AdmissionTerminalResultV1::Active) {
            return Ok(None);
        }
        let terminal = self
            .admission_attempts
            .load_terminal(projection.attempt_id)
            .await
            .map_err(admission::map_repository_error)
            .map_err(map_error)?
            .ok_or_else(|| error("active local join terminal is missing"))?;
        let binding = AdmissionIdentityBindingV1::decode(
            terminal
                .identity_binding
                .as_deref()
                .ok_or_else(|| error("active local join identity is missing"))?,
        )
        .map_err(|decode_error| error(decode_error.to_string()))?;
        let completion_digest: [u8; 32] = sha2::Sha256::digest(&terminal.replay_result).into();
        let acknowledgment = terminal
            .acknowledgment_rebuild
            .iter()
            .find(|record| record.payload_digest == completion_digest)
            .ok_or_else(|| error("active local join completion acknowledgment is missing"))?;
        let payload = postcard::to_stdvec(acknowledgment)
            .map_err(|encode_error| error(encode_error.to_string()))?;
        Ok(Some(PendingJoinerCompleteAck {
            sponsor_device_id: binding.sponsor_device_id,
            frame: complete_ack_frame(projection.attempt_id, acknowledgment.message_id, payload),
        }))
    }
}

fn map_error(
    error: crate::space::workspace_membership::WorkspaceConvergenceError,
) -> RecoverSpaceJoinCompletionError {
    RecoverSpaceJoinCompletionError(error.to_string())
}

fn error(message: impl Into<String>) -> RecoverSpaceJoinCompletionError {
    RecoverSpaceJoinCompletionError(message.into())
}
