use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
            if thread::Builder::new()
                .name("uc-engine-uniffi-reaper".to_owned())
                .spawn(move || {
                    let result = worker
                        .join()
                        .map_err(|_| BindingError::RuntimeUnavailable)
                        .and_then(|result| result);
                    lock(&shared.0).result = Some(result);
                    shared.1.notify_all();
                })
                .is_err()
            {
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
