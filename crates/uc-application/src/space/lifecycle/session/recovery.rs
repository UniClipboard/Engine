use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{SpaceActivityError, SpaceSessionActivityPort};

#[async_trait]
pub(crate) trait SpaceSessionRecoveryPort: Send + Sync {
    async fn request_activation(&self) -> Result<(), SpaceActivityError>;
    async fn pause_for_lock(&self) -> Result<LockGeneration, SpaceActivityError>;
    async fn finish_successful_lock(&self, generation: LockGeneration);
    async fn restore_after_failed_lock(
        &self,
        generation: LockGeneration,
    ) -> Result<(), anyhow::Error>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct LockGeneration(u64);

pub(crate) struct SpaceSessionRecovery {
    activity: Arc<dyn SpaceSessionActivityPort>,
    state: Mutex<RecoveryState>,
    closed: AtomicBool,
    retry_delays: Arc<[Duration]>,
}

#[derive(Default)]
struct RecoveryState {
    activation: Option<ActivationTask>,
    active: bool,
    lock_generation: u64,
    locking: Option<LockGeneration>,
}

struct ActivationTask {
    cancel: CancellationToken,
    handle: JoinHandle<bool>,
}

impl SpaceSessionRecovery {
    pub(crate) fn new(activity: Arc<dyn SpaceSessionActivityPort>) -> Arc<Self> {
        Self::new_with_retry_delays(
            activity,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
                Duration::from_secs(10),
                Duration::from_secs(30),
            ],
        )
    }

    fn new_with_retry_delays(
        activity: Arc<dyn SpaceSessionActivityPort>,
        retry_delays: Vec<Duration>,
    ) -> Arc<Self> {
        Arc::new(Self {
            activity,
            state: Mutex::new(RecoveryState::default()),
            closed: AtomicBool::new(false),
            retry_delays: retry_delays.into(),
        })
    }

    pub(crate) async fn shutdown(&self) -> Result<(), SpaceActivityError> {
        self.closed.store(true, Ordering::Release);
        let mut state = self.state.lock().await;
        cancel_activation(&mut state).await
    }
}

#[async_trait]
impl SpaceSessionRecoveryPort for SpaceSessionRecovery {
    async fn request_activation(&self) -> Result<(), SpaceActivityError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SpaceActivityError::Unavailable);
        }
        let mut state = self.state.lock().await;
        if self.closed.load(Ordering::Acquire) {
            return Err(SpaceActivityError::Unavailable);
        }
        if state.locking.is_some() {
            return Ok(());
        }
        if state.active {
            return Ok(());
        }
        if state
            .activation
            .as_ref()
            .is_some_and(|task| !task.handle.is_finished())
        {
            return Ok(());
        }
        if let Some(task) = state.activation.take() {
            state.active = task.handle.await.map_err(SpaceActivityError::Task)?;
            if state.active {
                return Ok(());
            }
        }

        let activity = Arc::clone(&self.activity);
        let retry_delays = Arc::clone(&self.retry_delays);
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            let mut attempt = 0_usize;
            loop {
                let activation = tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => return false,
                    result = activity.resume_after_session_ready() => result,
                };
                if activation.is_ok() {
                    return true;
                }
                tracing::warn!(
                    attempt,
                    "space session background activation failed; retrying"
                );
                let delay = retry_delays
                    .get(attempt)
                    .or_else(|| retry_delays.last())
                    .copied()
                    .unwrap_or(Duration::ZERO);
                attempt = attempt.saturating_add(1);
                tokio::select! {
                    biased;
                    _ = task_cancel.cancelled() => return false,
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        });
        state.activation = Some(ActivationTask { cancel, handle });
        Ok(())
    }

    async fn pause_for_lock(&self) -> Result<LockGeneration, SpaceActivityError> {
        let mut state = self.state.lock().await;
        if state.locking.is_some() {
            return Err(SpaceActivityError::Unavailable);
        }
        state.lock_generation = state.lock_generation.wrapping_add(1);
        let generation = LockGeneration(state.lock_generation);
        state.locking = Some(generation);
        if let Err(error) = cancel_activation(&mut state).await {
            clear_matching_lock(&mut state, generation);
            return Err(error);
        }
        state.active = false;
        if let Err(error) = self.activity.pause_for_lock().await {
            clear_matching_lock(&mut state, generation);
            return Err(error);
        }
        Ok(generation)
    }

    async fn finish_successful_lock(&self, generation: LockGeneration) {
        let mut state = self.state.lock().await;
        clear_matching_lock(&mut state, generation);
    }

    async fn restore_after_failed_lock(
        &self,
        generation: LockGeneration,
    ) -> Result<(), anyhow::Error> {
        let mut state = self.state.lock().await;
        if state.locking != Some(generation) {
            return Ok(());
        }
        let restoration = self.activity.restore_after_failed_lock().await;
        state.active = restoration.is_ok();
        clear_matching_lock(&mut state, generation);
        restoration
    }
}

fn clear_matching_lock(state: &mut RecoveryState, generation: LockGeneration) {
    if state.locking == Some(generation) {
        state.locking = None;
    }
}

async fn cancel_activation(state: &mut RecoveryState) -> Result<(), SpaceActivityError> {
    let Some(task) = state.activation.take() else {
        return Ok(());
    };
    task.cancel.cancel();
    task.handle
        .await
        .map(|_| ())
        .map_err(SpaceActivityError::Task)
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::sync::Notify;
    use tokio::time::timeout;

    use super::*;

    struct RecordingActivity {
        activations: AtomicUsize,
        failures_before_success: usize,
        pauses: AtomicUsize,
        restores: AtomicUsize,
        block_activation: bool,
        entered: Notify,
    }

    impl RecordingActivity {
        fn succeeds() -> Arc<Self> {
            Arc::new(Self {
                activations: AtomicUsize::new(0),
                failures_before_success: 0,
                pauses: AtomicUsize::new(0),
                restores: AtomicUsize::new(0),
                block_activation: false,
                entered: Notify::new(),
            })
        }

        fn blocked() -> Arc<Self> {
            Arc::new(Self {
                activations: AtomicUsize::new(0),
                failures_before_success: 0,
                pauses: AtomicUsize::new(0),
                restores: AtomicUsize::new(0),
                block_activation: true,
                entered: Notify::new(),
            })
        }
    }

    #[async_trait]
    impl SpaceSessionActivityPort for RecordingActivity {
        async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError> {
            let attempt = self.activations.fetch_add(1, Ordering::SeqCst) + 1;
            self.entered.notify_waiters();
            if self.block_activation {
                pending::<()>().await;
            }
            if attempt <= self.failures_before_success {
                return Err(SpaceActivityError::Unavailable);
            }
            Ok(())
        }

        async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
            self.pauses.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error> {
            self.restores.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn duplicate_activation_requests_share_one_attempt() {
        let activity = RecordingActivity::blocked();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        recovery.request_activation().await.unwrap();
        activity.entered.notified().await;
        recovery.request_activation().await.unwrap();

        assert_eq!(activity.activations.load(Ordering::SeqCst), 1);
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn temporary_activation_failure_is_retried() {
        let activity = Arc::new(RecordingActivity {
            activations: AtomicUsize::new(0),
            failures_before_success: 2,
            pauses: AtomicUsize::new(0),
            restores: AtomicUsize::new(0),
            block_activation: false,
            entered: Notify::new(),
        });
        let recovery =
            SpaceSessionRecovery::new_with_retry_delays(activity.clone(), vec![Duration::ZERO]);

        recovery.request_activation().await.unwrap();

        timeout(Duration::from_secs(1), async {
            while activity.activations.load(Ordering::SeqCst) < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn completed_activation_is_not_started_again() {
        let activity = RecordingActivity::succeeds();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        recovery.request_activation().await.unwrap();
        activity.entered.notified().await;
        tokio::task::yield_now().await;
        recovery.request_activation().await.unwrap();

        assert_eq!(activity.activations.load(Ordering::SeqCst), 1);
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn pause_cancels_activation_before_pausing_activity() {
        let activity = RecordingActivity::blocked();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        recovery.request_activation().await.unwrap();
        activity.entered.notified().await;
        let generation = timeout(Duration::from_secs(1), recovery.pause_for_lock())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(activity.activations.load(Ordering::SeqCst), 1);
        assert_eq!(activity.pauses.load(Ordering::SeqCst), 1);
        recovery.finish_successful_lock(generation).await;
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn activation_remains_deferred_until_lock_finishes() {
        let activity = RecordingActivity::succeeds();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        let generation = recovery.pause_for_lock().await.unwrap();
        recovery.request_activation().await.unwrap();
        tokio::task::yield_now().await;

        assert_eq!(activity.activations.load(Ordering::SeqCst), 0);

        recovery.finish_successful_lock(generation).await;
        recovery.request_activation().await.unwrap();
        activity.entered.notified().await;

        assert_eq!(activity.activations.load(Ordering::SeqCst), 1);
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stale_failed_lock_does_not_restore_during_newer_lock() {
        let activity = RecordingActivity::succeeds();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        let stale = recovery.pause_for_lock().await.unwrap();
        recovery.finish_successful_lock(stale).await;
        let current = recovery.pause_for_lock().await.unwrap();
        recovery.restore_after_failed_lock(stale).await.unwrap();
        recovery.request_activation().await.unwrap();
        tokio::task::yield_now().await;

        assert_eq!(activity.activations.load(Ordering::SeqCst), 0);
        assert_eq!(activity.restores.load(Ordering::SeqCst), 0);

        recovery.restore_after_failed_lock(current).await.unwrap();
        recovery.request_activation().await.unwrap();
        tokio::task::yield_now().await;

        assert_eq!(activity.activations.load(Ordering::SeqCst), 0);
        assert_eq!(activity.restores.load(Ordering::SeqCst), 1);
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn overlapping_lock_pause_is_rejected() {
        let activity = RecordingActivity::succeeds();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        let generation = recovery.pause_for_lock().await.unwrap();

        assert!(matches!(
            recovery.pause_for_lock().await,
            Err(SpaceActivityError::Unavailable)
        ));
        assert_eq!(activity.pauses.load(Ordering::SeqCst), 1);

        recovery.finish_successful_lock(generation).await;
        recovery.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_rejects_later_activation() {
        let activity = RecordingActivity::succeeds();
        let recovery = SpaceSessionRecovery::new(activity);

        recovery.shutdown().await.unwrap();

        assert!(matches!(
            recovery.request_activation().await,
            Err(SpaceActivityError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn shutdown_cancels_an_inflight_activation() {
        let activity = RecordingActivity::blocked();
        let recovery = SpaceSessionRecovery::new(activity.clone());

        recovery.request_activation().await.unwrap();
        activity.entered.notified().await;
        timeout(Duration::from_secs(1), recovery.shutdown())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(activity.activations.load(Ordering::SeqCst), 1);
    }
}
