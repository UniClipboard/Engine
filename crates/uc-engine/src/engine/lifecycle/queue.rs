use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{oneshot, Notify};
use tokio_util::sync::CancellationToken;

use super::super::event_stream::EventSender;
use super::super::in_flight::{InFlightOperations, RegisteredOperation};
use super::super::invalid_state_error;
use super::{Request, Transition};
use crate::{EngineError, EngineErrorCategory, EngineEvent, EngineState};

#[derive(Default)]
pub(in super::super) struct TransitionQueue {
    state: Mutex<State>,
    drained: Notify,
}

#[derive(Default)]
struct State {
    admission_closed: bool,
    resume_cancellation: CancellationToken,
    running: bool,
    pending: VecDeque<QueuedRequest>,
}

struct QueuedRequest {
    request: Request,
    cancellation: CancellationToken,
    transition: Transition,
    response: oneshot::Sender<Result<(), EngineError>>,
}

impl TransitionQueue {
    pub(super) fn enqueue(
        self: &Arc<Self>,
        request: Request,
        transition: Transition,
    ) -> oneshot::Receiver<Result<(), EngineError>> {
        let (response, completion) = oneshot::channel();
        let mut state = self.state();
        if transition.stop_requested.load(Ordering::Acquire) {
            let _ = response.send(Err(invalid_state_error()));
            return completion;
        }
        if matches!(request, Request::Suspend(_) | Request::Quiesce(_)) {
            state.admission_closed = true;
            state.resume_cancellation.cancel();
            state.resume_cancellation = CancellationToken::new();
        }
        let cancellation = state.resume_cancellation.clone();
        state.pending.push_back(QueuedRequest {
            request,
            cancellation,
            transition,
            response,
        });
        if !state.running {
            state.running = true;
            let owner = Arc::clone(self);
            tokio::spawn(async move { owner.run().await });
        }
        completion
    }

    pub(in super::super) fn accept_shutdown(&self, stop_requested: &AtomicBool) {
        let mut state = self.state();
        stop_requested.store(true, Ordering::Release);
        state.admission_closed = true;
        state.resume_cancellation.cancel();
        for request in state.pending.drain(..) {
            let _ = request.response.send(Err(invalid_state_error()));
        }
    }

    pub(in super::super) async fn wait_empty(&self) {
        loop {
            let drained = self.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if !self.state().running {
                return;
            }
            drained.await;
        }
    }

    pub(in super::super) fn check_admission(&self) -> Result<(), EngineError> {
        if self.state().admission_closed {
            return Err(invalid_state_error());
        }
        Ok(())
    }

    pub(in super::super) fn register_operation(
        &self,
        operations: &InFlightOperations,
        prefix: &str,
    ) -> Result<RegisteredOperation, EngineError> {
        let state = self.state();
        if state.admission_closed {
            return Err(invalid_state_error());
        }
        // 登记与暂停接收不可交错；已接收的操作必须进入同一排空清单。
        Ok(operations.register(prefix))
    }

    pub(super) fn publish_resume(
        &self,
        cancellation: &CancellationToken,
        stop_requested: &AtomicBool,
        state: &mut EngineState,
        events: &EventSender,
    ) -> Result<(), EngineError> {
        let mut requests = self.state();
        if stop_requested.load(Ordering::Acquire) || cancellation.is_cancelled() {
            return Err(invalid_state_error());
        }
        // 发布与接收新暂停共用同一顺序，过期恢复不能重新开放入口。
        *state = EngineState::Running;
        requests.admission_closed = false;
        events.send(EngineEvent::StateChanged {
            state: EngineState::Running,
        });
        Ok(())
    }

    async fn run(self: Arc<Self>) {
        loop {
            let next = {
                let mut state = self.state();
                match state.pending.pop_front() {
                    Some(next) => next,
                    None => {
                        state.running = false;
                        self.drained.notify_waiters();
                        return;
                    }
                }
            };
            let QueuedRequest {
                request,
                cancellation,
                transition,
                response,
            } = next;
            // 每个请求拥有完整执行；调用方离开或一次意外退出不丢弃其余已接收请求。
            let result =
                tokio::spawn(async move { transition.execute(request, cancellation).await })
                    .await
                    .unwrap_or_else(|_| {
                        Err(EngineError::new(1108, EngineErrorCategory::Internal, true))
                    });
            let _ = response.send(result);
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
