use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::Instrument;
use uc_application::deps::ApplicationClipboardAdapters;
use uc_application::facade::{
    ApplicationRuntimeError, ClipboardOutboundError, ClipboardOutboundOutcome,
    LocalClipboardOutcome, LocalClipboardProcessError, ResendEntryError, ResendReport,
    TransferProgressQueryError, TransferProgressReconciliation,
};
use uc_core::clipboard::{ClipboardEntry, ClipboardRepositoryError, ClipboardSelectionDecision};
use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, DispatchReport, SyncPayload,
};
use uc_core::ports::{
    CommitInboundReceivePort, InboundReceiveCommitError, InboundReceiveSettlement,
    SaveClipboardEntryPort, SystemClipboardPort,
};
use uc_core::SystemClipboardSnapshot;
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, ObservationContext, OperationCompletion, OperationContext,
};

pub(crate) fn observe_clipboard(
    mut adapters: ApplicationClipboardAdapters,
) -> ApplicationClipboardAdapters {
    adapters.clipboard_dispatch = Arc::new(ObservedClipboardDispatch {
        inner: adapters.clipboard_dispatch,
    });
    adapters
}

/// 只装饰既有完整存储与系统能力，不了解调用方内部步骤。
pub(crate) fn observe_clipboard_dependencies(deps: &mut uc_application::deps::ApplicationDeps) {
    deps.clipboard.entry_ports.save = Arc::new(ObservedSave {
        inner: Arc::clone(&deps.clipboard.entry_ports.save),
    });
    deps.clipboard.system_clipboard = Arc::new(ObservedSystemClipboard {
        inner: Arc::clone(&deps.clipboard.system_clipboard),
    });
    deps.storage.directory_receive.commit_inbound = Arc::new(ObservedInboundCommit {
        inner: Arc::clone(&deps.storage.directory_receive.commit_inbound),
    });
}

/// 包装已有完整本机复制调用；不读取内容、条目身份或业务步骤。
pub(crate) async fn observe_local_copy(
    action: impl std::future::Future<Output = Result<LocalClipboardOutcome, ApplicationRuntimeError>>,
) -> Result<LocalClipboardOutcome, ApplicationRuntimeError> {
    observe_local_clipboard(DiagnosticOperation::ClipboardCopyAndSync, action).await
}

pub(crate) async fn observe_explicit_send(
    action: impl std::future::Future<Output = Result<LocalClipboardOutcome, ApplicationRuntimeError>>,
) -> Result<LocalClipboardOutcome, ApplicationRuntimeError> {
    observe_local_clipboard(DiagnosticOperation::ClipboardSend, action).await
}

async fn observe_local_clipboard(
    operation: DiagnosticOperation,
    action: impl std::future::Future<Output = Result<LocalClipboardOutcome, ApplicationRuntimeError>>,
) -> Result<LocalClipboardOutcome, ApplicationRuntimeError> {
    let started = Instant::now();
    let span = clipboard_local_span(operation);
    let mut cancellation = ClipboardOperationCancellation {
        span: span.clone(),
        operation,
        started,
        finished: false,
    };
    let result = action.instrument(span.clone()).await;
    cancellation.finished = true;
    span.in_scope(|| {
        let completion = match &result {
            Ok(outcome) => copy_completion(operation, outcome, started.elapsed()),
            Err(error) => failure_completion(operation, error, started.elapsed()),
        };
        complete_operation(completion);
    });
    result
}

/// 只按既有错误类型分类：运行时不可用、派送取消和派送失败各有自己的类别。
/// 捕获失败的下层来源是未分类的 `anyhow` 链，类别保持 `internal`，细分需要 Application 先提供类型化错误。
fn failure_completion(
    operation: DiagnosticOperation,
    error: &ApplicationRuntimeError,
    elapsed: std::time::Duration,
) -> OperationCompletion {
    let domain = DiagnosticDomain::Clipboard;
    let role = DiagnosticRole::Local;
    let class = match error {
        ApplicationRuntimeError::Unavailable => DiagnosticErrorType::Unavailable,
        ApplicationRuntimeError::LocalClipboard {
            source:
                LocalClipboardProcessError::Dispatch {
                    source: ClipboardOutboundError::Cancelled(_),
                },
        } => return OperationCompletion::cancelled(domain, operation, role, elapsed),
        ApplicationRuntimeError::LocalClipboard {
            source: LocalClipboardProcessError::Dispatch { .. },
        } => DiagnosticErrorType::DeliveryFailed,
        ApplicationRuntimeError::LocalClipboard { .. } => DiagnosticErrorType::Internal,
    };
    OperationCompletion::failed(domain, operation, role, class, elapsed)
}

/// 只解释既有派送汇总，不读取目标身份，不改变派送或恢复策略。
fn copy_completion(
    operation: DiagnosticOperation,
    outcome: &LocalClipboardOutcome,
    elapsed: std::time::Duration,
) -> OperationCompletion {
    let domain = DiagnosticDomain::Clipboard;
    let role = DiagnosticRole::Local;
    let LocalClipboardOutcome::Completed(completion) = outcome else {
        return OperationCompletion::skipped(domain, operation, role, elapsed);
    };
    let Some(ClipboardOutboundOutcome::Dispatched {
        accepted,
        duplicate,
        offline,
        errored,
        pending,
        ..
    }) = &completion.dispatch
    else {
        return OperationCompletion::skipped(domain, operation, role, elapsed);
    };
    delivery_completion(
        operation,
        *accepted != 0 || *duplicate != 0,
        *offline != 0 || *pending != 0,
        *errored != 0,
        elapsed,
    )
}

fn delivery_completion(
    operation: DiagnosticOperation,
    completed: bool,
    waiting: bool,
    failed: bool,
    elapsed: std::time::Duration,
) -> OperationCompletion {
    let domain = DiagnosticDomain::Clipboard;
    let role = DiagnosticRole::Local;
    if completed {
        if waiting || failed {
            OperationCompletion::partial(domain, operation, role, elapsed)
        } else {
            OperationCompletion::succeeded(domain, operation, role, elapsed)
        }
    } else if waiting {
        OperationCompletion::deferred(domain, operation, role, elapsed)
    } else if failed {
        OperationCompletion::failed(
            domain,
            operation,
            role,
            DiagnosticErrorType::DeliveryFailed,
            elapsed,
        )
    } else {
        OperationCompletion::skipped(domain, operation, role, elapsed)
    }
}

pub(crate) async fn observe_resend(
    action: impl std::future::Future<Output = Result<ResendReport, ResendEntryError>>,
) -> Result<ResendReport, ResendEntryError> {
    let operation = DiagnosticOperation::ClipboardResend;
    let started = Instant::now();
    let span = clipboard_local_span(operation);
    let mut cancellation = ClipboardOperationCancellation {
        span: span.clone(),
        operation,
        started,
        finished: false,
    };
    let observation = span.in_scope(ObservationContext::capture);
    let result = observation.scope(action.instrument(span.clone())).await;
    cancellation.finished = true;
    span.in_scope(|| {
        let domain = DiagnosticDomain::Clipboard;
        let role = DiagnosticRole::Local;
        let elapsed = started.elapsed();
        let completion = match &result {
            Ok(report) => delivery_completion(
                operation,
                report.accepted != 0
                    || report.duplicate != 0
                    || report.reconciled.iter().any(|target| {
                        target.progress.state == TransferProgressReconciliation::Settled
                            && target
                                .progress
                                .current
                                .as_ref()
                                .is_some_and(|view| view.summary.counts.completed != 0)
                    }),
                report.offline != 0
                    || report.pending != 0
                    || report.reconciled.iter().any(|target| {
                        !target.new_transfer_requested
                            && target.progress.state != TransferProgressReconciliation::Settled
                    }),
                report.errored != 0
                    || report.reconciled.iter().any(|target| {
                        target.progress.state == TransferProgressReconciliation::Settled
                            && !target.new_transfer_requested
                            && target.progress.current.as_ref().is_some_and(|view| {
                                view.summary.counts.failed != 0
                                    || view.summary.counts.cancelled != 0
                            })
                    }),
                elapsed,
            ),
            Err(
                ResendEntryError::SynchronizationDisabled | ResendEntryError::NoEligibleTargets,
            ) => OperationCompletion::skipped(domain, operation, role, elapsed),
            Err(
                ResendEntryError::EntryNotFound(_)
                | ResendEntryError::EntryNotResendable { .. }
                | ResendEntryError::TargetNotTrusted(_),
            ) => OperationCompletion::rejected(domain, operation, role, elapsed),
            Err(ResendEntryError::Reconciliation(
                TransferProgressQueryError::AccessDenied(_)
                | TransferProgressQueryError::InvalidInput(_),
            )) => OperationCompletion::rejected(domain, operation, role, elapsed),
            Err(ResendEntryError::Reconciliation(TransferProgressQueryError::InvalidData(_))) => {
                OperationCompletion::failed(
                    domain,
                    operation,
                    role,
                    DiagnosticErrorType::Corrupt,
                    elapsed,
                )
            }
            Err(ResendEntryError::Reconciliation(TransferProgressQueryError::Unavailable(_))) => {
                OperationCompletion::failed(
                    domain,
                    operation,
                    role,
                    DiagnosticErrorType::Unavailable,
                    elapsed,
                )
            }
            Err(ResendEntryError::Storage(_)) => OperationCompletion::failed(
                domain,
                operation,
                role,
                DiagnosticErrorType::Storage,
                elapsed,
            ),
            Err(ResendEntryError::Dispatch(_)) => OperationCompletion::failed(
                domain,
                operation,
                role,
                DiagnosticErrorType::DeliveryFailed,
                elapsed,
            ),
        };
        complete_operation(completion);
    });
    result
}

async fn observe_local_operation<T, E>(
    operation: DiagnosticOperation,
    error_type: DiagnosticErrorType,
    action: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let started = Instant::now();
    let span = clipboard_local_span(operation);
    let mut cancellation = ClipboardOperationCancellation {
        span: span.clone(),
        operation,
        started,
        finished: false,
    };
    let result = action.instrument(span.clone()).await;
    cancellation.finished = true;
    span.in_scope(|| complete_local(operation, error_type, started, result.is_ok()));
    result
}

/// 调用被取消时也恰好结算一次，不把没有完成的动作标为成功。
struct ClipboardOperationCancellation {
    span: tracing::Span,
    operation: DiagnosticOperation,
    started: Instant,
    finished: bool,
}

impl Drop for ClipboardOperationCancellation {
    fn drop(&mut self) {
        if !self.finished {
            self.span.in_scope(|| {
                complete_operation(OperationCompletion::cancelled(
                    DiagnosticDomain::Clipboard,
                    self.operation,
                    DiagnosticRole::Local,
                    self.started.elapsed(),
                ))
            });
        }
    }
}

fn clipboard_local_span(operation: DiagnosticOperation) -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::Clipboard,
        operation,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    })
}

fn complete_local(
    operation: DiagnosticOperation,
    error_type: DiagnosticErrorType,
    started: Instant,
    succeeded: bool,
) {
    complete_operation(if succeeded {
        OperationCompletion::succeeded(
            DiagnosticDomain::Clipboard,
            operation,
            DiagnosticRole::Local,
            started.elapsed(),
        )
    } else {
        OperationCompletion::failed(
            DiagnosticDomain::Clipboard,
            operation,
            DiagnosticRole::Local,
            error_type,
            started.elapsed(),
        )
    });
}

struct ObservedSave {
    inner: Arc<dyn SaveClipboardEntryPort>,
}

#[async_trait]
impl SaveClipboardEntryPort for ObservedSave {
    async fn save_entry_and_selection(
        &self,
        entry: &ClipboardEntry,
        selection: &ClipboardSelectionDecision,
    ) -> Result<(), ClipboardRepositoryError> {
        observe_local_operation(
            DiagnosticOperation::ClipboardPersist,
            DiagnosticErrorType::Storage,
            self.inner.save_entry_and_selection(entry, selection),
        )
        .await
    }
}

struct ObservedInboundCommit {
    inner: Arc<dyn CommitInboundReceivePort>,
}

#[async_trait]
impl CommitInboundReceivePort for ObservedInboundCommit {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        observe_local_operation(
            DiagnosticOperation::ClipboardPersist,
            DiagnosticErrorType::Storage,
            self.inner.commit_inbound_receive(settlement),
        )
        .await
    }
}

struct ObservedSystemClipboard {
    inner: Arc<dyn SystemClipboardPort>,
}

impl SystemClipboardPort for ObservedSystemClipboard {
    fn read_snapshot(&self) -> anyhow::Result<SystemClipboardSnapshot> {
        self.inner.read_snapshot()
    }
    fn write_snapshot(&self, snapshot: SystemClipboardSnapshot) -> anyhow::Result<()> {
        let operation = DiagnosticOperation::ClipboardWriteSystem;
        let started = Instant::now();
        clipboard_local_span(operation).in_scope(|| {
            let result = self.inner.write_snapshot(snapshot);
            complete_local(
                operation,
                DiagnosticErrorType::Unavailable,
                started,
                result.is_ok(),
            );
            result
        })
    }
}

struct ObservedClipboardDispatch {
    inner: Arc<dyn ClipboardDispatchPort>,
}

#[async_trait]
impl ClipboardDispatchPort for ObservedClipboardDispatch {
    async fn dispatch(
        &self,
        target: &DeviceId,
        header: &ClipboardHeader,
        payload: SyncPayload,
    ) -> DispatchReport {
        let started = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Clipboard,
            operation: DiagnosticOperation::ClipboardDispatch,
            role: DiagnosticRole::Client,
            kind: DiagnosticSpanKind::Client,
        });
        let report = self
            .inner
            .dispatch(target, header, payload)
            .instrument(span.clone())
            .await;
        span.in_scope(|| record_completion(started, &report));
        report
    }
}

fn record_completion(started: Instant, report: &DispatchReport) {
    let completion = match &report.outcome {
        Ok(_) => OperationCompletion::succeeded(
            DiagnosticDomain::Clipboard,
            DiagnosticOperation::ClipboardDispatch,
            DiagnosticRole::Client,
            started.elapsed(),
        ),
        Err(error) => OperationCompletion::failed(
            DiagnosticDomain::Clipboard,
            DiagnosticOperation::ClipboardDispatch,
            DiagnosticRole::Client,
            error_type(error),
            started.elapsed(),
        ),
    };
    complete_operation(completion);
}

fn error_type(error: &ClipboardDispatchError) -> DiagnosticErrorType {
    match error {
        ClipboardDispatchError::Offline => DiagnosticErrorType::AddressUnavailable,
        ClipboardDispatchError::LocalPolicyExceeded(_) => DiagnosticErrorType::LocalPolicyExceeded,
        ClipboardDispatchError::PeerRejected(_) => DiagnosticErrorType::PeerRejected,
        ClipboardDispatchError::PeerIncompatible => DiagnosticErrorType::PeerIncompatible,
        ClipboardDispatchError::Io(_) => DiagnosticErrorType::StreamFailed,
        ClipboardDispatchError::Internal(_) => DiagnosticErrorType::Internal,
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn resend_uses_delivery_results_and_preserves_refusal() {
        use super::observe_resend;
        for (report, expected) in [
            (
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 1,
                    duplicate: 0,
                    offline: 1,
                    errored: 0,
                    pending: 0,
                },
                "partial",
            ),
            (
                ResendReport {
                    reconciled: Vec::new(),
                    accepted: 0,
                    duplicate: 0,
                    offline: 1,
                    errored: 0,
                    pending: 0,
                },
                "deferred",
            ),
        ] {
            let writer = CapturedWriter::default();
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(writer.clone())
                .finish();
            let result = observe_resend(async { Ok(report.clone()) })
                .with_subscriber(subscriber)
                .await
                .expect("result");
            assert_eq!(result, report);
            let output = String::from_utf8(writer.0.lock().expect("logs").clone()).expect("utf8");
            assert!(output.contains("clipboard.resend"));
            assert!(output.contains(&format!("uc.outcome=\"{expected}\"")));
        }
        assert!(matches!(
            observe_resend(async { Err(ResendEntryError::SynchronizationDisabled) }).await,
            Err(ResendEntryError::SynchronizationDisabled)
        ));
    }
    use uc_application::facade::{
        ClipboardOutboundOutcome, LocalClipboardCompletion, LocalClipboardIndexStatus,
        LocalClipboardOutcome,
    };

    #[tokio::test]
    async fn copy_result_reflects_delivery_instead_of_only_returning_ok() {
        for (accepted, duplicate, offline, errored, pending, expected) in [
            (0, 0, 1, 0, 0, "deferred"),
            (0, 0, 0, 1, 0, "error"),
            (1, 0, 0, 1, 0, "partial"),
            (1, 0, 1, 0, 0, "partial"),
            (0, 1, 0, 0, 1, "partial"),
            (0, 0, 1, 1, 0, "deferred"),
            (0, 0, 0, 0, 1, "deferred"),
            (0, 0, 0, 0, 0, "skipped"),
            (1, 0, 0, 0, 0, "ok"),
            (0, 1, 0, 0, 0, "ok"),
            (usize::MAX, usize::MAX, 0, 0, 0, "ok"),
        ] {
            let outcome = LocalClipboardOutcome::Completed(LocalClipboardCompletion {
                entry_id: "PRIVATE_ENTRY".into(),
                snapshot_hash: "PRIVATE_HASH".into(),
                deduplicated: false,
                index: LocalClipboardIndexStatus::NotAttempted,
                dispatch: Some(ClipboardOutboundOutcome::Dispatched {
                    snapshot_hash: "PRIVATE_HASH".into(),
                    per_target: vec![],
                    accepted,
                    duplicate,
                    offline,
                    errored,
                    pending,
                    pending_targets: vec![],
                    at_ms: 0,
                    blob_ref_count: 0,
                }),
            });
            assert_copy_observation(outcome, expected).await;
        }
        assert_copy_observation(LocalClipboardOutcome::Empty, "skipped").await;
        for dispatch in [
            None,
            Some(ClipboardOutboundOutcome::Skipped {
                reason: "PRIVATE_REASON".into(),
            }),
        ] {
            assert_copy_observation(
                LocalClipboardOutcome::Completed(LocalClipboardCompletion {
                    entry_id: "PRIVATE_ENTRY".into(),
                    snapshot_hash: "PRIVATE_HASH".into(),
                    deduplicated: false,
                    index: LocalClipboardIndexStatus::NotAttempted,
                    dispatch,
                }),
                "skipped",
            )
            .await;
        }
    }

    async fn assert_copy_observation(outcome: LocalClipboardOutcome, expected: &str) {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(writer.clone()),
        );
        let result =
            observe_local_copy(async { Ok::<_, ApplicationRuntimeError>(outcome.clone()) })
                .with_subscriber(subscriber)
                .await;
        assert_eq!(result.ok(), Some(outcome));
        let output = String::from_utf8(writer.0.lock().expect("logs").clone()).expect("utf8");
        assert!(
            output.contains(&format!("uc.outcome=\"{expected}\"")),
            "expected {expected}, got {output}"
        );
        assert_eq!(
            output
                .lines()
                .filter(|line| line.contains("event.name=\"uc.operation.completed\""))
                .count(),
            1
        );
        assert!(!output.contains("PRIVATE_"));
    }

    #[tokio::test]
    async fn copy_failures_get_a_class_per_error_variant_and_keep_their_source() {
        use std::error::Error as _;
        use uc_application::deps::ActivePullPreparationCancelled;
        let private = || {
            anyhow::Error::new(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "PRIVATE_CAPTURE_ERROR",
            ))
            .context("PRIVATE_CAPTURE_CONTEXT")
        };
        let cases: Vec<(ApplicationRuntimeError, &str, Option<&str>)> = vec![
            (
                ApplicationRuntimeError::Unavailable,
                "error",
                Some("unavailable"),
            ),
            (
                ApplicationRuntimeError::LocalClipboard {
                    source: LocalClipboardProcessError::Capture {
                        source: uc_application::facade::ClipboardCaptureFacadeError::Internal(
                            private(),
                        ),
                    },
                },
                "error",
                Some("internal"),
            ),
            (
                ApplicationRuntimeError::LocalClipboard {
                    source: LocalClipboardProcessError::Dispatch {
                        source: ClipboardOutboundError::Internal(private()),
                    },
                },
                "error",
                Some("delivery_failed"),
            ),
            (
                ApplicationRuntimeError::LocalClipboard {
                    source: LocalClipboardProcessError::Dispatch {
                        source: ClipboardOutboundError::Cancelled(ActivePullPreparationCancelled),
                    },
                },
                "cancelled",
                None,
            ),
        ];
        for (error, outcome, error_type) in cases {
            let writer = CapturedWriter::default();
            let subscriber = tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .without_time()
                    .with_ansi(false)
                    .with_writer(writer.clone()),
            );
            let result = observe_local_copy(async { Err::<LocalClipboardOutcome, _>(error) })
                .with_subscriber(subscriber)
                .await;
            let error = result.expect_err("failure unchanged");
            if error_type == Some("internal") || error_type == Some("delivery_failed") {
                assert!(error.source().is_some(), "typed source is preserved");
            }
            let output = String::from_utf8(writer.0.lock().expect("logs").clone()).expect("utf8");
            assert!(
                output.contains(&format!("uc.outcome=\"{outcome}\"")),
                "{output}"
            );
            match error_type {
                Some(class) => assert!(
                    output.contains(&format!("error.type=\"{class}\"")),
                    "expected {class}: {output}"
                ),
                None => assert!(!output.contains("error.type=\""), "{output}"),
            }
            assert_eq!(
                output
                    .lines()
                    .filter(|line| line.contains("event.name=\"uc.operation.completed\""))
                    .count(),
                1
            );
            assert!(!output.contains("PRIVATE_"));
        }
    }
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;
    use uc_core::ports::ConnectionChannel;

    struct FailingDispatch {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ClipboardDispatchPort for FailingDispatch {
        async fn dispatch(
            &self,
            _target: &DeviceId,
            _header: &ClipboardHeader,
            _payload: SyncPayload,
        ) -> DispatchReport {
            self.calls.fetch_add(1, Ordering::SeqCst);
            DispatchReport {
                transport: ConnectionChannel::Direct,
                outcome: Err(ClipboardDispatchError::PeerRejected(
                    "PRIVATE_REMOTE_ERROR".to_owned(),
                )),
            }
        }
    }

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for CapturedWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> MakeWriter<'writer> for CapturedWriter {
        type Writer = Self;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn cancelled_copy_records_one_cancelled_result() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(writer.clone()),
        );
        // 当前线程测试中保持 subscriber 到 future 析构结束；取消发生在 poll 之外。
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            observe_local_copy(std::future::pending::<
                Result<LocalClipboardOutcome, ApplicationRuntimeError>,
            >()),
        )
        .await;
        assert!(result.is_err());
        let output = String::from_utf8(writer.0.lock().expect("logs").clone()).expect("utf8");
        assert_eq!(
            output
                .lines()
                .filter(
                    |line| line.contains("event.name=\"uc.operation.completed\"")
                        && line.contains("uc.outcome=\"cancelled\"")
                )
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn decorator_calls_once_preserves_result_and_records_only_stable_fields() {
        let inner = Arc::new(FailingDispatch {
            calls: AtomicUsize::new(0),
        });
        let observed = ObservedClipboardDispatch {
            inner: Arc::clone(&inner) as Arc<_>,
        };
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(writer.clone()),
        );
        let target = DeviceId::new("PRIVATE_DEVICE_ID");
        let header = ClipboardHeader {
            source_entry: None,
            version: ClipboardHeader::CURRENT_VERSION,
            snapshot_hash: "PRIVATE_SNAPSHOT_HASH".to_owned(),
            captured_at_ms: 1,
            origin_device_id: "PRIVATE_ORIGIN_ID".to_owned(),
            origin_device_name: "PRIVATE_DEVICE_NAME".to_owned(),
            payload_version: 3,
        };
        let report = observed
            .dispatch(
                &target,
                &header,
                SyncPayload {
                    ciphertext: b"PRIVATE_PAYLOAD".to_vec().into(),
                },
            )
            .with_subscriber(subscriber)
            .await;

        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            report.outcome,
            Err(ClipboardDispatchError::PeerRejected(_))
        ));
        let output = String::from_utf8(
            writer
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone(),
        )
        .expect("UTF-8 logs");
        assert!(output.contains("error.type=\"peer_rejected\""));
        for secret in [
            "PRIVATE_REMOTE_ERROR",
            "PRIVATE_DEVICE_ID",
            "PRIVATE_SNAPSHOT_HASH",
            "PRIVATE_ORIGIN_ID",
            "PRIVATE_DEVICE_NAME",
            "PRIVATE_PAYLOAD",
        ] {
            assert!(!output.contains(secret), "leaked {secret}");
        }
    }
}
