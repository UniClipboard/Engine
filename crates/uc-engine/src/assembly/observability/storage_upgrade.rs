use std::time::Instant;

use uc_infra::security::{ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome};

pub(crate) fn record_profile_storage_upgrade(
    started: Instant,
    result: &Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError>,
) {
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match result {
        Ok(outcome) => tracing::info!(
            target: "storage.performance",
            operation = "profile_storage_upgrade",
            elapsed_ms,
            outcome = "ok",
            result = outcome_kind(outcome),
            "profile storage upgrade completed"
        ),
        Err(error) => tracing::info!(
            target: "storage.performance",
            operation = "profile_storage_upgrade",
            elapsed_ms,
            outcome = "error",
            error_kind = error_kind(error),
            "profile storage upgrade completed"
        ),
    }
}

fn outcome_kind(outcome: &ProfileStorageUpgradeOutcome) -> &'static str {
    match outcome {
        ProfileStorageUpgradeOutcome::UpToDate => "up_to_date",
        ProfileStorageUpgradeOutcome::Upgraded => "upgraded",
        ProfileStorageUpgradeOutcome::FreshReady { .. } => "fresh_ready",
        ProfileStorageUpgradeOutcome::LegacyReady { .. } => "legacy_ready",
        ProfileStorageUpgradeOutcome::Pending => "pending",
        ProfileStorageUpgradeOutcome::Busy => "busy",
    }
}

fn error_kind(error: &ProfileStorageUpgradeError) -> &'static str {
    match error {
        ProfileStorageUpgradeError::Storage { .. } => "storage",
        ProfileStorageUpgradeError::Security { .. } => "security",
        ProfileStorageUpgradeError::Corrupt { .. } => "corrupt",
        ProfileStorageUpgradeError::SourceChanged => "source_changed",
        ProfileStorageUpgradeError::Manifest { .. } => "manifest",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use uc_infra::security::{ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome};

    use super::record_profile_storage_upgrade;

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
        assert!(output.contains("profile_storage_upgrade"));
        assert!(output.contains("result=\"upgraded\""));
        assert!(output.contains("outcome=\"ok\""));
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error_kind=\"security\""));
        assert!(!output.contains(secret));
    }
}
