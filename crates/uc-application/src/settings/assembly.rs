//! Settings 领域对象图装配。
//!
//! Engine 只选择 relay 等具体 adapter；设置、诊断、存储、配置迁移、
//! 升级检测与启动期 relay 恢复的组合由本模块唯一持有。

use std::sync::Arc;

use uc_core::app_dirs::AppPaths;
use uc_core::settings::model::CongestionController;

use crate::deps::ApplicationDeps;

use super::config_migration::ConfigMigrationFacade;
use super::diagnostics::{DiagnosticsFacade, DiagnosticsFacadeDeps};
use super::relay_credentials::RelayCredentials;
use super::relay_diagnostic::RelayDiagnosticPort;
use super::storage::{StorageFacade, StorageFacadeDeps};
use super::upgrade::{UpgradeFacade, UpgradeFacadeDeps};
use super::{SettingsFacade, SettingsFacadeError};

pub struct PreparedNetworkSettings {
    pub allow_relay_fallback: bool,
    pub allow_overlay_network_addrs: bool,
    pub custom_relay_urls: Vec<String>,
    pub congestion_controller: CongestionController,
    pub relay_credentials: RelayCredentials,
}

pub struct SettingsAssemblyParts {
    pub settings: Arc<SettingsFacade>,
    pub diagnostics: Arc<DiagnosticsFacade>,
    pub storage: Arc<StorageFacade>,
    pub config_migration: Arc<ConfigMigrationFacade>,
    pub upgrade: Arc<UpgradeFacade>,
}

#[derive(Clone)]
pub struct SettingsAssembly {
    settings: Arc<SettingsFacade>,
    diagnostics: Arc<DiagnosticsFacade>,
    storage: Arc<StorageFacade>,
    config_migration: Arc<ConfigMigrationFacade>,
    upgrade: Arc<UpgradeFacade>,
    relay_credentials: RelayCredentials,
}

impl SettingsAssembly {
    pub fn build(
        deps: &ApplicationDeps,
        paths: &AppPaths,
        relay_diagnostic: Option<Arc<dyn RelayDiagnosticPort>>,
    ) -> Self {
        let relay_credentials = RelayCredentials::new(Arc::clone(&deps.security.secure_storage));
        let mut settings = SettingsFacade::new(Arc::clone(&deps.settings))
            .with_relay_credentials(relay_credentials.clone());
        if let Some(relay_diagnostic) = relay_diagnostic {
            settings = settings.with_relay_diagnostic(relay_diagnostic);
        }

        Self {
            settings: Arc::new(settings),
            diagnostics: Arc::new(DiagnosticsFacade::new(DiagnosticsFacadeDeps {
                settings: Arc::clone(&deps.settings),
                logs_dir: paths.logs_dir.clone(),
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
            })),
            storage: Arc::new(StorageFacade::new(StorageFacadeDeps {
                db_path: paths.db_path.clone(),
                vault_dir: paths.vault_dir.clone(),
                cache_dir: paths.cache_dir.clone(),
                logs_dir: paths.logs_dir.clone(),
                app_data_root_dir: paths.app_data_root_dir.clone(),
                cache_fs: Arc::clone(&deps.system.cache_fs),
            })),
            config_migration: Arc::new(ConfigMigrationFacade::new(deps.config_migration.clone())),
            upgrade: Arc::new(UpgradeFacade::new(UpgradeFacadeDeps {
                app_version_state: Arc::clone(&deps.app_version_state),
                current_space_identity: Arc::clone(&deps.current_space_identity),
            })),
            relay_credentials,
        }
    }

    pub async fn prepare_network(&self) -> Result<PreparedNetworkSettings, SettingsFacadeError> {
        let settings = self.settings.prepare_network_settings().await?;
        Ok(PreparedNetworkSettings {
            allow_relay_fallback: settings.network.allow_relay_fallback,
            allow_overlay_network_addrs: settings.network.allow_overlay_network_addrs,
            custom_relay_urls: settings.network.custom_relay_urls,
            congestion_controller: settings.network.congestion_controller,
            relay_credentials: self.relay_credentials.clone(),
        })
    }

    pub fn upgrade(&self) -> Arc<UpgradeFacade> {
        Arc::clone(&self.upgrade)
    }

    pub fn into_parts(self) -> SettingsAssemblyParts {
        SettingsAssemblyParts {
            settings: self.settings,
            diagnostics: self.diagnostics,
            storage: self.storage,
            config_migration: self.config_migration,
            upgrade: self.upgrade,
        }
    }
}
