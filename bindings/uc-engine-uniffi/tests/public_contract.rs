use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use uc_engine::{EngineError, EngineErrorCategory};

use uc_engine_uniffi::{
    core_version, BindingAnalyticsEvent, BindingAnalyticsGroupIdentify, BindingAnalyticsHost,
    BindingAnalyticsHostError, BindingAnalyticsIdentify, BindingAnalyticsIdentityChange,
    BindingClipboardRepresentation, BindingClipboardRestoreMode, BindingClipboardRestoreOutcome,
    BindingClipboardSnapshot, BindingConfig, BindingEngineState, BindingError,
    BindingErrorCategory, BindingEvent, BindingFileMetadata, BindingHost, BindingOperationTerminal,
    HostBindingError, InvitationIssued, MembershipConvergenceState, MobileEngine, SendReport,
};

static ENGINE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn core_version_uses_the_binding_package_version() {
    assert_eq!(core_version(), format!("v{}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn engine_errors_keep_their_stable_code_category_and_retryability() {
    let cases = [
        (
            EngineErrorCategory::InvalidInput,
            BindingErrorCategory::InvalidInput,
        ),
        (
            EngineErrorCategory::InvalidState,
            BindingErrorCategory::InvalidState,
        ),
        (
            EngineErrorCategory::Unauthorized,
            BindingErrorCategory::Unauthorized,
        ),
        (
            EngineErrorCategory::NotFound,
            BindingErrorCategory::NotFound,
        ),
        (
            EngineErrorCategory::Conflict,
            BindingErrorCategory::Conflict,
        ),
        (
            EngineErrorCategory::Unavailable,
            BindingErrorCategory::Unavailable,
        ),
        (
            EngineErrorCategory::DeadlineExceeded,
            BindingErrorCategory::DeadlineExceeded,
        ),
        (
            EngineErrorCategory::Internal,
            BindingErrorCategory::Internal,
        ),
    ];

    for (index, (engine_category, binding_category)) in cases.into_iter().enumerate() {
        let code = 2000 + index as u32;
        let retryable = index % 2 == 0;
        assert_eq!(
            BindingError::from(EngineError::new(code, engine_category, retryable)),
            BindingError::Engine {
                code,
                category: binding_category,
                retryable,
            }
        );
    }
}

struct MemoryHost {
    private_data: PathBuf,
    cache: PathBuf,
    temporary: PathBuf,
    values: Mutex<HashMap<String, Vec<u8>>>,
    files: Mutex<HashMap<String, TestFile>>,
    clipboard: Mutex<BindingClipboardSnapshot>,
    clipboard_writes: Mutex<Vec<BindingClipboardSnapshot>>,
    finished_files: Mutex<Vec<String>>,
}

struct TestFile {
    metadata: BindingFileMetadata,
    bytes: Vec<u8>,
}

impl MemoryHost {
    fn new(root: &Path) -> Self {
        Self {
            private_data: root.join("data"),
            cache: root.join("cache"),
            temporary: root.join("temporary"),
            values: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            clipboard: Mutex::new(BindingClipboardSnapshot {
                observed_at_ms: 0,
                representations: Vec::new(),
            }),
            clipboard_writes: Mutex::new(Vec::new()),
            finished_files: Mutex::new(Vec::new()),
        }
    }

    fn values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.values.lock() {
            Ok(values) => values,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn register_input_file(
        &self,
        handle: &str,
        display_name: &str,
        mime_type: Option<&str>,
        bytes: Vec<u8>,
    ) {
        self.files().insert(
            handle.to_owned(),
            TestFile {
                metadata: BindingFileMetadata {
                    display_name: display_name.to_owned(),
                    size_bytes: bytes.len() as u64,
                    mime_type: mime_type.map(str::to_owned),
                },
                bytes,
            },
        );
    }

    fn files(&self) -> MutexGuard<'_, HashMap<String, TestFile>> {
        match self.files.lock() {
            Ok(files) => files,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn file_bytes(&self, handle: &str) -> Option<Vec<u8>> {
        self.files().get(handle).map(|file| file.bytes.clone())
    }

    fn file_finished(&self, handle: &str) -> bool {
        lock(&self.finished_files)
            .iter()
            .any(|finished| finished == handle)
    }

    fn set_clipboard(&self, snapshot: BindingClipboardSnapshot) {
        *lock(&self.clipboard) = snapshot;
    }

    fn clipboard_writes(&self) -> MutexGuard<'_, Vec<BindingClipboardSnapshot>> {
        lock(&self.clipboard_writes)
    }
}

impl BindingHost for MemoryHost {
    fn private_data_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.private_data.to_string_lossy().into_owned())
    }

    fn cache_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.cache.to_string_lossy().into_owned())
    }

    fn temporary_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.temporary.to_string_lossy().into_owned())
    }

    fn secure_storage_get(&self, key: String) -> Result<Option<Vec<u8>>, HostBindingError> {
        Ok(self.values().get(&key).cloned())
    }

    fn secure_storage_set(&self, key: String, value: Vec<u8>) -> Result<(), HostBindingError> {
        self.values().insert(key, value);
        Ok(())
    }

    fn secure_storage_delete(&self, key: String) -> Result<(), HostBindingError> {
        self.values().remove(&key);
        Ok(())
    }

    fn file_metadata(&self, handle: String) -> Result<BindingFileMetadata, HostBindingError> {
        self.files()
            .get(&handle)
            .map(|file| file.metadata.clone())
            .ok_or(HostBindingError::InvalidHandle)
    }

    fn file_read_chunk(
        &self,
        handle: String,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostBindingError> {
        let files = self.files();
        let file = files.get(&handle).ok_or(HostBindingError::InvalidHandle)?;
        let start = usize::try_from(offset).map_err(|_| HostBindingError::InvalidHandle)?;
        let max_bytes = usize::try_from(max_bytes).map_err(|_| HostBindingError::InvalidHandle)?;
        let end = start.saturating_add(max_bytes).min(file.bytes.len());
        file.bytes
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or(HostBindingError::InvalidHandle)
    }

    fn file_write_chunk(
        &self,
        handle: String,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), HostBindingError> {
        let mut files = self.files();
        let file = files
            .get_mut(&handle)
            .ok_or(HostBindingError::InvalidHandle)?;
        let start = usize::try_from(offset).map_err(|_| HostBindingError::InvalidHandle)?;
        let end = start
            .checked_add(bytes.len())
            .ok_or(HostBindingError::InvalidHandle)?;
        if file.bytes.len() < end {
            file.bytes.resize(end, 0);
        }
        file.bytes[start..end].copy_from_slice(&bytes);
        Ok(())
    }

    fn file_finish_write(&self, handle: String) -> Result<(), HostBindingError> {
        if !self.files().contains_key(&handle) {
            return Err(HostBindingError::InvalidHandle);
        }
        lock(&self.finished_files).push(handle);
        Ok(())
    }

    fn clipboard_read(&self) -> Result<BindingClipboardSnapshot, HostBindingError> {
        Ok(lock(&self.clipboard).clone())
    }

    fn clipboard_write(&self, snapshot: BindingClipboardSnapshot) -> Result<(), HostBindingError> {
        self.clipboard_writes().push(snapshot);
        Ok(())
    }
}

struct RecordingAnalyticsHost {
    events: Mutex<Vec<BindingAnalyticsEvent>>,
    identifies: Mutex<Vec<BindingAnalyticsIdentify>>,
    group_identifies: Mutex<Vec<BindingAnalyticsGroupIdentify>>,
    anonymous_id: Mutex<String>,
    space_person_id: Mutex<Option<String>>,
}

impl RecordingAnalyticsHost {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            identifies: Mutex::new(Vec::new()),
            group_identifies: Mutex::new(Vec::new()),
            anonymous_id: Mutex::new("018f47de-4a26-7d31-a7d8-93c20d64f813".to_owned()),
            space_person_id: Mutex::new(None),
        }
    }

    fn event_names(&self) -> Vec<String> {
        lock(&self.events)
            .iter()
            .map(|event| event.name.clone())
            .collect()
    }
}

impl BindingAnalyticsHost for RecordingAnalyticsHost {
    fn capture(&self, event: BindingAnalyticsEvent) -> Result<(), BindingAnalyticsHostError> {
        lock(&self.events).push(event);
        Ok(())
    }

    fn identify(&self, payload: BindingAnalyticsIdentify) -> Result<(), BindingAnalyticsHostError> {
        lock(&self.identifies).push(payload);
        Ok(())
    }

    fn group_identify(
        &self,
        payload: BindingAnalyticsGroupIdentify,
    ) -> Result<(), BindingAnalyticsHostError> {
        lock(&self.group_identifies).push(payload);
        Ok(())
    }

    fn adopt_space_person(
        &self,
        space_person_id: String,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError> {
        let mut current = lock(&self.space_person_id);
        let previous_distinct_id = current
            .clone()
            .unwrap_or_else(|| lock(&self.anonymous_id).clone());
        *current = Some(space_person_id.clone());
        Ok(BindingAnalyticsIdentityChange {
            previous_distinct_id,
            new_distinct_id: space_person_id,
        })
    }

    fn release_space_person(
        &self,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError> {
        let mut current = lock(&self.space_person_id);
        let anonymous_id = lock(&self.anonymous_id).clone();
        let previous_distinct_id = current.take().unwrap_or_else(|| anonymous_id.clone());
        Ok(BindingAnalyticsIdentityChange {
            previous_distinct_id,
            new_distinct_id: anonymous_id,
        })
    }

    fn current_space_person_id(&self) -> Result<Option<String>, BindingAnalyticsHostError> {
        Ok(lock(&self.space_person_id).clone())
    }

    fn reset_telemetry_identity(
        &self,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError> {
        let mut current = lock(&self.space_person_id);
        let mut anonymous_id = lock(&self.anonymous_id);
        let previous_distinct_id = current.take().unwrap_or_else(|| anonymous_id.clone());
        *anonymous_id = "018f47de-4a26-7d31-a7d8-93c20d64f814".to_owned();
        Ok(BindingAnalyticsIdentityChange {
            previous_distinct_id,
            new_distinct_id: anonymous_id.clone(),
        })
    }
}

#[test]
fn mobile_host_can_create_a_space_with_analytics_and_shutdown_through_the_binding() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let analytics = Arc::new(RecordingAnalyticsHost::new());
    let engine = MobileEngine::start_with_analytics(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-contract".to_owned(),
        },
        host.clone(),
        analytics.clone(),
    )
    .expect("analytics-enabled binding engine must start");

    let created = engine
        .create_space(
            Some("mobile-contract-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must return the create-space result");
    assert!(!created.space_id.is_empty());
    assert!(!created.self_device_id.is_empty());
    assert!(!created.identity_fingerprint.is_empty());
    assert!(
        !host.values().is_empty(),
        "create-space must persist secrets through the host callback"
    );

    let event_names = analytics.event_names();
    assert!(event_names.iter().any(|name| name == "setup_started"));
    assert!(event_names.iter().any(|name| name == "device_name_set"));
    assert!(event_names.iter().any(|name| name == "setup_completed"));
    let device_name_event = lock(&analytics.events)
        .iter()
        .find(|event| event.name == "device_name_set")
        .cloned()
        .expect("device name event must be captured");
    assert!(device_name_event
        .properties_json
        .contains("name_length_bucket"));
    assert!(!device_name_event
        .properties_json
        .contains("mobile-contract-host"));
    assert_eq!(lock(&analytics.identifies).len(), 1);
    assert_eq!(lock(&analytics.group_identifies).len(), 1);
    assert!(lock(&analytics.space_person_id).is_some());

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn dropping_a_running_binding_stops_its_worker() {
    let _test_guard = engine_test_guard();
    let (finished, completion) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let root = tempfile::tempdir().expect("temporary host root must be available");
        let host = Arc::new(MemoryHost::new(root.path()));
        let engine = MobileEngine::start(
            BindingConfig {
                app_version: "1.2.3".to_owned(),
                profile_id: "binding-drop".to_owned(),
            },
            host,
        )
        .expect("binding engine must start");

        drop(engine);
        let _ = finished.send(());
    });

    completion
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("dropping the binding must not leave its worker waiting on the event stream");
}

#[test]
fn mobile_host_observes_suspend_and_resume_state_events() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-lifecycle".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-lifecycle-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space before lifecycle transitions");

    wait_for_state(&engine, BindingEngineState::Running);
    engine.suspend().expect("binding engine must suspend");
    wait_for_state(&engine, BindingEngineState::Suspended);
    engine.resume().expect("binding engine must resume");
    wait_for_state(&engine, BindingEngineState::Running);

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn peer_refresh_returns_explicit_connection_counts() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-peer-refresh".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-peer-refresh".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report = engine
        .refresh_peer_connections()
        .expect("binding must refresh peer connections");
    assert_eq!(report.total, 0);
    assert_eq!(report.online, 0);
    assert_eq!(report.offline, 0);
    assert_eq!(report.errors, 0);

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn space_management_preserves_state_devices_resend_outcomes_and_local_history() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-space-management".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    let created = engine
        .create_space(
            Some("mobile-space-manager".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let state = engine
        .query_space_state()
        .expect("binding must query setup state");
    assert!(state.has_completed);
    assert_eq!(state.space_id.as_deref(), Some(created.space_id.as_str()));
    assert_eq!(state.device_name.as_deref(), Some("mobile-space-manager"));

    let devices = engine.list_devices().expect("binding must list devices");
    assert!(devices.iter().all(|device| !device.device_id.is_empty()));
    let local_devices = devices
        .iter()
        .filter(|device| device.is_local)
        .collect::<Vec<_>>();
    assert_eq!(
        local_devices.len(),
        1,
        "the roster must identify exactly one local device"
    );
    assert!(
        local_devices[0].online,
        "the local device must be shown as online"
    );
    let remove_local = engine.remove_member(local_devices[0].device_id.clone());
    assert!(
        matches!(remove_local, Err(BindingError::Engine { .. })),
        "the binding must reject removing the local device"
    );

    let resend = engine
        .resend_entry("missing-entry".to_owned(), Vec::new())
        .expect("missing entry must remain a structured business outcome");
    assert!(matches!(
        resend,
        uc_engine_uniffi::ResendEntryOutcome::EntryNotFound { ref entry_id }
            if entry_id == "missing-entry"
    ));

    let remove = engine.remove_member("missing-device".to_owned());
    assert!(matches!(remove, Err(BindingError::Engine { .. })));

    engine
        .leave_space()
        .expect("binding must leave the local space");
    let left = engine
        .query_space_state()
        .expect("binding must query state after leaving");
    assert!(!left.has_completed);
    assert!(left.space_id.is_none());
    assert!(
        engine
            .list_devices()
            .expect("binding must list devices after leaving")
            .is_empty(),
        "leaving a space must not retain its member roster"
    );

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn mobile_host_recovers_the_same_identity_after_process_restart() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let config = BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: "binding-session-recovery".to_owned(),
    };
    let first =
        MobileEngine::start(config.clone(), host.clone()).expect("first binding engine must start");
    let created = first
        .create_space(
            Some("mobile-recovery-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("first binding engine must create a space");
    first
        .shutdown(5_000)
        .expect("first binding engine must shut down");

    let restarted = MobileEngine::start(config, host).expect("restarted binding engine must start");
    let recovery = restarted
        .recover_session(true)
        .expect("restarted binding engine must recover its persisted session");
    assert!(recovery.unlocked);
    assert!(recovery.resumed);
    let local = restarted
        .query_local_device()
        .expect("restarted binding engine must expose its recovered identity");
    assert_eq!(local.device_id, created.self_device_id);
    assert_eq!(
        restarted
            .lifecycle_state()
            .expect("restarted binding engine must expose its lifecycle state"),
        BindingEngineState::Running
    );

    restarted
        .shutdown(5_000)
        .expect("restarted binding engine must shut down");
}

#[test]
fn mobile_binding_exposes_membership_convergence() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-membership-convergence".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-convergence-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let status = engine
        .query_membership_convergence()
        .expect("binding must expose membership convergence");

    assert_eq!(status.state, MembershipConvergenceState::Complete);
    assert_eq!(status.pending_count, 0);
    assert_eq!(status.waiting_for_peer_count, 0);
    assert_eq!(status.waiting_for_update_count, 0);
    assert_eq!(status.version_incompatible_count, 0);
    assert_eq!(status.blocked_count, 0);
    assert_eq!(status.rejected_count, 0);
    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

fn wait_for_state(engine: &MobileEngine, expected: BindingEngineState) {
    for _ in 0..50 {
        if let Some(BindingEvent::StateChanged { state }) = engine.next_event(100) {
            if state == expected {
                return;
            }
        }
    }
    panic!("binding did not deliver the expected state: {expected:?}");
}

#[test]
fn completed_engine_operations_are_available_as_structured_events() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-operation-events".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-event-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    for _ in 0..50 {
        if let Some(BindingEvent::OperationFinished {
            operation_id,
            terminal,
            failure,
        }) = engine.next_event(100)
        {
            assert!(!operation_id.is_empty());
            assert_eq!(terminal, BindingOperationTerminal::Succeeded);
            assert_eq!(failure, None);
            engine
                .shutdown(5_000)
                .expect("binding engine must shut down within the deadline");
            return;
        }
    }
    panic!("binding did not deliver a completed-operation event");
}

#[test]
fn pairing_methods_return_invitation_data_and_stable_join_errors() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-pairing".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-pairing-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let invitation: InvitationIssued = engine
        .issue_invitation()
        .expect("binding must return a pairing invitation");
    assert!(!invitation.invitation_code.is_empty());
    assert!(invitation.expires_at_ms > 0);

    let error = engine
        .join_space(
            invitation.invitation_code,
            Some("  ".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect_err("blank device name must be rejected by the core");
    assert!(matches!(
        error,
        BindingError::Engine {
            category: BindingErrorCategory::InvalidInput,
            retryable: false,
            ..
        }
    ));

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn text_send_returns_a_content_free_delivery_summary() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-send-text".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-text-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report: SendReport = engine
        .send_text("private binding text".to_owned(), Vec::new())
        .expect("binding must send text through the core");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_duplicate, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_pending, 0);
    assert!(!format!("{report:?}").contains("private binding text"));

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn image_send_returns_the_shared_content_free_delivery_summary() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-send-image".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-image-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let mut private_image = vec![0; 64 * 1024 + 1];
    private_image[..8].copy_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let report: SendReport = engine
        .send_image(private_image.clone(), "image/png".to_owned(), Vec::new())
        .expect("binding must send an image through the core");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_duplicate, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_pending, 0);
    assert!(!format!("{report:?}").contains(&format!("{private_image:?}")));

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn file_send_reads_opaque_host_handles_and_returns_the_shared_summary() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let private_file = b"private binding file".to_vec();
    host.register_input_file(
        "input-file-1",
        "private-name.txt",
        Some("text/plain"),
        private_file.clone(),
    );
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-send-file".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-file-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report: SendReport = engine
        .send_files(vec!["input-file-1".to_owned()], Vec::new())
        .expect("binding must send a host file through the core");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert!(!format!("{report:?}").contains("private-name.txt"));
    assert!(!format!("{report:?}").contains("private binding file"));

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn capture_current_clipboard_reads_a_structured_host_snapshot() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"private clipboard text".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-capture".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-capture-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let entry_id = engine
        .capture_current_clipboard()
        .expect("binding must capture the host clipboard")
        .expect("non-empty clipboard must create an entry");
    assert!(!entry_id.is_empty());

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn observed_clipboard_change_returns_a_delivery_report_for_local_content() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"private observed clipboard text".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-observe-clipboard".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-observe-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report = engine
        .observe_clipboard_change(true)
        .expect("binding must process the observed clipboard change")
        .expect("local content must be dispatched");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_duplicate, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_pending, 0);

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn observed_clipboard_change_can_capture_without_dispatching() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"private local-only clipboard text".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-observe-local-only".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-observe-local-only".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    assert_eq!(
        engine
            .observe_clipboard_change(false)
            .expect("binding must observe without dispatching"),
        None
    );

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn restore_clipboard_writes_a_structured_snapshot_to_the_host() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let private_text = b"private restored clipboard".to_vec();
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: private_text.clone(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-restore".to_owned(),
        },
        host.clone(),
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-restore-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");
    let entry_id = engine
        .capture_current_clipboard()
        .expect("binding must capture the host clipboard")
        .expect("non-empty clipboard must create an entry");

    assert_eq!(
        engine
            .restore_clipboard(entry_id, BindingClipboardRestoreMode::Standard)
            .expect("binding must restore through the host clipboard"),
        BindingClipboardRestoreOutcome::Restored
    );
    let writes = host.clipboard_writes();
    assert_eq!(writes.len(), 1);
    assert!(matches!(
        writes[0].representations.as_slice(),
        [BindingClipboardRepresentation::Inline { bytes, .. }] if bytes == &private_text
    ));
    drop(writes);

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn active_clipboard_can_be_queried_after_the_activation_event_was_missed() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"current active clipboard".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-query-active-clipboard".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-query-active-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");
    let entry_id = engine
        .observe_clipboard_change(true)
        .expect("binding must observe the host clipboard")
        .expect("changed clipboard must be dispatched")
        .entry_id;
    let local_device = engine
        .query_local_device()
        .expect("binding must expose the local device");

    let active = engine
        .query_active_clipboard()
        .expect("binding must query the active clipboard")
        .expect("captured clipboard must be active");

    assert_eq!(active.entry_id, entry_id);
    assert_eq!(active.activated_by, local_device.device_id);

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down");
}

#[test]
fn active_clipboard_query_returns_empty_before_the_first_activation() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-query-empty-active-clipboard".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-query-empty-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    assert_eq!(
        engine
            .query_active_clipboard()
            .expect("binding must query an empty active clipboard"),
        None
    );

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down");
}

#[test]
fn active_clipboard_query_survives_session_recovery() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"persisted active clipboard".to_vec(),
        }],
    });
    let config = BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: "binding-query-recovered-active-clipboard".to_owned(),
    };
    let first =
        MobileEngine::start(config.clone(), host.clone()).expect("first binding engine must start");
    first
        .create_space(
            Some("mobile-query-recovery-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("first binding engine must create a space");
    let entry_id = first
        .observe_clipboard_change(true)
        .expect("first binding engine must observe the clipboard")
        .expect("observed clipboard must be dispatched")
        .entry_id;
    let activated_by = first
        .query_local_device()
        .expect("first binding engine must expose the local device")
        .device_id;
    first
        .shutdown(5_000)
        .expect("first binding engine must shut down");

    let restarted = MobileEngine::start(config, host).expect("restarted binding engine must start");
    let recovery = restarted
        .recover_session(true)
        .expect("restarted binding engine must recover its persisted session");
    assert!(recovery.unlocked);
    assert!(recovery.resumed);

    let active = restarted
        .query_active_clipboard()
        .expect("restarted binding engine must query the active clipboard")
        .expect("recovered active clipboard must be available");
    assert_eq!(active.entry_id, entry_id);
    assert_eq!(active.activated_by, activated_by);

    restarted
        .shutdown(5_000)
        .expect("restarted binding engine must shut down");
}

#[test]
fn export_entry_writes_to_an_opaque_host_handle_and_finishes_it() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let private_text = b"private exported clipboard".to_vec();
    host.set_clipboard(BindingClipboardSnapshot {
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: private_text.clone(),
        }],
    });
    host.register_input_file(
        "output-file-1",
        "private-export.txt",
        Some("text/plain"),
        Vec::new(),
    );
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-export".to_owned(),
        },
        host.clone(),
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-export-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");
    let entry_id = engine
        .capture_current_clipboard()
        .expect("binding must capture the host clipboard")
        .expect("non-empty clipboard must create an entry");

    engine
        .export_entry(entry_id, "output-file-1".to_owned())
        .expect("binding must export to the host handle");
    assert_eq!(host.file_bytes("output-file-1"), Some(private_text.clone()));
    assert!(host.file_finished("output-file-1"));

    engine
        .shutdown(5_000)
        .expect("binding engine must shut down within the deadline");
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn engine_test_guard() -> MutexGuard<'static, ()> {
    match ENGINE_TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
