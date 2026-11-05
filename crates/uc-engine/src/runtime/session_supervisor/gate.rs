use super::*;

impl SessionOperationGate {
    pub(super) fn new_open() -> Self {
        Self {
            inner: Arc::new(SessionOperationGateInner {
                state: StdMutex::new(SessionOperationGateState {
                    open: true,
                    active: 0,
                    cancellation: CancellationToken::new(),
                }),
                changed: Notify::new(),
            }),
        }
    }

    pub(super) fn acquire(&self) -> Result<SessionOperationLease, EngineError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.open {
            return Err(super::operation_unavailable_error());
        }
        state.active = state.active.saturating_add(1);
        Ok(SessionOperationLease {
            gate: Arc::clone(&self.inner),
            cancellation: state.cancellation.clone(),
        })
    }

    pub(super) fn reopen(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.open = true;
        state.cancellation = CancellationToken::new();
        self.inner.changed.notify_waiters();
    }

    pub(super) async fn close_and_wait(
        &self,
        current_operation: Option<SessionOperationLease>,
        deadline: Option<Instant>,
    ) -> Result<(), EngineError> {
        observe_local_result(LocalWorkStep::SessionDrainOperations, async {
            if current_operation
                .as_ref()
                .is_some_and(|lease| !Arc::ptr_eq(&lease.gate, &self.inner))
            {
                return Err(super::operation_unavailable_error());
            }
            let cancellation = {
                let mut state = self
                    .inner
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                state.open = false;
                state.cancellation.clone()
            };
            drop(current_operation);
            let grace = Instant::now() + SESSION_OPERATION_GRACE;
            let drained = observe_local_result(
                LocalWorkStep::SessionDrainGrace,
                timeout_at(
                    deadline.map_or(grace, |end| end.min(grace)),
                    self.wait_for_drain(),
                ),
            )
            .await;
            if drained.is_err() {
                cancellation.cancel();
                if observe_local_result(
                    LocalWorkStep::SessionDrainCancellation,
                    timeout_at(
                        deadline.unwrap_or_else(|| Instant::now() + SESSION_OPERATION_GRACE),
                        self.wait_for_drain(),
                    ),
                )
                .await
                .is_err()
                {
                    uc_warn!(
                        error_kind = "session_operation_drain_timeout",
                        "session operation did not stop after cancellation"
                    );
                    return Err(EngineError::new(
                        1106,
                        EngineErrorCategory::DeadlineExceeded,
                        true,
                    ));
                }
            }
            Ok(())
        })
        .await
    }

    pub(super) async fn wait_for_drain(&self) {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let active = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .active;
            if active == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl SessionOperationLease {
    pub(in super::super) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Drop for SessionOperationLease {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = state.active.saturating_sub(1);
        drop(state);
        self.gate.changed.notify_waiters();
    }
}
