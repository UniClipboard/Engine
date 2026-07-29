use std::sync::Arc;

use tokio::sync::Mutex;
use uc_core::{ports::SettingsPort, settings::model::Settings};

use super::{
    models::{apply_settings_patch, validate_settings, SettingsPatch},
    RelayAccessToken, RelayCredentialEdit, RelayCredentials, RelayCredentialsError,
};

#[derive(Debug, thiserror::Error)]
pub enum RelayConfigurationError {
    #[error("failed to load settings: {0}")]
    Load(String),
    #[error("failed to save settings: {0}")]
    Save(String),
    #[error("invalid settings: {0}")]
    Invalid(String),
    #[error("relay credentials are unavailable")]
    CredentialsUnavailable,
    #[error(transparent)]
    Credentials(#[from] RelayCredentialsError),
}

pub(crate) struct RelayConfigurationUpdate {
    pub(crate) settings: Settings,
    pub(crate) configured_before_save: bool,
}

/// Owns the all-or-nothing relationship between Relay Configuration and its
/// Relay Access Tokens. Settings and secure storage are separate adapters, so
/// a durable recovery record is retained until both writes have committed.
pub struct RelayConfiguration {
    settings: Arc<dyn SettingsPort>,
    credentials: Option<RelayCredentials>,
    mutation_gate: Mutex<()>,
}

impl RelayConfiguration {
    pub fn new(settings: Arc<dyn SettingsPort>) -> Self {
        Self {
            settings,
            credentials: None,
            mutation_gate: Mutex::new(()),
        }
    }

    pub fn with_credentials(mut self, credentials: RelayCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Restores the pre-transaction settings and credentials after an
    /// interrupted Relay Configuration write. Calling it repeatedly is safe:
    /// the recovery record is only cleared after both restores succeed.
    pub async fn recover(&self) -> Result<(), RelayConfigurationError> {
        let _guard = self.mutation_gate.lock().await;
        self.recover_locked().await
    }

    pub(crate) async fn apply(
        &self,
        patch: SettingsPatch,
        edit: Option<&RelayCredentialEdit>,
    ) -> Result<RelayConfigurationUpdate, RelayConfigurationError> {
        let _guard = self.mutation_gate.lock().await;
        self.recover_locked().await?;

        let existing = self
            .settings
            .load()
            .await
            .map_err(|error| RelayConfigurationError::Load(error.to_string()))?;
        let previous_relay_urls = existing.network.custom_relay_urls.clone();
        let merged = apply_settings_patch(existing.clone(), patch);
        validate_settings(&merged).map_err(RelayConfigurationError::Invalid)?;

        let relay_urls_changed = previous_relay_urls != merged.network.custom_relay_urls;
        let credentials = self.credentials.clone();
        let configured_before_save = match (&credentials, edit) {
            (Some(credentials), Some(edit @ RelayCredentialEdit::Keep { .. })) => {
                credentials.is_configured(edit.url())?
            }
            _ => false,
        };

        let transaction_started = match credentials.as_ref() {
            Some(credentials) if relay_urls_changed || edit.is_some() => {
                credentials.begin_settings_transaction(
                    &existing,
                    &previous_relay_urls,
                    &merged.network.custom_relay_urls,
                    edit,
                )?;
                true
            }
            Some(_) => false,
            None if edit.is_some() => return Err(RelayConfigurationError::CredentialsUnavailable),
            None => false,
        };

        if let Err(error) = self.settings.save(&merged).await {
            if transaction_started {
                self.recover_locked().await?;
            }
            return Err(RelayConfigurationError::Save(error.to_string()));
        }

        if transaction_started {
            let Some(credentials) = credentials else {
                return Err(RelayConfigurationError::CredentialsUnavailable);
            };
            if let Err(error) = credentials.complete_settings_transaction() {
                self.recover_locked().await?;
                return Err(error.into());
            }
        }

        Ok(RelayConfigurationUpdate {
            settings: merged,
            configured_before_save,
        })
    }

    pub fn credential_status(&self, relay_url: &str) -> Result<bool, RelayConfigurationError> {
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(RelayConfigurationError::CredentialsUnavailable)?;
        Ok(credentials.is_configured(relay_url)?)
    }

    pub fn load_access_token(
        &self,
        relay_url: &str,
    ) -> Result<Option<RelayAccessToken>, RelayConfigurationError> {
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(RelayConfigurationError::CredentialsUnavailable)?;
        Ok(credentials.load(relay_url)?)
    }

    #[cfg(test)]
    pub(crate) fn set_access_token(
        &self,
        relay_url: &str,
        token: &RelayAccessToken,
    ) -> Result<(), RelayConfigurationError> {
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(RelayConfigurationError::CredentialsUnavailable)?;
        Ok(credentials.set(relay_url, token)?)
    }

    fn credentials(&self) -> Option<&RelayCredentials> {
        self.credentials.as_ref()
    }

    async fn recover_locked(&self) -> Result<(), RelayConfigurationError> {
        let Some(credentials) = self.credentials() else {
            return Ok(());
        };
        let Some(previous_settings) = credentials.restore_pending_settings_transaction()? else {
            return Ok(());
        };
        self.settings
            .save(&previous_settings)
            .await
            .map_err(|error| RelayConfigurationError::Save(error.to_string()))?;
        credentials.complete_settings_transaction()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use async_trait::async_trait;
    use uc_core::{
        ports::{SecureStorageError, SecureStoragePort, SettingsPort},
        settings::model::Settings,
    };

    use super::{
        super::{NetworkSettingsPatch, SettingsPatch},
        RelayAccessToken, RelayConfiguration, RelayCredentialEdit, RelayCredentials,
    };

    #[derive(Default)]
    struct InMemorySettings {
        value: Mutex<Settings>,
    }

    #[async_trait]
    impl uc_core::ports::SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.value.lock().unwrap().clone())
        }

        async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
            *self.value.lock().unwrap() = settings.clone();
            Ok(())
        }
    }

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

    #[tokio::test]
    async fn recovery_restores_settings_and_tokens_after_interrupted_commit() {
        let old_relay = "https://old-relay.example.com/";
        let new_relay = "https://new-relay.example.com/";
        let mut previous = Settings::default();
        previous.network.custom_relay_urls = vec![old_relay.to_string()];
        let mut committed = previous.clone();
        committed.network.custom_relay_urls = vec![new_relay.to_string()];

        let settings = Arc::new(InMemorySettings {
            value: Mutex::new(previous.clone()),
        });
        let credentials = RelayCredentials::new(Arc::new(InMemorySecureStorage::default()));
        let old_token = RelayAccessToken::new("old-relay-token".to_string()).unwrap();
        let new_token = RelayAccessToken::new("new-relay-token".to_string()).unwrap();
        credentials.set(old_relay, &old_token).unwrap();

        credentials
            .begin_settings_transaction(
                &previous,
                &previous.network.custom_relay_urls,
                &committed.network.custom_relay_urls,
                Some(&RelayCredentialEdit::Set {
                    url: new_relay.to_string(),
                    access_token: new_token,
                }),
            )
            .unwrap();
        settings.save(&committed).await.unwrap();

        let configuration =
            RelayConfiguration::new(settings.clone()).with_credentials(credentials.clone());
        configuration.recover().await.unwrap();

        let restored = settings.load().await.unwrap();
        assert_eq!(
            restored.network.custom_relay_urls,
            vec![old_relay.to_string()]
        );
        assert_eq!(
            credentials
                .load(old_relay)
                .unwrap()
                .unwrap()
                .expose_secret(),
            "old-relay-token"
        );
        assert!(credentials.load(new_relay).unwrap().is_none());
    }

    #[tokio::test]
    async fn successful_apply_clears_the_recovery_record() {
        let old_relay = "https://old-relay.example.com/";
        let new_relay = "https://new-relay.example.com/";
        let mut previous = Settings::default();
        previous.network.custom_relay_urls = vec![old_relay.to_string()];
        let settings = Arc::new(InMemorySettings {
            value: Mutex::new(previous),
        });
        let credentials = RelayCredentials::new(Arc::new(InMemorySecureStorage::default()));
        let old_token = RelayAccessToken::new("old-relay-token".to_string()).unwrap();
        let new_token = RelayAccessToken::new("new-relay-token".to_string()).unwrap();
        credentials.set(old_relay, &old_token).unwrap();
        let configuration =
            RelayConfiguration::new(settings.clone()).with_credentials(credentials.clone());

        configuration
            .apply(
                SettingsPatch {
                    network: Some(NetworkSettingsPatch {
                        custom_relay_urls: Some(vec![new_relay.to_string()]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Some(&RelayCredentialEdit::Set {
                    url: new_relay.to_string(),
                    access_token: new_token,
                }),
            )
            .await
            .unwrap();
        configuration.recover().await.unwrap();

        assert_eq!(
            settings.load().await.unwrap().network.custom_relay_urls,
            vec![new_relay.to_string()]
        );
        assert!(credentials.load(old_relay).unwrap().is_none());
        assert_eq!(
            credentials
                .load(new_relay)
                .unwrap()
                .unwrap()
                .expose_secret(),
            "new-relay-token"
        );
    }

    #[tokio::test]
    async fn recovery_is_idempotent_after_a_completed_restore() {
        let relay = "https://relay.example.com/";
        let mut previous = Settings::default();
        previous.network.custom_relay_urls = vec![relay.to_string()];
        let settings = Arc::new(InMemorySettings {
            value: Mutex::new(previous.clone()),
        });
        let credentials = RelayCredentials::new(Arc::new(InMemorySecureStorage::default()));
        let old_token = RelayAccessToken::new("old-relay-token".to_string()).unwrap();
        let new_token = RelayAccessToken::new("new-relay-token".to_string()).unwrap();
        credentials.set(relay, &old_token).unwrap();
        credentials
            .begin_settings_transaction(
                &previous,
                &previous.network.custom_relay_urls,
                &previous.network.custom_relay_urls,
                Some(&RelayCredentialEdit::Set {
                    url: relay.to_string(),
                    access_token: new_token,
                }),
            )
            .unwrap();

        let configuration =
            RelayConfiguration::new(settings.clone()).with_credentials(credentials.clone());
        configuration.recover().await.unwrap();
        configuration.recover().await.unwrap();

        assert_eq!(
            credentials.load(relay).unwrap().unwrap().expose_secret(),
            "old-relay-token"
        );
    }
}
