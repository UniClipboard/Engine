use std::{fmt, sync::Arc};

use tokio::sync::Mutex;
use uc_core::{ports::SettingsPort, settings::model::Settings};

use super::{
    models::{apply_settings_patch, validate_settings, NetworkSettingsPatch, SettingsPatch},
    RelayAccessToken, RelayCredentialEdit, RelayCredentials, RelayCredentialsError,
};

#[derive(Clone, PartialEq, Eq)]
pub struct RelayConfigurationEntry {
    pub url: String,
    pub credential_configured: bool,
}

impl fmt::Debug for RelayConfigurationEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RelayConfigurationEntry")
            .field("credential_configured", &self.credential_configured)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum RelayConfigurationMutation {
    Add {
        url: String,
        access_token: Option<RelayAccessToken>,
    },
    Edit {
        previous_url: String,
        url: String,
        access_token: Option<RelayAccessToken>,
    },
    Delete {
        url: String,
    },
}

impl fmt::Debug for RelayConfigurationMutation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayConfigurationMutation([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelayConfigurationRejection {
    #[error("invalid relay URL")]
    InvalidUrl,
    #[error("relay URL is already configured")]
    Duplicate,
    #[error("relay URL is no longer configured")]
    NotFound,
}

#[derive(Debug, thiserror::Error)]
pub enum RelayConfigurationError {
    #[error("failed to load settings")]
    Load(#[source] anyhow::Error),
    #[error("failed to save settings")]
    Save(#[source] anyhow::Error),
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
            .map_err(|error| RelayConfigurationError::Load(anyhow::Error::from(error)))?;
        let previous_relay_urls = existing.network.custom_relay_urls.clone();
        let merged = apply_settings_patch(existing.clone(), patch);
        validate_settings(&merged).map_err(RelayConfigurationError::Invalid)?;
        self.commit(existing, previous_relay_urls, merged, edit)
            .await
    }

    async fn commit(
        &self,
        existing: Settings,
        previous_relay_urls: Vec<String>,
        merged: Settings,
        edit: Option<&RelayCredentialEdit>,
    ) -> Result<RelayConfigurationUpdate, RelayConfigurationError> {
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
            return Err(RelayConfigurationError::Save(anyhow::Error::from(error)));
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

    pub async fn list(&self) -> Result<Vec<RelayConfigurationEntry>, RelayConfigurationError> {
        let _guard = self.mutation_gate.lock().await;
        self.recover_locked().await?;
        let settings = self
            .settings
            .load()
            .await
            .map_err(|error| RelayConfigurationError::Load(anyhow::Error::from(error)))?;
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(RelayConfigurationError::CredentialsUnavailable)?;
        canonical_entries(&settings.network.custom_relay_urls, credentials)
    }

    pub async fn mutate(
        &self,
        mutation: RelayConfigurationMutation,
    ) -> Result<
        Result<Vec<RelayConfigurationEntry>, RelayConfigurationRejection>,
        RelayConfigurationError,
    > {
        let _guard = self.mutation_gate.lock().await;
        self.recover_locked().await?;
        let existing = self
            .settings
            .load()
            .await
            .map_err(|error| RelayConfigurationError::Load(anyhow::Error::from(error)))?;
        let credentials = self
            .credentials
            .as_ref()
            .ok_or(RelayConfigurationError::CredentialsUnavailable)?;
        let mut urls = match canonical_urls(&existing.network.custom_relay_urls) {
            Ok(urls) => urls,
            Err(rejection) => return Ok(Err(rejection)),
        };
        let edit = match mutation {
            RelayConfigurationMutation::Add { url, access_token } => {
                let url = match canonical_url(&url) {
                    Ok(url) => url,
                    Err(error) => return Ok(Err(error)),
                };
                if urls.contains(&url) {
                    return Ok(Err(RelayConfigurationRejection::Duplicate));
                }
                urls.push(url.clone());
                access_token.map_or(
                    RelayCredentialEdit::Keep { url: url.clone() },
                    |access_token| RelayCredentialEdit::Set { url, access_token },
                )
            }
            RelayConfigurationMutation::Edit {
                previous_url,
                url,
                access_token,
            } => {
                let previous_url = match canonical_url(&previous_url) {
                    Ok(url) => url,
                    Err(error) => return Ok(Err(error)),
                };
                let url = match canonical_url(&url) {
                    Ok(url) => url,
                    Err(error) => return Ok(Err(error)),
                };
                let Some(index) = urls
                    .iter()
                    .position(|configured| configured == &previous_url)
                else {
                    return Ok(Err(RelayConfigurationRejection::NotFound));
                };
                if url != previous_url && urls.contains(&url) {
                    return Ok(Err(RelayConfigurationRejection::Duplicate));
                }
                urls[index] = url.clone();
                match access_token.or(credentials.load(&previous_url)?) {
                    Some(access_token) => RelayCredentialEdit::Set { url, access_token },
                    None => RelayCredentialEdit::Keep { url },
                }
            }
            RelayConfigurationMutation::Delete { url } => {
                let url = match canonical_url(&url) {
                    Ok(url) => url,
                    Err(error) => return Ok(Err(error)),
                };
                let Some(index) = urls.iter().position(|configured| configured == &url) else {
                    return Ok(Err(RelayConfigurationRejection::NotFound));
                };
                urls.remove(index);
                RelayCredentialEdit::Delete { url }
            }
        };
        let patch = SettingsPatch {
            network: Some(NetworkSettingsPatch {
                custom_relay_urls: Some(urls),
                ..Default::default()
            }),
            ..Default::default()
        };
        let saved = self.apply_locked(existing, patch, Some(&edit)).await?;
        canonical_entries(&saved.settings.network.custom_relay_urls, credentials).map(Ok)
    }

    async fn apply_locked(
        &self,
        existing: Settings,
        patch: SettingsPatch,
        edit: Option<&RelayCredentialEdit>,
    ) -> Result<RelayConfigurationUpdate, RelayConfigurationError> {
        let previous_relay_urls = existing.network.custom_relay_urls.clone();
        let merged = apply_settings_patch(existing.clone(), patch);
        validate_settings(&merged).map_err(RelayConfigurationError::Invalid)?;
        self.commit(existing, previous_relay_urls, merged, edit)
            .await
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
            .map_err(|error| RelayConfigurationError::Save(anyhow::Error::from(error)))?;
        credentials.complete_settings_transaction()?;
        Ok(())
    }
}

fn canonical_url(raw: &str) -> Result<String, RelayConfigurationRejection> {
    let url = url::Url::parse(raw.trim()).map_err(|_| RelayConfigurationRejection::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RelayConfigurationRejection::InvalidUrl);
    }
    Ok(url.to_string())
}

fn canonical_urls(urls: &[String]) -> Result<Vec<String>, RelayConfigurationRejection> {
    let mut result = Vec::with_capacity(urls.len());
    for raw in urls {
        let url = canonical_url(raw)?;
        if !result.contains(&url) {
            result.push(url);
        }
    }
    Ok(result)
}

fn canonical_entries(
    urls: &[String],
    credentials: &RelayCredentials,
) -> Result<Vec<RelayConfigurationEntry>, RelayConfigurationError> {
    let urls = canonical_urls(urls)
        .map_err(|_| RelayConfigurationError::Invalid("invalid custom relay URL".to_string()))?;
    urls.into_iter()
        .map(|url| {
            let credential_configured = credentials.is_configured(&url)?;
            Ok(RelayConfigurationEntry {
                url,
                credential_configured,
            })
        })
        .collect()
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
        RelayAccessToken, RelayConfiguration, RelayConfigurationMutation,
        RelayConfigurationRejection, RelayCredentialEdit, RelayCredentials,
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

    #[tokio::test]
    async fn authoritative_list_is_empty_for_default_settings() {
        let configuration = RelayConfiguration::new(Arc::new(InMemorySettings::default()))
            .with_credentials(RelayCredentials::new(Arc::new(
                InMemorySecureStorage::default(),
            )));

        assert!(configuration.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn authoritative_mutations_normalize_reject_duplicates_and_return_full_list() {
        let settings = Arc::new(InMemorySettings::default());
        let credentials = RelayCredentials::new(Arc::new(InMemorySecureStorage::default()));
        let configuration = RelayConfiguration::new(settings).with_credentials(credentials.clone());
        let token = RelayAccessToken::new("secret-token".to_string()).unwrap();

        let added = configuration
            .mutate(RelayConfigurationMutation::Add {
                url: " https://relay.example.com ".to_string(),
                access_token: Some(token),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].url, "https://relay.example.com/");
        assert!(added[0].credential_configured);

        let two_relays = configuration
            .mutate(RelayConfigurationMutation::Add {
                url: "https://relay-two.example.com".to_string(),
                access_token: None,
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(two_relays.len(), 2);
        assert!(!two_relays[1].credential_configured);

        let duplicate = configuration
            .mutate(RelayConfigurationMutation::Add {
                url: "https://relay.example.com/".to_string(),
                access_token: None,
            })
            .await
            .unwrap();
        assert_eq!(duplicate, Err(RelayConfigurationRejection::Duplicate));

        let edited = configuration
            .mutate(RelayConfigurationMutation::Edit {
                previous_url: "https://relay.example.com".to_string(),
                url: "https://new-relay.example.com".to_string(),
                access_token: None,
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(edited[0].url, "https://new-relay.example.com/");
        assert!(edited[0].credential_configured);
        assert_eq!(edited[1].url, "https://relay-two.example.com/");
        assert_eq!(
            credentials
                .load("https://new-relay.example.com/")
                .unwrap()
                .unwrap()
                .expose_secret(),
            "secret-token"
        );

        let deleted = configuration
            .mutate(RelayConfigurationMutation::Delete {
                url: "https://new-relay.example.com".to_string(),
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].url, "https://relay-two.example.com/");

        let invalid = configuration
            .mutate(RelayConfigurationMutation::Add {
                url: "ftp://relay.example.com".to_string(),
                access_token: None,
            })
            .await
            .unwrap();
        assert_eq!(invalid, Err(RelayConfigurationRejection::InvalidUrl));
    }

    #[tokio::test]
    async fn stale_edit_does_not_overwrite_current_configuration() {
        let mut value = Settings::default();
        value.network.custom_relay_urls = vec!["https://current.example.com/".to_string()];
        let settings = Arc::new(InMemorySettings {
            value: Mutex::new(value),
        });
        let configuration = RelayConfiguration::new(settings.clone()).with_credentials(
            RelayCredentials::new(Arc::new(InMemorySecureStorage::default())),
        );

        let result = configuration
            .mutate(RelayConfigurationMutation::Edit {
                previous_url: "https://stale.example.com/".to_string(),
                url: "https://replacement.example.com/".to_string(),
                access_token: None,
            })
            .await
            .unwrap();

        assert_eq!(result, Err(RelayConfigurationRejection::NotFound));
        assert_eq!(
            settings.load().await.unwrap().network.custom_relay_urls,
            vec!["https://current.example.com/".to_string()]
        );
    }
}
