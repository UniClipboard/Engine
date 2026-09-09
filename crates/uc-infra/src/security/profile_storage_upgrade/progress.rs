use std::sync::{Arc, Mutex};

use super::journal::{UpgradeJournalV1, UpgradePhaseV1};
use super::{ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome};

/// 存储维护的稳定工作类别，不暴露 journal 或 generation。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageUpgradeStep {
    Checking,
    Contents,
    LargeContents,
    RelatedRecords,
    Verifying,
    Preparing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageUpgradeUnit {
    Representations,
    LargeContents,
    Records,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageUpgradeStepProgress {
    pub step: StorageUpgradeStep,
    pub processed: u64,
    pub total: Option<u64>,
    pub unit: Option<StorageUpgradeUnit>,
    pub warning_count: Option<u64>,
    pub completed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageUpgradeFailure {
    StorageFull,
    PermissionDenied,
    StorageUnavailable,
    ProtectionUnavailable,
    CorruptData,
    SourceChanged,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageUpgradeProgressOutcome {
    Completed,
    NotNeeded,
    Pending,
    Failed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageUpgradeSnapshot {
    pub required: bool,
    pub recovering: bool,
    pub current_step: Option<StorageUpgradeStep>,
    pub steps: Vec<StorageUpgradeStepProgress>,
    pub outcome: Option<StorageUpgradeProgressOutcome>,
    pub failure: Option<StorageUpgradeFailure>,
}

/// 只供组合根接收内存摘要。实现必须快速返回，不能执行宿主回调或等待消费者。
pub trait StorageUpgradeObserver: Send + Sync {
    fn update(&self, snapshot: StorageUpgradeSnapshot);
}

pub(super) struct UpgradeProgress {
    observer: Option<Arc<dyn StorageUpgradeObserver>>,
    state: Mutex<StorageUpgradeSnapshot>,
}

impl UpgradeProgress {
    pub(super) fn new(observer: Option<Arc<dyn StorageUpgradeObserver>>) -> Self {
        Self {
            observer,
            state: Mutex::new(StorageUpgradeSnapshot::default()),
        }
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> StorageUpgradeSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn update(&self, change: impl FnOnce(&mut StorageUpgradeSnapshot)) {
        let snapshot = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            change(&mut state);
            state.clone()
        };
        if let Some(observer) = &self.observer {
            observer.update(snapshot);
        }
    }

    pub(super) fn required(&self, required: bool) {
        self.update(|state| state.required = required);
    }

    pub(super) fn recovering(&self) {
        self.update(|state| state.recovering = true);
    }

    pub(super) fn restart(&self) {
        self.update(|state| {
            state.recovering = true;
            state.steps.clear();
            state.current_step = None;
        });
        self.begin(StorageUpgradeStep::Checking, None, None);
    }

    pub(super) fn begin(
        &self,
        step: StorageUpgradeStep,
        total: Option<u64>,
        unit: Option<StorageUpgradeUnit>,
    ) {
        self.update(|state| {
            state.current_step = Some(step);
            let next = StorageUpgradeStepProgress {
                step,
                processed: 0,
                total,
                unit,
                warning_count: Some(0),
                completed: false,
            };
            if let Some(previous) = state.steps.iter_mut().find(|item| item.step == step) {
                *previous = next;
            } else {
                state.steps.push(next);
            }
        });
    }

    pub(super) fn processed(&self, step: StorageUpgradeStep, count: u64, warnings: u64) {
        self.update(|state| {
            if let Some(item) = state.steps.iter_mut().find(|item| item.step == step) {
                item.processed = count;
                item.warning_count = Some(warnings);
            }
        });
    }

    pub(super) fn complete(&self, step: StorageUpgradeStep) {
        self.update(|state| {
            if let Some(item) = state.steps.iter_mut().find(|item| item.step == step) {
                item.completed = true;
            }
        });
    }

    pub(super) fn restore(
        &self,
        step: StorageUpgradeStep,
        count: u64,
        unit: StorageUpgradeUnit,
        warnings: Option<u64>,
    ) {
        self.update(|state| {
            let restored = StorageUpgradeStepProgress {
                step,
                processed: count,
                total: if step == StorageUpgradeStep::RelatedRecords && warnings.is_none() {
                    None
                } else {
                    Some(count)
                },
                unit: Some(unit),
                warning_count: warnings,
                completed: true,
            };
            if let Some(item) = state.steps.iter_mut().find(|item| item.step == step) {
                *item = restored;
            } else {
                state.steps.push(restored);
            }
        });
    }

    pub(super) fn restore_from_journal(&self, journal: &UpgradeJournalV1) {
        if !matches!(
            journal.phase(),
            UpgradePhaseV1::Detected | UpgradePhaseV1::TargetStaged
        ) {
            self.complete(StorageUpgradeStep::Checking);
        }
        if let Some(count) = journal.converted_inline_count() {
            self.restore(
                StorageUpgradeStep::Contents,
                count,
                StorageUpgradeUnit::Representations,
                Some(0),
            );
        }
        if let Some(count) = journal.converted_blob_count() {
            self.restore(
                StorageUpgradeStep::LargeContents,
                count,
                StorageUpgradeUnit::LargeContents,
                journal.preserved_blob_count(),
            );
        }
        if let Some(count) = journal.converted_derived_count() {
            let warnings = journal.unavailable_derived_count();
            self.restore(
                StorageUpgradeStep::RelatedRecords,
                count.saturating_add(warnings.unwrap_or(0)),
                StorageUpgradeUnit::Records,
                warnings,
            );
        }
        if matches!(
            journal.phase(),
            UpgradePhaseV1::Verified | UpgradePhaseV1::Promoted | UpgradePhaseV1::CleanupPending
        ) {
            self.begin(StorageUpgradeStep::Verifying, None, None);
            self.complete(StorageUpgradeStep::Verifying);
        }
    }

    pub(super) fn finish(
        &self,
        result: &Result<ProfileStorageUpgradeOutcome, ProfileStorageUpgradeError>,
    ) {
        self.update(|state| {
            state.outcome = Some(match result {
                Ok(ProfileStorageUpgradeOutcome::Busy) => {
                    state.failure = Some(StorageUpgradeFailure::Busy);
                    StorageUpgradeProgressOutcome::Failed
                }
                Ok(ProfileStorageUpgradeOutcome::Pending) => StorageUpgradeProgressOutcome::Pending,
                Ok(_) if !state.required => StorageUpgradeProgressOutcome::NotNeeded,
                Ok(_) => StorageUpgradeProgressOutcome::Completed,
                Err(error) => {
                    state.failure = Some(classify_failure(error));
                    StorageUpgradeProgressOutcome::Failed
                }
            });
        });
    }
}

fn classify_failure(error: &ProfileStorageUpgradeError) -> StorageUpgradeFailure {
    let source = match error {
        ProfileStorageUpgradeError::Storage { source }
        | ProfileStorageUpgradeError::Security { source }
        | ProfileStorageUpgradeError::Corrupt { source }
        | ProfileStorageUpgradeError::Manifest { source } => Some(source),
        ProfileStorageUpgradeError::SourceChanged => None,
    };
    if let Some(source) = source {
        for cause in source.chain() {
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                match io.kind() {
                    std::io::ErrorKind::StorageFull => return StorageUpgradeFailure::StorageFull,
                    std::io::ErrorKind::PermissionDenied => {
                        return StorageUpgradeFailure::PermissionDenied
                    }
                    _ => {}
                }
            }
            if let Some(diesel::result::Error::DatabaseError(kind, _)) =
                cause.downcast_ref::<diesel::result::Error>()
            {
                if *kind == diesel::result::DatabaseErrorKind::ReadOnlyTransaction {
                    return StorageUpgradeFailure::PermissionDenied;
                }
            }
        }
    }
    match error {
        ProfileStorageUpgradeError::Storage { .. } => StorageUpgradeFailure::StorageUnavailable,
        ProfileStorageUpgradeError::Security { .. } => StorageUpgradeFailure::ProtectionUnavailable,
        ProfileStorageUpgradeError::Corrupt { .. } => StorageUpgradeFailure::CorruptData,
        ProfileStorageUpgradeError::SourceChanged => StorageUpgradeFailure::SourceChanged,
        ProfileStorageUpgradeError::Manifest { .. } => StorageUpgradeFailure::ProtectionUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_io_failures_survive_context_without_leaking_messages() {
        for (kind, expected) in [
            (
                std::io::ErrorKind::StorageFull,
                StorageUpgradeFailure::StorageFull,
            ),
            (
                std::io::ErrorKind::PermissionDenied,
                StorageUpgradeFailure::PermissionDenied,
            ),
        ] {
            let error = ProfileStorageUpgradeError::Storage {
                source: anyhow::Error::new(std::io::Error::new(kind, "private sentinel"))
                    .context("perform storage upgrade"),
            };
            assert_eq!(classify_failure(&error), expected);
            assert!(std::error::Error::source(&error).is_some());
            let progress = UpgradeProgress::new(None);
            progress.finish(&Err(error));
            assert!(!format!("{:?}", progress.snapshot()).contains("private sentinel"));
        }
    }

    #[test]
    fn protection_failure_never_claims_that_a_password_was_rejected() {
        let error = ProfileStorageUpgradeError::Security {
            source: anyhow::anyhow!("private protection detail"),
        };
        assert_eq!(
            classify_failure(&error),
            StorageUpgradeFailure::ProtectionUnavailable
        );
    }
}
