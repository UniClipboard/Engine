use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    ProfileUpgradeBackupError, ProfileUpgradeBackupPort, ProfileUpgradeSource,
    ProfileUpgradeVersions,
};
use uc_infra::security::{
    StorageUpgradeFailure, StorageUpgradeObserver, StorageUpgradeProgressOutcome,
    StorageUpgradeSnapshot, StorageUpgradeStep, StorageUpgradeUnit,
};

use crate::engine::startup::StartupProgressStore;
use crate::{
    StartupFailure, StartupFailureReason, StartupProgressUnit, StartupState, StartupStepProgress,
    StartupUpgradeProgress, StartupUpgradeStep,
};

impl StorageUpgradeObserver for StartupProgressStore {
    fn update(&self, upgrade: StorageUpgradeSnapshot) {
        let storage_required = upgrade.required;
        let recovering = upgrade.recovering;
        let storage_completed = matches!(
            upgrade.outcome,
            Some(
                StorageUpgradeProgressOutcome::Completed | StorageUpgradeProgressOutcome::NotNeeded
            )
        );
        let current_step = upgrade.current_step.map(upgrade_step);
        let storage_steps = upgrade
            .steps
            .into_iter()
            .map(|step| StartupStepProgress {
                step: upgrade_step(step.step),
                processed: step.processed,
                total: step.total,
                unit: step.unit.map(|unit| match unit {
                    StorageUpgradeUnit::Representations => {
                        StartupProgressUnit::ContentRepresentations
                    }
                    StorageUpgradeUnit::LargeContents => StartupProgressUnit::LargeContents,
                    StorageUpgradeUnit::Records => StartupProgressUnit::RelatedRecords,
                }),
                warning_count: step.warning_count,
                completed: step.completed,
            })
            .collect::<Vec<_>>();
        self.update(|snapshot| {
            let backup = snapshot.upgrade.as_ref().and_then(|existing| {
                existing
                    .steps
                    .iter()
                    .find(|step| step.step == StartupUpgradeStep::BackingUp)
                    .cloned()
            });
            let backup_completed = backup.as_ref().is_none_or(|step| step.completed);
            let required = storage_required || backup.is_some();
            snapshot.state = if required {
                StartupState::Upgrading
            } else {
                StartupState::Preparing
            };
            snapshot.failure = upgrade.failure.map(|failure| StartupFailure {
                reason: match failure {
                    StorageUpgradeFailure::StorageFull => StartupFailureReason::StorageFull,
                    StorageUpgradeFailure::PermissionDenied => {
                        StartupFailureReason::PermissionDenied
                    }
                    StorageUpgradeFailure::StorageUnavailable => {
                        StartupFailureReason::StorageUnavailable
                    }
                    StorageUpgradeFailure::ProtectionUnavailable => {
                        StartupFailureReason::ProtectionUnavailable
                    }
                    StorageUpgradeFailure::CorruptData => StartupFailureReason::CorruptData,
                    StorageUpgradeFailure::SourceChanged => StartupFailureReason::SourceChanged,
                    StorageUpgradeFailure::Busy => StartupFailureReason::AlreadyRunning,
                },
                retryable: matches!(
                    failure,
                    StorageUpgradeFailure::StorageFull
                        | StorageUpgradeFailure::PermissionDenied
                        | StorageUpgradeFailure::StorageUnavailable
                        | StorageUpgradeFailure::SourceChanged
                        | StorageUpgradeFailure::Busy
                ),
            });
            let mut steps = storage_steps;
            if let Some(backup) = backup {
                steps.insert(0, backup);
            }
            snapshot.upgrade = Some(StartupUpgradeProgress {
                required,
                recovering,
                completed: storage_completed && backup_completed,
                current_step,
                steps,
            });
        });
    }
}

pub(crate) struct StartupProfileUpgradeBackup {
    inner: Arc<dyn ProfileUpgradeBackupPort>,
    progress: Arc<StartupProgressStore>,
}

impl StartupProfileUpgradeBackup {
    pub(crate) fn new(
        inner: Arc<dyn ProfileUpgradeBackupPort>,
        progress: Arc<StartupProgressStore>,
    ) -> Self {
        Self { inner, progress }
    }

    async fn track(
        &self,
        operation: impl Future<Output = Result<(), ProfileUpgradeBackupError>>,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.progress.backup_started();
        let result = operation.await;
        if result.is_ok() {
            self.progress.backup_completed();
        } else {
            self.progress.backup_failed();
        }
        result
    }
}

#[async_trait]
impl ProfileUpgradeBackupPort for StartupProfileUpgradeBackup {
    async fn list_backups(
        &self,
    ) -> Result<Vec<uc_application::deps::ProfileUpgradeBackupEntry>, ProfileUpgradeBackupError>
    {
        self.inner.list_backups().await
    }

    async fn delete_backup(&self, id: &str) -> Result<(), ProfileUpgradeBackupError> {
        self.inner.delete_backup(id).await
    }

    fn read_source(&self) -> Result<ProfileUpgradeSource, ProfileUpgradeBackupError> {
        self.inner.read_source()
    }

    fn read_prepared_target(
        &self,
    ) -> Result<Option<ProfileUpgradeVersions>, ProfileUpgradeBackupError> {
        self.inner.read_prepared_target()
    }

    async fn capture_verified(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.track(self.inner.capture_verified(target)).await
    }

    async fn verify_prepared(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.track(self.inner.verify_prepared(target)).await
    }

    async fn preserve_security_materials(
        &self,
        target: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.track(self.inner.preserve_security_materials(target))
            .await
    }
}

fn upgrade_step(step: StorageUpgradeStep) -> StartupUpgradeStep {
    match step {
        StorageUpgradeStep::Checking => StartupUpgradeStep::Checking,
        StorageUpgradeStep::Contents => StartupUpgradeStep::ConvertingContents,
        StorageUpgradeStep::LargeContents => StartupUpgradeStep::ConvertingLargeContents,
        StorageUpgradeStep::RelatedRecords => StartupUpgradeStep::ConvertingRelatedRecords,
        StorageUpgradeStep::Verifying => StartupUpgradeStep::Verifying,
        StorageUpgradeStep::Preparing => StartupUpgradeStep::Preparing,
    }
}
