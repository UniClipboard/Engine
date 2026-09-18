use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::shutdown::wait_timeout;
use super::{LifecycleCommand, MobileEngine};
use crate::{BindingError, BindingErrorCategory};

impl MobileEngine {
    pub(super) fn suspend_inner(&self, budget: Duration) -> Result<(), BindingError> {
        let deadline = Instant::now()
            .checked_add(budget)
            .ok_or(BindingError::Engine {
                code: 1003,
                category: BindingErrorCategory::InvalidInput,
                retryable: false,
            })?;
        let commands = self.lifecycle_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(LifecycleCommand::Suspend { deadline, response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => wait_timeout(),
                RecvTimeoutError::Disconnected => BindingError::RuntimeUnavailable,
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::{
        mpsc, BindingError, BindingErrorCategory, Duration, Instant, LifecycleCommand, MobileEngine,
    };
    use crate::runtime::worker_join::WorkerJoin;
    use crate::runtime::EventQueue;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[test]
    fn zero_budget_keeps_the_enqueued_deadline_and_reports_timeout() {
        let (commands, _) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let (captured, deadline) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let Some(LifecycleCommand::Suspend { deadline, response }) = requests.blocking_recv()
            else {
                panic!("expected suspend");
            };
            captured.send(deadline).unwrap();
            wait.recv().unwrap();
            let _ = response.send(Ok(()));
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            events: Arc::new(EventQueue::new(1)),
            worker: WorkerJoin::new(worker),
        };
        assert!(matches!(
            engine.suspend_with_deadline(0),
            Err(BindingError::Engine {
                category: BindingErrorCategory::DeadlineExceeded,
                ..
            })
        ));
        assert!(deadline.recv_timeout(Duration::from_secs(1)).unwrap() <= Instant::now());
        release.send(()).unwrap();
        engine.join_worker(Duration::from_secs(1)).unwrap();
    }
}
