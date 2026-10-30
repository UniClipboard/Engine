//! 负责一个执行器的线程和可重复等待的停止结果。
use std::future::Future;
use std::io;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[cfg(feature = "dev-tools")]
pub(crate) static PROBE_START_FAILURE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(feature = "dev-tools")]
static PROBE_UNFENCED_CAUGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub(crate) enum OwnedExecutorStartError {
    #[error("owned executor thread creation failed")]
    Thread(#[source] io::Error),
    #[error("owned runtime creation failed")]
    Runtime(#[source] io::Error),
    #[error("owned executor initialization result was lost")]
    Initialization(#[source] std::sync::mpsc::RecvError),
    #[error("owned executor thread terminated during initialization")]
    WorkerTerminated(#[source] Box<OwnedExecutorStartError>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecutorStop {
    Pending,
    Stopped,
    Failed,
}

enum StopRequest {
    Stop,
    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    Panic,
    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    UnfencedPanic,
}

struct State {
    stop: Option<oneshot::Sender<StopRequest>>,
    stopping: bool,
    reservations: usize,
    result: Option<ExecutorStop>,
}

pub(crate) struct ExecutorReservation {
    state: Arc<(Mutex<State>, Condvar)>,
}

fn signal_stop_if_ready(state: &mut State) {
    if state.stopping && state.reservations == 0 {
        if let Some(stop) = state.stop.take() {
            // 接收方消失时仍须等线程完成，不能提前发布失败。
            let _ = stop.send(StopRequest::Stop);
        }
    }
}

impl Drop for ExecutorReservation {
    fn drop(&mut self) {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.reservations -= 1;
        signal_stop_if_ready(&mut state);
        changed.notify_all();
    }
}

struct CompletionGuard {
    state: Arc<(Mutex<State>, Condvar)>,
    result: ExecutorStop,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        let (lock, changed) = &*self.state;
        lock.lock().unwrap_or_else(PoisonError::into_inner).result = Some(self.result);
        changed.notify_all();
    }
}

pub(crate) struct OwnedExecutor {
    handle: Handle,
    state: Arc<(Mutex<State>, Condvar)>,
}

impl OwnedExecutor {
    pub(crate) fn start() -> Result<Self, OwnedExecutorStartError> {
        #[cfg(feature = "dev-tools")]
        if PROBE_START_FAILURE
            .compare_exchange(
                1,
                0,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            return Err(OwnedExecutorStartError::Thread(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "controlled thread failure",
            )));
        }
        let (ready, initialized) = std::sync::mpsc::sync_channel(1);
        let (stop, stopped) = oneshot::channel();
        let state = Arc::new((
            Mutex::new(State {
                stop: Some(stop),
                stopping: false,
                reservations: 0,
                result: None,
            }),
            Condvar::new(),
        ));
        let worker_state = Arc::clone(&state);
        let worker = std::thread::Builder::new()
            .name("uc-engine-owned-executor".to_owned())
            .spawn(move || {
                let mut completion = CompletionGuard {
                    state: worker_state,
                    result: ExecutorStop::Failed,
                };
                #[cfg(feature = "dev-tools")]
                let injected = PROBE_START_FAILURE
                    .compare_exchange(
                        2,
                        0,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok();
                #[cfg(not(feature = "dev-tools"))]
                let injected = false;
                let runtime = match if injected {
                    Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "controlled runtime failure",
                    ))
                } else {
                    Builder::new_current_thread().enable_all().build()
                } {
                    Ok(runtime) => runtime,
                    Err(source) => {
                        let _ = ready.send(Err(OwnedExecutorStartError::Runtime(source)));
                        return;
                    }
                };
                if ready.send(Ok(runtime.handle().clone())).is_err() {
                    return;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(async {
                        match stopped.await {
                            Ok(StopRequest::Stop) => Ok(()),
                            #[cfg(feature = "dev-tools")]
                            Ok(StopRequest::Panic) => panic!("controlled executor panic"),
                            #[cfg(feature = "dev-tools")]
                            Ok(StopRequest::UnfencedPanic) => {
                                panic!("controlled unexpected executor panic")
                            }
                            Err(error) => Err(error),
                        }
                    })
                }));
                if result.is_err() {
                    let (lock, changed) = &*completion.state;
                    lock.lock().unwrap_or_else(PoisonError::into_inner).stopping = true;
                    changed.notify_all();
                }
                #[cfg(feature = "dev-tools")]
                if result.is_err() {
                    PROBE_UNFENCED_CAUGHT.store(true, std::sync::atomic::Ordering::Release);
                }
                // Tokio 结束时等待此执行器接受的阻塞任务。
                drop(runtime);
                if matches!(result, Ok(Ok(()))) {
                    completion.result = ExecutorStop::Stopped;
                }
            })
            .map_err(OwnedExecutorStartError::Thread)?;
        let handle = match initialized.recv() {
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                return if worker.join().is_ok() {
                    Err(error)
                } else {
                    Err(OwnedExecutorStartError::WorkerTerminated(Box::new(error)))
                };
            }
            Err(source) => {
                let error = OwnedExecutorStartError::Initialization(source);
                return if worker.join().is_ok() {
                    Err(error)
                } else {
                    Err(OwnedExecutorStartError::WorkerTerminated(Box::new(error)))
                };
            }
        };
        Ok(Self { handle, state })
    }

    pub(crate) fn handle(&self) -> &Handle {
        &self.handle
    }

    /// 接收任务与保留其执行责任是同一个动作。
    pub(crate) fn spawn<F>(&self, future: F) -> Result<JoinHandle<F::Output>, F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let Some(reservation) = self.reserve() else {
            return Err(future);
        };
        Ok(self.handle.spawn(async move {
            let _reservation = reservation;
            future.await
        }))
    }

    /// 在执行器停止前接收完整任务，直到任务的守卫结束。
    pub(crate) fn reserve(&self) -> Option<ExecutorReservation> {
        let (lock, _) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if state.stopping || state.result.is_some() {
            return None;
        }
        state.reservations = state.reservations.checked_add(1)?;
        Some(ExecutorReservation {
            state: Arc::clone(&self.state),
        })
    }

    /// 仅供端到端场景模拟执行器线程异常。
    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_panic(&self) {
        let (lock, _) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.stopping = true;
        if let Some(stop) = state.stop.take() {
            let _ = stop.send(StopRequest::Panic);
        }
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_unfenced_panic(&self) {
        PROBE_UNFENCED_CAUGHT.store(false, std::sync::atomic::Ordering::Release);
        let (lock, _) = &*self.state;
        if let Some(stop) = lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stop
            .take()
        {
            let _ = stop.send(StopRequest::UnfencedPanic);
        }
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_unfenced_caught(&self) -> bool {
        PROBE_UNFENCED_CAUGHT.load(std::sync::atomic::Ordering::Acquire)
    }

    /// 只等待已经接受的停止，不触发新的关闭。
    pub(crate) fn wait_if_stopping(&self, deadline: Duration) -> Option<ExecutorStop> {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.stopping {
            return state.result;
        }
        let started = Instant::now();
        loop {
            if let Some(result) = state.result {
                return Some(result);
            }
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                return Some(ExecutorStop::Pending);
            };
            let (next, _) = changed
                .wait_timeout(state, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }

    /// 调用方先关闭业务入口，再请求执行器停止；期限只约束等待。
    pub(crate) fn stop(&self, deadline: Duration) -> ExecutorStop {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.stopping = true;
        signal_stop_if_ready(&mut state);
        let started = Instant::now();
        loop {
            if let Some(result) = state.result {
                return result;
            }
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                return ExecutorStop::Pending;
            };
            let (next, _) = changed
                .wait_timeout(state, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }
}
