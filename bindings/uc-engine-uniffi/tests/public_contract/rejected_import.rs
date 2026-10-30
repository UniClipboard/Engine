//! 正式 UniFFI 方法经真实 Engine 查询持久拒绝状态，不读取私有状态作为查询结果。

use super::*;
use uc_engine_uniffi::ConfigImportRejectionReason;

#[test]
fn retained_import_query_uses_engine_state_across_restarts_and_preserves_errors() {
    let _guard = engine_test_guard();
    let root = tempfile::tempdir().expect("synthetic binding profiles");
    let mut trace = Vec::new();
    for schema in [1, 2] {
        let host = Arc::new(MemoryHost::new(
            &root.path().join(format!("schema-{schema}")),
        ));
        let config = || BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: format!("rejected-import-{schema}"),
        };
        let engine = MobileEngine::start(config(), host.clone()).expect("initial binding start");
        assert!(engine
            .query_rejected_config_import()
            .expect("no rejected input")
            .is_none());
        let space = engine
            .create_space(
                Some("synthetic binding host".to_owned()),
                "correct horse battery staple".to_owned(),
            )
            .expect("original Space")
            .space_id;
        let content = b"synthetic binding retained history".to_vec();
        host.set_clipboard(BindingClipboardSnapshot {
            identity: None,
            observed_at_ms: 1,
            representations: vec![BindingClipboardRepresentation::Inline {
                format: "text/plain".to_owned(),
                mime_type: Some("text/plain".to_owned()),
                bytes: content.clone(),
            }],
        });
        let entry = engine
            .capture_current_clipboard()
            .expect("capture original")
            .expect("entry");
        engine
            .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
            .expect("initial shutdown");
        let marker = serde_json::to_vec(&serde_json::json!({"schema_ver":schema,"staging_dir":"import-staging","has_kek":true,"staged_at_unix_ms":0})).expect("marker");
        let source = b"synthetic opaque staged source";
        fs::create_dir_all(host.private_data.join("import-staging/db")).expect("staging");
        fs::write(host.private_data.join("pending-import.json"), &marker).expect("marker input");
        fs::write(
            host.private_data.join("import-staging/db/uniclipboard.db"),
            source,
        )
        .expect("unique source input");
        for boot in 0..2 {
            let engine =
                MobileEngine::start(config(), host.clone()).expect("binding starts original Space");
            assert_eq!(
                engine
                    .query_space_state()
                    .expect("original state")
                    .space_id
                    .as_ref(),
                Some(&space)
            );
            let rejected = engine
                .query_rejected_config_import()
                .expect("Engine-backed query")
                .expect("retained rejection");
            assert_eq!(
                rejected.reason,
                if schema == 1 {
                    ConfigImportRejectionReason::UnsupportedFormat
                } else {
                    ConfigImportRejectionReason::MissingRuntimeMembers
                }
            );
            assert_eq!(rejected.message, "Import was not completed. Re-export the configuration bundle using the new version.");
            assert_eq!(
                engine
                    .restore_clipboard(entry.clone(), BindingClipboardRestoreMode::Standard)
                    .expect("read original encrypted history"),
                BindingClipboardRestoreOutcome::Restored
            );
            assert!(
                matches!(host.clipboard_writes().last().expect("clipboard write").representations.as_slice(), [BindingClipboardRepresentation::Inline { bytes, .. }] if bytes == &content)
            );
            let new_entry = engine
                .send_text(
                    format!("synthetic binding write {schema}-{boot}"),
                    Vec::new(),
                )
                .expect("new encrypted write");
            assert_eq!(
                engine
                    .restore_clipboard(new_entry.entry_id, BindingClipboardRestoreMode::Standard)
                    .expect("read new content"),
                BindingClipboardRestoreOutcome::Restored
            );
            assert_eq!(
                fs::read(host.private_data.join("pending-import.json")).expect("retained marker"),
                marker
            );
            assert_eq!(
                fs::read(host.private_data.join("import-staging/db/uniclipboard.db"))
                    .expect("retained source"),
                source
            );
            let journal = host.private_data.join(".config-import-adoption-v1");
            fs::create_dir(&journal).expect("synthetic inspection failure");
            let error = engine
                .query_rejected_config_import()
                .expect_err("failure must not become None");
            assert_eq!(
                error,
                BindingError::Engine {
                    code: uc_engine::error_codes::QUERY_DIAGNOSTICS_FAILED_CODE,
                    category: BindingErrorCategory::Internal,
                    retryable: false
                }
            );
            fs::remove_dir(&journal).expect("remove only synthetic fault");
            assert!(engine
                .query_rejected_config_import()
                .expect("query recovered")
                .is_some());
            trace.push(serde_json::json!({"schema":schema,"boot":boot,"reason":format!("{:?}",rejected.reason),"message":rejected.message,"source_bytes_retained":true,"original_history_read":true,"new_encrypted_write_read":true,"query_error":format!("{error:?}")}));
            engine
                .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
                .expect("binding shutdown");
        }
    }
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        let directory = PathBuf::from(directory).join("binding-rejected-import");
        fs::create_dir_all(&directory).expect("artifact directory");
        fs::write(
            directory.join("trace.json"),
            serde_json::to_vec_pretty(&trace).expect("trace json"),
        )
        .expect("binding trace artifact");
    }
}
