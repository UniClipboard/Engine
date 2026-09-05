use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tracing::Instrument;
use uc_application::deps::{
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, SpaceMembershipAdapters,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    GroupUpdateDispatchError, GroupUpdateDispatchPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, PendingGroupUpdate,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

#[derive(Clone, Copy)]
enum MembershipOperation {
    HistoryExchange,
    RestrictedDelivery,
    GroupUpdateDispatch,
}

impl MembershipOperation {
    const fn diagnostic_operation(self) -> DiagnosticOperation {
        match self {
            Self::GroupUpdateDispatch => DiagnosticOperation::MembershipGroupUpdate,
            Self::HistoryExchange | Self::RestrictedDelivery => {
                DiagnosticOperation::MembershipHistorySync
            }
        }
    }
}

pub(crate) fn observe_membership(adapters: SpaceMembershipAdapters) -> SpaceMembershipAdapters {
    SpaceMembershipAdapters {
        membership_history_transport: Arc::new(ObservedMembershipHistoryExchange {
            inner: adapters.membership_history_transport,
        }),
        restricted_membership_delivery: Arc::new(ObservedRestrictedMembershipDelivery {
            inner: adapters.restricted_membership_delivery,
        }),
        group_update_dispatch: Arc::new(ObservedGroupUpdateDispatch {
            inner: adapters.group_update_dispatch,
        }),
        ..adapters
    }
}

fn membership_span(operation: MembershipOperation) -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceMembership,
        operation: operation.diagnostic_operation(),
        role: DiagnosticRole::Member,
        kind: DiagnosticSpanKind::Client,
    })
}

fn record_membership_completion(
    operation: MembershipOperation,
    elapsed: Duration,
    result: Result<(), MembershipCompletionKind>,
) {
    let operation = operation.diagnostic_operation();
    let completion = match result {
        Ok(()) => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceMembership,
            operation,
            DiagnosticRole::Member,
            elapsed,
        ),
        Err(MembershipCompletionKind::Deferred) => OperationCompletion::deferred(
            DiagnosticDomain::SpaceMembership,
            operation,
            DiagnosticRole::Member,
            elapsed,
        ),
        Err(MembershipCompletionKind::Rejected) => OperationCompletion::rejected(
            DiagnosticDomain::SpaceMembership,
            operation,
            DiagnosticRole::Member,
            elapsed,
        ),
        Err(MembershipCompletionKind::Failed(error)) => OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            operation,
            DiagnosticRole::Member,
            error,
            elapsed,
        ),
    };
    complete_operation(completion);
}

#[derive(Clone, Copy)]
enum MembershipCompletionKind {
    Deferred,
    Rejected,
    Failed(DiagnosticErrorType),
}

struct ObservedMembershipHistoryExchange {
    inner: Arc<dyn MembershipHistoryExchangePort>,
}

#[async_trait]
impl MembershipHistoryExchangePort for ObservedMembershipHistoryExchange {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let started = Instant::now();
        let span = membership_span(MembershipOperation::HistoryExchange);
        let result = self
            .inner
            .exchange_membership_history(recipient, message)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            record_membership_completion(
                MembershipOperation::HistoryExchange,
                started.elapsed(),
                result
                    .as_ref()
                    .map(|_| ())
                    .map_err(history_exchange_completion),
            )
        });
        result
    }
}

fn history_exchange_completion(error: &MembershipHistoryExchangeError) -> MembershipCompletionKind {
    match error {
        MembershipHistoryExchangeError::Offline => {
            MembershipCompletionKind::Failed(DiagnosticErrorType::Unavailable)
        }
        MembershipHistoryExchangeError::Rejected => MembershipCompletionKind::Rejected,
        MembershipHistoryExchangeError::Transport => {
            MembershipCompletionKind::Failed(DiagnosticErrorType::StreamFailed)
        }
    }
}

struct ObservedRestrictedMembershipDelivery {
    inner: Arc<dyn RestrictedMembershipDeliveryPort>,
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for ObservedRestrictedMembershipDelivery {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        let started = Instant::now();
        let span = membership_span(MembershipOperation::RestrictedDelivery);
        let result = self
            .inner
            .deliver_restricted_membership(peer, delivery)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            record_membership_completion(
                MembershipOperation::RestrictedDelivery,
                started.elapsed(),
                result.as_ref().copied().map_err(|error| match error {
                    RestrictedMembershipDeliveryError::Deferred => {
                        MembershipCompletionKind::Deferred
                    }
                    RestrictedMembershipDeliveryError::Rejected => {
                        MembershipCompletionKind::Rejected
                    }
                }),
            )
        });
        result
    }
}

struct ObservedGroupUpdateDispatch {
    inner: Arc<dyn GroupUpdateDispatchPort>,
}

#[async_trait]
impl GroupUpdateDispatchPort for ObservedGroupUpdateDispatch {
    async fn dispatch_group_update(
        &self,
        update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError> {
        let started = Instant::now();
        let span = membership_span(MembershipOperation::GroupUpdateDispatch);
        let result = self
            .inner
            .dispatch_group_update(update)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            record_membership_completion(
                MembershipOperation::GroupUpdateDispatch,
                started.elapsed(),
                result.as_ref().copied().map_err(|error| match error {
                    GroupUpdateDispatchError::Offline => {
                        MembershipCompletionKind::Failed(DiagnosticErrorType::Unavailable)
                    }
                    GroupUpdateDispatchError::Rejected => MembershipCompletionKind::Rejected,
                    GroupUpdateDispatchError::Transport => {
                        MembershipCompletionKind::Failed(DiagnosticErrorType::StreamFailed)
                    }
                }),
            )
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        history_exchange_completion, MembershipCompletionKind, MembershipHistoryExchangeError,
    };
    use uc_observability_contract::diagnostics::DiagnosticErrorType;

    #[test]
    fn history_exchange_errors_keep_stable_result_categories() {
        assert!(matches!(
            history_exchange_completion(&MembershipHistoryExchangeError::Offline),
            MembershipCompletionKind::Failed(DiagnosticErrorType::Unavailable)
        ));
        assert!(matches!(
            history_exchange_completion(&MembershipHistoryExchangeError::Rejected),
            MembershipCompletionKind::Rejected
        ));
        assert!(matches!(
            history_exchange_completion(&MembershipHistoryExchangeError::Transport),
            MembershipCompletionKind::Failed(DiagnosticErrorType::StreamFailed)
        ));
    }
}
