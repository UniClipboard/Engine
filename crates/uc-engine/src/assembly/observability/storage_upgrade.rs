use std::time::Instant;

use uc_infra::security::{ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

pub(crate) fn profile_storage_upgrade_span() -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
        flow: None,
    })
}

pub(crate) fn record_profile_storage_upgrade(
    started: Instant,
    result: &Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError>,
) {
    match result {
        Ok(_) => complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            started.elapsed(),
        )),
        Err(error) => complete_operation(OperationCompletion::failed(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            error_type(error),
            started.elapsed(),
        )),
    }
}

fn error_type(error: &ProfileStorageUpgradeError) -> DiagnosticErrorType {
    match error {
        ProfileStorageUpgradeError::Storage { .. } => DiagnosticErrorType::Storage,
        ProfileStorageUpgradeError::Security { .. } => DiagnosticErrorType::Security,
        ProfileStorageUpgradeError::Corrupt { .. } => DiagnosticErrorType::Corrupt,
        ProfileStorageUpgradeError::SourceChanged => DiagnosticErrorType::SourceChanged,
        ProfileStorageUpgradeError::Manifest { .. } => DiagnosticErrorType::Manifest,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use uc_infra::security::{ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome};

    use super::{profile_storage_upgrade_span, record_profile_storage_upgrade};

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

    #[test]
    fn records_safe_upgrade_outcomes_and_error_kinds() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let secret = "SECRET_UPGRADE_SOURCE";

        tracing::dispatcher::with_default(&dispatch, || {
            let span = profile_storage_upgrade_span();
            let _entered = span.enter();
            record_profile_storage_upgrade(
                Instant::now() - Duration::from_millis(12),
                &Ok(ProfileStorageUpgradeOutcome::Upgraded),
            );
            record_profile_storage_upgrade(
                Instant::now(),
                &Err(ProfileStorageUpgradeError::Security {
                    source: anyhow::anyhow!(secret),
                }),
            );
        });

        let output = writer.output();
        assert!(output.contains("uc.operation"));
        assert!(output.contains("profile_storage_upgrade"));
        assert!(output.contains("outcome=\"ok\""));
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error.type=\"security\""));
        assert!(!output.contains(secret));
    }
}
