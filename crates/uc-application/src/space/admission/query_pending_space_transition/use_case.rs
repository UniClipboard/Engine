use std::sync::Arc;

use super::QueryPendingSpaceTransitionError;
use crate::space::membership::MembershipLedger;

pub(crate) struct QueryPendingSpaceTransitionUseCase {
    ledger: Arc<MembershipLedger>,
}

impl QueryPendingSpaceTransitionUseCase {
    pub(crate) fn new(ledger: Arc<MembershipLedger>) -> Self {
        Self { ledger }
    }

    pub(crate) async fn execute(&self) -> Result<bool, QueryPendingSpaceTransitionError> {
        let attempts = self
            .ledger
            .recoverable_join_records()
            .await
            .map_err(|error| QueryPendingSpaceTransitionError(error.to_string()))?;
        Ok(attempts.into_iter().any(|attempt| {
            attempt.is_joiner()
                && attempt.completion.is_some()
                && attempt.space_transition.is_some()
                && attempt.space_transition_result.is_none()
        }))
    }
}
