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
    ready: Notify,
    drained: Notify,
}

#[derive(Default)]
struct State {
    binding: Binding,
    admission_closed: bool,
    owner_dropped: bool,
    resume_cancellation: CancellationToken,
    active_request: bool,
    pending: VecDeque<QueuedItem>,
}

#[derive(Default)]
enum Binding {
    #[default]
    Waiting,
    Ready(Arc<Transition>),
    Failed(EngineError),
    Stopped,
}

struct QueuedRequest {
    request: Request,
    cancellation: CancellationToken,
    response: oneshot::Sender<Result<(), EngineError>>,
}

enum QueuedItem {
    Request(QueuedRequest),
    StartupHandoff(oneshot::Sender<()>),
}

impl TransitionQueue {
    pub(super) fn enqueue(
        self: &Arc<Self>,
        request: Request,
    ) -> oneshot::Receiver<Result<(), EngineError>> {
        let (response, completion) = oneshot::channel();
        let mut state = self.state();
        if state.owner_dropped {
            let _ = response.send(Err(invalid_state_error()));
            return completion;
        }
        match &state.binding {
            Binding::Stopped => {
                let _ = response.send(Err(invalid_state_error()));
                return completion;
            }
            Binding::Failed(error) => {
                let _ = response.send(Err(error.clone()));
                return completion;
            }
            Binding::Ready(transition) if transition.stop_requested.load(Ordering::Acquire) => {
                let _ = response.send(Err(invalid_state_error()));
                return completion;
            }
            _ => {}
        }
        if matches!(request, Request::Suspend(_) | Request::Quiesce(_)) {
            state.admission_closed = true;
            state.resume_cancellation.cancel();
            state.resume_cancellation = CancellationToken::new();
        }
        let cancellation = state.resume_cancellation.clone();
        state.pending.push_back(QueuedItem::Request(QueuedRequest {
            request,
            cancellation,
            response,
        }));
        drop(state);
        self.ready.notify_one();
        completion
    }

    pub(super) fn bind(
        self: &Arc<Self>,
        transition: Transition,
        announce_start: bool,
    ) -> oneshot::Receiver<()> {
        let (handoff, completion) = oneshot::channel();
        let mut state = self.state();
        if announce_start && !state.admission_closed {
            transition.events.send(EngineEvent::StateChanged {
                state: EngineState::Running,
            });
        }
        // 输入只交给一次启动；绑定后原队列继续处理交接后的宿主通知。
        state.binding = Binding::Ready(Arc::new(transition));
        state.pending.push_back(QueuedItem::StartupHandoff(handoff));
        let owner = Arc::clone(self);
        tokio::spawn(async move { owner.run().await });
        completion
    }

    pub(in super::super) fn fail_startup(&self, error: EngineError) {
        let mut state = self.state();
        if !matches!(state.binding, Binding::Waiting) {
            return;
        }
        state.binding = Binding::Failed(error.clone());
        state.admission_closed = true;
        state.resume_cancellation.cancel();
        for item in state.pending.drain(..) {
            if let QueuedItem::Request(request) = item {
                let _ = request.response.send(Err(error.clone()));
            }
        }
        drop(state);
        self.ready.notify_one();
    }

    pub(in super::super) fn accept_shutdown(&self, stop_requested: &AtomicBool) {
        let mut state = self.state();
        stop_requested.store(true, Ordering::Release);
        state.binding = Binding::Stopped;
        state.admission_closed = true;
        state.resume_cancellation.cancel();
        for item in state.pending.drain(..) {
            if let QueuedItem::Request(request) = item {
                let _ = request.response.send(Err(invalid_state_error()));
            }
        }
        drop(state);
        self.ready.notify_one();
    }

    pub(in super::super) fn abandon_owner(&self) {
        let mut state = self.state();
        state.owner_dropped = true;
        state.admission_closed = true;
        drop(state);
        self.ready.notify_one();
    }

    pub(in super::super) async fn wait_empty(&self) {
        loop {
            let drained = self.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            let is_empty = {
                let state = self.state();
                let has_pending_request = state
                    .pending
                    .iter()
                    .any(|item| matches!(item, QueuedItem::Request(_)));
                !state.active_request && !has_pending_request
            };
            if is_empty {
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
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            let next = {
                let mut state = self.state();
                let Binding::Ready(transition) = &state.binding else {
                    self.drained.notify_waiters();
                    return;
                };
                let transition = Arc::clone(transition);
                match state.pending.pop_front() {
                    Some(next) => {
                        state.active_request = matches!(next, QueuedItem::Request(_));
                        Some((next, transition))
                    }
                    None if state.owner_dropped => {
                        state.binding = Binding::Stopped;
                        self.drained.notify_waiters();
                        return;
                    }
                    None => None,
                }
            };
            let Some((next, transition)) = next else {
                ready.await;
                continue;
            };
            let (request, cancellation, response) = match next {
                QueuedItem::Request(request) => {
                    (request.request, request.cancellation, request.response)
                }
                QueuedItem::StartupHandoff(completion) => {
                    let _ = completion.send(());
                    continue;
                }
            };
            // 每个请求拥有完整执行；调用方离开或一次意外退出不丢弃其余已接收请求。
            let requests = Arc::clone(&self);
            let result =
                tokio::spawn(
                    async move { transition.execute(request, cancellation, &requests).await },
                )
                .await
                .unwrap_or_else(|_| {
                    Err(EngineError::new(1108, EngineErrorCategory::Internal, true))
                });
            let _ = response.send(result);
            self.state().active_request = false;
            self.drained.notify_waiters();
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
