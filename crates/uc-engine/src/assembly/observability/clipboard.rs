use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::Instrument;
use uc_application::deps::ApplicationClipboardAdapters;
use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, DispatchReport, SyncPayload,
};
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
