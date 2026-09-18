use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use uc_engine::{
    ClipboardRestoreMode, ExportEntryInput, HostCapabilities, Operation, OperationResult,
    QueryHistoryInput, RestoreClipboardInput, SendFilesInput, SendTextInput, StartupLifecycle,
    StartupProgress,
};

use super::secure_storage::ControlledSecureStorage;
use super::{engine_error_kind, host_secure_storage, probe_error, store_engine, ProbeState};

pub(super) async fn suspend_during_clipboard_read(
    state: &ProbeState,
    block_ms: u64,
    deadline_ms: u64,
) -> Value {
    let Some(engine) = state.engine.as_ref().cloned() else {
        return probe_error("not_started");
    };
    state.clipboard.prepare_blocked_text_read();
    let capture = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.execute(Operation::CaptureCurrentClipboard).await }
    });
    if tokio::time::timeout(
        Duration::from_secs(5),
        state.clipboard.wait_until_read_starts(),
    )
    .await
    .is_err()
    {
        state.clipboard.release_read();
        let _ = capture.await;
        return probe_error("clipboard_read_not_started");
    }
    let clipboard = state.clipboard.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(block_ms)).await;
        clipboard.release_read();
    });
    let started_at = Instant::now();
    let suspend = engine
        .suspend_with_deadline(Duration::from_millis(deadline_ms))
        .await;
    let elapsed_ms = started_at.elapsed().as_millis();
    let capture = match capture.await {
        Ok(Ok(_)) => "completed",
        Ok(Err(_)) => "cancelled",
        Err(_) => "failed",
    };
    match suspend {
        Ok(()) => json!({
            "ok": true,
            "kind": "suspended_during_clipboard_read",
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "capture": capture,
        }),
        Err(error) => json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "capture": capture,
        }),
    }
}

pub(super) async fn suspend_during_clipboard_write(
    state: &ProbeState,
    block_ms: u64,
    deadline_ms: u64,
) -> Value {
    let Some(engine) = state.engine.as_ref().cloned() else {
        return probe_error("not_started");
    };
    let entry_id = match latest_history_entry_id(engine.as_ref()).await {
        Ok(entry_id) => entry_id,
        Err(error) => return error,
    };
    state.clipboard.prepare_blocked_write();
    let restore = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                    entry_id,
                    mode: ClipboardRestoreMode::Standard,
                }))
                .await
        }
    });
    if tokio::time::timeout(
        Duration::from_secs(5),
        state.clipboard.wait_until_write_starts(),
    )
    .await
    .is_err()
    {
        state.clipboard.release_write();
        let _ = restore.await;
        return probe_error("clipboard_write_not_started");
    }
    let clipboard = state.clipboard.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(block_ms)).await;
        clipboard.release_write();
    });
    let started_at = Instant::now();
    let suspend = engine
        .suspend_with_deadline(Duration::from_millis(deadline_ms))
        .await;
    let elapsed_ms = started_at.elapsed().as_millis();
    let restore = match restore.await {
        Ok(Ok(_)) => "completed",
        Ok(Err(_)) => "cancelled",
        Err(_) => "failed",
    };
    match suspend {
        Ok(()) => json!({
            "ok": true,
            "kind": "suspended_during_clipboard_write",
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "restore": restore,
        }),
        Err(error) => json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "restore": restore,
        }),
    }
}

pub(super) async fn suspend_during_file_read(
    state: &ProbeState,
    block_ms: u64,
    deadline_ms: u64,
) -> Value {
    const FIXTURE_BYTES: usize = 1024 * 1024;

    let Some(engine) = state.engine.as_ref().cloned() else {
        return probe_error("not_started");
    };
    let handle = state.files.register_fixture(vec![0x5a; FIXTURE_BYTES]);
    state.files.prepare_blocked_read();
    let send = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::SendFiles(SendFilesInput {
                    files: vec![handle],
                    target_devices: Vec::new(),
                }))
                .await
        }
    });
    if tokio::time::timeout(Duration::from_secs(5), state.files.wait_until_read_starts())
        .await
        .is_err()
    {
        state.files.release_read();
        let _ = send.await;
        return probe_error("file_read_not_started");
    }
    let files = state.files.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(block_ms)).await;
        files.release_read();
    });
    let started_at = Instant::now();
    let suspend = engine
        .suspend_with_deadline(Duration::from_millis(deadline_ms))
        .await;
    let elapsed_ms = started_at.elapsed().as_millis();
    let outcome = match send.await {
        Ok(Ok(_)) => "completed",
        Ok(Err(_)) => "cancelled",
        Err(_) => "failed",
    };
    match suspend {
        Ok(()) => json!({
            "ok": true,
            "kind": "suspended_during_file_read",
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": outcome,
        }),
        Err(error) => json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": outcome,
        }),
    }
}

pub(super) async fn suspend_during_file_write(
    state: &ProbeState,
    block_ms: u64,
    deadline_ms: u64,
) -> Value {
    let Some(engine) = state.engine.as_ref().cloned() else {
        return probe_error("not_started");
    };
    let entry_id = match latest_history_entry_id(engine.as_ref()).await {
        Ok(entry_id) => entry_id,
        Err(error) => return error,
    };
    let destination = state.files.register_output();
    state.files.prepare_blocked_write();
    let export = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::ExportEntry(ExportEntryInput {
                    entry_id,
                    destination,
                }))
                .await
        }
    });
    if tokio::time::timeout(
        Duration::from_secs(5),
        state.files.wait_until_write_starts(),
    )
    .await
    .is_err()
    {
        state.files.release_write();
        let _ = export.await;
        return probe_error("file_write_not_started");
    }
    let files = state.files.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(block_ms)).await;
        files.release_write();
    });
    let started_at = Instant::now();
    let suspend = engine
        .suspend_with_deadline(Duration::from_millis(deadline_ms))
        .await;
    let elapsed_ms = started_at.elapsed().as_millis();
    let outcome = match export.await {
        Ok(Ok(_)) => "completed",
        Ok(Err(_)) => "cancelled",
        Err(_) => "failed",
    };
    match suspend {
        Ok(()) => json!({
            "ok": true,
            "kind": "suspended_during_file_write",
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": outcome,
        }),
        Err(error) => json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": outcome,
        }),
    }
}

pub(super) async fn verify_stale_clipboard_change_after_suspend(
    state: &ProbeState,
    deadline_ms: u64,
) -> Value {
    let Some(engine) = state.engine.as_ref().cloned() else {
        return probe_error("not_started");
    };
    let marker = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let stale_text = format!("mobile stale callback {marker}");
    let fresh_text = format!("mobile fresh work {marker}");

    let suspend_started_at = Instant::now();
    if let Err(error) = engine
        .suspend_with_deadline(Duration::from_millis(deadline_ms))
        .await
    {
        return json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "phase": "suspend",
            "elapsed_ms": suspend_started_at.elapsed().as_millis(),
        });
    }
    let suspend_elapsed_ms = suspend_started_at.elapsed().as_millis();
    if !state.clipboard.publish_text_change(&stale_text) {
        return probe_error("clipboard_change_stream_unavailable");
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let resume_started_at = Instant::now();
    if let Err(error) = engine.resume().await {
        return json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "phase": "resume",
            "elapsed_ms": resume_started_at.elapsed().as_millis(),
        });
    }
    let resume_elapsed_ms = resume_started_at.elapsed().as_millis();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let stale_count = match history_match_count(engine.as_ref(), &stale_text).await {
        Ok(count) => count,
        Err(error) => return error,
    };
    if stale_count != 0 {
        return json!({
            "ok": false,
            "kind": "stale_clipboard_change_written",
            "stale_count": stale_count,
            "suspend_elapsed_ms": suspend_elapsed_ms,
            "resume_elapsed_ms": resume_elapsed_ms,
        });
    }

    if let Err(error) = engine
        .execute(Operation::SendText(SendTextInput {
            text: fresh_text.clone(),
            target_devices: Vec::new(),
        }))
        .await
    {
        return probe_error(engine_error_kind(&error));
    }
    let fresh_count = match history_match_count(engine.as_ref(), &fresh_text).await {
        Ok(count) => count,
        Err(error) => return error,
    };
    if fresh_count != 1 {
        return json!({
            "ok": false,
            "kind": "fresh_clipboard_write_missing",
            "stale_count": stale_count,
            "fresh_count": fresh_count,
            "suspend_elapsed_ms": suspend_elapsed_ms,
            "resume_elapsed_ms": resume_elapsed_ms,
        });
    }

    json!({
        "ok": true,
        "kind": "stale_clipboard_change_stayed_silent",
        "stale_count": stale_count,
        "fresh_count": fresh_count,
        "suspend_elapsed_ms": suspend_elapsed_ms,
        "resume_elapsed_ms": resume_elapsed_ms,
    })
}

async fn history_match_count(engine: &uc_engine::Engine, query: &str) -> Result<usize, Value> {
    match engine
        .execute(Operation::QueryHistory(QueryHistoryInput {
            cursor: None,
            limit: 10,
            query: Some(query.to_owned()),
        }))
        .await
    {
        Ok(OperationResult::HistoryPage { entries, .. }) => Ok(entries.len()),
        Ok(_) => Err(probe_error("history_unavailable")),
        Err(error) => Err(probe_error(engine_error_kind(&error))),
    }
}

async fn latest_history_entry_id(engine: &uc_engine::Engine) -> Result<String, Value> {
    match engine
        .execute(Operation::QueryHistory(QueryHistoryInput {
            cursor: None,
            limit: 1,
            query: None,
        }))
        .await
    {
        Ok(OperationResult::HistoryPage { entries, .. }) => entries
            .into_iter()
            .next()
            .map(|entry| entry.entry_id)
            .ok_or_else(|| probe_error("history_empty")),
        Ok(_) => Err(probe_error("history_unavailable")),
        Err(error) => Err(probe_error(engine_error_kind(&error))),
    }
}

pub(super) async fn suspend_during_startup(
    state: &mut ProbeState,
    block_ms: u64,
    deadline_ms: u64,
) -> Value {
    let Some(engine) = state.engine.take() else {
        return probe_error("not_started");
    };
    let Some(start_config) = state.start_config.clone() else {
        state.engine = Some(engine);
        return probe_error("startup_config_unavailable");
    };
    if let Err(error) = engine.shutdown(Duration::from_secs(15)).await {
        return probe_error(engine_error_kind(&error));
    }

    let storage = ControlledSecureStorage::new(host_secure_storage());
    storage.prepare_blocked_get();
    let host = HostCapabilities::new(
        start_config.directories,
        Box::new(storage.clone()),
        Box::new(state.clipboard.clone()),
        Box::new(state.files.clone()),
    );
    let (progress_input, _) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let startup = tokio::spawn(uc_engine::Engine::start_with_lifecycle(
        start_config.engine,
        host,
        progress_input,
        lifecycle_input,
    ));
    if tokio::time::timeout(Duration::from_secs(5), storage.wait_until_get_starts())
        .await
        .is_err()
    {
        storage.release_get();
        if let Ok(Ok((engine, stream))) = startup.await {
            store_engine(state, engine, stream);
        }
        return probe_error("secure_storage_get_not_started");
    }
    let storage_release = storage.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(block_ms)).await;
        storage_release.release_get();
    });
    let started_at = Instant::now();
    let suspend = lifecycle
        .suspend_with_deadline(Duration::from_millis(deadline_ms))
        .await;
    let elapsed_ms = started_at.elapsed().as_millis();
    let startup = startup.await;
    let startup_completed = match startup {
        Ok(Ok((engine, stream))) => {
            store_engine(state, engine, stream);
            true
        }
        Ok(Err(_)) | Err(_) => false,
    };
    match (suspend, startup_completed) {
        (Ok(()), true) => json!({
            "ok": true,
            "kind": "suspended_during_startup",
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": "completed",
        }),
        (Ok(()), false) => json!({
            "ok": false,
            "kind": "startup_failed",
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": "failed",
        }),
        (Err(error), completed) => json!({
            "ok": false,
            "kind": engine_error_kind(&error),
            "elapsed_ms": elapsed_ms,
            "block_ms": block_ms,
            "outcome": if completed { "completed" } else { "failed" },
        }),
    }
}
