//! 清空先停止全部出生动作，再等待实际结束；不改变外部会话的打开状态。
use super::{Completion, ReceiveRegistrationError, ReceiveWorkError, ReceiveWorkRegistry};
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};
use tokio::sync::{watch, OwnedMutexGuard};

pub(crate) struct ReceiveClearScope {
    registry: Arc<ReceiveWorkRegistry>,
    waiting: Vec<(watch::Receiver<Completion>, bool)>,
    complete: bool,
    _maintenance: Option<OwnedMutexGuard<()>>,
}

impl ReceiveWorkRegistry {
    pub(crate) async fn clear_scope(
        self: &Arc<Self>,
    ) -> Result<ReceiveClearScope, ReceiveRegistrationError> {
        let mut scope = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.clearing {
                return Err(ReceiveRegistrationError::CleanupInProgress);
            }
            state.clearing = true;
            let waiting = state
                .born
                .iter()
                .filter_map(|birth| birth.upgrade())
                .map(|birth| {
                    birth.cancel.cancel();
                    (
                        birth.completion.subscribe(),
                        birth.registered.load(Ordering::Acquire),
                    )
                })
                .collect();
            ReceiveClearScope {
                registry: self.clone(),
                waiting,
                complete: false,
                _maintenance: None,
            }
        };
        scope._maintenance = Some(self.maintenance.clone().lock_owned().await);
        Ok(scope)
    }

    /// 恢复与清空互斥；清空已接受时，不把临时封口误判为需要启动恢复。
    pub(in super::super) async fn recovery_lease(self: &Arc<Self>) -> OwnedMutexGuard<()> {
        loop {
            let changed = self.changed.notified();
            let lease = self.maintenance.clone().lock_owned().await;
            if !self
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clearing
            {
                return lease;
            }
            drop(lease);
            changed.await;
        }
    }
}

impl ReceiveClearScope {
    pub(crate) async fn stop(&mut self) -> Result<(), ReceiveClearFailure> {
        let mut failures = Vec::new();
        for (mut result, registered) in self.waiting.drain(..) {
            loop {
                if let Some(completion) = result.borrow_and_update().clone() {
                    if let Err(source) = completion {
                        if !matches!(source.as_ref(), ReceiveWorkError::Stopped) {
                            failures.push(source);
                        }
                    }
                    break;
                }
                if let Err(source) = result.changed().await {
                    if registered {
                        failures.push(Arc::new(ReceiveWorkError::Completion(source)));
                    }
                    // discarded-source[channel]: 未登记动作的最后持有者已退出，没有已接受的接收需要结算。
                    break;
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ReceiveClearFailure(failures))
        }
    }

    pub(crate) fn complete(&mut self) {
        self.complete = true;
    }
}

impl Drop for ReceiveClearScope {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.clearing = false;
        state.clear_failed = !self.complete;
        if self.complete {
            state.receipts.clear();
        }
        self.registry.changed.notify_waiters();
    }
}

#[derive(Debug)]
pub(crate) struct ReceiveClearFailure(pub(crate) Vec<Arc<ReceiveWorkError>>);
impl Display for ReceiveClearFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("receive actions did not settle before history clear")
    }
}
impl Error for ReceiveClearFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0
            .first()
            .map(|error| error.as_ref() as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ReceiveWorkError, ReceiveWorkRegistry};
    use std::io;
    use std::sync::Arc;
    use tokio::time::{timeout, Duration};
    use uc_core::ids::EntryId;

    #[tokio::test]
    async fn clear_stops_all_births_before_waiting_and_never_revives_old_or_mid_clear_actions() {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let a = registry.action();
        let b = registry.action();
        let preparing = registry.action();
        a.register(&EntryId::from("a"), "a").unwrap();
        b.register(&EntryId::from("b"), "b").unwrap();
        let mut scope = registry.clear_scope().await.unwrap();
        assert!(a.cancellation().is_cancelled() && b.cancellation().is_cancelled());
        assert!(preparing.cancellation().is_cancelled());
        let during = registry.action();
        assert!(during.cancellation().is_cancelled());
        assert!(!registry.is_open());
        assert!(registry.claim_cleanup("other").is_none());
        let mut waiting = Box::pin(scope.stop());
        a.finish(Ok(()));
        drop(preparing);
        assert!(timeout(Duration::from_millis(10), waiting.as_mut())
            .await
            .is_err());
        b.finish(Ok(()));
        waiting.await.unwrap();
        scope.complete();
        drop(scope);
        assert!(registry.is_open());
        assert!(a.register(&EntryId::from("late-a"), "late").is_err());
        assert!(during.register(&EntryId::from("late-b"), "late").is_err());
        let next = registry.action();
        next.register(&EntryId::from("new"), "new").unwrap();
        next.finish(Ok(()));
    }

    #[tokio::test]
    async fn clear_completion_does_not_open_an_externally_closed_session() {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let mut scope = registry.clear_scope().await.unwrap();
        scope.stop().await.unwrap();
        registry.close();
        scope.complete();
        drop(scope);
        assert!(!registry.is_open());
    }

    #[tokio::test]
    async fn clear_collects_all_failure_sources_and_keeps_admission_closed_until_retry() {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let a = registry.action();
        let b = registry.action();
        a.register(&EntryId::from("a"), "a").unwrap();
        b.register(&EntryId::from("b"), "b").unwrap();
        let mut scope = registry.clear_scope().await.unwrap();
        let mut waiting = Box::pin(scope.stop());
        a.finish(Err(ReceiveWorkError::Action(
            io::Error::from(io::ErrorKind::PermissionDenied).into(),
        )));
        assert!(timeout(Duration::from_millis(10), waiting.as_mut())
            .await
            .is_err());
        b.finish(Err(ReceiveWorkError::Action(
            io::Error::from(io::ErrorKind::StorageFull).into(),
        )));
        let failure = waiting.await.unwrap_err();
        assert_eq!(failure.0.len(), 2);
        let kinds = failure
            .0
            .iter()
            .map(|source| {
                let ReceiveWorkError::Action(source) = source.as_ref() else {
                    panic!("original action source")
                };
                source.downcast_ref::<io::Error>().unwrap().kind()
            })
            .collect::<Vec<_>>();
        assert!(kinds.contains(&io::ErrorKind::PermissionDenied));
        assert!(kinds.contains(&io::ErrorKind::StorageFull));
        drop(scope);
        assert!(!registry.is_open());
        assert!(registry
            .action()
            .register(&EntryId::from("blocked"), "blocked")
            .is_err());
        drop(a);
        drop(b);
        let mut retry = registry.clear_scope().await.unwrap();
        retry.stop().await.unwrap();
        retry.complete();
        drop(retry);
        assert!(registry.is_open());
    }

    #[tokio::test]
    async fn lost_registered_completion_cannot_be_mistaken_for_an_unregistered_prepare_exit() {
        let registry = Arc::new(ReceiveWorkRegistry::default());
        let action = registry.action();
        action.register(&EntryId::from("entry"), "attempt").unwrap();
        let mut scope = registry.clear_scope().await.unwrap();
        drop(action);
        let failure = scope.stop().await.unwrap_err();
        assert!(matches!(
            failure.0[0].as_ref(),
            ReceiveWorkError::Completion(_)
        ));
        drop(scope);
        assert!(!registry.is_open());
    }
}
