use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::Instrument;
use uc_application::deps::ApplicationClipboardAdapters;
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
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
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
pub(crate) async fn observe_local_copy<T, E>(
    action: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    observe_local_operation(
        DiagnosticOperation::ClipboardCopyAndSync,
        DiagnosticErrorType::Internal,
        action,
    )
    .await
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
            observe_local_copy(std::future::pending::<Result<(), ()>>()),
        )
        .await;
        assert!(result.is_err());
        let output = String::from_utf8(writer.0.lock().expect("logs").clone()).expect("utf8");
        assert_eq!(output.matches("uc.outcome=\"cancelled\"").count(), 1);
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
