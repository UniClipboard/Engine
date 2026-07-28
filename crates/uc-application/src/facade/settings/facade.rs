use std::sync::Arc;

use tracing::instrument;

use uc_core::ports::SettingsPort;

use crate::facade::settings::models::{
    apply_settings_patch, validate_settings, SettingsPatch, SettingsView,
};
use crate::facade::settings::relay_diagnostic::{
    RelayDiagnosticPort, RelayProbeError, RelayProbeReport,
};
use crate::facade::settings::{RelayAccessToken, RelayCredentials, RelayCredentialsError};

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
            RelayCredentialsError::Storage(_) => Self::RelayCredentialStorage,
            RelayCredentialsError::Corrupt => Self::RelayCredentialCorrupt,
        }
    }
}

pub struct SettingsFacade {
    settings: Arc<dyn SettingsPort>,
    relay_diagnostic: Option<Arc<dyn RelayDiagnosticPort>>,
    relay_credentials: Option<RelayCredentials>,
}

impl SettingsFacade {
    pub fn new(settings: Arc<dyn SettingsPort>) -> Self {
        Self {
            settings,
            relay_diagnostic: None,
            relay_credentials: None,
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
            configured: credentials.load(url)?.is_some(),
        })
    }

    pub fn set_relay_access_token(
        &self,
        url: &str,
        token: String,
    ) -> Result<RelayCredentialStatusView, SettingsFacadeError> {
        let credentials = self
            .relay_credentials
            .as_ref()
            .ok_or(SettingsFacadeError::RelayCredentialsUnavailable)?;
        let token = RelayAccessToken::new(token)?;
        credentials.set(url, &token)?;
        Ok(RelayCredentialStatusView { configured: true })
    }

    pub fn delete_relay_access_token(&self, url: &str) -> Result<bool, SettingsFacadeError> {
        let credentials = self
            .relay_credentials
            .as_ref()
            .ok_or(SettingsFacadeError::RelayCredentialsUnavailable)?;
        credentials.delete(url).map_err(Into::into)
    }

    /// 对一个候选中继 URL 发起一次可达性探测。
    ///
    /// 不读取也不修改任何已持久化的设置,允许重复调用。失败时把领域错误
    /// 翻译到 [`SettingsFacadeError`] 的细分变体,便于上层做有针对性的
    /// 用户提示。
    #[instrument(skip(self), fields(relay_url = %url))]
    pub async fn probe_relay_url(
        &self,
        url: &str,
    ) -> Result<RelayProbeReportView, SettingsFacadeError> {
        let port = self
            .relay_diagnostic
            .as_ref()
            .ok_or(SettingsFacadeError::RelayProbeUnavailable)?;
        let access_token = match self.relay_credentials.as_ref() {
            Some(credentials) => credentials.load(url)?,
            None => None,
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
        let existing = self
            .settings
            .load()
            .await
            .map_err(|err| SettingsFacadeError::Load(err.to_string()))?;
        let previous_relay_urls = existing.network.custom_relay_urls.clone();
        let merged = apply_settings_patch(existing, patch);
        validate_settings(&merged).map_err(SettingsFacadeError::Invalid)?;
        self.settings
            .save(&merged)
            .await
            .map_err(|err| SettingsFacadeError::Save(err.to_string()))?;
        if let Some(credentials) = self.relay_credentials.as_ref() {
            credentials
                .delete_unreferenced(&previous_relay_urls, &merged.network.custom_relay_urls)?;
        }
        Ok(merged.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use std::{collections::BTreeMap, sync::Mutex};
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
        facade
            .set_relay_access_token(relay_a, "relay-a-token".to_string())
            .expect("set relay A credential");
        facade
            .set_relay_access_token(relay_b, "relay-b-token".to_string())
            .expect("set relay B credential");

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
        let status = facade
            .set_relay_access_token("https://relay.example.com", "top-secret-token".to_string())
            .expect("set credential");
        assert!(status.configured);
        assert!(facade
            .delete_relay_access_token("https://relay.example.com")
            .expect("delete credential"));
    }

    #[tokio::test]
    async fn relay_probe_uses_only_the_target_relay_credential() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = crate::facade::settings::RelayCredentials::new(storage);
        let diagnostic = Arc::new(RecordingRelayDiagnostic::default());
        let facade = facade_with(Settings::default())
            .with_relay_credentials(credentials)
            .with_relay_diagnostic(diagnostic.clone());
        facade
            .set_relay_access_token("https://relay-a.example.com", "relay-a-token".to_string())
            .expect("set credential");

        facade
            .probe_relay_url("https://relay-b.example.com")
            .await
            .expect("probe relay without credential");
        assert_eq!(*diagnostic.token.lock().unwrap(), None);

        facade
            .probe_relay_url("https://relay-a.example.com")
            .await
            .expect("probe relay with credential");
        assert_eq!(
            diagnostic.token.lock().unwrap().as_deref(),
            Some("relay-a-token")
        );
    }
}
