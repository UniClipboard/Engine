use std::sync::Arc;

use futures::FutureExt;
use tokio::time::Instant;
use tracing::Instrument;

use super::{
    NetworkRecoveryEvent, NetworkRecoveryInner, NetworkRecoveryPhase, NetworkRecoveryRequestError,
    RebuildNetworkSessionError, RecoveryCompletion, RETRY_DELAYS,
};

pub(super) fn start_cycle(inner: Arc<NetworkRecoveryInner>) -> RecoveryCompletion {
    let task = tokio::spawn(run_recovery_cycle(Arc::clone(&inner)).in_current_span());
    let completion = async move {
        match task.await {
            Ok(result) => result,
            Err(source) => {
                let source = Arc::new(source);
                let mut state = inner.state.lock().await;
                state.failure = Some(NetworkRecoveryRequestError::Task(Arc::clone(&source)));
                if state.phase != NetworkRecoveryPhase::Stopped && !inner.cancel.is_cancelled() {
                    state.phase = NetworkRecoveryPhase::Failed;
                    let _ = inner
                        .events
                        .send(NetworkRecoveryEvent::Failed { retryable: false });
                }
                state.next_retry_at = None;
                state.in_flight = None;
                Err(NetworkRecoveryRequestError::Task(source))
            }
        }
    }
    .boxed()
    .shared();
    let owner = completion.clone();
    // 共享结果只负责等待；独立所有者保证最后一个请求离开后仍执行并记录结果。
    tokio::spawn(async move {
        let _ = owner.await;
    });
    completion
}

async fn run_recovery_cycle(
    inner: Arc<NetworkRecoveryInner>,
) -> Result<(), NetworkRecoveryRequestError> {
    let mut last_error = RebuildNetworkSessionError::Retryable;
    for attempt in 0..=RETRY_DELAYS.len() {
        if attempt > 0 {
            let delay = RETRY_DELAYS[attempt - 1];
            {
                let mut state = inner.state.lock().await;
                if state.phase == NetworkRecoveryPhase::Stopped || inner.cancel.is_cancelled() {
                    return Err(NetworkRecoveryRequestError::Stopped);
                }
                state.phase = NetworkRecoveryPhase::RetryScheduled;
                state.next_retry_at = Some(Instant::now() + delay);
                let _ = inner
                    .events
                    .send(NetworkRecoveryEvent::RetryScheduled { delay });
            }
            tokio::select! {
                _ = inner.cancel.cancelled() => return Err(NetworkRecoveryRequestError::Stopped),
                _ = inner.manual_wake.notified() => {}
                _ = tokio::time::sleep(delay) => {}
            }
        }

        {
            let mut state = inner.state.lock().await;
            if state.phase == NetworkRecoveryPhase::Stopped || inner.cancel.is_cancelled() {
                return Err(NetworkRecoveryRequestError::Stopped);
            }
            let resumed_from_retry = state.phase == NetworkRecoveryPhase::RetryScheduled;
            state.phase = NetworkRecoveryPhase::Recovering;
            state.next_retry_at = None;
            if resumed_from_retry {
                let _ = inner.events.send(NetworkRecoveryEvent::Started);
            }
        }
        // 完整重建可能正在收尾旧会话或写入磁盘，停止通知不能丢弃这个动作。
        let result = inner
            .port
            .rebuild_network_session()
            .await
            .map_err(NetworkRecoveryRequestError::from);
        if inner.cancel.is_cancelled() {
            return finish_cycle(&inner, result, false).await;
        }
        match result {
            Ok(()) => {
                return finish_cycle(&inner, Ok(()), false).await;
            }
            Err(NetworkRecoveryRequestError::Rebuild(RebuildNetworkSessionError::Retryable)) => {
                last_error = RebuildNetworkSessionError::Retryable;
            }
            Err(error) => {
                return finish_cycle(&inner, Err(error), false).await;
            }
        }
    }
    finish_cycle(
        &inner,
        Err(NetworkRecoveryRequestError::Rebuild(last_error)),
        true,
    )
    .await
}

async fn finish_cycle(
    inner: &NetworkRecoveryInner,
    result: Result<(), NetworkRecoveryRequestError>,
    retryable: bool,
) -> Result<(), NetworkRecoveryRequestError> {
    let mut state = inner.state.lock().await;
    state.in_flight = None;
    state.next_retry_at = None;
    if inner.cancel.is_cancelled() {
        state.phase = NetworkRecoveryPhase::Stopped;
        if let Err(error) = &result {
            state.failure = Some(error.clone());
        }
        return result.and(Err(NetworkRecoveryRequestError::Stopped));
    }
    let event = if result.is_ok() {
        state.phase = NetworkRecoveryPhase::Idle;
        NetworkRecoveryEvent::Succeeded
    } else {
        state.phase = NetworkRecoveryPhase::Failed;
        NetworkRecoveryEvent::Failed { retryable }
    };
    // 结果发布与关闭接收使用同一把锁，不能在关闭确认后再发布旧成功。
    let _ = inner.events.send(event);
    result
}
