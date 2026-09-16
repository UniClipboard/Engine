use std::time::Instant;
use tracing::Instrument;

use uc_infra::security::{ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome};
use uc_observability_contract::diagnostics::{
    complete_operation, complete_unassociated_operation, operation_span, DiagnosticDomain,
    DiagnosticErrorType, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
    OperationCompletion, OperationContext,
};

fn profile_storage_upgrade_span() -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    })
}

/// 包装既有完整能力，不探查升级内部阶段；结果和来源原样交回调用方。
pub(crate) async fn observe_profile_storage_upgrade(
    action: impl std::future::Future<
        Output = Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError>,
    >,
) -> Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError> {
    let started = Instant::now();
    let span = profile_storage_upgrade_span();
    let mut guard = UpgradeObservationGuard {
        span: span.clone(),
        started,
        completed: false,
    };
    let result = action.instrument(span.clone()).await;
    guard.completed = true;
    if matches!(result, Ok(ProfileStorageUpgradeOutcome::UpToDate)) {
        // 完成后才能知道这是空检查。编码端有意省略该节点，不计为丢失；日志不关联到被省略的节点。
        span.record("uc.outcome", "skipped");
        complete_unassociated_operation(upgrade_completion(started, &result));
    } else {
        if matches!(result, Ok(ProfileStorageUpgradeOutcome::FreshReady { .. })) {
            span.record("uc.display.name", "storage.initialize_profile");
        }
        span.in_scope(|| record_profile_storage_upgrade(started, &result));
    }
    result
}

struct UpgradeObservationGuard {
    span: tracing::Span,
    started: Instant,
    completed: bool,
}

impl Drop for UpgradeObservationGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.span.in_scope(|| {
                complete_operation(OperationCompletion::cancelled(
                    DiagnosticDomain::Storage,
                    DiagnosticOperation::ProfileStorageUpgrade,
                    DiagnosticRole::Local,
                    self.started.elapsed(),
                ))
            });
        }
    }
}

fn record_profile_storage_upgrade(
    started: Instant,
    result: &Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError>,
) {
    complete_operation(upgrade_completion(started, result));
}

fn upgrade_completion(
    started: Instant,
    result: &Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError>,
) -> OperationCompletion {
    match result {
        Ok(ProfileStorageUpgradeOutcome::UpToDate) => OperationCompletion::skipped(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            started.elapsed(),
        ),
        Ok(ProfileStorageUpgradeOutcome::Pending) => OperationCompletion::partial(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            started.elapsed(),
        ),
        Ok(ProfileStorageUpgradeOutcome::Busy) => OperationCompletion::deferred(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            started.elapsed(),
        ),
        Ok(
            ProfileStorageUpgradeOutcome::Upgraded
            | ProfileStorageUpgradeOutcome::FreshReady { .. }
            | ProfileStorageUpgradeOutcome::LegacyReady { .. },
        ) => OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            started.elapsed(),
        ),
        Err(error) => OperationCompletion::failed(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            error_type(error),
            started.elapsed(),
        ),
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
    #[tokio::test]
    async fn wrapper_keeps_result_and_records_completion_inside_the_operation() {
        use tracing::instrument::WithSubscriber;
        for (outcome, expected) in [
            (ProfileStorageUpgradeOutcome::Upgraded, "ok"),
            (ProfileStorageUpgradeOutcome::Pending, "partial"),
            (ProfileStorageUpgradeOutcome::Busy, "deferred"),
            (
                ProfileStorageUpgradeOutcome::FreshReady {
                    profile_data_generation: [1; 16],
                    space_control_generation: [2; 16],
                },
                "ok",
            ),
        ] {
            let writer = CapturedWriter::default();
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(writer.clone())
                .finish();
            let result = super::observe_profile_storage_upgrade(async { Ok(outcome.clone()) })
                .with_subscriber(subscriber)
                .await
                .expect("result");
            assert_eq!(result, outcome);
            let output = writer.output();
            let line = output
                .lines()
                .find(|line| line.contains("event.name=\"uc.operation.completed\""))
                .expect("completion");
            assert!(
                line.contains("uc.operation{"),
                "completion must remain within the operation"
            );
            assert!(line.contains(&format!("uc.outcome=\"{expected}\"")));
            if matches!(outcome, ProfileStorageUpgradeOutcome::FreshReady { .. }) {
                assert!(line.contains("storage.initialize_profile"));
            }
        }
    }

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
    fn unfinished_upgrade_is_not_reported_as_success() {
        for (outcome, expected) in [
            (ProfileStorageUpgradeOutcome::Pending, "partial"),
            (ProfileStorageUpgradeOutcome::Busy, "deferred"),
            (ProfileStorageUpgradeOutcome::UpToDate, "skipped"),
        ] {
            let writer = CapturedWriter::default();
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(writer.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                profile_storage_upgrade_span()
                    .in_scope(|| record_profile_storage_upgrade(Instant::now(), &Ok(outcome)));
            });
            assert!(
                writer
                    .output()
                    .contains(&format!("uc.outcome=\"{expected}\"")),
                "expected {expected}: {}",
                writer.output()
            );
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
