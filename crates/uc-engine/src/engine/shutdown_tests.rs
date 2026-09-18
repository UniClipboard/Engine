use std::future::{poll_fn, Future};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use tokio::time::timeout;

use super::tests::FakeRuntime;
use super::Engine;
use crate::{EngineErrorCategory, EngineEvent, EngineState, Operation, OperationTerminal};

#[tokio::test]
async fn unbounded_shutdown_keeps_cleanup_owned_after_its_waiter_leaves() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.block_shutdown.store(true, Ordering::SeqCst);
    let (engine, _events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    let owner = Arc::clone(&engine);
    let waiter = tokio::spawn(async move { owner.shutdown_until_complete().await });
    timeout(Duration::from_secs(1), runtime.shutdown_started.notified())
        .await
        .unwrap();
    assert!(runtime.shutdown_deadline.lock().unwrap().is_none());
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    runtime.shutdown_release.notify_one();
    timeout(Duration::from_secs(1), engine.shutdown_until_complete())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
}

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
async fn timed_out_shutdown_keeps_work_alive_and_repeated_wait_does_not_restart_it() {
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
    let second_error = engine
        .shutdown(Duration::from_millis(20))
        .await
        .unwrap_err();
    assert_eq!(
        second_error.category(),
        EngineErrorCategory::DeadlineExceeded
    );
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
    runtime.shutdown_release.notify_one();
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn abandoned_shutdown_waiter_and_engine_do_not_cancel_cleanup() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.block_shutdown.store(true, Ordering::SeqCst);
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
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
    drop(engine);
    runtime.shutdown_release.notify_one();
    timeout(Duration::from_secs(1), async {
        let mut stopped = false;
        while let Some(event) = events.next().await {
            if let EngineEvent::StateChanged {
                state: EngineState::Stopped,
            } = event
            {
                stopped = true;
            }
        }
        assert!(stopped, "调用方离开后也必须完成实际收尾");
    })
    .await
    .unwrap();
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_deadline_includes_lifecycle_queue_wait() {
    let runtime = Arc::new(FakeRuntime::default());
    let (engine, _events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let occupied = engine.lifecycle_gate.lock().await;
    let error = timeout(
        Duration::from_secs(1),
        engine.shutdown(Duration::from_millis(20)),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 0);
    drop(occupied);
    timeout(Duration::from_secs(1), async {
        while engine.lifecycle_state().await != EngineState::Stopped {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn abandoned_shutdown_queue_keeps_ownership_after_engine_drop() {
    let runtime = Arc::new(FakeRuntime::default());
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let occupied = Arc::clone(&engine.lifecycle_gate).lock_owned().await;
    {
        let mut request = Box::pin(engine.shutdown(Duration::from_secs(1)));
        poll_fn(|cx| {
            assert!(request.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert!(engine.stop_requested.load(Ordering::Acquire));
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 0);
    drop(engine);
    drop(occupied);
    timeout(Duration::from_secs(1), async {
        let mut stopped = false;
        while let Some(event) = events.next().await {
            if event
                == (EngineEvent::StateChanged {
                    state: EngineState::Stopped,
                })
            {
                stopped = true;
            }
        }
        assert!(stopped);
    })
    .await
    .unwrap();
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unrepresentable_shutdown_deadline_is_rejected_without_changing_state() {
    let runtime = Arc::new(FakeRuntime::default());
    let (engine, _events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let error = engine.shutdown(Duration::MAX).await.unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::InvalidInput);
    assert_eq!(engine.lifecycle_state().await, EngineState::Running);
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 0);
    assert!(!engine.stop_requested.load(Ordering::Acquire));
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
}

#[tokio::test]
async fn zero_wait_budget_still_starts_cleanup_and_does_not_cancel_it() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.block_shutdown.store(true, Ordering::SeqCst);
    let (engine, _events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let error = engine.shutdown(Duration::ZERO).await.unwrap_err();
    assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
    timeout(Duration::from_secs(1), runtime.shutdown_started.notified())
        .await
        .unwrap();
    runtime.shutdown_release.notify_one();
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn abandoned_waiter_during_operation_drain_keeps_resources_until_actual_exit() {
    let runtime = Arc::new(FakeRuntime::default());
    let (engine, mut events) = Engine::from_runtime(Arc::clone(&runtime), 16);
    let engine = Arc::new(engine);
    let operation = engine.operations.register("held-operation");
    let caller = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.shutdown(Duration::from_millis(30)).await }
    });
    assert_eq!(
        timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        Some(EngineEvent::StateChanged {
            state: EngineState::ShuttingDown
        })
    );
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(engine.execute(Operation::ListDevices).await.is_err());
    assert!(engine.resume().await.is_err());
    drop(engine);

    timeout(Duration::from_secs(1), operation.cancellation.cancelled())
        .await
        .unwrap();
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        Some(EngineEvent::OperationFinished {
            operation_id: operation.id.clone(),
            terminal: OperationTerminal::Cancelled,
        })
    );
    assert!(timeout(Duration::from_millis(20), events.next())
        .await
        .is_err());
    drop(operation);
    assert_eq!(
        timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        Some(EngineEvent::StateChanged {
            state: EngineState::Stopped
        })
    );
    assert_eq!(
        timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap(),
        None
    );
    assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
}
