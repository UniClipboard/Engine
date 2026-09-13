use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use uc_engine::Operation;

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
