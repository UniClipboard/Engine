use std::io::Write;

use tracing::instrument::WithSubscriber;

use super::factory::engine_event_for_active_clipboard;
use super::*;

#[derive(Clone, Default)]
struct CapturedWriter(Arc<StdMutex<Vec<u8>>>);

impl Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedWriter {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn session_lifecycle_records_an_early_failure_as_the_total_result() {
    let writer = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();

    let result = observe_session_lifecycle(async { Err::<(), _>(operation_unavailable_error()) })
        .with_subscriber(subscriber)
        .await;

    assert!(result.is_err());
    let output = String::from_utf8(
        writer
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("UTF-8 logs");
    assert!(output.contains("uc.operation=\"session_lifecycle\""));
    assert!(output.contains("uc.outcome=\"error\""));
    assert!(output.contains("error.type=\"unavailable\""));
}

#[tokio::test]
async fn requested_session_transition_records_start_and_failure_without_hiding_the_result() {
    let writer = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();
    let result = observe_session_request(
        uc_observability_contract::diagnostics::connectivity::SessionTransition::Suspend,
        async {
            Err::<uc_observability_contract::diagnostics::connectivity::SessionTransitionResult, _>(
                operation_unavailable_error(),
            )
        },
    )
    .with_subscriber(subscriber)
    .await;
    assert!(result.is_err());
    let output = String::from_utf8(writer.0.lock().expect("capture").clone()).expect("logs");
    assert!(output.contains("suspend"));
    assert!(output.contains("session.transition.started"));
    assert!(output.contains("failed"));
    assert!(
        output.contains("unavailable"),
        "the known failure category must remain visible"
    );
    assert_eq!(output.lines().count(), 2);
}

#[tokio::test]
async fn cancelled_session_transition_records_interruption_instead_of_success() {
    let writer = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(writer.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(5),
        observe_session_request(
            uc_observability_contract::diagnostics::connectivity::SessionTransition::Suspend,
            std::future::pending::<
                Result<
                    uc_observability_contract::diagnostics::connectivity::SessionTransitionResult,
                    EngineError,
                >,
            >(),
        ),
    )
    .await;
    assert!(result.is_err());
    let output = String::from_utf8(writer.0.lock().expect("capture").clone()).expect("logs");
    assert!(
        output.contains("interrupted"),
        "dropping the operation must not leave a false active transition"
    );
    assert!(!output.contains("completed"));
}

#[test]
fn session_runtime_failure_remains_retryable_without_becoming_an_operation_error() {
    let error = session_runtime_error("prepare p2p session", "test failure");

    assert_eq!(error.code(), SESSION_RUNTIME_FAILED_CODE);
    assert_eq!(error.category(), EngineErrorCategory::Unavailable);
    assert!(error.is_retryable());
}

#[test]
fn space_transition_runtime_failure_remains_retryable_for_background_recovery() {
    let error =
        retryable_space_transition_runtime_error("complete Space transition", "test failure");

    assert_eq!(error.code(), 1103);
    assert_eq!(error.category(), EngineErrorCategory::Unavailable);
    assert!(error.is_retryable());
}

#[test]
fn missing_active_join_stops_background_transition_recovery() {
    let error = space_transition_error(
        "complete Space transition",
        CompletePendingSpaceTransitionError::JoinNotActive,
    );

    assert_eq!(error.code(), 1103);
    assert_eq!(error.category(), EngineErrorCategory::Internal);
    assert!(!error.is_retryable());
}

#[test]
fn active_clipboard_event_preserves_mobile_sse_identity() {
    let state = uc_core::clipboard::ActiveClipboardState::new(
        "hash-1",
        uc_core::ids::EntryId::from("entry-1"),
        42,
        uc_core::ids::DeviceId::new("device-1"),
    );

    assert_eq!(
        engine_event_for_active_clipboard(&state),
        EngineEvent::ActiveClipboardChanged(ActiveClipboardChanged {
            snapshot_hash: "hash-1".into(),
            entry_id: "entry-1".into(),
            activated_at_ms: 42,
            activated_by: "device-1".into(),
        })
    );
}

#[tokio::test]
async fn closed_gate_rejects_new_operations_and_waits_for_the_existing_one() {
    let gate = Arc::new(SessionOperationGate::new_open());
    let lease = gate.acquire().expect("new gate accepts an operation");
    let closing = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move { gate.close_and_wait(None, None).await }
    });
    tokio::task::yield_now().await;
    assert!(gate.acquire().is_err());
    assert!(!closing.is_finished());

    drop(lease);
    match closing.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("operation gate close failed: {error}"),
        Err(error) => panic!("operation gate close task did not complete: {error}"),
    }
}

// 忽略取消的工作仍持有资源时，关闭不能报告成功，且继续拒绝新工作。
#[tokio::test(start_paused = true)]
async fn closed_gate_stops_waiting_when_an_operation_ignores_cancellation() {
    let gate = Arc::new(SessionOperationGate::new_open());
    let _lease = gate.acquire().expect("new gate accepts an operation");

    let error = tokio::time::timeout(
        SESSION_OPERATION_GRACE.saturating_mul(2) + Duration::from_millis(1),
        gate.close_and_wait(None, None),
    )
    .await
    .expect("gate close must remain bounded after cancellation")
    .expect_err("active operations prevent successful suspension");
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    assert!(gate.acquire().is_err());
    drop(_lease);
    gate.close_and_wait(None, None).await.unwrap();
}

#[tokio::test]
async fn transition_operation_excludes_itself_but_waits_for_other_operations() {
    let gate = Arc::new(SessionOperationGate::new_open());
    let transition = gate.acquire().expect("transition acquires ordinary lease");
    let other = gate.acquire().expect("concurrent operation acquires lease");
    let closing = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move { gate.close_and_wait(Some(transition), None).await }
    });

    tokio::task::yield_now().await;
    assert!(gate.acquire().is_err());
    assert!(!closing.is_finished());

    drop(other);
    closing.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn shared_deadline_limits_both_drain_and_cancellation_waits() {
    let gate = SessionOperationGate::new_open();
    let lease = gate.acquire().unwrap();
    let started = Instant::now();
    let deadline = Some(started + Duration::from_millis(10));
    let error = gate.close_and_wait(None, deadline).await.unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(gate.acquire().is_err());
    drop(lease);
    gate.close_and_wait(None, deadline).await.unwrap();
}
