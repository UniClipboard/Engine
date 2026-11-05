use super::*;

pub(crate) struct RecoveryBootstrap {
    /// 口令恢复或恢复出厂只能各自消费一次启动输入；消费后只能重启。
    pub(crate) input_available: Mutex<bool>,
    pub(crate) gate: Mutex<()>,
    pub(crate) summary: StdMutex<ProfileRecoverySummary>,
    pub(crate) progress: Arc<StartupProgressStore>,
    pub(crate) recovered_runtime: StdMutex<Option<Arc<ProductionRuntime>>>,
    pub(crate) construction: StdMutex<Option<Arc<StartupCleanupOwner>>>,
    pub(crate) construction_inflight: AtomicBool,
    pub(crate) construction_interrupted: AtomicBool,
    pub(crate) construction_changed: Notify,
    pub(crate) construction_blocking_changed: Condvar,
    pub(crate) construction_blocking_gate: StdMutex<()>,
}

/// 构造尚未返回时也保留它已登记的资源和回滚负责人。
pub(crate) struct RecoveryConstructionGuard {
    pub(crate) bootstrap: Arc<RecoveryBootstrap>,
    pub(crate) owner: Arc<StartupCleanupOwner>,
    pub(crate) events: EventSender,
    pub(crate) completed: bool,
}

impl RecoveryConstructionGuard {
    pub(crate) fn new(
        bootstrap: Arc<RecoveryBootstrap>,
        host: Arc<ReusableHost>,
        events: EventSender,
    ) -> Self {
        let owner = Arc::new(StartupCleanupOwner::default());
        owner.accept_host(host);
        *bootstrap.lock_construction() = Some(Arc::clone(&owner));
        {
            let _gate = bootstrap
                .construction_blocking_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            bootstrap
                .construction_inflight
                .store(true, Ordering::Release);
        }
        Self {
            bootstrap,
            owner,
            events,
            completed: false,
        }
    }

    pub(crate) fn handoff(mut self, runtime: Arc<ProductionRuntime>) -> RecoveryRuntimeGuard {
        let guard =
            RecoveryRuntimeGuard::new(Arc::clone(&self.bootstrap), runtime, self.events.clone());
        self.owner.deliver();
        *self.bootstrap.lock_construction() = None;
        self.completed = true;
        self.bootstrap.finish_construction();
        guard
    }

    pub(crate) fn handoff_mode(mut self) {
        self.owner.deliver();
        *self.bootstrap.lock_construction() = None;
        self.completed = true;
        self.bootstrap.finish_construction();
    }

    pub(crate) fn finish_without_runtime(mut self) -> bool {
        let settled = self.owner.settle_recovery_construction();
        if settled {
            *self.bootstrap.lock_construction() = None;
        }
        self.completed = true;
        self.bootstrap.finish_construction();
        settled
    }
}

impl Drop for RecoveryConstructionGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.bootstrap
                .construction_interrupted
                .store(true, Ordering::Release);
            let summary = {
                let mut summary = self.bootstrap.lock_summary();
                summary.state = ProfileRecoveryState::Failed;
                summary.can_submit_passphrase = false;
                summary.restart_required = true;
                summary.background_ready = false;
                summary.clone()
            };
            publish_profile_recovery(&self.events, summary);
            self.bootstrap.finish_construction();
        }
    }
}

pub(crate) struct RecoveryRuntimeGuard {
    pub(crate) bootstrap: Arc<RecoveryBootstrap>,
    pub(crate) events: EventSender,
    pub(crate) completed: bool,
}

impl RecoveryRuntimeGuard {
    pub(crate) fn new(
        bootstrap: Arc<RecoveryBootstrap>,
        runtime: Arc<ProductionRuntime>,
        events: EventSender,
    ) -> Self {
        *bootstrap.lock_recovered_runtime() = Some(runtime);
        Self {
            bootstrap,
            events,
            completed: false,
        }
    }

    pub(crate) fn complete(mut self) {
        *self.bootstrap.lock_recovered_runtime() = None;
        self.completed = true;
    }
}

impl Drop for RecoveryRuntimeGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let summary = {
            let mut summary = self.bootstrap.lock_summary();
            summary.state = ProfileRecoveryState::Failed;
            summary.can_submit_passphrase = false;
            summary.restart_required = true;
            summary.background_ready = false;
            summary.clone()
        };
        publish_profile_recovery(&self.events, summary);
    }
}

impl RecoveryBootstrap {
    pub(crate) fn new(
        progress: Arc<StartupProgressStore>,
        summary: ProfileRecoverySummary,
        input_available: bool,
    ) -> Self {
        Self {
            input_available: Mutex::new(input_available),
            gate: Mutex::new(()),
            progress,
            summary: StdMutex::new(summary),
            recovered_runtime: StdMutex::new(None),
            construction: StdMutex::new(None),
            construction_inflight: AtomicBool::new(false),
            construction_interrupted: AtomicBool::new(false),
            construction_changed: Notify::new(),
            construction_blocking_changed: Condvar::new(),
            construction_blocking_gate: StdMutex::new(()),
        }
    }

    pub(crate) fn lock_summary(&self) -> StdMutexGuard<'_, ProfileRecoverySummary> {
        self.summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn finish_construction(&self) {
        {
            let _gate = self
                .construction_blocking_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.construction_inflight.store(false, Ordering::Release);
            self.construction_blocking_changed.notify_all();
        }
        self.construction_changed.notify_waiters();
    }

    pub(crate) fn wait_for_construction_blocking(&self) {
        let mut gate = self
            .construction_blocking_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while self.construction_inflight.load(Ordering::Acquire) {
            gate = self
                .construction_blocking_changed
                .wait(gate)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(crate) async fn wait_for_construction(&self) {
        loop {
            let notified = self.construction_changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.construction_inflight.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    pub(crate) async fn wait_accepted_work(&self) {
        self.wait_for_construction().await;
        let owner = self.lock_construction().clone();
        if let Some(owner) = owner {
            let _ = owner.wait_recovery_backup().await;
        }
    }

    pub(crate) fn wait_accepted_work_blocking(&self) {
        self.wait_for_construction_blocking();
        let owner = self.lock_construction().clone();
        if let Some(owner) = owner {
            let _ = owner.wait_recovery_backup_blocking();
        }
    }

    pub(crate) fn lock_construction(&self) -> StdMutexGuard<'_, Option<Arc<StartupCleanupOwner>>> {
        self.construction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn lock_recovered_runtime(
        &self,
    ) -> StdMutexGuard<'_, Option<Arc<ProductionRuntime>>> {
        self.recovered_runtime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
