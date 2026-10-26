use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::shutdown_wait::ShutdownReply;
use super::{lock, LifecycleCommand, MobileEngine, ShutdownWait};
use crate::{BindingError, BindingErrorCategory, BindingResourceRelease};

impl MobileEngine {
    pub(super) fn shutdown_inner(&self, budget: Duration, join: bool) -> BindingResourceRelease {
        let Some(deadline) = Instant::now().checked_add(budget) else {
            return BindingResourceRelease::Failed {
                error: BindingError::Engine {
                    code: 1003,
                    category: BindingErrorCategory::InvalidInput,
                    retryable: false,
                },
            };
        };
        lock(&self.commands).take();
        if !lock(&self.events.state).closed {
            match self.request_shutdown(deadline) {
                Ok(BindingResourceRelease::Pending) => return BindingResourceRelease::Pending,
                Ok(BindingResourceRelease::Failed { error }) | Err(error) => {
                    self.shutdown_pending.store(false, Ordering::Release);
                    // 接收端缺失不能证明清理已经执行。
                    if !lock(&self.events.state).closed {
                        return BindingResourceRelease::Failed { error };
                    }
                }
                Ok(BindingResourceRelease::Released { .. }) => {}
            }
        }
        lock(&self.lifecycle_commands).take();
        if join {
            return self
                .worker
                .wait_outcome(deadline.saturating_duration_since(Instant::now()));
        }
        BindingResourceRelease::Pending
    }

    fn request_shutdown(&self, deadline: Instant) -> Result<BindingResourceRelease, BindingError> {
        let waiting = {
            let mut pending = lock(&self.shutdown_wait);
            if let Some(waiting) = pending.as_ref() {
                waiting.clone()
            } else {
                let commands = lock(&self.lifecycle_commands)
                    .clone()
                    .ok_or(BindingError::AlreadyStopped)?;
                let waiting = Arc::new(ShutdownWait::default());
                commands
                    .send(LifecycleCommand::Shutdown {
                        deadline,
                        response: ShutdownReply(waiting.clone()),
                    })
                    // discarded-source[channel]: the receiver is unavailable; no release is inferred
                    .map_err(|_| BindingError::RuntimeUnavailable)?;
                self.shutdown_pending.store(true, Ordering::Release);
                *pending = Some(waiting.clone());
                waiting
            }
        };
        let outcome = waiting.wait(deadline);
        if matches!(outcome, BindingResourceRelease::Failed { .. }) {
            let mut pending = lock(&self.shutdown_wait);
            // 旧等待方不能移除较新的显式重试。
            if pending
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &waiting))
            {
                pending.take();
            }
        }
        Ok(outcome)
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
    use super::{lock, Duration, Instant, LifecycleCommand, MobileEngine};
    use crate::runtime::worker_join::WorkerJoin;
    use crate::runtime::EventQueue;
    use crate::{BindingError, BindingErrorCategory};
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc, Mutex};

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
            shutdown_pending: AtomicBool::new(false),
            shutdown_wait: Mutex::new(None),
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
                let Some(LifecycleCommand::Shutdown { response, .. }) =
                    lifecycle_requests.blocking_recv()
                else {
                    panic!("关闭重试通道提前关闭")
                };
                if attempt == 0 {
                    response
                        .send(
                            Err(BindingError::Engine {
                                code: 1108,
                                category: BindingErrorCategory::Internal,
                                retryable: true,
                            })
                            .into(),
                        )
                        .unwrap();
                } else {
                    worker_events.close();
                    response.send(Ok(()).into()).unwrap();
                }
            }
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            shutdown_wait: Mutex::new(None),
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
            let Some(LifecycleCommand::Shutdown { deadline, response }) =
                lifecycle_requests.blocking_recv()
            else {
                panic!("缺少关闭请求")
            };
            assert_eq!(
                deadline.saturating_duration_since(Instant::now()),
                Duration::ZERO
            );
            worker_events.close();
            let _ = response.send(Ok(()).into());
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            shutdown_wait: Mutex::new(None),
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

    #[test]
    fn unavailable_shutdown_channel_can_be_retried() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        drop(requests);
        drop(lifecycle_requests);
        let worker = std::thread::spawn(|| Ok(()));
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            shutdown_wait: Mutex::new(None),
            events: Arc::new(EventQueue::new(1)),
            worker: WorkerJoin::new(worker),
        };

        assert_eq!(engine.shutdown(1000), Err(BindingError::RuntimeUnavailable));
        assert!(!engine
            .shutdown_pending
            .load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn a_failed_drop_shutdown_and_a_leaked_worker_are_recorded() {
        let recorder = crate::runtime::event_recorder::EventRecorder::default();
        let dispatch = tracing::Dispatch::new(recorder.clone());
        let _guard = tracing::dispatcher::set_default(&dispatch);
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        drop(requests);
        drop(lifecycle_requests);
        let (release, released) = mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = released.recv();
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            shutdown_wait: Mutex::new(None),
            events: Arc::new(EventQueue::new(1)),
            worker: WorkerJoin::new(worker),
        };

        drop(engine);

        let lines = recorder.lines().join("\n");
        assert_eq!(
            lines.matches("mobile operation failed").count(),
            2,
            "{lines}"
        );
        assert!(lines.contains("drop_shutdown_failed"), "{lines}");
        assert!(lines.contains("drop_worker_leaked"), "{lines}");
        release.send(()).unwrap();
    }

    #[test]
    fn disconnected_reply_after_event_close_joins_the_completed_worker() {
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let (lifecycle_commands, mut lifecycle_requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(1));
        let worker_events = Arc::clone(&events);
        let worker = std::thread::spawn(move || {
            drop(requests);
            let _ = lifecycle_requests.blocking_recv();
            worker_events.close();
            Ok(())
        });
        let engine = MobileEngine {
            commands: Mutex::new(Some(commands)),
            lifecycle_commands: Mutex::new(Some(lifecycle_commands)),
            shutdown_pending: AtomicBool::new(false),
            shutdown_wait: Mutex::new(None),
            events,
            worker: WorkerJoin::new(worker),
        };

        engine.shutdown(1000).unwrap();
    }
}
