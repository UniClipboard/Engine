use std::sync::Arc;
use std::time::Instant;

use tokio::sync::watch;

use crate::{
    EngineError, StartupAllowedActions, StartupFailure, StartupFailureReason, StartupSnapshot,
    StartupState,
};

/// 启动前创建、可在窗口之间共享的只读观察者。
#[derive(Clone)]
pub struct StartupProgress {
    receiver: watch::Receiver<StartupSnapshot>,
}

/// 只能交给一次启动；丢弃输入或启动 future 会结算为 Interrupted。
pub struct StartupProgressInput {
    pub(crate) store: Arc<StartupProgressStore>,
}

pub(crate) struct StartupProgressStore {
    sender: watch::Sender<StartupSnapshot>,
    started: Instant,
}

impl StartupProgress {
    pub fn channel() -> (StartupProgressInput, Self) {
        let (sender, receiver) = watch::channel(StartupSnapshot {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            sequence: 0,
            state: StartupState::Preparing,
            elapsed_ms: 0,
            upgrade: None,
            failure: None,
            allowed_actions: StartupAllowedActions {
                retry: false,
                export_diagnostics: true,
            },
        });
        (
            StartupProgressInput {
                store: Arc::new(StartupProgressStore {
                    sender,
                    started: Instant::now(),
                }),
            },
            Self { receiver },
        )
    }

    /// 返回自有数据，不向宿主暴露会阻塞写入的 watch 借用。
    pub fn snapshot(&self) -> StartupSnapshot {
        self.receiver.borrow().clone()
    }

    /// 高频变化可能合并；最后快照和步骤记录始终可重读。
    pub async fn changed(&mut self) -> Option<StartupSnapshot> {
        self.receiver.changed().await.ok()?;
        Some(self.receiver.borrow_and_update().clone())
    }
}

impl StartupProgressStore {
    pub(crate) fn update(&self, change: impl FnOnce(&mut StartupSnapshot)) {
        self.sender.send_if_modified(|snapshot| {
            if snapshot.state.is_terminal() {
                return false;
            }
            change(snapshot);
            snapshot.sequence = snapshot.sequence.saturating_add(1);
            snapshot.elapsed_ms =
                self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
            true
        });
    }

    pub(crate) fn starting_services(&self) {
        self.update(|snapshot| snapshot.state = StartupState::StartingServices);
    }
}

impl StartupProgressInput {
    pub(crate) fn finish<T>(&self, result: &Result<T, EngineError>) {
        self.store.update(|snapshot| match result {
            Ok(_) => {
                snapshot.state = StartupState::Ready;
                snapshot.failure = None;
                snapshot.allowed_actions = StartupAllowedActions {
                    retry: false,
                    export_diagnostics: false,
                };
            }
            Err(error) => {
                snapshot.state = StartupState::Failed;
                let failure = snapshot.failure.get_or_insert(StartupFailure {
                    reason: StartupFailureReason::StartupFailed,
                    retryable: error.is_retryable(),
                });
                snapshot.allowed_actions.retry = failure.retryable;
            }
        });
    }
}

impl Drop for StartupProgressInput {
    fn drop(&mut self) {
        self.store.update(|snapshot| {
            snapshot.state = StartupState::Interrupted;
            snapshot.allowed_actions.retry = true;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uc_infra::security::{
        StorageUpgradeObserver, StorageUpgradeProgressOutcome, StorageUpgradeSnapshot,
        StorageUpgradeStep, StorageUpgradeUnit,
    };

    #[tokio::test(start_paused = true)]
    async fn unknown_total_remains_in_progress_past_host_watchdog_windows() {
        let (input, progress) = StartupProgress::channel();
        StorageUpgradeObserver::update(
            input.store.as_ref(),
            StorageUpgradeSnapshot {
                required: true,
                current_step: Some(StorageUpgradeStep::Verifying),
                steps: vec![uc_infra::security::StorageUpgradeStepProgress {
                    step: StorageUpgradeStep::Verifying,
                    processed: 0,
                    total: None,
                    unit: None,
                    warning_count: None,
                    completed: false,
                }],
                ..StorageUpgradeSnapshot::default()
            },
        );
        for seconds in [13, 33, 101] {
            tokio::time::advance(std::time::Duration::from_secs(seconds)).await;
            let snapshot = progress.snapshot();
            assert_eq!(snapshot.state, StartupState::Upgrading);
            assert!(!snapshot.allowed_actions.retry);
            assert_eq!(snapshot.upgrade.unwrap().steps[0].total, None);
        }
        input.finish(&Ok::<(), crate::EngineError>(()));
        assert_eq!(progress.snapshot().state, StartupState::Ready);
    }

    #[test]
    fn upgrade_completion_preserves_records_but_does_not_finish_startup() {
        let (input, progress) = StartupProgress::channel();
        StorageUpgradeObserver::update(
            input.store.as_ref(),
            StorageUpgradeSnapshot {
                required: true,
                recovering: true,
                current_step: Some(StorageUpgradeStep::Preparing),
                steps: vec![uc_infra::security::StorageUpgradeStepProgress {
                    step: StorageUpgradeStep::LargeContents,
                    processed: 3,
                    total: Some(3),
                    unit: Some(StorageUpgradeUnit::LargeContents),
                    warning_count: Some(1),
                    completed: true,
                }],
                outcome: Some(StorageUpgradeProgressOutcome::Completed),
                failure: None,
            },
        );
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.state, StartupState::Upgrading);
        assert!(!snapshot.allowed_actions.retry);
        input.store.starting_services();
        input.finish(&Err::<(), _>(crate::EngineError::new(
            1101,
            crate::EngineErrorCategory::Unavailable,
            true,
        )));
        let failed = progress.snapshot();
        assert_eq!(failed.state, StartupState::Failed);
        assert_eq!(failed.upgrade, snapshot.upgrade);
        assert!(failed.upgrade.unwrap().completed);
    }

    #[tokio::test]
    async fn terminal_snapshot_survives_sender_drop_and_slow_observers() {
        let (input, mut progress) = StartupProgress::channel();
        let attempt = progress.snapshot().attempt_id;
        for _ in 0..10_000 {
            input.store.starting_services();
        }
        input.finish(&Ok::<_, crate::EngineError>(()));
        drop(input);
        let terminal = progress.changed().await.unwrap();
        assert_eq!(terminal.attempt_id, attempt);
        assert_eq!(terminal.state, StartupState::Ready);
        assert!(terminal.sequence > 0);
        assert_eq!(progress.snapshot(), terminal);
        assert!(progress.changed().await.is_none());
    }

    #[tokio::test]
    async fn abandoned_attempt_is_interrupted_and_cannot_overwrite_a_retry() {
        let (input, mut first) = StartupProgress::channel();
        let (retry_input, retry) = StartupProgress::channel();
        assert_ne!(first.snapshot().attempt_id, retry.snapshot().attempt_id);
        drop(input);
        assert_eq!(
            first.changed().await.unwrap().state,
            StartupState::Interrupted
        );
        assert_eq!(retry.snapshot().state, StartupState::Preparing);
        retry_input.finish(&Ok::<_, crate::EngineError>(()));
        drop(retry_input);
        assert_eq!(retry.snapshot().state, StartupState::Ready);
    }

    #[test]
    fn disconnected_observers_do_not_cancel_or_block_completion() {
        let (input, progress) = StartupProgress::channel();
        drop(progress);
        input.store.starting_services();
        input.finish(&Ok::<_, crate::EngineError>(()));
        assert_eq!(input.store.sender.borrow().state, StartupState::Ready);
    }

    #[test]
    fn service_failure_after_upgrade_does_not_become_ready() {
        let (input, progress) = StartupProgress::channel();
        input.store.starting_services();
        input.finish(&Err::<(), _>(crate::EngineError::new(
            1101,
            crate::EngineErrorCategory::Unavailable,
            true,
        )));
        drop(input);
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.state, StartupState::Failed);
        assert_eq!(
            snapshot.failure.unwrap().reason,
            StartupFailureReason::StartupFailed
        );
    }
}
