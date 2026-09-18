use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::time::{advance, timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::{Engine, EngineRuntime, EventStream};
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, Operation, OperationResult,
};

mod targets;

#[derive(Default)]
struct HeldRuntime {
    suspend_held: AtomicBool,
    resume_held: AtomicBool,
    panic_resume: AtomicBool,
    suspend_calls: AtomicUsize,
    resume_calls: AtomicUsize,
    entered: Notify,
    release: Notify,
    resource: Arc<()>,
    suspend_deadline: StdMutex<Option<Instant>>,
}

#[async_trait]
impl EngineRuntime for HeldRuntime {
    async fn execute(
        &self,
        _operation: Operation,
        _cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        Ok(OperationResult::Devices(Vec::new()))
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        *self.suspend_deadline.lock().unwrap() = deadline;
        self.suspend_calls.fetch_add(1, Ordering::SeqCst);
        if self.suspend_held.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }

    async fn resume(&self, _cancellation: CancellationToken) -> Result<(), EngineError> {
        self.resume_calls.fetch_add(1, Ordering::SeqCst);
        if self.resume_held.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        assert!(
            !self.panic_resume.load(Ordering::SeqCst),
            "intentional resume panic"
        );
        Ok(())
    }

    async fn shutdown(&self, _deadline: Option<Instant>) -> Result<(), EngineError> {
        Ok(())
    }
}

async fn wait_state(events: &mut EventStream, expected: EngineState) {
    timeout(Duration::from_secs(2), async {
        while let Some(event) = events.next().await {
            if matches!(event, EngineEvent::StateChanged { state } if state == expected) {
                return;
            }
        }
        panic!("状态发布前事件流已结束");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn accepted_shutdown_prevents_a_late_resume_from_reopening_operations() {
    let runtime = Arc::new(HeldRuntime::default());
    runtime.resume_held.store(true, Ordering::SeqCst);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    engine.suspend().await.unwrap();
    wait_state(&mut events, EngineState::Suspended).await;
    let resuming = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.resume().await })
    };
    runtime.entered.notified().await;
    let error = engine.shutdown(Duration::ZERO).await.unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    runtime.release.notify_one();
    assert!(resuming.await.unwrap().is_err());
    assert!(engine.execute(Operation::ListDevices).await.is_err());
    timeout(Duration::from_secs(1), async {
        while let Some(event) = events.next().await {
            assert_ne!(
                event,
                EngineEvent::StateChanged {
                    state: EngineState::Running
                }
            );
        }
    })
    .await
    .unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
}

#[tokio::test]
async fn cancelled_suspend_waiter_does_not_interrupt_completion_or_repeat_work() {
    let runtime = Arc::new(HeldRuntime::default());
    runtime.suspend_held.store(true, Ordering::SeqCst);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    let waiter = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.suspend().await }
    });
    runtime.entered.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
    assert_eq!(
        timeout(
            Duration::from_millis(10),
            engine.execute(Operation::ListDevices)
        )
        .await
        .unwrap()
        .unwrap_err()
        .category(),
        EngineErrorCategory::InvalidState
    );
    runtime.release.notify_one();
    wait_state(&mut events, EngineState::Suspended).await;
    assert!(engine.execute(Operation::ListDevices).await.is_err());
    engine.suspend().await.unwrap();
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
    engine.resume().await.unwrap();
    assert!(engine.execute(Operation::ListDevices).await.is_ok());
}

#[tokio::test]
async fn cancelled_resume_waiter_does_not_leave_a_running_runtime_marked_suspended() {
    let runtime = Arc::new(HeldRuntime::default());
    runtime.resume_held.store(true, Ordering::SeqCst);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    engine.suspend().await.unwrap();
    let waiter = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.resume().await }
    });
    runtime.entered.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
    runtime.release.notify_one();
    wait_state(&mut events, EngineState::Running).await;
    engine.resume().await.unwrap();
    assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 1);
    assert!(engine.execute(Operation::ListDevices).await.is_ok());
}

#[tokio::test]
async fn queued_suspend_survives_waiter_and_engine_destruction() {
    let runtime = Arc::new(HeldRuntime::default());
    let resource = Arc::clone(&runtime.resource);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let gate = Arc::clone(&engine.lifecycle_gate).lock_owned().await;
    let mut request = Box::pin(engine.suspend());
    poll_fn(|context| {
        assert!(request.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(request);
    drop(engine);
    drop(runtime);
    assert_eq!(Arc::strong_count(&resource), 2);
    drop(gate);
    wait_state(&mut events, EngineState::Suspended).await;
    assert_eq!(Arc::strong_count(&resource), 1);
}

#[tokio::test(start_paused = true)]
async fn abandoned_quiesce_keeps_the_deadline_from_before_queueing() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, mut events) = Engine::from_runtime(runtime, 16);
    let operation = engine.operations.register("test");
    let gate = Arc::clone(&engine.lifecycle_gate).lock_owned().await;
    let started = Instant::now();
    let mut request = Box::pin(engine.quiesce(Duration::from_secs(1)));
    poll_fn(|context| {
        assert!(request.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(request);
    advance(Duration::from_secs(2)).await;
    drop(gate);
    wait_state(&mut events, EngineState::Quiesced).await;
    assert!(started.elapsed() < Duration::from_millis(2100));
    assert!(operation.cancellation.is_cancelled());
}

#[tokio::test(start_paused = true)]
async fn suspend_timeout_keeps_cleanup_owned_until_actual_completion() {
    let runtime = Arc::new(HeldRuntime::default());
    runtime.suspend_held.store(true, Ordering::SeqCst);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let error = engine
        .suspend_with_deadline(Duration::from_millis(10))
        .await
        .unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
    runtime.release.notify_one();
    wait_state(&mut events, EngineState::Suspended).await;
    engine
        .suspend_with_deadline(Duration::from_millis(10))
        .await
        .unwrap();
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn queued_suspend_keeps_its_expired_deadline_after_the_caller_times_out() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let gate = Arc::clone(&engine.lifecycle_gate).lock_owned().await;
    let started = Instant::now();
    let budget = Duration::from_millis(10);
    assert_eq!(
        engine
            .suspend_with_deadline(budget)
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::DeadlineExceeded
    );
    advance(Duration::from_millis(20)).await;
    drop(gate);
    wait_state(&mut events, EngineState::Suspended).await;
    assert_eq!(
        *runtime.suspend_deadline.lock().unwrap(),
        Some(started + budget)
    );
}

#[tokio::test]
async fn unrepresentable_suspend_deadline_is_rejected_before_acceptance() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, _) = Engine::from_runtime(Arc::clone(&runtime), 16);
    assert_eq!(
        engine
            .suspend_with_deadline(Duration::MAX)
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::InvalidInput
    );
    assert_eq!(engine.lifecycle_state().await, EngineState::Running);
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 0);
}
