use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{StartupLifecycle, StartupLifecycleInput};
use crate::engine::{Engine, EngineRuntime};
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, Operation, OperationResult,
};

#[derive(Default)]
struct RecordingRuntime {
    suspends: AtomicUsize,
    resumes: AtomicUsize,
    deadlines: Mutex<Vec<Option<Instant>>>,
}

#[async_trait]
impl EngineRuntime for RecordingRuntime {
    async fn execute(
        &self,
        _: Operation,
        _: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        Ok(OperationResult::Devices(Vec::new()))
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        self.suspends.fetch_add(1, Ordering::SeqCst);
        self.deadlines.lock().unwrap().push(deadline);
        Ok(())
    }

    async fn resume(&self, _: CancellationToken) -> Result<(), EngineError> {
        self.resumes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn shutdown(&self, _: Option<Instant>) -> Result<(), EngineError> {
        Ok(())
    }
}

async fn accept(future: std::pin::Pin<&mut impl Future<Output = Result<(), EngineError>>>) {
    let mut future = future;
    poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
}

fn bind(input: &StartupLifecycleInput, runtime: Arc<RecordingRuntime>) -> Engine {
    Engine::from_runtime_with_queue(runtime, 16, input.requests.clone()).0
}

#[tokio::test]
async fn an_unused_or_failed_startup_finishes_pending_notifications() {
    for failed in [false, true] {
        let (input, control) = StartupLifecycle::channel();
        let mut waiting = Box::pin(control.suspend());
        accept(waiting.as_mut()).await;
        if failed {
            input.requests.fail_startup(EngineError::new(
                9101,
                EngineErrorCategory::Unavailable,
                false,
            ));
        }
        drop(input);
        let expected = if failed {
            EngineErrorCategory::Unavailable
        } else {
            EngineErrorCategory::Internal
        };
        assert_eq!(waiting.await.unwrap_err().category(), expected);
        assert_eq!(control.resume().await.unwrap_err().category(), expected);
    }
}

#[tokio::test]
async fn a_deadline_before_binding_still_pauses_with_the_original_deadline() {
    let (input, control) = StartupLifecycle::channel();
    let error = control
        .suspend_with_deadline(Duration::ZERO)
        .await
        .unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    let runtime = Arc::new(RecordingRuntime::default());
    let engine = bind(&input, runtime.clone());
    drop(input);
    engine.lifecycle_requests.wait_empty().await;
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    assert_eq!(runtime.suspends.load(Ordering::SeqCst), 1);
    assert!(runtime.deadlines.lock().unwrap()[0].unwrap() <= Instant::now());
    assert!(engine.execute(Operation::ListDevices).await.is_err());
    control.resume().await.unwrap();
    assert!(engine.execute(Operation::ListDevices).await.is_ok());
    engine.shutdown_until_complete().await.unwrap();
    assert_eq!(
        control.resume().await.unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    drop(engine);
    assert_eq!(Arc::strong_count(&runtime), 1);
}

#[tokio::test]
async fn queued_startup_targets_share_order_and_cancel_superseded_resumes() {
    let (input, control) = StartupLifecycle::channel();
    let mut first = Box::pin(control.suspend());
    accept(first.as_mut()).await;
    let mut resume = Box::pin(control.resume());
    accept(resume.as_mut()).await;
    let mut last = Box::pin(control.suspend());
    accept(last.as_mut()).await;
    drop(first);
    let runtime = Arc::new(RecordingRuntime::default());
    let (engine, mut events) =
        Engine::from_runtime_with_queue(runtime.clone(), 16, input.requests.clone());
    drop(input);
    assert_eq!(
        resume.await.unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    last.await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    assert_eq!(runtime.suspends.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.resumes.load(Ordering::SeqCst), 0);
    while let Ok(Some(event)) = tokio::time::timeout(Duration::ZERO, events.next()).await {
        assert!(!matches!(
            event,
            EngineEvent::StateChanged {
                state: EngineState::Running
            }
        ));
    }
    engine.shutdown_until_complete().await.unwrap();
}
