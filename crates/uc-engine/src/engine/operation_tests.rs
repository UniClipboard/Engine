use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::operation::await_operation_completion;
use super::{Engine, EngineRuntime};
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, Operation, OperationResult,
    OperationTerminal,
};

struct DiskResource(Arc<AtomicBool>);

impl Drop for DiskResource {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct BlockingRuntime {
    started: Arc<Notify>,
    release: Arc<Barrier>,
    disk_finished: Arc<AtomicBool>,
    shutdown_calls: AtomicUsize,
    suspend_calls: AtomicUsize,
    panic: bool,
}

impl BlockingRuntime {
    fn new(panic: bool) -> Self {
        Self {
            started: Arc::new(Notify::new()),
            release: Arc::new(Barrier::new(2)),
            disk_finished: Arc::new(AtomicBool::new(false)),
            shutdown_calls: AtomicUsize::new(0),
            suspend_calls: AtomicUsize::new(0),
            panic,
        }
    }
}

#[async_trait]
impl EngineRuntime for BlockingRuntime {
    async fn execute(
        &self,
        _operation: Operation,
        _cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        assert!(!self.panic, "private-operation-panic");
        let started = Arc::clone(&self.started);
        let release = Arc::clone(&self.release);
        let finished = Arc::clone(&self.disk_finished);
        tokio::task::spawn_blocking(move || {
            let _resource = DiskResource(finished);
            started.notify_one();
            release.wait();
            Ok(OperationResult::Devices(Vec::new()))
        })
        .await
        .unwrap()
    }

    async fn suspend(&self, _deadline: Option<Instant>) -> Result<(), EngineError> {
        assert!(self.disk_finished.load(Ordering::SeqCst));
        self.suspend_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn resume(&self, _cancellation: CancellationToken) -> Result<(), EngineError> {
        Ok(())
    }
    async fn shutdown(&self, _deadline: Option<Instant>) -> Result<(), EngineError> {
        self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn abandoned_operation_keeps_disk_work_owned_until_shutdown_finishes() {
    let runtime = Arc::new(BlockingRuntime::new(false));
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    let caller = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.execute(Operation::ListDevices).await })
    };
    runtime.started.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let still_registered = !engine.operations.wait_until_empty(Duration::ZERO).await;
    let result = engine.shutdown(Duration::ZERO).await;
    tokio::task::yield_now().await;
    let still_held = !runtime.disk_finished.load(Ordering::SeqCst);
    let shutdown_waited = runtime.shutdown_calls.load(Ordering::SeqCst) == 0;
    drop(engine);
    runtime.release.wait();
    let (cancelled, stopped) = timeout(Duration::from_secs(1), async {
        let mut cancelled = 0;
        let mut stopped = false;
        while let Some(event) = events.next().await {
            match event {
                EngineEvent::OperationFinished {
                    terminal: OperationTerminal::Cancelled,
                    ..
                } => cancelled += 1,
                EngineEvent::StateChanged {
                    state: EngineState::Stopped,
                } => stopped = true,
                _ => {}
            }
        }
        (cancelled, stopped)
    })
    .await
    .unwrap();
    assert_eq!(
        result.unwrap_err().category(),
        EngineErrorCategory::DeadlineExceeded
    );
    assert!(still_registered && still_held && shutdown_waited);
    assert!(runtime.disk_finished.load(Ordering::SeqCst));
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
    assert_eq!(cancelled, 1);
    assert!(stopped);
}

#[tokio::test]
async fn suspend_timeout_during_disk_work_still_completes_without_another_request() {
    let runtime = Arc::new(BlockingRuntime::new(false));
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    let caller = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.execute(Operation::ListDevices).await })
    };
    runtime.started.notified().await;
    let result = engine
        .suspend_with_deadline(Duration::from_millis(10))
        .await;
    let suspended_before_disk = runtime.suspend_calls.load(Ordering::SeqCst);
    caller.abort();
    let abandoned = caller.await;
    drop(engine);
    runtime.release.wait();
    match abandoned {
        Ok(result) => assert_eq!(
            result.unwrap_err().category(),
            EngineErrorCategory::DeadlineExceeded
        ),
        Err(error) => assert!(error.is_cancelled()),
    }
    assert_eq!(
        result.unwrap_err().category(),
        EngineErrorCategory::DeadlineExceeded
    );
    assert_eq!(suspended_before_disk, 0);
    timeout(Duration::from_secs(1), async {
        while let Some(event) = events.next().await {
            if matches!(
                event,
                EngineEvent::StateChanged {
                    state: EngineState::Suspended
                }
            ) {
                return;
            }
        }
        panic!("suspension ended without completion");
    })
    .await
    .unwrap();
    assert!(runtime.disk_finished.load(Ordering::SeqCst));
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn operation_panic_releases_registration_and_reports_one_failure() {
    let runtime = Arc::new(BlockingRuntime::new(true));
    let (engine, mut events) = Engine::from_runtime(runtime, 16);
    let error = engine.execute(Operation::ListDevices).await.unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::Internal);
    assert!(!format!("{error:?}").contains("private-operation-panic"));
    assert!(engine.operations.wait_until_empty(Duration::ZERO).await);
    assert!(matches!(
        events.next().await,
        Some(EngineEvent::OperationFinished {
            terminal: OperationTerminal::Failed(_),
            ..
        })
    ));
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn cancellation_wins_when_operation_result_is_already_ready() {
    let cancellation = CancellationToken::new();
    let task = tokio::spawn(async { Ok::<_, EngineError>(OperationResult::Devices(Vec::new())) });
    while !task.is_finished() {
        tokio::task::yield_now().await;
    }
    cancellation.cancel();

    let error = await_operation_completion(cancellation, task)
        .await
        .unwrap_err();

    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
}
