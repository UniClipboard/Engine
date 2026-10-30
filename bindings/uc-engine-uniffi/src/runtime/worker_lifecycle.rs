use std::future::{poll_fn, Future};
use std::sync::{mpsc, Arc};
use std::task::Poll;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinSet;
use uc_engine::{Engine, ResourceReleaseOutcome};

use super::map_engine_state;
use super::shutdown_wait::ShutdownReply;
use crate::observability::schedule_flush_after_lifecycle;
use crate::{BindingEngineState, BindingError, BindingResourceRelease};

pub(super) enum LifecycleCommand {
    Suspend {
        deadline: Instant,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Resume {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Shutdown {
        deadline: Instant,
        response: ShutdownReply,
    },
    LifecycleState {
        response: mpsc::Sender<BindingEngineState>,
    },
}

pub(super) async fn run<F, T>(
    engine: Arc<Engine>,
    mut requests: UnboundedReceiver<LifecycleCommand>,
    shutdown: F,
) -> (Vec<ShutdownReply>, ResourceReleaseOutcome)
where
    F: Fn(Arc<Engine>, Option<Duration>) -> T,
    T: Future<Output = ResourceReleaseOutcome>,
{
    let mut pending = JoinSet::new();
    let mut failed = None;
    let (response, outcome) = loop {
        tokio::select! {
            biased;
            completed = pending.join_next(), if !pending.is_empty() => {
                match completed {
                    Some(Ok(Some(response))) => break (Some(response), None),
                    Some(Err(_)) => break (None, failed),
                    _ => {}
                }
            }
            command = requests.recv() => {
                let Some(command) = command else { break (None, failed) };
                if let LifecycleCommand::Shutdown { response, deadline } = command {
                    // 调用方的等待期限由 ShutdownWait 管理，不取消已接收的关闭。
                    let outcome = shutdown(Arc::clone(&engine), Some(deadline.saturating_duration_since(Instant::now()))).await;
                    schedule_flush_after_lifecycle();
                    if matches!(outcome, ResourceReleaseOutcome::Released { .. }) {
                        break (Some(response), Some(outcome));
                    }
                    let _ = response.send(BindingResourceRelease::from(outcome.clone()));
                    failed = Some(outcome);
                    continue;
                }
                if let Some(Some(response)) = forward(&mut pending, dispatch(Arc::clone(&engine), command)).await {
                    break (Some(response), None);
                }
            }
        }
    };
    requests.close();
    let mut responses: Vec<_> = response.into_iter().collect();
    // 通道离开也由 Engine 完成实际关闭，再回收所有仅负责等待结果的转交任务。
    let outcome = match outcome {
        Some(outcome) => outcome,
        None => {
            let outcome = shutdown(Arc::clone(&engine), None).await;
            schedule_flush_after_lifecycle();
            outcome
        }
    };
    while let Some(completed) = pending.join_next().await {
        if let Ok(Some(reply)) = completed {
            responses.push(reply);
        }
    }
    while let Ok(command) = requests.try_recv() {
        if let LifecycleCommand::Shutdown { response, .. } = command {
            responses.push(response);
        }
    }
    (responses, outcome)
}

async fn forward<T: Send + 'static>(
    pending: &mut JoinSet<T>,
    work: impl Future<Output = T> + Send + 'static,
) -> Option<T> {
    let mut work = Box::pin(work);
    // 按通道顺序先驱动到 Engine 接收点，再并发等待；任务调度不能重排通知。
    match poll_fn(|context| Poll::Ready(work.as_mut().poll(context))).await {
        Poll::Ready(result) => Some(result),
        Poll::Pending => {
            pending.spawn(work);
            None
        }
    }
}

async fn dispatch(engine: Arc<Engine>, command: LifecycleCommand) -> Option<ShutdownReply> {
    match command {
        LifecycleCommand::Suspend { deadline, response } => {
            let result = engine
                .suspend_with_deadline(deadline.saturating_duration_since(Instant::now()))
                .await
                .map_err(BindingError::from);
            schedule_flush_after_lifecycle();
            let _ = response.send(result);
        }
        LifecycleCommand::Resume { response } => {
            let _ = response.send(engine.resume().await.map_err(BindingError::from));
        }
        LifecycleCommand::LifecycleState { response } => {
            let _ = response.send(map_engine_state(engine.lifecycle_state().await));
        }
        LifecycleCommand::Shutdown { .. } => {
            unreachable!("shutdown is handled by the lifecycle owner")
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::Notify;
    use tokio::task::JoinSet;

    use super::forward;

    #[tokio::test]
    async fn forwarding_preserves_acceptance_order_without_waiting_for_prior_completion() {
        let order = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(Notify::new());
        let mut pending = JoinSet::new();
        for target in 0..3 {
            let order = order.clone();
            let release = release.clone();
            assert!(forward(&mut pending, async move {
                order.lock().unwrap().push(target);
                release.notified().await;
                target
            })
            .await
            .is_none());
        }
        assert_eq!(*order.lock().unwrap(), vec![0, 1, 2]);
        release.notify_waiters();
        let mut finished = Vec::new();
        while let Some(result) = pending.join_next().await {
            finished.push(result.unwrap());
        }
        finished.sort();
        assert_eq!(finished, vec![0, 1, 2]);
    }
}
