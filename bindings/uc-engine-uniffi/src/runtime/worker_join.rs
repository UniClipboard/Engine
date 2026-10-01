use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use uc_engine::observability::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_engine::observability::uc_warn;

use super::{lock, shutdown::wait_timeout};
use crate::BindingError;

struct JoinState {
    worker: Option<JoinHandle<Result<(), BindingError>>>,
    result: Option<Result<(), BindingError>>,
}

pub(super) struct WorkerJoin {
    state: Arc<(Mutex<JoinState>, Condvar)>,
}

impl WorkerJoin {
    pub(super) fn new(worker: JoinHandle<Result<(), BindingError>>) -> Self {
        Self {
            state: Arc::new((
                Mutex::new(JoinState {
                    worker: Some(worker),
                    result: None,
                }),
                Condvar::new(),
            )),
        }
    }

    pub(super) fn wait(&self, budget: Duration) -> Result<(), BindingError> {
        let deadline = Instant::now()
            .checked_add(budget)
            .ok_or_else(wait_timeout)?;
        let mut state = lock(&self.state.0);
        if let Some(worker) = state.worker.take() {
            let shared = Arc::clone(&self.state);
            // reaper 是新线程，沿用等待方当前生效的订阅者，异常退出记录才会走同一条输出。
            let dispatch = tracing::dispatcher::get_default(Clone::clone);
            if thread::Builder::new()
                .name("uc-engine-uniffi-reaper".to_owned())
                .spawn(move || {
                    let result = worker
                        .join()
                        // discarded-source[no-information]: the error value carries no usable diagnostic information
                        .map_err(|_| {
                            tracing::dispatcher::with_default(&dispatch, || {
                                record_task_join_failure(DiagnosticTaskKind::MobileWorker);
                            });
                            BindingError::RuntimeUnavailable
                        })
                        .and_then(|result| result);
                    lock(&shared.0).result = Some(result);
                    shared.1.notify_all();
                })
                .is_err()
            {
                uc_warn!(
                    error_kind = "reaper_spawn_failed",
                    "engine worker reaper thread could not start"
                );
                state.result = Some(Err(BindingError::RuntimeUnavailable));
            }
        }
        loop {
            if let Some(result) = &state.result {
                return result.clone();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(wait_timeout());
            }
            let (next, _) = self
                .state
                .1
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{thread, Duration, WorkerJoin};
    use crate::BindingError;
    use std::sync::mpsc;

    #[test]
    fn repeated_wait_does_not_treat_an_unfinished_join_as_success() {
        let (release, wait) = mpsc::channel();
        let owner = WorkerJoin::new(thread::spawn(move || {
            wait.recv().unwrap();
            Ok(())
        }));
        assert!(owner.wait(Duration::from_millis(10)).is_err());
        assert!(owner.wait(Duration::from_millis(10)).is_err());
        release.send(()).unwrap();
        owner.wait(Duration::from_secs(1)).unwrap();
        owner.wait(Duration::ZERO).unwrap();
    }

    #[test]
    fn repeated_wait_preserves_a_returned_shutdown_failure() {
        let error = BindingError::Engine {
            code: 1108,
            category: crate::BindingErrorCategory::Internal,
            retryable: true,
        };
        let failure = error.clone();
        let owner = WorkerJoin::new(thread::spawn(move || Err(failure)));
        assert_eq!(owner.wait(Duration::from_secs(1)), Err(error.clone()));
        assert_eq!(owner.wait(Duration::ZERO), Err(error));
    }

    #[test]
    fn worker_panic_is_recorded_as_a_mobile_worker_join_failure() {
        let recorder = crate::runtime::event_recorder::EventRecorder::default();
        let dispatch = tracing::Dispatch::new(recorder.clone());
        tracing::dispatcher::with_default(&dispatch, || {
            let owner = WorkerJoin::new(thread::spawn(|| panic!("PRIVATE_WORKER_FAILURE")));
            assert_eq!(
                owner.wait(Duration::from_secs(1)),
                Err(BindingError::RuntimeUnavailable)
            );
        });
        let lines = recorder.lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("task.kind=mobile_worker"), "{lines:?}");
        assert!(!lines[0].contains("PRIVATE"));
    }

    #[test]
    fn repeated_wait_preserves_the_completed_worker_failure() {
        let owner = WorkerJoin::new(thread::spawn(|| panic!("test worker failure")));
        assert_eq!(
            owner.wait(Duration::from_secs(1)),
            Err(BindingError::RuntimeUnavailable)
        );
        assert_eq!(
            owner.wait(Duration::ZERO),
            Err(BindingError::RuntimeUnavailable)
        );
    }
}
