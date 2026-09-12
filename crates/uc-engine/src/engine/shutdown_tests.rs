use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::timeout;

use super::tests::FakeRuntime;
use super::Engine;
use crate::{EngineErrorCategory, EngineEvent, EngineState, Operation};

#[tokio::test]
async fn failed_shutdown_keeps_stream_open_and_allows_cleanup_retry() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.fail_shutdown.store(true, Ordering::SeqCst);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let error = engine.shutdown(Duration::from_secs(1)).await.unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::Internal);
    assert_eq!(engine.lifecycle_state().await, EngineState::ShuttingDown);
    assert!(engine.resume().await.is_err());
    assert!(engine.execute(Operation::ListDevices).await.is_err());

    loop {
        match timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap()
        {
            Some(EngineEvent::Fatal { error: observed }) => {
                assert_eq!(observed, error);
                break;
            }
            Some(EngineEvent::StateChanged { state }) => assert_ne!(state, EngineState::Stopped),
            None => panic!("失败收尾不能关闭事件流"),
            _ => {}
        }
    }
    assert!(timeout(Duration::from_millis(20), events.next())
        .await
        .is_err());
    runtime.fail_shutdown.store(false, Ordering::SeqCst);
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 2);
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
    timeout(Duration::from_secs(1), async {
        let mut stopped = 0;
        while let Some(event) = events.next().await {
            if let EngineEvent::StateChanged {
                state: EngineState::Stopped,
            } = event
            {
                stopped += 1;
            }
        }
        assert_eq!(stopped, 1);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn timed_out_shutdown_does_not_claim_stopped_and_can_retry() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.block_shutdown.store(true, Ordering::SeqCst);
    let (engine, _events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let error = engine
        .shutdown(Duration::from_millis(20))
        .await
        .unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    assert_eq!(engine.lifecycle_state().await, EngineState::ShuttingDown);
    assert!(engine.resume().await.is_err());
    runtime.block_shutdown.store(false, Ordering::SeqCst);
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
}

#[tokio::test]
async fn abandoned_shutdown_waiter_does_not_prevent_cleanup_retry() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.block_shutdown.store(true, Ordering::SeqCst);
    let (engine, _events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    let caller = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.shutdown(Duration::from_secs(60)).await }
    });
    timeout(Duration::from_secs(1), runtime.shutdown_started.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert_eq!(engine.lifecycle_state().await, EngineState::ShuttingDown);
    assert!(engine.resume().await.is_err());
    runtime.block_shutdown.store(false, Ordering::SeqCst);
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
}
