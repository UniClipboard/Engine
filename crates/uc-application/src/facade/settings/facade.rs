use std::sync::Arc;

use tracing::instrument;

use uc_core::ports::SettingsPort;

use crate::facade::settings::models::{
    apply_settings_patch, validate_settings, SettingsPatch, SettingsView,
};
use crate::facade::settings::relay_diagnostic::{
    RelayDiagnosticPort, RelayProbeError, RelayProbeReport,
};
use crate::facade::settings::{
    RelayCredentialEdit, RelayCredentials, RelayCredentialsError, RelayProbeCredential,
};

#[derive(Debug, thiserror::Error)]
pub enum SettingsFacadeError {
    #[error("failed to load settings: {0}")]
    Load(String),
    #[error("failed to save settings: {0}")]
    Save(String),
    #[error("invalid settings: {0}")]
    Invalid(String),
    /// Relay 探测能力未在本进程装配。常见于 webserver / 单元测试场景。
    #[error("relay probe is unavailable in this runtime")]
    RelayProbeUnavailable,
    #[error("invalid relay URL: {0}")]
    RelayProbeInvalidUrl(String),
    #[error("dns lookup failed: {0}")]
    RelayProbeDns(String),
    #[error("tls handshake failed: {0}")]
    RelayProbeTls(String),
    #[error("relay handshake failed: {0}")]
    RelayProbeHandshake(String),
    #[error("relay probe timed out")]
    RelayProbeTimeout,
    #[error("relay probe failed: {0}")]
    RelayProbeOther(String),
    #[error("relay credential storage is unavailable")]
    RelayCredentialsUnavailable,
    #[error("invalid relay credential URL")]
    RelayCredentialInvalidUrl,
    #[error("invalid relay access token")]
    RelayCredentialInvalidToken,
    #[error("relay credential URL does not match the saved relay settings")]
    RelayCredentialInvalidTarget,
    #[error("relay credential storage failed")]
    RelayCredentialStorage,
    #[error("stored relay credential is corrupt")]
    RelayCredentialCorrupt,
}

/// 应用层暴露的中继探测结果视图。沿用核心层的字段语义,但与 core 类型解耦,
/// 上层(daemon / tauri / cli)只需要消费此类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayProbeReportView {
    pub latency_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayCredentialStatusView {
    pub configured: bool,
}

#[derive(Debug, Clone)]
pub struct RelaySaveView {
    pub settings: SettingsView,
    pub credential_status: RelayCredentialStatusView,
}

impl From<RelayProbeReport> for RelayProbeReportView {
    fn from(value: RelayProbeReport) -> Self {
        Self {
            latency_ms: value.latency_ms,
        }
    }
}

impl From<RelayProbeError> for SettingsFacadeError {
    fn from(value: RelayProbeError) -> Self {
        match value {
            RelayProbeError::InvalidUrl(msg) => SettingsFacadeError::RelayProbeInvalidUrl(msg),
            RelayProbeError::Dns(msg) => SettingsFacadeError::RelayProbeDns(msg),
            RelayProbeError::Tls(msg) => SettingsFacadeError::RelayProbeTls(msg),
            RelayProbeError::Handshake(msg) => SettingsFacadeError::RelayProbeHandshake(msg),
            RelayProbeError::Timeout => SettingsFacadeError::RelayProbeTimeout,
            RelayProbeError::Other(msg) => SettingsFacadeError::RelayProbeOther(msg),
        }
    }
}

impl From<RelayCredentialsError> for SettingsFacadeError {
    fn from(value: RelayCredentialsError) -> Self {
        match value {
            RelayCredentialsError::InvalidRelayUrl => Self::RelayCredentialInvalidUrl,
            RelayCredentialsError::InvalidToken => Self::RelayCredentialInvalidToken,
            RelayCredentialsError::InvalidTarget => Self::RelayCredentialInvalidTarget,
            RelayCredentialsError::Storage(_) => Self::RelayCredentialStorage,
            RelayCredentialsError::Corrupt => Self::RelayCredentialCorrupt,
        }
    }
}

pub struct SettingsFacade {
    settings: Arc<dyn SettingsPort>,
    relay_diagnostic: Option<Arc<dyn RelayDiagnosticPort>>,
    relay_credentials: Option<RelayCredentials>,
    mutation_gate: tokio::sync::Mutex<()>,
}

impl SettingsFacade {
    pub fn new(settings: Arc<dyn SettingsPort>) -> Self {
        Self {
            settings,
            relay_diagnostic: None,
            relay_credentials: None,
            mutation_gate: tokio::sync::Mutex::new(()),
        }
    }

    /// 注入中继诊断端口。Production daemon 会通过 bootstrap 调用,
    /// webserver / 单元测试可以不装配,此时 [`Self::probe_relay_url`]
    /// 会返回 [`SettingsFacadeError::RelayProbeUnavailable`]。
    pub fn with_relay_diagnostic(mut self, port: Arc<dyn RelayDiagnosticPort>) -> Self {
        self.relay_diagnostic = Some(port);
        self
    }

    pub fn with_relay_credentials(mut self, credentials: RelayCredentials) -> Self {
        self.relay_credentials = Some(credentials);
        self
    }

    pub fn relay_credential_status(
        &self,
        url: &str,
    ) -> Result<RelayCredentialStatusView, SettingsFacadeError> {
        let credentials = self
            .relay_credentials
            .as_ref()
            .ok_or(SettingsFacadeError::RelayCredentialsUnavailable)?;
        Ok(RelayCredentialStatusView {
            configured: credentials.is_configured(url)?,
        })
    }

    /// 对一个候选中继 URL 发起一次可达性探测。
    ///
    /// 不读取也不修改任何已持久化的设置,允许重复调用。失败时把领域错误
    /// 翻译到 [`SettingsFacadeError`] 的细分变体,便于上层做有针对性的
    /// 用户提示。
    #[instrument(name = "settings.probe_relay", level = "info", skip_all)]
    pub async fn probe_relay_url(
        &self,
        url: &str,
        credential: RelayProbeCredential,
    ) -> Result<RelayProbeReportView, SettingsFacadeError> {
        let port = self
            .relay_diagnostic
            .as_ref()
            .ok_or(SettingsFacadeError::RelayProbeUnavailable)?;
        let access_token = match credential {
            RelayProbeCredential::Override(token) => Some(token),
            RelayProbeCredential::None => None,
            RelayProbeCredential::Stored => match self.relay_credentials.as_ref() {
                Some(credentials) => credentials.load(url)?,
                None => None,
            },
        };
        let report = port.probe(url, access_token.as_ref()).await?;
        Ok(report.into())
    }

    #[instrument(skip_all)]
    pub async fn get(&self) -> Result<SettingsView, SettingsFacadeError> {
        self.settings
            .load()
            .await
            .map(SettingsView::from)
            .map_err(|err| SettingsFacadeError::Load(err.to_string()))
    }

    #[instrument(skip_all)]
    pub async fn update(&self, patch: SettingsPatch) -> Result<SettingsView, SettingsFacadeError> {
        let result = self.persist_update(patch, None).await.map(|saved| saved.0);
        tracing::debug!(success = result.is_ok(), "settings update completed");
        result
    }

    #[instrument(name = "settings.save_relay", level = "info", skip_all)]
    pub async fn save_relay(
        &self,
        patch: SettingsPatch,
        edit: RelayCredentialEdit,
    ) -> Result<RelaySaveView, SettingsFacadeError> {
        let result = self.persist_update(patch, Some(&edit)).await.map(
            |(settings, configured_before_save)| RelaySaveView {
                settings,
                credential_status: RelayCredentialStatusView {
                    configured: edit.configured_after_save(configured_before_save),
                },
            },
        );
        tracing::info!(success = result.is_ok(), "relay settings save completed");
        result
    }

    async fn persist_update(
        &self,
        patch: SettingsPatch,
        edit: Option<&RelayCredentialEdit>,
    ) -> Result<(SettingsView, bool), SettingsFacadeError> {
        let _mutation_guard = self.mutation_gate.lock().await;
        let existing = self
            .settings
            .load()
            .await
            .map_err(|err| SettingsFacadeError::Load(err.to_string()))?;
        let previous_relay_urls = existing.network.custom_relay_urls.clone();
        let merged = apply_settings_patch(existing, patch);
        validate_settings(&merged).map_err(SettingsFacadeError::Invalid)?;
        let (credential_restore, configured_before_save) = match self.relay_credentials.as_ref() {
            Some(credentials) => {
                let configured = match edit {
                    Some(edit @ RelayCredentialEdit::Keep { .. }) => {
                        credentials.is_configured(edit.url())?
                    }
                    Some(RelayCredentialEdit::Set { .. })
                    | Some(RelayCredentialEdit::Delete { .. })
                    | None => false,
                };
                let restore = credentials.apply_settings_edit(
                    &previous_relay_urls,
                    &merged.network.custom_relay_urls,
                    edit,
                )?;
                (Some(restore), configured)
            }
            None if edit.is_some() => return Err(SettingsFacadeError::RelayCredentialsUnavailable),
            None => (None, false),
        };
        if let Err(error) = self.settings.save(&merged).await {
            if let (Some(credentials), Some(restore_point)) =
                (self.relay_credentials.as_ref(), credential_restore.as_ref())
            {
                credentials.restore(restore_point)?;
            }
            return Err(SettingsFacadeError::Save(error.to_string()));
        }
        Ok((merged.into(), configured_before_save))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };
    use tokio::{sync::Notify, time::Duration};
    use uc_core::ports::{SecureStorageError, SecureStoragePort};
    use uc_core::settings::model::Settings;

    #[derive(Default)]
    struct InMemorySecureStorage {
        values: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl SecureStoragePort for InMemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FailingDeleteSecureStorage {
        values: Mutex<BTreeMap<String, Vec<u8>>>,
        fail_delete: AtomicBool,
        fail_on_delete: AtomicUsize,
        delete_count: AtomicUsize,
        fail_on_set: AtomicUsize,
        set_count: AtomicUsize,
    }

    struct InterleavingSettings {
        settings: Mutex<Settings>,
        load_count: AtomicUsize,
        first_save_started: Notify,
        release_first_save: Notify,
        second_load_started: Notify,
        save_count: AtomicUsize,
    }

    #[async_trait]
    impl SettingsPort for InterleavingSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            let load_count = self.load_count.fetch_add(1, Ordering::SeqCst) + 1;
            if load_count == 2 {
                self.second_load_started.notify_one();
            }
            Ok(self.settings.lock().unwrap().clone())
        }

        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            let save_count = self.save_count.fetch_add(1, Ordering::SeqCst) + 1;
            if save_count == 1 {
                self.first_save_started.notify_one();
                self.release_first_save.notified().await;
            }
            *self.settings.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    impl SecureStoragePort for FailingDeleteSecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            let set_count = self.set_count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_on_set.load(Ordering::SeqCst) == set_count {
                return Err(SecureStorageError::Unavailable(
                    "credential store unavailable".to_string(),
                ));
            }
            self.values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            let delete_count = self.delete_count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_delete.load(Ordering::SeqCst)
                || self.fail_on_delete.load(Ordering::SeqCst) == delete_count
            {
                return Err(SecureStorageError::Unavailable(
                    "credential store unavailable".to_string(),
                ));
            }
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingRelayDiagnostic {
        token: Mutex<Option<String>>,
    }

    #[async_trait]
    impl RelayDiagnosticPort for RecordingRelayDiagnostic {
        async fn probe(
            &self,
            _url: &str,
            access_token: Option<&crate::facade::settings::RelayAccessToken>,
        ) -> Result<RelayProbeReport, RelayProbeError> {
            *self.token.lock().unwrap() =
                access_token.map(|token| token.expose_secret().to_string());
            Ok(RelayProbeReport { latency_ms: 1 })
        }
    }

    struct InMemorySettings {
        settings: Mutex<Settings>,
        fail_save: bool,
    }

    #[async_trait]
    impl SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.settings.lock().unwrap().clone())
        }

        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            if self.fail_save {
                anyhow::bail!("disk full");
            }
            *self.settings.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

    fn facade_with(settings: Settings) -> SettingsFacade {
        SettingsFacade::new(Arc::new(InMemorySettings {
            settings: Mutex::new(settings),
            fail_save: false,
        }))
    }

    fn seed_relay_credential(facade: &SettingsFacade, url: &str, token: &str) {
        let token = crate::facade::settings::RelayAccessToken::new(token.to_string())
            .expect("valid relay token");
        facade
            .relay_credentials
            .as_ref()
            .expect("relay credentials")
            .set(url, &token)
            .expect("seed relay credential");
    }

    #[tokio::test]
    async fn update_merges_general_and_sync_patch_without_exposing_core_model() {
        let mut seed = Settings::default();
        seed.general.device_name = Some("old".to_string());
        seed.sync.content_types.image = true;

        let facade = facade_with(seed);
        let view = facade
            .update(SettingsPatch {
                general: Some(crate::facade::settings::GeneralSettingsPatch {
                    device_name: Some(Some("new".to_string())),
                    ..Default::default()
                }),
                sync: Some(crate::facade::settings::SyncSettingsPatch {
                    content_types: Some(crate::facade::settings::ContentTypesPatch {
                        text: Some(false),
                        image: None,
                        link: None,
                        file: None,
                        code_snippet: None,
                        rich_text: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect("settings update ok");

        assert_eq!(view.general.device_name.as_deref(), Some("new"));
        assert!(!view.sync.content_types.text);
        assert!(view.sync.content_types.image);
    }

    #[tokio::test]
    async fn update_surfaces_save_failure() {
        let facade = SettingsFacade::new(Arc::new(InMemorySettings {
            settings: Mutex::new(Settings::default()),
            fail_save: true,
        }));

        let err = facade.update(SettingsPatch::default()).await.unwrap_err();
        assert!(matches!(err, SettingsFacadeError::Save(_)));
    }

    #[tokio::test]
    async fn update_rejects_invalid_custom_relay_url() {
        let facade = facade_with(Settings::default());
        let err = facade
            .update(SettingsPatch {
                network: Some(crate::facade::settings::NetworkSettingsPatch {
                    custom_relay_urls: Some(vec!["ftp://relay.example.com".to_string()]),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap_err();

        assert!(matches!(err, SettingsFacadeError::Invalid(_)));
    }

    #[tokio::test]
    async fn removing_a_custom_relay_deletes_only_its_stored_credential() {
        let relay_a = "https://relay-a.example.com/";
        let relay_b = "https://relay-b.example.com/";
        let mut settings = Settings::default();
        settings.network.custom_relay_urls = vec![relay_a.to_string(), relay_b.to_string()];
        let credentials = crate::facade::settings::RelayCredentials::new(Arc::new(
            InMemorySecureStorage::default(),
        ));
        let facade = facade_with(settings).with_relay_credentials(credentials);
        seed_relay_credential(&facade, relay_a, "relay-a-token");
        seed_relay_credential(&facade, relay_b, "relay-b-token");

        facade
            .update(SettingsPatch {
                network: Some(crate::facade::settings::NetworkSettingsPatch {
                    custom_relay_urls: Some(vec![relay_b.to_string()]),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect("remove relay A");

        assert!(
            !facade
                .relay_credential_status(relay_a)
                .expect("query relay A credential")
                .configured
        );
        assert!(
            facade
                .relay_credential_status(relay_b)
                .expect("query relay B credential")
                .configured
        );
    }

    #[tokio::test]
    async fn credential_cleanup_failure_keeps_the_previous_settings() {
        let relay = "https://relay.example.com/";
        let mut seed = Settings::default();
        seed.network.custom_relay_urls = vec![relay.to_string()];
        let settings = Arc::new(InMemorySettings {
            settings: Mutex::new(seed),
            fail_save: false,
        });
        let storage = Arc::new(FailingDeleteSecureStorage::default());
        let facade = SettingsFacade::new(settings.clone()).with_relay_credentials(
            crate::facade::settings::RelayCredentials::new(storage.clone()),
        );
        seed_relay_credential(&facade, relay, "relay-token");
        storage.fail_delete.store(true, Ordering::SeqCst);

        let error = facade
            .update(SettingsPatch {
                network: Some(crate::facade::settings::NetworkSettingsPatch {
                    custom_relay_urls: Some(Vec::new()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect_err("credential cleanup must fail");

        assert!(matches!(error, SettingsFacadeError::RelayCredentialStorage));
        assert_eq!(
            settings
                .load()
                .await
                .expect("load settings")
                .network
                .custom_relay_urls,
            vec![relay.to_string()]
        );
    }

    #[tokio::test]
    async fn partial_credential_cleanup_failure_restores_every_credential() {
        let relay_a = "https://relay-a.example.com/";
        let relay_b = "https://relay-b.example.com/";
        let mut seed = Settings::default();
        seed.network.custom_relay_urls = vec![relay_a.to_string(), relay_b.to_string()];
        let settings = Arc::new(InMemorySettings {
            settings: Mutex::new(seed),
            fail_save: false,
        });
        let storage = Arc::new(FailingDeleteSecureStorage::default());
        let facade = SettingsFacade::new(settings.clone()).with_relay_credentials(
            crate::facade::settings::RelayCredentials::new(storage.clone()),
        );
        seed_relay_credential(&facade, relay_a, "relay-a-token");
        seed_relay_credential(&facade, relay_b, "relay-b-token");
        storage.fail_on_delete.store(2, Ordering::SeqCst);

        facade
            .update(SettingsPatch {
                network: Some(crate::facade::settings::NetworkSettingsPatch {
                    custom_relay_urls: Some(Vec::new()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect_err("second credential deletion must fail");

        storage.fail_on_delete.store(0, Ordering::SeqCst);
        assert_eq!(
            settings
                .load()
                .await
                .expect("load settings")
                .network
                .custom_relay_urls,
            vec![relay_a.to_string(), relay_b.to_string()]
        );
        assert!(
            facade
                .relay_credential_status(relay_a)
                .expect("query relay A credential")
                .configured
        );
        assert!(
            facade
                .relay_credential_status(relay_b)
                .expect("query relay B credential")
                .configured
        );
    }

    #[tokio::test]
    async fn credential_rollback_continues_after_one_restore_fails() {
        let relay_a = "https://relay-a.example.com/";
        let relay_b = "https://relay-b.example.com/";
        let mut seed = Settings::default();
        seed.network.custom_relay_urls = vec![relay_a.to_string(), relay_b.to_string()];
        let settings = Arc::new(InMemorySettings {
            settings: Mutex::new(seed),
            fail_save: false,
        });
        let storage = Arc::new(FailingDeleteSecureStorage::default());
        let facade = SettingsFacade::new(settings).with_relay_credentials(
            crate::facade::settings::RelayCredentials::new(storage.clone()),
        );
        seed_relay_credential(&facade, relay_a, "relay-a-token");
        seed_relay_credential(&facade, relay_b, "relay-b-token");
        storage.set_count.store(0, Ordering::SeqCst);
        storage.fail_on_delete.store(2, Ordering::SeqCst);
        storage.fail_on_set.store(1, Ordering::SeqCst);

        facade
            .update(SettingsPatch {
                network: Some(crate::facade::settings::NetworkSettingsPatch {
                    custom_relay_urls: Some(Vec::new()),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .expect_err("credential cleanup and first restore must fail");

        storage.fail_on_set.store(0, Ordering::SeqCst);
        let configured = [relay_a, relay_b]
            .into_iter()
            .filter(|relay| {
                facade
                    .relay_credential_status(relay)
                    .expect("query relay credential")
                    .configured
            })
            .count();
        assert_eq!(configured, 2, "all possible restores must be attempted");
    }

    #[tokio::test]
    async fn credential_status_never_returns_the_token() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let facade = facade_with(Settings::default())
            .with_relay_credentials(crate::facade::settings::RelayCredentials::new(storage));

        assert!(
            !facade
                .relay_credential_status("https://relay.example.com")
                .expect("query status")
                .configured
        );
        let relay = "https://relay.example.com/";
        let settings = SettingsPatch {
            network: Some(crate::facade::settings::NetworkSettingsPatch {
                custom_relay_urls: Some(vec![relay.to_string()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let saved = facade
            .save_relay(
                settings.clone(),
                RelayCredentialEdit::Set {
                    url: relay.to_string(),
                    access_token: crate::facade::settings::RelayAccessToken::new(
                        "top-secret-token".to_string(),
                    )
                    .expect("valid relay token"),
                },
            )
            .await
            .expect("save credential");
        assert!(saved.credential_status.configured);
        assert!(
            facade
                .relay_credential_status(relay)
                .expect("query saved credential")
                .configured
        );

        let saved = facade
            .save_relay(
                settings,
                RelayCredentialEdit::Delete {
                    url: relay.to_string(),
                },
            )
            .await
            .expect("delete credential");
        assert!(!saved.credential_status.configured);
    }

    #[tokio::test]
    async fn relay_save_restores_the_previous_token_when_settings_persistence_fails() {
        let relay = "https://relay.example.com/";
        let mut seed = Settings::default();
        seed.network.custom_relay_urls = vec![relay.to_string()];
        let settings = Arc::new(InMemorySettings {
            settings: Mutex::new(seed),
            fail_save: true,
        });
        let credentials = crate::facade::settings::RelayCredentials::new(Arc::new(
            InMemorySecureStorage::default(),
        ));
        let facade = SettingsFacade::new(settings).with_relay_credentials(credentials);
        seed_relay_credential(&facade, relay, "old-token");

        let error = facade
            .save_relay(
                SettingsPatch {
                    network: Some(crate::facade::settings::NetworkSettingsPatch {
                        custom_relay_urls: Some(vec![relay.to_string()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                crate::facade::settings::RelayCredentialEdit::Set {
                    url: relay.to_string(),
                    access_token: crate::facade::settings::RelayAccessToken::new(
                        "replacement-token".to_string(),
                    )
                    .expect("valid replacement token"),
                },
            )
            .await
            .expect_err("settings persistence must fail");

        assert!(matches!(error, SettingsFacadeError::Save(_)));
        let stored = facade
            .relay_credentials
            .as_ref()
            .expect("relay credentials")
            .load(relay)
            .expect("load stored credential")
            .expect("stored credential");
        assert_eq!(stored.expose_secret(), "old-token");
    }

    #[tokio::test]
    async fn relay_save_deletes_a_token_and_updates_settings_together() {
        let relay = "https://relay.example.com/";
        let mut seed = Settings::default();
        seed.network.custom_relay_urls = vec![relay.to_string()];
        let facade = facade_with(seed).with_relay_credentials(
            crate::facade::settings::RelayCredentials::new(Arc::new(
                InMemorySecureStorage::default(),
            )),
        );
        seed_relay_credential(&facade, relay, "old-token");

        let saved = facade
            .save_relay(
                SettingsPatch {
                    network: Some(crate::facade::settings::NetworkSettingsPatch {
                        custom_relay_urls: Some(vec![relay.to_string()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                crate::facade::settings::RelayCredentialEdit::Delete {
                    url: relay.to_string(),
                },
            )
            .await
            .expect("save relay without a credential");

        assert!(!saved.credential_status.configured);
        assert_eq!(
            saved.settings.network.custom_relay_urls,
            vec![relay.to_string()]
        );
    }

    #[tokio::test]
    async fn concurrent_relay_and_settings_saves_preserve_both_changes() {
        let relay = "https://relay.example.com/";
        let settings = Arc::new(InterleavingSettings {
            settings: Mutex::new(Settings::default()),
            load_count: AtomicUsize::new(0),
            first_save_started: Notify::new(),
            release_first_save: Notify::new(),
            second_load_started: Notify::new(),
            save_count: AtomicUsize::new(0),
        });
        let facade = Arc::new(
            SettingsFacade::new(settings.clone()).with_relay_credentials(
                crate::facade::settings::RelayCredentials::new(Arc::new(
                    InMemorySecureStorage::default(),
                )),
            ),
        );

        let relay_save = {
            let facade = facade.clone();
            tokio::spawn(async move {
                facade
                    .save_relay(
                        SettingsPatch {
                            network: Some(crate::facade::settings::NetworkSettingsPatch {
                                custom_relay_urls: Some(vec![relay.to_string()]),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        RelayCredentialEdit::Keep {
                            url: relay.to_string(),
                        },
                    )
                    .await
            })
        };
        settings.first_save_started.notified().await;

        let settings_save = {
            let facade = facade.clone();
            tokio::spawn(async move {
                facade
                    .update(SettingsPatch {
                        network: Some(crate::facade::settings::NetworkSettingsPatch {
                            allow_overlay_network_addrs: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                    .await
            })
        };
        let _ = tokio::time::timeout(
            Duration::from_millis(100),
            settings.second_load_started.notified(),
        )
        .await;
        settings.release_first_save.notify_one();

        relay_save
            .await
            .expect("relay save task")
            .expect("save relay");
        settings_save
            .await
            .expect("settings save task")
            .expect("save settings");
        let persisted = settings.load().await.expect("load final settings");
        assert_eq!(persisted.network.custom_relay_urls, vec![relay]);
        assert!(persisted.network.allow_overlay_network_addrs);
    }

    #[tokio::test]
    async fn relay_probe_can_explicitly_ignore_a_stored_token() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = crate::facade::settings::RelayCredentials::new(storage);
        let diagnostic = Arc::new(RecordingRelayDiagnostic::default());
        let facade = facade_with(Settings::default())
            .with_relay_credentials(credentials)
            .with_relay_diagnostic(diagnostic.clone());
        seed_relay_credential(&facade, "https://relay.example.com", "stored-token");

        facade
            .probe_relay_url(
                "https://relay.example.com",
                crate::facade::settings::RelayProbeCredential::None,
            )
            .await
            .expect("probe without credential");

        assert_eq!(*diagnostic.token.lock().unwrap(), None);
    }

    #[tokio::test]
    async fn relay_probe_prefers_the_unsaved_token_then_falls_back_to_the_stored_token() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = crate::facade::settings::RelayCredentials::new(storage);
        let diagnostic = Arc::new(RecordingRelayDiagnostic::default());
        let facade = facade_with(Settings::default())
            .with_relay_credentials(credentials)
            .with_relay_diagnostic(diagnostic.clone());
        seed_relay_credential(&facade, "https://relay-a.example.com", "relay-a-token");

        facade
            .probe_relay_url(
                "https://relay-b.example.com",
                crate::facade::settings::RelayProbeCredential::Stored,
            )
            .await
            .expect("probe relay without credential");
        assert_eq!(*diagnostic.token.lock().unwrap(), None);

        facade
            .probe_relay_url(
                "https://relay-a.example.com",
                crate::facade::settings::RelayProbeCredential::Override(
                    crate::facade::settings::RelayAccessToken::new("draft-token".to_string())
                        .expect("valid draft token"),
                ),
            )
            .await
            .expect("probe relay with unsaved credential");
        assert_eq!(
            diagnostic.token.lock().unwrap().as_deref(),
            Some("draft-token")
        );

        facade
            .probe_relay_url(
                "https://relay-a.example.com",
                crate::facade::settings::RelayProbeCredential::Stored,
            )
            .await
            .expect("probe relay with stored credential");
        assert_eq!(
            diagnostic.token.lock().unwrap().as_deref(),
            Some("relay-a-token")
        );
    }
}
