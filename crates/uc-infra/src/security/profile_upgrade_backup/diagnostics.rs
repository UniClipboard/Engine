use uc_application::deps::ProfileUpgradeBackupError;
use uc_core::ports::SecureStorageError;
use uc_observability_contract::diagnostics::record_profile_upgrade_backup_failure;

use crate::security::ProfileBackupArchiveError;

/// 只为本地备份诊断记录稳定且不泄露隐私的元数据。
///
/// 此分类依赖具体存储错误，因此保留在 Infra。它不创建业务 trace，也不通过 port
/// 暴露备份内部细节。
pub(super) fn record_backup_failure(
    fallback_action: &'static str,
    error: &ProfileUpgradeBackupError,
) {
    let backup_action = find_source::<BackupActionError>(error)
        .map(|source| source.action)
        .unwrap_or(fallback_action);
    let io = find_source::<std::io::Error>(error);
    let io_kind = io.map(|source| format!("{:?}", source.kind()));
    record_profile_upgrade_backup_failure(
        backup_action,
        classify_error(error),
        io_kind.as_deref(),
        io.and_then(std::io::Error::raw_os_error),
    );
}

#[derive(Debug, thiserror::Error)]
#[error("profile backup action failed")]
struct BackupActionError {
    action: &'static str,
    #[source]
    source: anyhow::Error,
}

pub(super) fn with_backup_action(
    action: &'static str,
    error: ProfileUpgradeBackupError,
) -> ProfileUpgradeBackupError {
    if find_source::<BackupActionError>(&error).is_some() {
        return error;
    }
    ProfileUpgradeBackupError {
        source: BackupActionError {
            action,
            source: error.source,
        }
        .into(),
    }
}

fn classify_error(error: &(dyn std::error::Error + 'static)) -> &'static str {
    let io = find_source::<std::io::Error>(error);
    if let Some(archive) = find_source::<ProfileBackupArchiveError>(error) {
        return match archive {
            ProfileBackupArchiveError::Storage { .. } => io.map(classify_io).unwrap_or("storage"),
            ProfileBackupArchiveError::SourceChanged => "source_changed",
            ProfileBackupArchiveError::StateChanged => "verification_failed",
            ProfileBackupArchiveError::OverlappingDirectories => "overlapping_directories",
        };
    }
    if let Some(storage) = find_source::<SecureStorageError>(error) {
        return match storage {
            SecureStorageError::Unavailable(_) => "protection_unavailable",
            SecureStorageError::PermissionDenied(_) => "permission_denied",
            SecureStorageError::Corrupt(_) => "invalid_protection_record",
            SecureStorageError::Other(_) => "protection_storage",
        };
    }
    if find_source::<serde_json::Error>(error).is_some()
        || find_source::<uuid::Error>(error).is_some()
    {
        return "invalid_backup_record";
    }
    if let Some(io) = io {
        return classify_io(io);
    }
    if find_source::<tokio::task::JoinError>(error).is_some() {
        return "backup_task_failed";
    }
    "backup_internal"
}

fn classify_io(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "permission_denied",
        std::io::ErrorKind::StorageFull => "storage_full",
        std::io::ErrorKind::WouldBlock => "busy",
        std::io::ErrorKind::InvalidData => "invalid_backup_data",
        std::io::ErrorKind::NotFound => "missing_backup_data",
        _ => "storage",
    }
}

fn find_source<'a, T>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a T>
where
    T: std::error::Error + 'static,
{
    let mut current = Some(error);
    while let Some(source) = current {
        if let Some(found) = source.downcast_ref::<T>() {
            return Some(found);
        }
        current = source.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing_subscriber::prelude::*;

    struct Fields(BTreeMap<String, String>);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }
    }

    struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut fields = Fields(BTreeMap::new());
            event.record(&mut fields);
            self.0.lock().unwrap().push(fields.0);
        }
    }

    fn capture(error: ProfileUpgradeBackupError) -> BTreeMap<String, String> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(events.clone()));
        tracing::subscriber::with_default(subscriber, || {
            record_backup_failure("capture_profile", &error);
        });
        let mut events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        events.pop().unwrap()
    }

    #[test]
    fn permission_failure_records_action_and_system_code_without_source_text() {
        let fields = capture(ProfileUpgradeBackupError {
            source: anyhow::Error::new(std::io::Error::from_raw_os_error(13))
                .context("private-path/secret-name"),
        });

        assert_eq!(fields["backup_action"], "capture_profile");
        assert_eq!(fields["error_kind"], "permission_denied");
        assert_eq!(fields["io_error_code"], "13");
        assert_eq!(fields["retryable"], "true");
        assert!(!format!("{fields:?}").contains("private-path"));
        assert!(!format!("{fields:?}").contains("secret-name"));
    }

    #[test]
    fn archive_source_change_has_a_stable_non_io_classification() {
        let error = ProfileUpgradeBackupError {
            source: ProfileBackupArchiveError::SourceChanged.into(),
        };
        let fields = capture(with_backup_action("capture_profile_files", error));

        assert_eq!(fields["backup_action"], "capture_profile_files");
        assert_eq!(fields["error_kind"], "source_changed");
        assert!(!fields.contains_key("io_error_kind"));
        assert!(!fields.contains_key("io_error_code"));
    }

    #[test]
    fn outer_action_does_not_replace_the_more_specific_inner_action() {
        let error = ProfileUpgradeBackupError {
            source: ProfileBackupArchiveError::SourceChanged.into(),
        };
        let error = with_backup_action("capture_profile_files", error);
        let fields = capture(with_backup_action("capture_profile", error));

        assert_eq!(fields["backup_action"], "capture_profile_files");
    }

    #[test]
    fn archive_storage_failure_keeps_the_specific_io_classification() {
        let fields = capture(ProfileUpgradeBackupError {
            source: ProfileBackupArchiveError::Storage {
                source: std::io::Error::from_raw_os_error(28),
            }
            .into(),
        });

        assert_eq!(fields["error_kind"], "storage_full");
        assert_eq!(fields["io_error_code"], "28");
    }

    #[test]
    fn secure_storage_failure_does_not_expose_its_message() {
        let fields = capture(ProfileUpgradeBackupError {
            source: SecureStorageError::PermissionDenied("private-account".into()).into(),
        });

        assert_eq!(fields["error_kind"], "permission_denied");
        assert!(!format!("{fields:?}").contains("private-account"));
    }
}
