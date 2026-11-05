use super::*;
use crate::engine::event_stream::event_channel;

#[test]
fn a_state_change_is_recorded_with_its_state_and_flags_only() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
    let (events, _stream) = event_channel(4);

    publish_profile_recovery(
        &events,
        ProfileRecoverySummary {
            state: ProfileRecoveryState::Failed,
            can_submit_passphrase: false,
            restart_required: true,
            background_ready: false,
            cleanup_pending: false,
            losses: Vec::new(),
            admission: None,
        },
    );

    assert_eq!(logs.count("profile recovery state changed"), 1);
    assert!(logs.output().contains("recovery_state=Failed"));
    assert!(logs.output().contains("restart_required=true"));
}

#[test]
fn recovery_storage_errors_keep_stable_public_categories() {
    for (source, expected) in [
        (
            SecureStorageError::Unavailable("unavailable".to_owned()),
            HostCapabilityErrorCategory::Unavailable,
        ),
        (
            SecureStorageError::PermissionDenied("denied".to_owned()),
            HostCapabilityErrorCategory::PermissionDenied,
        ),
        (
            SecureStorageError::Other("other".to_owned()),
            HostCapabilityErrorCategory::Io,
        ),
    ] {
        let error = HostCapabilityError::from(source);
        assert_eq!(error.category(), expected);
    }
}

#[test]
fn recovery_failures_keep_stable_engine_categories_and_retryability() {
    let cases = [
        (
            ProfileKeyRecoveryError::WrongPassphrase,
            UNLOCK_SPACE_UNAUTHORIZED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        (
            ProfileKeyRecoveryError::Corrupt,
            UNLOCK_SPACE_CORRUPTED_CODE,
            EngineErrorCategory::Internal,
            false,
        ),
        (
            ProfileKeyRecoveryError::Unsupported,
            PROFILE_RECOVERY_UNSUPPORTED_CODE,
            EngineErrorCategory::Unavailable,
            false,
        ),
        (
            ProfileKeyRecoveryError::Storage(anyhow::anyhow!("storage")),
            PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
    ];
    for (source, code, category, retryable) in cases {
        let error = EngineError::from(source);
        assert_eq!(error.code(), code);
        assert_eq!(error.category(), category);
        assert_eq!(error.is_retryable(), retryable);
    }
}
