use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tracing::Instrument;
use uc_application::deps::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipBranchRecoveryChannelError, MembershipBranchRecoveryChannelPort,
    MembershipBranchRecoveryCommit, MembershipBranchRecoveryRequest, MembershipLedgerError,
    MembershipLedgerMutation, RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort, SpaceMembershipAdapters,
};
use uc_application::facade::HostEventBus;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    GroupUpdateDispatchError, GroupUpdateDispatchPort, MembershipBranchRecoveryPackageV1,
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    PendingGroupUpdate,
};
use uc_core::ports::{HostEvent, MembershipHostEvent};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

const SLOW_LEDGER_LOAD: Duration = Duration::from_millis(50);

#[derive(Clone, Copy)]
enum MembershipOperation {
    LedgerLoad,
    LedgerCommit,
    HistoryExchange,
    RestrictedDelivery,
    GroupUpdateDispatch,
    BranchRecoveryGroupInfo,
    BranchRecoveryExternalCommit,
}

impl MembershipOperation {
    const fn diagnostic_operation(self) -> DiagnosticOperation {
        match self {
            Self::GroupUpdateDispatch => DiagnosticOperation::MembershipGroupUpdate,
            Self::LedgerLoad
            | Self::LedgerCommit
            | Self::HistoryExchange
            | Self::RestrictedDelivery
            | Self::BranchRecoveryGroupInfo
            | Self::BranchRecoveryExternalCommit => DiagnosticOperation::MembershipHistorySync,
        }
    }
}

fn membership_span(operation: MembershipOperation, role: DiagnosticRole) -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceMembership,
        operation: operation.diagnostic_operation(),
        role,
        kind: DiagnosticSpanKind::Client,
        flow: None,
    })
}

fn record_membership_completion(
    operation: MembershipOperation,
    role: DiagnosticRole,
    elapsed: Duration,
    error_kind: Option<&'static str>,
) {
    let diagnostic_operation = operation.diagnostic_operation();
    let completion = match error_kind {
        None => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            elapsed,
        ),
        Some("deferred") => OperationCompletion::deferred(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            elapsed,
        ),
        Some("rejected") => OperationCompletion::rejected(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            elapsed,
        ),
        Some("offline") | Some("unavailable") | Some("locked") => OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            DiagnosticErrorType::Unavailable,
            elapsed,
        ),
        Some("corrupt") | Some("recovery_required") | Some("conflict") => {
            OperationCompletion::failed(
                DiagnosticDomain::SpaceMembership,
                diagnostic_operation,
                role,
                DiagnosticErrorType::Corrupt,
                elapsed,
            )
        }
        Some("transport") => OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            DiagnosticErrorType::StreamFailed,
            elapsed,
        ),
        Some("invalid") => OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            DiagnosticErrorType::DecodeFailed,
            elapsed,
        ),
        Some(_) => OperationCompletion::failed(
            DiagnosticDomain::SpaceMembership,
            diagnostic_operation,
            role,
            DiagnosticErrorType::Internal,
            elapsed,
        ),
    };
    complete_operation(completion);
}

pub(crate) fn observe_membership(
    adapters: SpaceMembershipAdapters,
    host_events: Arc<HostEventBus>,
) -> SpaceMembershipAdapters {
    SpaceMembershipAdapters {
        load_membership_ledger: Arc::new(ObservedMembershipLedgerLoad {
            inner: adapters.load_membership_ledger,
        }),
        commit_membership_ledger: observe_membership_commit(
            adapters.commit_membership_ledger,
            host_events,
        ),
        membership_history_transport: Arc::new(ObservedMembershipHistoryExchange {
            inner: adapters.membership_history_transport,
        }),
        restricted_membership_delivery: Arc::new(ObservedRestrictedMembershipDelivery {
            inner: adapters.restricted_membership_delivery,
        }),
        group_update_dispatch: Arc::new(ObservedGroupUpdateDispatch {
            inner: adapters.group_update_dispatch,
        }),
        membership_branch_recovery_channel: Arc::new(ObservedMembershipBranchRecoveryChannel {
            inner: adapters.membership_branch_recovery_channel,
        }),
        ..adapters
    }
}

fn observe_membership_commit(
    inner: Arc<dyn CommitMembershipLedgerPort>,
    host_events: Arc<HostEventBus>,
) -> Arc<dyn CommitMembershipLedgerPort> {
    Arc::new(ObservedMembershipLedgerCommit { inner, host_events })
}

struct ObservedMembershipLedgerLoad {
    inner: Arc<dyn LoadMembershipLedgerPort>,
}

#[async_trait]
impl LoadMembershipLedgerPort for ObservedMembershipLedgerLoad {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let started = Instant::now();
        let result = self.inner.load().await;
        let elapsed = started.elapsed();
        if should_record_ledger_load(result.is_ok(), elapsed) {
            record_ledger(
                MembershipOperation::LedgerLoad,
                elapsed,
                result.as_ref().err(),
            );
        }
        result
    }
}

struct ObservedMembershipLedgerCommit {
    inner: Arc<dyn CommitMembershipLedgerPort>,
    host_events: Arc<HostEventBus>,
}

#[async_trait]
impl CommitMembershipLedgerPort for ObservedMembershipLedgerCommit {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let started = Instant::now();
        let result = self.inner.compare_and_commit(mutation).await;
        record_ledger(
            MembershipOperation::LedgerCommit,
            started.elapsed(),
            result.as_ref().err(),
        );
        if let Ok(committed) = &result {
            self.host_events.emit_or_warn(HostEvent::Membership(
                MembershipHostEvent::LedgerCommitted {
                    revision: committed.revision,
                },
            ));
        }
        result
    }
}

fn should_record_ledger_load(success: bool, elapsed: Duration) -> bool {
    !success || elapsed >= SLOW_LEDGER_LOAD
}

fn record_ledger(
    operation: MembershipOperation,
    elapsed: Duration,
    error: Option<&MembershipLedgerError>,
) {
    record_membership_completion(
        operation,
        DiagnosticRole::Local,
        elapsed,
        error.map(membership_ledger_error_kind),
    );
}

fn membership_ledger_error_kind(error: &MembershipLedgerError) -> &'static str {
    match error {
        MembershipLedgerError::Locked => "locked",
        MembershipLedgerError::Conflict => "conflict",
        MembershipLedgerError::Corrupt => "corrupt",
        MembershipLedgerError::Unavailable => "unavailable",
        MembershipLedgerError::RecoveryRequired => "recovery_required",
    }
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
        let span = membership_span(MembershipOperation::HistoryExchange, DiagnosticRole::Member);
        let result = self
            .inner
            .exchange_membership_history(recipient, message)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            record_membership_completion(
                MembershipOperation::HistoryExchange,
                DiagnosticRole::Member,
                started.elapsed(),
                result
                    .as_ref()
                    .err()
                    .map(membership_history_exchange_error_kind),
            );
        });
        result
    }
}

fn membership_history_exchange_error_kind(error: &MembershipHistoryExchangeError) -> &'static str {
    match error {
        MembershipHistoryExchangeError::Offline => "offline",
        MembershipHistoryExchangeError::Rejected => "rejected",
        MembershipHistoryExchangeError::Transport => "transport",
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
        let span = membership_span(
            MembershipOperation::RestrictedDelivery,
            DiagnosticRole::Member,
        );
        let result = self
            .inner
            .deliver_restricted_membership(peer, delivery)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            record_membership_completion(
                MembershipOperation::RestrictedDelivery,
                DiagnosticRole::Member,
                started.elapsed(),
                result
                    .as_ref()
                    .err()
                    .map(restricted_membership_delivery_error_kind),
            );
        });
        result
    }
}

fn restricted_membership_delivery_error_kind(
    error: &RestrictedMembershipDeliveryError,
) -> &'static str {
    match error {
        RestrictedMembershipDeliveryError::Deferred => "deferred",
        RestrictedMembershipDeliveryError::Rejected => "rejected",
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
        let span = membership_span(
            MembershipOperation::GroupUpdateDispatch,
            DiagnosticRole::Member,
        );
        let result = self
            .inner
            .dispatch_group_update(update)
            .instrument(span.clone())
            .await;
        span.in_scope(|| {
            record_membership_completion(
                MembershipOperation::GroupUpdateDispatch,
                DiagnosticRole::Member,
                started.elapsed(),
                result.as_ref().err().map(group_update_dispatch_error_kind),
            );
        });
        result
    }
}

fn group_update_dispatch_error_kind(error: &GroupUpdateDispatchError) -> &'static str {
    match error {
        GroupUpdateDispatchError::Offline => "offline",
        GroupUpdateDispatchError::Rejected => "rejected",
        GroupUpdateDispatchError::Transport => "transport",
    }
}

struct ObservedMembershipBranchRecoveryChannel {
    inner: Arc<dyn MembershipBranchRecoveryChannelPort>,
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for ObservedMembershipBranchRecoveryChannel {
    async fn request_membership_branch_group_info(
        &self,
        request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        let started = Instant::now();
        let result = self
            .inner
            .request_membership_branch_group_info(request)
            .await;
        record_branch_recovery(
            MembershipOperation::BranchRecoveryGroupInfo,
            started.elapsed(),
            result.as_ref().err(),
        );
        result
    }

    async fn submit_membership_branch_external_commit(
        &self,
        request: MembershipBranchRecoveryCommit,
    ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError> {
        let started = Instant::now();
        let result = self
            .inner
            .submit_membership_branch_external_commit(request)
            .await;
        record_branch_recovery(
            MembershipOperation::BranchRecoveryExternalCommit,
            started.elapsed(),
            result.as_ref().err(),
        );
        result
    }
}

fn record_branch_recovery(
    operation: MembershipOperation,
    elapsed: Duration,
    error: Option<&MembershipBranchRecoveryChannelError>,
) {
    record_membership_completion(
        operation,
        DiagnosticRole::Local,
        elapsed,
        error.map(membership_branch_recovery_error_kind),
    );
}

fn membership_branch_recovery_error_kind(
    error: &MembershipBranchRecoveryChannelError,
) -> &'static str {
    match error {
        MembershipBranchRecoveryChannelError::Unavailable { .. } => "unavailable",
        MembershipBranchRecoveryChannelError::Rejected { .. } => "rejected",
        MembershipBranchRecoveryChannelError::Invalid { .. } => "invalid",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use uc_application::deps::{
        CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
        MembershipBranchRecoveryChannelError, MembershipBranchRecoveryChannelPort,
        MembershipBranchRecoveryCommit, MembershipBranchRecoveryRequest, MembershipLedgerError,
        MembershipLedgerMutation, RestrictedMembershipDeliveryError,
    };
    use uc_application::facade::HostEventBus;
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        GroupUpdateDispatchError, MemberInstanceId, MembershipBranchId,
        MembershipBranchRecoveryPackageV1, MembershipConflictId, MembershipHistoryExchangeError,
    };
    use uc_core::ports::{EmitError, HostEvent, HostEventEmitterPort, MembershipHostEvent};

    use super::{
        group_update_dispatch_error_kind, membership_branch_recovery_error_kind,
        membership_history_exchange_error_kind, membership_ledger_error_kind,
        observe_membership_commit, record_branch_recovery,
        restricted_membership_delivery_error_kind, should_record_ledger_load, MembershipOperation,
        ObservedMembershipBranchRecoveryChannel, ObservedMembershipLedgerLoad,
    };

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("captured writer lock")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedWriter {
        type Writer = CapturedWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    impl CapturedWriter {
        fn output(&self) -> String {
            String::from_utf8(self.0.lock().expect("captured writer lock").clone())
                .expect("captured events should be UTF-8")
        }
    }

    struct FailingLedgerLoad {
        calls: AtomicUsize,
    }

    struct SuccessfulLedgerCommit;

    #[async_trait]
    impl CommitMembershipLedgerPort for SuccessfulLedgerCommit {
        async fn compare_and_commit(
            &self,
            mutation: MembershipLedgerMutation,
        ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Ok(mutation.replacement)
        }
    }

    struct FailingLedgerCommit;

    #[async_trait]
    impl CommitMembershipLedgerPort for FailingLedgerCommit {
        async fn compare_and_commit(
            &self,
            _mutation: MembershipLedgerMutation,
        ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            Err(MembershipLedgerError::Conflict)
        }
    }

    #[derive(Default)]
    struct HostEventRecorder {
        events: Mutex<Vec<HostEvent>>,
    }

    impl HostEventEmitterPort for HostEventRecorder {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            self.events.lock().expect("host events lock").push(event);
            Ok(())
        }
    }

    #[async_trait]
    impl LoadMembershipLedgerPort for FailingLedgerLoad {
        async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(MembershipLedgerError::RecoveryRequired)
        }
    }

    struct FailingBranchRecoveryChannel;

    #[async_trait]
    impl MembershipBranchRecoveryChannelPort for FailingBranchRecoveryChannel {
        async fn request_membership_branch_group_info(
            &self,
            _request: MembershipBranchRecoveryRequest,
        ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
            Err(MembershipBranchRecoveryChannelError::Unavailable {
                source: anyhow::anyhow!("SECRET_ERROR"),
            })
        }

        async fn submit_membership_branch_external_commit(
            &self,
            _request: MembershipBranchRecoveryCommit,
        ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError>
        {
            Err(MembershipBranchRecoveryChannelError::Rejected {
                source: anyhow::anyhow!("SECRET_ERROR"),
            })
        }
    }

    #[test]
    fn ledger_load_policy_records_errors_and_slow_successes() {
        assert!(!should_record_ledger_load(true, Duration::from_millis(49)));
        assert!(should_record_ledger_load(true, Duration::from_millis(50)));
        assert!(should_record_ledger_load(true, Duration::from_millis(51)));
        assert!(should_record_ledger_load(false, Duration::ZERO));
    }

    #[test]
    fn typed_errors_map_to_stable_safe_kinds() {
        assert_eq!(
            membership_ledger_error_kind(&MembershipLedgerError::RecoveryRequired),
            "recovery_required"
        );
        assert_eq!(
            membership_history_exchange_error_kind(&MembershipHistoryExchangeError::Transport),
            "transport"
        );
        assert_eq!(
            restricted_membership_delivery_error_kind(&RestrictedMembershipDeliveryError::Deferred),
            "deferred"
        );
        assert_eq!(
            group_update_dispatch_error_kind(&GroupUpdateDispatchError::Offline),
            "offline"
        );
        assert_eq!(
            membership_branch_recovery_error_kind(&MembershipBranchRecoveryChannelError::Invalid {
                source: anyhow::anyhow!("SECRET_ERROR"),
            }),
            "invalid"
        );
    }

    #[tokio::test]
    async fn ledger_decorator_calls_inner_once_and_preserves_typed_error() {
        let inner = Arc::new(FailingLedgerLoad {
            calls: AtomicUsize::new(0),
        });
        let observed = ObservedMembershipLedgerLoad {
            inner: inner.clone(),
        };

        let result = observed.load().await;

        assert!(matches!(
            result,
            Err(MembershipLedgerError::RecoveryRequired)
        ));
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn successful_ledger_commit_publishes_device_trust_revision() {
        let recorder = Arc::new(HostEventRecorder::default());
        let host_events = Arc::new(HostEventBus::new());
        host_events.register("test", recorder.clone());
        let observed = observe_membership_commit(Arc::new(SuccessfulLedgerCommit), host_events);
        let mut replacement = LoadedMembershipLedger::no_current_space();
        replacement.revision = 7;

        let committed = observed
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: 6,
                expected_history_digest: None,
                replacement,
            })
            .await
            .expect("membership commit should succeed");

        assert_eq!(committed.revision, 7);
        let events = recorder.events.lock().expect("host events lock");
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.first(),
            Some(HostEvent::Membership(
                MembershipHostEvent::LedgerCommitted { revision: 7 }
            ))
        ));
    }

    #[tokio::test]
    async fn failed_ledger_commit_does_not_publish_device_trust_change() {
        let recorder = Arc::new(HostEventRecorder::default());
        let host_events = Arc::new(HostEventBus::new());
        host_events.register("test", recorder.clone());
        let observed = observe_membership_commit(Arc::new(FailingLedgerCommit), host_events);

        let result = observed
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: 0,
                expected_history_digest: None,
                replacement: LoadedMembershipLedger::no_current_space(),
            })
            .await;

        assert!(matches!(result, Err(MembershipLedgerError::Conflict)));
        assert!(recorder.events.lock().expect("host events lock").is_empty());
    }

    #[tokio::test]
    async fn branch_decorator_preserves_error_source() {
        let observed = ObservedMembershipBranchRecoveryChannel {
            inner: Arc::new(FailingBranchRecoveryChannel),
        };
        let result = observed
            .request_membership_branch_group_info(MembershipBranchRecoveryRequest {
                peer_device_id: DeviceId::new("SECRET_DEVICE"),
                conflict_id: MembershipConflictId::from_bytes([1; 32]),
                target_branch_id: MembershipBranchId::from_bytes([2; 32]),
                recipient_member: MemberInstanceId::from_bytes([3; 32]),
            })
            .await;

        match result {
            Err(MembershipBranchRecoveryChannelError::Unavailable { source }) => {
                assert_eq!(source.to_string(), "SECRET_ERROR");
            }
            _ => panic!("unexpected branch recovery result"),
        }
    }

    #[test]
    fn branch_event_uses_only_fixed_fields_and_does_not_format_source() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let error = MembershipBranchRecoveryChannelError::Invalid {
            source: anyhow::anyhow!("SECRET_ERROR"),
        };

        tracing::dispatcher::with_default(&dispatch, || {
            record_branch_recovery(
                MembershipOperation::BranchRecoveryGroupInfo,
                Duration::from_millis(12),
                Some(&error),
            );
        });

        let output = writer.output();
        assert!(output.contains("uc.domain=\"space_membership\""));
        assert!(output.contains("uc.operation=\"membership_history_sync\""));
        assert!(output.contains("duration_ms=12"));
        assert!(output.contains("uc.outcome=\"error\""));
        assert!(output.contains("error.type=\"decode_failed\""));
        assert!(!output.contains("membership.performance"));
        assert!(!output.contains("SECRET_ERROR"));
    }
}
