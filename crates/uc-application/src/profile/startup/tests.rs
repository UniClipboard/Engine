use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::*;
use crate::profile::factory_reset::{
    ProfileGeneration, ProfileLifecycle, ProfileLifecycleRepositoryError,
    ProfileLifecycleRepositoryPort, ProfileLifecycleState,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackupMode {
    Fresh,
    Upgrade,
    Current,
    CurrentEngineOnly,
    Retry,
    Fail,
    Pending,
    Denied,
}

struct Fixture {
    mode: BackupMode,
    lifecycle: Mutex<Option<ProfileLifecycle>>,
    events: Mutex<Vec<&'static str>>,
    fail_import: bool,
}

impl Fixture {
    fn new(mode: BackupMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            lifecycle: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            fail_import: false,
        })
    }

    fn workflow(self: &Arc<Self>) -> PrepareProfileStartupUseCase {
        PrepareProfileStartupUseCase::new(self.clone(), self.clone(), self.clone(), target())
    }

    fn event(&self, value: &'static str) {
        self.events.lock().unwrap().push(value);
    }
}

fn target() -> ProfileUpgradeVersions {
    ProfileUpgradeVersions {
        product: "2.0.0".into(),
        engine: "3.0.0".into(),
    }
}

#[async_trait]
impl ProfileUpgradeBackupPort for Fixture {
    async fn list_backups(
        &self,
    ) -> Result<Vec<ProfileUpgradeBackupEntry>, ProfileUpgradeBackupError> {
        Ok(Vec::new())
    }

    async fn delete_backup(&self, _: &str) -> Result<(), ProfileUpgradeBackupError> {
        Ok(())
    }

    fn read_source(&self) -> Result<ProfileUpgradeSource, ProfileUpgradeBackupError> {
        self.event("read backup state");
        Ok(ProfileUpgradeSource {
            has_data: self.mode != BackupMode::Fresh,
            source_product: (self.mode == BackupMode::Current).then(|| target().product),
            source_engine: matches!(
                self.mode,
                BackupMode::Current | BackupMode::CurrentEngineOnly
            )
            .then(|| target().engine),
        })
    }
    fn read_prepared_target(
        &self,
    ) -> Result<Option<ProfileUpgradeVersions>, ProfileUpgradeBackupError> {
        assert!(!matches!(
            self.mode,
            BackupMode::Fresh | BackupMode::Current | BackupMode::CurrentEngineOnly
        ));
        Ok((self.mode == BackupMode::Retry).then(target))
    }
    async fn capture_verified(
        &self,
        _: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.event("capture and verify");
        match self.mode {
            BackupMode::Fail => Err(ProfileUpgradeBackupError {
                source: std::io::Error::from(std::io::ErrorKind::StorageFull).into(),
            }),
            BackupMode::Pending => std::future::pending().await,
            _ => Ok(()),
        }
    }
    async fn verify_prepared(
        &self,
        _: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.event("verify existing backup");
        Ok(())
    }
    async fn preserve_security_materials(
        &self,
        _: &ProfileUpgradeVersions,
    ) -> Result<(), ProfileUpgradeBackupError> {
        self.event("preserve security materials");
        Ok(())
    }
}

impl ProfileStartupStoragePort for Fixture {
    fn adopt_legacy_layout(&self) -> Result<(), ProfileStartupStorageError> {
        self.event("adopt old layout");
        Ok(())
    }
    fn apply_pending_import(&self) -> Result<(), ProfileStartupStorageError> {
        self.event("apply import");
        if self.fail_import {
            Err(ProfileStartupStorageError {
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied).into(),
            })
        } else {
            Ok(())
        }
    }
}

impl ProfileLifecycleRepositoryPort for Fixture {
    fn load(&self) -> Result<Option<ProfileLifecycle>, ProfileLifecycleRepositoryError> {
        self.event("read lifecycle");
        if self.mode == BackupMode::Denied {
            return Err(ProfileLifecycleRepositoryError::Unavailable);
        }
        Ok(self.lifecycle.lock().unwrap().clone())
    }
    fn compare_and_swap(
        &self,
        expected: Option<&ProfileLifecycle>,
        next: &ProfileLifecycle,
    ) -> Result<(), ProfileLifecycleRepositoryError> {
        self.event("create lifecycle");
        let mut current = self.lifecycle.lock().unwrap();
        assert_eq!(current.as_ref(), expected);
        *current = Some(next.clone());
        Ok(())
    }
}

#[tokio::test]
async fn preparation_owns_the_complete_order_before_returning() {
    let fixture = Fixture::new(BackupMode::Upgrade);
    let lifecycle = fixture.workflow().execute().await.unwrap();
    assert_eq!(lifecycle.state(), ProfileLifecycleState::Ready);
    assert_eq!(
        *fixture.events.lock().unwrap(),
        [
            "read backup state",
            "capture and verify",
            "read lifecycle",
            "preserve security materials",
            "adopt old layout",
            "apply import",
            "create lifecycle"
        ]
    );
}

#[tokio::test]
async fn repeated_preparation_preserves_the_created_lifecycle() {
    let fixture = Fixture::new(BackupMode::Fresh);
    let first = fixture.workflow().execute().await.unwrap();
    assert_eq!(fixture.workflow().execute().await.unwrap(), first);
    assert_eq!(
        fixture
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| **event == "create lifecycle")
            .count(),
        1
    );
}

#[tokio::test]
async fn backup_failure_preserves_source_error_and_stops_all_mutations() {
    let fixture = Fixture::new(BackupMode::Fail);
    let error = fixture.workflow().execute().await.unwrap_err();
    assert_eq!(
        *fixture.events.lock().unwrap(),
        ["read backup state", "capture and verify"]
    );
    let ProfileStartupError::Backup(error) = error else {
        panic!("wrong failure");
    };
    assert_eq!(
        error
            .source
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::StorageFull
    );
    assert!(fixture.lifecycle.lock().unwrap().is_none());
}

#[tokio::test]
async fn cancelled_backup_wait_cannot_adopt_import_or_create_lifecycle() {
    let fixture = Fixture::new(BackupMode::Pending);
    let workflow = fixture.workflow();
    let mut preparation = Box::pin(workflow.execute());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut preparation)
            .await
            .is_err()
    );
    drop(preparation);
    assert_eq!(
        *fixture.events.lock().unwrap(),
        ["read backup state", "capture and verify"]
    );
    assert!(fixture.lifecycle.lock().unwrap().is_none());
}

#[tokio::test]
async fn retry_verifies_original_backup_and_reuses_lifecycle() {
    let fixture = Fixture::new(BackupMode::Retry);
    let existing = ProfileLifecycle::new(ProfileGeneration::new());
    *fixture.lifecycle.lock().unwrap() = Some(existing.clone());
    let actual = fixture.workflow().execute().await.unwrap();
    assert_eq!(actual, existing);
    assert_eq!(
        *fixture.events.lock().unwrap(),
        [
            "read backup state",
            "verify existing backup",
            "read lifecycle",
            "preserve security materials",
            "adopt old layout",
            "apply import"
        ]
    );
}

#[tokio::test]
async fn fresh_or_current_version_prepares_without_backup() {
    for mode in [
        BackupMode::Fresh,
        BackupMode::Current,
        BackupMode::CurrentEngineOnly,
    ] {
        let fixture = Fixture::new(mode);
        fixture.workflow().execute().await.unwrap();
        assert_eq!(
            *fixture.events.lock().unwrap(),
            [
                "read backup state",
                "read lifecycle",
                "adopt old layout",
                "apply import",
                "create lifecycle"
            ]
        );
    }
}

#[tokio::test]
async fn factory_reset_preserves_files_before_returning_the_existing_lifecycle() {
    let fixture = Fixture::new(BackupMode::Upgrade);
    let generation = ProfileGeneration::new();
    let mut lifecycle = ProfileLifecycle::new(generation);
    lifecycle.begin_factory_reset(generation).unwrap();
    *fixture.lifecycle.lock().unwrap() = Some(lifecycle.clone());
    assert_eq!(fixture.workflow().execute().await.unwrap(), lifecycle);
    assert_eq!(
        *fixture.events.lock().unwrap(),
        ["read backup state", "capture and verify", "read lifecycle"]
    );
}

#[tokio::test]
async fn failed_import_does_not_create_a_new_lifecycle() {
    let mut fixture = Fixture::new(BackupMode::Upgrade);
    Arc::get_mut(&mut fixture).unwrap().fail_import = true;
    assert!(matches!(
        fixture.workflow().execute().await,
        Err(ProfileStartupError::Storage(_))
    ));
    assert_eq!(
        *fixture.events.lock().unwrap(),
        [
            "read backup state",
            "capture and verify",
            "read lifecycle",
            "preserve security materials",
            "adopt old layout",
            "apply import"
        ]
    );
    assert!(fixture.lifecycle.lock().unwrap().is_none());
}

#[tokio::test]
async fn keychain_failure_occurs_after_file_backup_and_before_mutation() {
    let fixture = Fixture::new(BackupMode::Denied);
    assert!(matches!(
        fixture.workflow().execute().await,
        Err(ProfileStartupError::Lifecycle(_))
    ));
    assert_eq!(
        *fixture.events.lock().unwrap(),
        ["read backup state", "capture and verify", "read lifecycle"]
    );
}
