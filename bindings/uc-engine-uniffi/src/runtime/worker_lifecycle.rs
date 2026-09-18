use std::future::{poll_fn, Future};
use std::sync::{mpsc, Arc};
use std::task::Poll;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinSet;
use uc_engine::Engine;

use super::map_engine_state;
use crate::observability::schedule_flush_after_success;
use crate::{BindingEngineState, BindingError};

type ShutdownReply = mpsc::Sender<Result<(), BindingError>>;

pub(super) enum LifecycleCommand {
    Suspend {
        deadline: Instant,
        response: ShutdownReply,
    },
    Resume {
        response: ShutdownReply,
    },
    Shutdown {
        deadline: Instant,
        response: ShutdownReply,
    },
    LifecycleState {
        response: mpsc::Sender<BindingEngineState>,
    },
}

pub(super) async fn run(
    engine: Arc<Engine>,
    mut requests: UnboundedReceiver<LifecycleCommand>,
) -> Vec<ShutdownReply> {
    let mut pending = JoinSet::new();
    let response = loop {
        tokio::select! {
            biased;
            completed = pending.join_next(), if !pending.is_empty() => {
                match completed {
                    Some(Ok(Some(response))) => break Some(response),
                    Some(Err(_)) => break None,
                    _ => {}
                }
            }
            command = requests.recv() => {
                let Some(command) = command else { break None };
                if let Some(Some(response)) = forward(&mut pending, dispatch(Arc::clone(&engine), command)).await {
                    break Some(response);
                }
            }
        }
    };
    requests.close();
    let mut responses: Vec<_> = response.into_iter().collect();
    // 通道离开也由 Engine 完成实际关闭，再回收所有仅负责等待结果的转交任务。
    let _ = engine.shutdown_until_complete().await;
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
    responses
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
            schedule_flush_after_success(&result);
            let _ = response.send(result);
        }
        LifecycleCommand::Resume { response } => {
            let _ = response.send(engine.resume().await.map_err(BindingError::from));
        }
        LifecycleCommand::LifecycleState { response } => {
            let _ = response.send(map_engine_state(engine.lifecycle_state().await));
        }
        LifecycleCommand::Shutdown { deadline, response } => {
            let result = engine
                .shutdown(deadline.saturating_duration_since(Instant::now()))
                .await
                .map_err(BindingError::from);
            schedule_flush_after_success(&result);
            if result.is_ok() {
                return Some(response);
            }
            let _ = response.send(result);
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
