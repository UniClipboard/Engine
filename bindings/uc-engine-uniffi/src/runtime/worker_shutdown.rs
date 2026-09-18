use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinHandle;
use uc_engine::EngineError;

use super::EventQueue;
use crate::BindingError;

pub(super) async fn finish_shutdown(
    shutdown: impl Future<Output = Result<(), EngineError>>,
    forwarder: JoinHandle<()>,
    events: Arc<EventQueue>,
) -> Result<(), BindingError> {
    let result = shutdown.await.map_err(BindingError::from);
    // 只在实际关闭已返回失败后结束纯事件转发，不能等待永远不会出现的成功通知。
    if result.is_err() {
        forwarder.abort();
    }
    let forwarded = forwarder.await;
    events.close();
    result?;
    forwarded.map_err(|_| BindingError::RuntimeUnavailable)
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
        assert_eq!(result, Err(BindingError::from(expected)));
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
            .unwrap();
        assert!(lock(&events.state).closed);
        assert!(matches!(
            events.next(Duration::ZERO),
            Some(BindingEvent::RefreshRequired { .. })
        ));
    }
}
