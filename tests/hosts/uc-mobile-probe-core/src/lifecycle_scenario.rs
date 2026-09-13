use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uc_engine::{
    ClipboardRestoreMode, ExportEntryInput, Operation, OperationResult, QueryHistoryInput,
    RestoreClipboardInput, SendFilesInput,
};

use super::{engine_error_kind, probe_error, ProbeState};

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
