//! Shared setup-state query implementation.

use crate::error_codes::*;

use tracing::{error, info};
use uc_application::facade::{AppFacade, QuerySetupStateError};

use crate::{
    EngineError, EngineErrorCategory, OperationResult, SetupInvitationSummary, SetupStateSummary,
};

pub async fn execute_query_setup_state(facade: &AppFacade) -> Result<OperationResult, EngineError> {
    let state = facade
        .query_setup_state()
        .await
        .map_err(|error| match error {
            QuerySetupStateError::StorageFailed(_) | QuerySetupStateError::Internal(_) => {
                let source = match error {
                    QuerySetupStateError::StorageFailed(_) => "storage",
                    QuerySetupStateError::Internal(_) => "internal",
                };
                error!(
                    operation = "query_setup_state",
                    source,
                    error_code = QUERY_SETUP_STATE_FAILED_CODE,
                    error_category = "internal",
                    retryable = false,
                    "space state query failed"
                );
                EngineError::new(
                    QUERY_SETUP_STATE_FAILED_CODE,
                    EngineErrorCategory::Internal,
                    false,
                )
            }
        })?;

    let summary = SetupStateSummary {
        has_completed: state.has_completed,
        space_id: state.space_id.map(Into::into),
        current_invitation: state
            .current_invitation
            .map(|invitation| SetupInvitationSummary {
                invitation_code: invitation.code.as_str().to_string(),
                expires_at_ms: invitation.expires_at.timestamp_millis(),
            }),
        device_name: state.device_name,
        re_pairing_required: state.re_pairing_required,
    };
    info!(
        operation = "query_setup_state",
        has_completed = summary.has_completed,
        has_space = summary.space_id.is_some(),
        has_current_invitation = summary.current_invitation.is_some(),
        has_device_name = summary.device_name.is_some(),
        "space state query completed"
    );
    Ok(OperationResult::SetupState(summary))
}
