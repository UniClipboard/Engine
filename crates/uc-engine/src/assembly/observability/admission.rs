use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    HandleAuthenticatedSpaceAdmissionMessagePort, SpaceAdmissionMessageReply,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

pub(crate) fn observe_admission_endpoint(
    inner: Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort>,
) -> Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort> {
    Arc::new(ObservedAdmissionEndpoint { inner })
}

struct ObservedAdmissionEndpoint {
    inner: Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort>,
}

#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for ObservedAdmissionEndpoint {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let started = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Sponsor,
            kind: DiagnosticSpanKind::Internal,
        });
        let result = self.inner.handle(message).instrument(span.clone()).await;
        span.in_scope(|| {
            let completion = match result.as_ref().err() {
                None => OperationCompletion::succeeded(
                    DiagnosticDomain::SpaceAdmission,
                    DiagnosticOperation::SpaceAdmission,
                    DiagnosticRole::Sponsor,
                    started.elapsed(),
                ),
                Some(error) => OperationCompletion::failed(
                    DiagnosticDomain::SpaceAdmission,
                    DiagnosticOperation::SpaceAdmission,
                    DiagnosticRole::Sponsor,
                    endpoint_error_type(error),
                    started.elapsed(),
                ),
            };
            complete_operation(completion);
        });
        result
    }
}

fn endpoint_error_type(
    error: &HandleAuthenticatedSpaceAdmissionMessageError,
) -> DiagnosticErrorType {
    match error {
        HandleAuthenticatedSpaceAdmissionMessageError::Invalid { .. }
        | HandleAuthenticatedSpaceAdmissionMessageError::OutOfOrder { .. } => {
            DiagnosticErrorType::DecodeFailed
        }
        HandleAuthenticatedSpaceAdmissionMessageError::Conflict { .. }
        | HandleAuthenticatedSpaceAdmissionMessageError::StateChanged { .. }
        | HandleAuthenticatedSpaceAdmissionMessageError::RecoveryRequired { .. } => {
            DiagnosticErrorType::Corrupt
        }
        HandleAuthenticatedSpaceAdmissionMessageError::Locked { .. }
        | HandleAuthenticatedSpaceAdmissionMessageError::Unavailable { .. } => {
            DiagnosticErrorType::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tracing::instrument::WithSubscriber as _;
    use uc_application::deps::{
        AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
        HandleAuthenticatedSpaceAdmissionMessagePort, SpaceAdmissionMessageReply,
    };
    use uc_core::membership::{
        AdmissionChannelPeerId, AdmissionMessageId, AdmissionPeerBinding, AdmissionRole,
        SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
    };

    use super::{endpoint_error_type, observe_admission_endpoint};

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

    struct FailingEndpoint {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl HandleAuthenticatedSpaceAdmissionMessagePort for FailingEndpoint {
        async fn handle(
            &self,
            _message: AuthenticatedSpaceAdmissionMessage,
        ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(HandleAuthenticatedSpaceAdmissionMessageError::unavailable(
                anyhow::anyhow!("PRIVATE_ENDPOINT_ERROR"),
            ))
        }
    }

    #[test]
    fn endpoint_error_mapping_is_total_and_stable() {
        assert_eq!(
            endpoint_error_type(&HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                anyhow::anyhow!("private"),
            )),
            uc_observability_contract::diagnostics::DiagnosticErrorType::DecodeFailed
        );
        assert_eq!(
            endpoint_error_type(&HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                anyhow::anyhow!("private"),
            )),
            uc_observability_contract::diagnostics::DiagnosticErrorType::Corrupt
        );
        assert_eq!(
            endpoint_error_type(&HandleAuthenticatedSpaceAdmissionMessageError::locked(
                anyhow::anyhow!("private"),
            )),
            uc_observability_contract::diagnostics::DiagnosticErrorType::Unavailable
        );
    }

    #[tokio::test]
    async fn endpoint_decorator_calls_once_preserves_source_and_records_only_fixed_fields() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let calls = Arc::new(AtomicUsize::new(0));
        let endpoint = observe_admission_endpoint(Arc::new(FailingEndpoint {
            calls: Arc::clone(&calls),
        }));
        let admission_id = SpaceAdmissionId::from_bytes([3; 32]).expect("admission identifier");
        let envelope = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            1,
            AdmissionMessageId::from_bytes([4; 32]).expect("message identifier"),
            Some(AdmissionMessageId::from_bytes([6; 32]).expect("predecessor identifier")),
            SpaceAdmissionBodyV1::CancelRequested,
        )
        .expect("admission message");
        let binding = AdmissionPeerBinding::new(
            AdmissionChannelPeerId::from_bytes([1; 32]).expect("local peer"),
            AdmissionChannelPeerId::from_bytes([2; 32]).expect("remote peer"),
        )
        .expect("distinct peers");
        let message = AuthenticatedSpaceAdmissionMessage::new(binding, envelope, [5; 32], None)
            .expect("authenticated message");

        let result = endpoint
            .handle(message)
            .with_subscriber(tracing::Dispatch::new(subscriber))
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        match result {
            Err(HandleAuthenticatedSpaceAdmissionMessageError::Unavailable { source }) => {
                assert_eq!(source.to_string(), "PRIVATE_ENDPOINT_ERROR");
            }
            _ => panic!("unexpected endpoint result"),
        }
        let output = String::from_utf8(writer.0.lock().expect("captured output").clone())
            .expect("UTF-8 output");
        assert!(output.contains("uc.operation=\"space_admission\""));
        assert!(output.contains("uc.role=\"sponsor\""));
        assert!(output.contains("error.type=\"unavailable\""));
        assert!(!output.contains("PRIVATE_ENDPOINT_ERROR"));
    }
}
