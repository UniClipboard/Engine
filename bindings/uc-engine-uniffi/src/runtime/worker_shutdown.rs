use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinHandle;
use uc_engine::observability::uc_warn;
#[cfg(test)]
use uc_engine::EngineError;
use uc_engine::ResourceReleaseOutcome;

use super::EventQueue;
use crate::{BindingError, BindingResourceRelease};

pub(super) async fn finish_shutdown<T>(
    shutdown: impl Future<Output = T>,
    forwarder: JoinHandle<()>,
    events: Arc<EventQueue>,
) -> BindingResourceRelease
where
    T: Into<ResourceReleaseOutcome>,
{
    let mut result = BindingResourceRelease::from(shutdown.await.into());
    if !matches!(result, BindingResourceRelease::Released { .. }) {
        forwarder.abort();
    }
    let forwarded = forwarder.await;
    events.close();
    if forwarded.is_err() && matches!(result, BindingResourceRelease::Released { .. }) {
        uc_warn!(
            operation = "engine_shutdown",
            error_kind = "event_forwarder_join_failed",
            "mobile operation failed"
        );
        if let BindingResourceRelease::Released { error } = &mut result {
            error.get_or_insert(BindingError::RuntimeUnavailable);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use tokio::sync::Notify;
    use tokio::time::timeout;
    use uc_engine::EngineErrorCategory;

    use super::*;
    use crate::runtime::lock;
    use crate::{BindingEvent, BindingRefreshReason};

    struct ForwarderResource(Arc<AtomicBool>);

    impl Drop for ForwarderResource {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn failed_shutdown_stops_forwarding_only_after_the_real_result_arrives() {
        let events = Arc::new(EventQueue::new(2));
        let released = Arc::new(AtomicBool::new(false));
        let resource = ForwarderResource(Arc::clone(&released));
        let forwarder = tokio::spawn(async move {
            let _resource = resource;
            std::future::pending::<()>().await;
        });
        let finish = Arc::new(Notify::new());
        let release = Arc::clone(&finish);
        let queue = Arc::clone(&events);
        let expected = EngineError::new(1108, EngineErrorCategory::Internal, true);
        let failure = expected.clone();
        let worker = tokio::spawn(async move {
            finish_shutdown(
                async move {
                    release.notified().await;
                    Err(failure)
                },
                forwarder,
                queue,
            )
            .await
        });
        tokio::task::yield_now().await;
        assert!(!released.load(Ordering::SeqCst));
        assert!(!lock(&events.state).closed);
        finish.notify_one();
        let result = timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.into_result(), Err(BindingError::from(expected)));
        assert!(released.load(Ordering::SeqCst));
        assert!(lock(&events.state).closed);
    }

    #[tokio::test]
    async fn successful_shutdown_waits_for_the_last_event_before_closing_the_queue() {
        let events = Arc::new(EventQueue::new(2));
        let finish = Arc::new(Notify::new());
        let release = Arc::clone(&finish);
        let queue = Arc::clone(&events);
        let forwarder = tokio::spawn(async move {
            release.notified().await;
            queue.push(BindingEvent::RefreshRequired {
                reason: BindingRefreshReason::ConsumerLagged,
            });
        });
        let worker = tokio::spawn(finish_shutdown(
            async { Ok(()) },
            forwarder,
            Arc::clone(&events),
        ));
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        assert!(!lock(&events.state).closed);
        finish.notify_one();
        timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap()
            .into_result()
            .unwrap();
        assert!(lock(&events.state).closed);
        assert!(matches!(
            events.next(Duration::ZERO),
            Some(BindingEvent::RefreshRequired { .. })
        ));
    }

    #[tokio::test]
    async fn a_panicked_forwarder_after_successful_shutdown_is_recorded() {
        let recorder = crate::runtime::event_recorder::EventRecorder::default();
        let dispatch = tracing::Dispatch::new(recorder.clone());
        let _guard = tracing::dispatcher::set_default(&dispatch);
        let events = Arc::new(EventQueue::new(1));
        let forwarder = tokio::spawn(async { panic!("forwarder boom") });

        let result = finish_shutdown(async { Ok(()) }, forwarder, events).await;

        assert_eq!(result.into_result(), Err(BindingError::RuntimeUnavailable));
        let lines = recorder.lines().join("\n");
        assert!(
            lines.contains("error_kind=event_forwarder_join_failed"),
            "{lines}"
        );
        assert!(!lines.contains("boom"), "{lines}");
    }
}
