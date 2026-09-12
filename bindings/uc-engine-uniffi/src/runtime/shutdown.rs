use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use super::{lock, MobileEngine, WorkerCommand};
use crate::{BindingError, BindingErrorCategory};

impl MobileEngine {
    pub(super) fn shutdown_inner(&self, budget: Duration, join: bool) -> Result<(), BindingError> {
        let deadline = Instant::now()
            .checked_add(budget)
            .ok_or(BindingError::Engine {
                code: 1003,
                category: BindingErrorCategory::InvalidInput,
                retryable: false,
            })?;
        // 停止普通请求，但失败或等待超时后仍保留生命周期通道供继续收尾。
        lock(&self.commands).take();
        if !lock(&self.events.state).closed {
            if let Err(error) = self.request_shutdown(deadline) {
                if !lock(&self.events.state).closed {
                    return Err(error);
                }
            }
        }
        lock(&self.lifecycle_commands).take();
        if join {
            self.join_worker(deadline.saturating_duration_since(Instant::now()))?;
        }
        Ok(())
    }

    fn request_shutdown(&self, deadline: Instant) -> Result<(), BindingError> {
        let commands = self.lifecycle_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::Shutdown { deadline, response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => wait_timeout(),
                RecvTimeoutError::Disconnected => BindingError::RuntimeUnavailable,
            })?
    }

    pub(super) fn join_worker(&self, deadline: Duration) -> Result<(), BindingError> {
        self.worker.wait(deadline)
    }
}

pub(super) fn wait_timeout() -> BindingError {
    BindingError::Engine {
        code: 1002,
        category: BindingErrorCategory::DeadlineExceeded,
        retryable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{lock, mpsc, Duration, Instant, MobileEngine, WorkerCommand};
    use crate::runtime::worker_join::WorkerJoin;
    use crate::runtime::EventQueue;
    use crate::{BindingError, BindingErrorCategory};
    use std::sync::{Arc, Mutex};

    #[test]
    fn closed_event_queue_does_not_turn_a_failed_worker_into_success() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(2));
        let worker_events = Arc::clone(&events);
        let error = BindingError::Engine {
            code: 1108,
            category: BindingErrorCategory::Internal,
            retryable: true,
        };
        let failure = error.clone();
        let (ready, closed) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            drop(requests);
            drop(lifecycle_requests);
            worker_events.close();
            ready.send(()).unwrap();
            Err(failure)
        });
        closed.recv_timeout(Duration::from_secs(1)).unwrap();
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            events,
            worker: WorkerJoin::new(worker),
        };
        assert_eq!(engine.shutdown(1000), Err(error.clone()));
        assert_eq!(engine.shutdown(1000), Err(error));
    }

    #[test]
    fn failed_shutdown_keeps_worker_and_lifecycle_channel_for_retry() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(2));
        let worker_events = Arc::clone(&events);
        let worker = std::thread::spawn(move || {
            drop(requests);
            for attempt in 0..2 {
                let Some(WorkerCommand::Shutdown { response, .. }) =
                    lifecycle_requests.blocking_recv()
                else {
                    panic!("关闭重试通道提前关闭")
                };
                if attempt == 0 {
                    response
                        .send(Err(BindingError::Engine {
                            code: 1108,
                            category: BindingErrorCategory::Internal,
                            retryable: true,
                        }))
                        .unwrap();
                } else {
                    worker_events.close();
                    response.send(Ok(())).unwrap();
                }
            }
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            events,
            worker: WorkerJoin::new(worker),
        };
        assert!(engine.shutdown(1000).is_err());
        assert!(lock(&engine.commands).is_none());
        assert!(lock(&engine.lifecycle_commands).is_some());
        engine.shutdown(1000).unwrap();
        engine.shutdown(1000).unwrap();
    }

    #[test]
    fn queued_shutdown_keeps_original_deadline_and_can_confirm_late_completion() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(2));
        let worker_events = Arc::clone(&events);
        let (release, released) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            drop(requests);
            released.recv().unwrap();
            let Some(WorkerCommand::Shutdown { deadline, response }) =
                lifecycle_requests.blocking_recv()
            else {
                panic!("缺少关闭请求")
            };
            assert_eq!(
                deadline.saturating_duration_since(Instant::now()),
                Duration::ZERO
            );
            worker_events.close();
            let _ = response.send(Ok(()));
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            events,
            worker: WorkerJoin::new(worker),
        };
        assert!(matches!(
            engine.shutdown(10),
            Err(BindingError::Engine {
                category: BindingErrorCategory::DeadlineExceeded,
                ..
            })
        ));
        release.send(()).unwrap();
        engine.shutdown(1000).unwrap();
    }
}
