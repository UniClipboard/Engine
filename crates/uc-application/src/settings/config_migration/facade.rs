//! `ConfigMigrationFacade` implementation.
//!
//! Orchestrates whole-installation configuration migration (export / preview /
//! stage import) by enforcing the business preconditions and then delegating to
//! the migration intent ports. The facade owns the gating; the migration ports
//! themselves are mechanical and assume their precondition already holds.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::instrument;
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::SpaceId;
use uc_core::ports::config_migration::{
    ConfigImportPreview, ConfigMigrationError, ExportConfigBundlePort, PreviewConfigImportPort,
    StageConfigImportPort, StagedConfigImport,
};

use crate::space::current_space::{CurrentSpaceIdentityPort, PortableCurrentSpaceIdentityPort};
use crate::space::session::IsSpaceUnlockedPort;

/// Ports consumed by [`ConfigMigrationFacade`].
///
/// Three migration intent ports carry out the work; two read-only status ports
/// answer the gating questions ("is this installation initialized / unlocked?")
/// from the same sources the encryption facade uses, so there is a single truth
/// for those facts.
#[derive(Clone)]
pub struct ConfigMigrationDeps {
    /// Produces a password-protected bundle of the current configuration.
    pub export_bundle: Arc<dyn ExportConfigBundlePort>,
    /// Reads a bundle's descriptive metadata without applying it.
    pub preview_import: Arc<dyn PreviewConfigImportPort>,
    /// Records a bundle as a pending migration for the next restart.
    pub stage_import: Arc<dyn StageConfigImportPort>,
    /// Source of truth for whether this installation has completed setup
    /// (i.e. holds configuration). Shared with the encryption facade.
    pub current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    pub portable_current_space_identity: Arc<dyn PortableCurrentSpaceIdentityPort>,
    /// Reports whether the in-memory session is currently unlocked.
    pub is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
}

/// Application entry point for whole-installation configuration migration.
///
/// Enforces the migration preconditions before delegating:
/// * export requires an unlocked, initialized installation;
/// * preview is read-only and ungated;
/// * staging an import replaces whatever configuration the target currently
///   holds — there is no uninitialized-target precondition.
pub struct ConfigMigrationFacade {
    deps: ConfigMigrationDeps,
}

impl ConfigMigrationFacade {
    pub fn new(deps: ConfigMigrationDeps) -> Self {
        Self { deps }
    }

    /// Export the current installation's configuration into a self-protected
    /// bundle written to `destination`.
    ///
    /// No export secret is taken: the bundle is sealed with the installation's
    /// own key material, so reading it back later requires the space passphrase.
    ///
    /// Preconditions, checked in order before any material is read:
    /// 1. the installation must be initialized — otherwise
    ///    [`ConfigMigrationError::NotInitialized`];
    /// 2. the current session must be unlocked — otherwise
    ///    [`ConfigMigrationError::Locked`] (the unlocked session is the
    ///    authorization gate that proves the operator holds the passphrase).
    ///
    /// On success returns the path the bundle was written to.
    #[instrument(skip_all)]
    pub async fn export_config(&self, destination: &Path) -> Result<PathBuf, ConfigMigrationError> {
        let space_id = self.current_space_id().await?;
        if !self.deps.is_unlocked.is_unlocked(&space_id).await {
            return Err(ConfigMigrationError::Locked);
        }
        self.deps
            .portable_current_space_identity
            .prepare_portable_identity()
            .await
            .map_err(|error| ConfigMigrationError::Internal {
                details: format!("preparing portable current Space identity failed: {error}"),
            })?;

        self.deps.export_bundle.export_bundle(destination).await
    }

    /// Read the descriptive metadata of the bundle at `source`.
    ///
    /// Read-only and ungated: produces no side effects and does not depend on
    /// the local installation's initialized or unlocked state. Surfaces the
    /// migration port's errors (`InvalidPasswordOrCorrupt` / `IncompatibleBundle`
    /// / `Io`) unchanged.
    #[instrument(skip_all)]
    pub async fn preview_import(
        &self,
        password: &Passphrase,
        source: &Path,
    ) -> Result<ConfigImportPreview, ConfigMigrationError> {
        self.deps
            .preview_import
            .preview_import(password, source)
            .await
    }

    /// Validate the bundle at `source` and record it as a pending migration to
    /// be applied on the next restart.
    ///
    /// No initialized-target precondition: applying on the next restart replaces
    /// whatever configuration this installation currently holds. The import is a
    /// device-identity move, so the authorization to overwrite is the explicit
    /// confirmation enforced at the presentation boundary, not a gate here.
    #[instrument(skip_all)]
    pub async fn stage_import(
        &self,
        password: &Passphrase,
        source: &Path,
    ) -> Result<StagedConfigImport, ConfigMigrationError> {
        self.deps.stage_import.stage_import(password, source).await
    }

    /// Whether this installation has completed setup, i.e. holds configuration.
    ///
    /// Reads the same `has_completed` flag the encryption facade treats as the
    /// "initialized" truth, mapping a read failure to
    /// [`ConfigMigrationError::Internal`] (never surfacing secret material).
    async fn current_space_id(&self) -> Result<SpaceId, ConfigMigrationError> {
        self.deps
            .current_space_identity
            .current_space_id()
            .await
            .map_err(|err| ConfigMigrationError::Internal {
                details: format!("failed to read current Space identity: {err}"),
            })?
            .ok_or(ConfigMigrationError::NotInitialized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::ids::ProfileId;
    use uc_core::ports::config_migration::ConfigSourceMode;
    struct FakeCurrentSpace {
        space_id: Option<SpaceId>,
        portable_calls: Mutex<u32>,
    }

    impl FakeCurrentSpace {
        fn new(has_completed: bool) -> Arc<Self> {
            Arc::new(Self {
                space_id: has_completed.then(|| SpaceId::from("space")),
                portable_calls: Mutex::new(0),
            })
        }
    }

    #[async_trait]
    impl CurrentSpaceIdentityPort for FakeCurrentSpace {
        async fn current_space_id(
            &self,
        ) -> Result<Option<SpaceId>, crate::space::current_space::CurrentSpaceIdentityError>
        {
            Ok(self.space_id.clone())
        }
    }

    #[async_trait]
    impl PortableCurrentSpaceIdentityPort for FakeCurrentSpace {
        async fn prepare_portable_identity(
            &self,
        ) -> Result<(), crate::space::current_space::CurrentSpaceIdentityError> {
            *self.portable_calls.lock().expect("portable calls") += 1;
            Ok(())
        }
    }

    struct FailingCurrentSpace;

    #[async_trait]
    impl CurrentSpaceIdentityPort for FailingCurrentSpace {
        async fn current_space_id(
            &self,
        ) -> Result<Option<SpaceId>, crate::space::current_space::CurrentSpaceIdentityError>
        {
            Err(crate::space::current_space::CurrentSpaceIdentityError::Unavailable)
        }
    }

    #[async_trait]
    impl PortableCurrentSpaceIdentityPort for FailingCurrentSpace {
        async fn prepare_portable_identity(
            &self,
        ) -> Result<(), crate::space::current_space::CurrentSpaceIdentityError> {
            Ok(())
        }
    }

    struct FakeIsUnlocked {
        unlocked: bool,
    }

    #[async_trait]
    impl IsSpaceUnlockedPort for FakeIsUnlocked {
        async fn is_unlocked(&self, _space_id: &SpaceId) -> bool {
            self.unlocked
        }
    }

    /// Records each delegated call so tests can assert delegation happened only
    /// after the gates passed.
    #[derive(Default)]
    struct SpyMigrationPorts {
        export_calls: Mutex<u32>,
        preview_calls: Mutex<u32>,
        stage_calls: Mutex<u32>,
    }

    #[async_trait]
    impl ExportConfigBundlePort for SpyMigrationPorts {
        async fn export_bundle(&self, destination: &Path) -> Result<PathBuf, ConfigMigrationError> {
            *self.export_calls.lock().expect("export calls") += 1;
            Ok(destination.to_path_buf())
        }
    }

    #[async_trait]
    impl PreviewConfigImportPort for SpyMigrationPorts {
        async fn preview_import(
            &self,
            _password: &Passphrase,
            _source: &Path,
        ) -> Result<ConfigImportPreview, ConfigMigrationError> {
            *self.preview_calls.lock().expect("preview calls") += 1;
            Ok(ConfigImportPreview {
                app_version: "0.16.0".to_string(),
                source_mode: ConfigSourceMode::Portable,
                created_at_unix_ms: 1_700_000_000_000,
                profile_id: ProfileId::from("default".to_string()),
                device_fingerprint: "AB-CD-EF".to_string(),
            })
        }
    }

    #[async_trait]
    impl StageConfigImportPort for SpyMigrationPorts {
        async fn stage_import(
            &self,
            _password: &Passphrase,
            _source: &Path,
        ) -> Result<StagedConfigImport, ConfigMigrationError> {
            *self.stage_calls.lock().expect("stage calls") += 1;
            Ok(StagedConfigImport {
                unlock_required_after_apply: true,
            })
        }
    }

    fn facade_with(
        initialized: bool,
        unlocked: bool,
        ports: Arc<SpyMigrationPorts>,
    ) -> ConfigMigrationFacade {
        let current_space = FakeCurrentSpace::new(initialized);
        ConfigMigrationFacade::new(ConfigMigrationDeps {
            export_bundle: ports.clone(),
            preview_import: ports.clone(),
            stage_import: ports.clone(),
            current_space_identity: current_space.clone(),
            portable_current_space_identity: current_space,
            is_unlocked: Arc::new(FakeIsUnlocked { unlocked }),
        })
    }

    fn password() -> Passphrase {
        Passphrase::new("hunter2")
    }

    #[tokio::test]
    async fn export_refuses_when_not_initialized() {
        let ports = Arc::new(SpyMigrationPorts::default());
        let facade = facade_with(false, true, ports.clone());

        let err = facade
            .export_config(Path::new("/tmp/out.ucbundle"))
            .await
            .expect_err("export should be refused");

        assert!(matches!(err, ConfigMigrationError::NotInitialized));
        assert_eq!(*ports.export_calls.lock().expect("export calls"), 0);
    }

    #[tokio::test]
    async fn export_refuses_when_locked() {
        let ports = Arc::new(SpyMigrationPorts::default());
        let facade = facade_with(true, false, ports.clone());

        let err = facade
            .export_config(Path::new("/tmp/out.ucbundle"))
            .await
            .expect_err("export should be refused");

        assert!(matches!(err, ConfigMigrationError::Locked));
        assert_eq!(*ports.export_calls.lock().expect("export calls"), 0);
    }

    #[tokio::test]
    async fn export_delegates_when_initialized_and_unlocked() {
        let ports = Arc::new(SpyMigrationPorts::default());
        let facade = facade_with(true, true, ports.clone());

        let path = facade
            .export_config(Path::new("/tmp/out.ucbundle"))
            .await
            .expect("export should succeed");

        assert_eq!(path, PathBuf::from("/tmp/out.ucbundle"));
        assert_eq!(*ports.export_calls.lock().expect("export calls"), 1);
    }

    #[tokio::test]
    async fn export_maps_status_read_failure_to_internal() {
        let ports = Arc::new(SpyMigrationPorts::default());
        let facade = ConfigMigrationFacade::new(ConfigMigrationDeps {
            export_bundle: ports.clone(),
            preview_import: ports.clone(),
            stage_import: ports.clone(),
            current_space_identity: Arc::new(FailingCurrentSpace),
            portable_current_space_identity: Arc::new(FailingCurrentSpace),
            is_unlocked: Arc::new(FakeIsUnlocked { unlocked: true }),
        });

        let err = facade
            .export_config(Path::new("/tmp/out.ucbundle"))
            .await
            .expect_err("export should fail");

        assert!(matches!(err, ConfigMigrationError::Internal { .. }));
        assert_eq!(*ports.export_calls.lock().expect("export calls"), 0);
    }

    #[tokio::test]
    async fn preview_is_ungated_and_delegates() {
        let ports = Arc::new(SpyMigrationPorts::default());
        // Uninitialized + locked: preview must still go through.
        let facade = facade_with(false, false, ports.clone());

        let preview = facade
            .preview_import(&password(), Path::new("/tmp/in.ucbundle"))
            .await
            .expect("preview should succeed");

        assert_eq!(preview.source_mode, ConfigSourceMode::Portable);
        assert_eq!(*ports.preview_calls.lock().expect("preview calls"), 1);
    }

    #[tokio::test]
    async fn stage_proceeds_even_when_already_initialized() {
        // Import is a replace: an already-initialized target is no longer a
        // gate. Staging delegates regardless of initialized state; the boot-time
        // apply overwrites the existing configuration.
        let ports = Arc::new(SpyMigrationPorts::default());
        let facade = facade_with(true, true, ports.clone());

        let staged = facade
            .stage_import(&password(), Path::new("/tmp/in.ucbundle"))
            .await
            .expect("stage should proceed and replace");

        assert!(staged.unlock_required_after_apply);
        assert_eq!(*ports.stage_calls.lock().expect("stage calls"), 1);
    }

    #[tokio::test]
    async fn stage_delegates_when_uninitialized() {
        let ports = Arc::new(SpyMigrationPorts::default());
        let facade = facade_with(false, false, ports.clone());

        let staged = facade
            .stage_import(&password(), Path::new("/tmp/in.ucbundle"))
            .await
            .expect("stage should succeed");

        assert!(staged.unlock_required_after_apply);
        assert_eq!(*ports.stage_calls.lock().expect("stage calls"), 1);
    }
}
