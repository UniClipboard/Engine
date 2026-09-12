use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use tokio::time::timeout;

use super::{Engine, EngineErrorCategory, EngineEvent, EngineState, HeldRuntime};
use crate::Operation;

async fn accept<F: Future>(mut request: Pin<&mut F>) {
    poll_fn(|context| {
        assert!(request.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn accepted_pause_rejects_operations_before_it_can_acquire_the_transition_lock() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, _events) = Engine::from_runtime(runtime, 32);
    let gate = engine.lifecycle_gate.clone().lock_owned().await;
    let mut pause = Box::pin(engine.suspend());
    accept(pause.as_mut()).await;
    drop(pause);
    assert_eq!(engine.lifecycle_state().await, EngineState::Running);
    let result = timeout(
        Duration::from_millis(100),
        engine.execute(Operation::ListDevices),
    )
    .await;
    drop(gate);
    assert_eq!(
        result.unwrap().unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    engine.suspend().await.unwrap();
    engine.resume().await.unwrap();
    engine.execute(Operation::ListDevices).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn quiesce_during_resume_keeps_admission_closed_until_a_later_resume() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 32);
    engine.suspend().await.unwrap();
    runtime.resume_held.store(true, Ordering::SeqCst);
    let mut resume = Box::pin(engine.resume());
    accept(resume.as_mut()).await;
    runtime.entered.notified().await;
    let mut quiet = Box::pin(engine.quiesce(Duration::ZERO));
    accept(quiet.as_mut()).await;
    let result = timeout(
        Duration::from_millis(100),
        engine.execute(Operation::ListDevices),
    )
    .await;
    runtime.resume_held.store(false, Ordering::SeqCst);
    runtime.release.notify_one();
    assert_eq!(
        result.unwrap().unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    assert_eq!(
        resume.await.unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    quiet.await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
    assert!(engine.execute(Operation::ListDevices).await.is_err());
    engine.resume().await.unwrap();
    engine.execute(Operation::ListDevices).await.unwrap();
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn a_new_pause_discards_a_queued_resume_without_restarting_resources() {
    let runtime = Arc::new(HeldRuntime::default());
    runtime.suspend_held.store(true, Ordering::SeqCst);
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 32);
    let engine = Arc::new(engine);
    let first_pause = tokio::spawn({
        let engine = engine.clone();
        async move { engine.suspend().await }
    });
    runtime.entered.notified().await;
    let mut stale_resume = Box::pin(engine.resume());
    accept(stale_resume.as_mut()).await;
    let mut final_pause = Box::pin(engine.suspend());
    accept(final_pause.as_mut()).await;
    runtime.suspend_held.store(false, Ordering::SeqCst);
    runtime.release.notify_one();
    first_pause.await.unwrap().unwrap();
    let stale = stale_resume.await;
    final_pause.await.unwrap();
    assert_eq!(
        stale.unwrap_err().category(),
        EngineErrorCategory::InvalidState
    );
    assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn alternating_queued_targets_resume_only_after_the_accepted_pause_is_safe() {
    let runtime = Arc::new(HeldRuntime::default());
    runtime.suspend_held.store(true, Ordering::SeqCst);
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 32);
    let engine = Arc::new(engine);
    let first_pause = tokio::spawn({
        let engine = engine.clone();
        async move { engine.suspend().await }
    });
    runtime.entered.notified().await;
    let mut stale_resume = Box::pin(engine.resume());
    accept(stale_resume.as_mut()).await;
    let mut next_pause = Box::pin(engine.suspend());
    accept(next_pause.as_mut()).await;
    let mut final_resume = Box::pin(engine.resume());
    accept(final_resume.as_mut()).await;
    runtime.suspend_held.store(false, Ordering::SeqCst);
    runtime.release.notify_one();
    first_pause.await.unwrap().unwrap();
    assert!(stale_resume.await.is_err());
    next_pause.await.unwrap();
    final_resume.await.unwrap();
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine.lifecycle_state().await, EngineState::Running);
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn an_active_resume_cannot_publish_running_after_a_new_pause_was_accepted() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, mut events) = Engine::from_runtime(runtime.clone(), 32);
    let engine = Arc::new(engine);
    engine.suspend().await.unwrap();
    runtime.resume_held.store(true, Ordering::SeqCst);
    let first_resume = tokio::spawn({
        let engine = engine.clone();
        async move { engine.resume().await }
    });
    runtime.entered.notified().await;
    let mut next_pause = Box::pin(engine.suspend());
    accept(next_pause.as_mut()).await;
    let mut final_resume = Box::pin(engine.resume());
    accept(final_resume.as_mut()).await;
    runtime.resume_held.store(false, Ordering::SeqCst);
    runtime.release.notify_one();
    assert!(first_resume.await.unwrap().is_err());
    next_pause.await.unwrap();
    final_resume.await.unwrap();
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 2);
    let mut running = 0;
    while let Ok(Some(event)) = timeout(Duration::ZERO, events.next()).await {
        if matches!(
            event,
            EngineEvent::StateChanged {
                state: EngineState::Running
            }
        ) {
            running += 1;
        }
    }
    assert_eq!(running, 1, "only the latest resume may publish Running");
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn duplicate_resume_requests_share_the_running_result_without_another_start() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 32);
    let engine = Arc::new(engine);
    engine.suspend().await.unwrap();
    runtime.resume_held.store(true, Ordering::SeqCst);
    let first = tokio::spawn({
        let engine = engine.clone();
        async move { engine.resume().await }
    });
    runtime.entered.notified().await;
    let mut duplicate = Box::pin(engine.resume());
    accept(duplicate.as_mut()).await;
    runtime.resume_held.store(false, Ordering::SeqCst);
    runtime.release.notify_one();
    first.await.unwrap().unwrap();
    duplicate.await.unwrap();
    assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 1);
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn a_panicked_transition_does_not_abandon_the_following_accepted_pause() {
    let runtime = Arc::new(HeldRuntime::default());
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 32);
    let engine = Arc::new(engine);
    engine.suspend().await.unwrap();
    runtime.resume_held.store(true, Ordering::SeqCst);
    runtime.panic_resume.store(true, Ordering::SeqCst);
    let resuming = tokio::spawn({
        let engine = engine.clone();
        async move { engine.resume().await }
    });
    runtime.entered.notified().await;
    let mut pause = Box::pin(engine.suspend());
    accept(pause.as_mut()).await;
    runtime.release.notify_one();
    assert_eq!(
        resuming.await.unwrap().unwrap_err().category(),
        EngineErrorCategory::Internal
    );
    timeout(Duration::from_secs(1), pause)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
    assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 2);
    engine.shutdown_until_complete().await.unwrap();
}

#[tokio::test]
async fn final_shutdown_releases_all_queued_request_owners_before_confirmation() {
    let runtime = Arc::new(HeldRuntime::default());
    let resource = runtime.resource.clone();
    let (engine, _events) = Engine::from_runtime(runtime.clone(), 32);
    let gate = engine.lifecycle_gate.clone().lock_owned().await;
    let mut requests = Vec::new();
    for _ in 0..32 {
        let mut request = Box::pin(engine.suspend());
        accept(request.as_mut()).await;
        requests.push(request);
    }
    let mut closing = Box::pin(engine.shutdown_until_complete());
    accept(closing.as_mut()).await;
    drop(gate);
    closing.await.unwrap();
    drop(requests);
    drop(engine);
    drop(runtime);
    assert_eq!(
        Arc::strong_count(&resource),
        1,
        "queued requests retained the closed runtime"
    );
}
