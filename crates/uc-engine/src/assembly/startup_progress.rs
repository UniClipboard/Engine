use uc_infra::security::{
    StorageUpgradeFailure, StorageUpgradeObserver, StorageUpgradeProgressOutcome,
    StorageUpgradeSnapshot, StorageUpgradeStep, StorageUpgradeUnit,
};

use crate::engine::startup::StartupProgressStore;
use crate::{StartupFailure, StartupFailureReason, StartupState};

impl StorageUpgradeObserver for StartupProgressStore {
    fn update(&self, upgrade: StorageUpgradeSnapshot) {
        self.update(|snapshot| {
            snapshot.state = if upgrade.required {
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
            snapshot.upgrade = Some(crate::StartupUpgradeProgress {
                required: upgrade.required,
                recovering: upgrade.recovering,
                completed: matches!(
                    upgrade.outcome,
                    Some(
                        StorageUpgradeProgressOutcome::Completed
                            | StorageUpgradeProgressOutcome::NotNeeded
                    )
                ),
                current_step: upgrade.current_step.map(upgrade_step),
                steps: upgrade
                    .steps
                    .into_iter()
                    .map(|step| crate::StartupStepProgress {
                        step: upgrade_step(step.step),
                        processed: step.processed,
                        total: step.total,
                        unit: step.unit.map(|unit| match unit {
                            StorageUpgradeUnit::Representations => {
                                crate::StartupProgressUnit::ContentRepresentations
                            }
                            StorageUpgradeUnit::LargeContents => {
                                crate::StartupProgressUnit::LargeContents
                            }
                            StorageUpgradeUnit::Records => {
                                crate::StartupProgressUnit::RelatedRecords
                            }
                        }),
                        warning_count: step.warning_count,
                        completed: step.completed,
                    })
                    .collect(),
            });
        });
    }
}

fn upgrade_step(step: StorageUpgradeStep) -> crate::StartupUpgradeStep {
    match step {
        StorageUpgradeStep::Checking => crate::StartupUpgradeStep::Checking,
        StorageUpgradeStep::Contents => crate::StartupUpgradeStep::ConvertingContents,
        StorageUpgradeStep::LargeContents => crate::StartupUpgradeStep::ConvertingLargeContents,
        StorageUpgradeStep::RelatedRecords => crate::StartupUpgradeStep::ConvertingRelatedRecords,
        StorageUpgradeStep::Verifying => crate::StartupUpgradeStep::Verifying,
        StorageUpgradeStep::Preparing => crate::StartupUpgradeStep::Preparing,
    }
}
