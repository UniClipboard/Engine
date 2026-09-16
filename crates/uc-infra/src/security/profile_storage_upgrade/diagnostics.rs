use super::journal::UpgradePhaseV1;
use super::ProfileStorageUpgradeError;

/// 仅供本地排障，不新增业务 trace，也不把升级内部阶段暴露给调用方。
pub(super) struct UpgradeDiagnostics {
    pub(super) phase: Option<UpgradePhaseV1>,
    pub(super) action: &'static str,
    pub(super) target_activated: Option<bool>,
}

impl UpgradeDiagnostics {
    pub(super) fn new() -> Self {
        Self {
            phase: None,
            action: "acquire_lease",
            target_activated: None,
        }
    }

    pub(super) fn begin_step(&mut self, phase: UpgradePhaseV1) {
        self.phase = Some(phase);
        self.action = match phase {
            UpgradePhaseV1::Detected => "stage_target",
            UpgradePhaseV1::TargetStaged => "separate_stores",
            UpgradePhaseV1::StoresSeparated => "convert_primary_payloads",
            UpgradePhaseV1::PrimaryPayloadsConverted => "convert_derived_payloads",
            UpgradePhaseV1::PayloadsConverted => "validate_target",
            UpgradePhaseV1::Verified => "promote_target",
            UpgradePhaseV1::Promoted => "prepare_cleanup",
            UpgradePhaseV1::CleanupPending => "cleanup_source",
        };
    }

    pub(super) fn record_failure(&self, error: &ProfileStorageUpgradeError) {
        let error_kind = match error {
            ProfileStorageUpgradeError::Storage { .. } => "storage",
            ProfileStorageUpgradeError::Security { .. } => "security",
            ProfileStorageUpgradeError::Corrupt { .. } => "corrupt",
            ProfileStorageUpgradeError::SourceChanged => "source_changed",
            ProfileStorageUpgradeError::Manifest { .. } => "manifest",
        };
        let phase = self.phase.map(|phase| match phase {
            UpgradePhaseV1::Detected => "detected",
            UpgradePhaseV1::TargetStaged => "target_staged",
            UpgradePhaseV1::StoresSeparated => "stores_separated",
            UpgradePhaseV1::PrimaryPayloadsConverted => "primary_payloads_converted",
            UpgradePhaseV1::PayloadsConverted => "payloads_converted",
            UpgradePhaseV1::Verified => "verified",
            UpgradePhaseV1::Promoted => "promoted",
            UpgradePhaseV1::CleanupPending => "cleanup_pending",
        });
        let io = find_io_source(error);
        let io_kind = io.map(|source| format!("{:?}", source.kind()));
        tracing::error!(
            target: "uc_infra::security::profile_storage_upgrade",
            upgrade_phase = phase,
            upgrade_action = self.action,
            target_activated = self.target_activated,
            error_kind,
            io_error_kind = io_kind.as_deref(),
            io_error_code = io.and_then(std::io::Error::raw_os_error),
            "profile storage upgrade step failed"
        );
    }
}

fn find_io_source<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a std::io::Error> {
    let mut current = Some(error);
    while let Some(source) = current {
        if let Some(io) = source.downcast_ref::<std::io::Error>() {
            return Some(io);
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

    fn capture(error: ProfileStorageUpgradeError) -> BTreeMap<String, String> {
        let events = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(events.clone()));
        tracing::subscriber::with_default(subscriber, || {
            let mut diagnostic = UpgradeDiagnostics::new();
            diagnostic.begin_step(UpgradePhaseV1::TargetStaged);
            diagnostic.target_activated = Some(false);
            diagnostic.record_failure(&error);
        });
        let mut events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        events.pop().unwrap()
    }

    #[test]
    fn error_metadata_preserves_stage_and_system_code_without_source_text() {
        let fields = capture(ProfileStorageUpgradeError::Storage {
            source: anyhow::Error::new(std::io::Error::from_raw_os_error(5))
                .context("private-directory/private-file secret-content"),
        });
        assert_eq!(fields["upgrade_phase"], "target_staged");
        assert_eq!(fields["upgrade_action"], "separate_stores");
        assert_eq!(fields["io_error_code"], "5");
        assert_eq!(fields["target_activated"], "false");
        assert_eq!(fields["error_kind"], "storage");
        assert!(!format!("{fields:?}").contains("private"));
        assert!(!format!("{fields:?}").contains("secret-content"));
    }

    #[test]
    fn custom_io_error_records_only_its_kind() {
        let fields = capture(ProfileStorageUpgradeError::Storage {
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "private-path")
                .into(),
        });
        assert_eq!(fields["io_error_kind"], "PermissionDenied");
        assert!(!fields.contains_key("io_error_code"));
        assert!(!format!("{fields:?}").contains("private-path"));
    }

    #[test]
    fn non_io_failure_does_not_fabricate_system_error_fields() {
        let fields = capture(ProfileStorageUpgradeError::SourceChanged);
        assert_eq!(fields["error_kind"], "source_changed");
        assert!(!fields.contains_key("io_error_kind"));
        assert!(!fields.contains_key("io_error_code"));
    }
}
