use std::{collections::BTreeSet, fmt, sync::Arc};

use serde::{Deserialize, Serialize};
use uc_core::{
    ports::{SecureStorageError, SecureStoragePort},
    settings::model::Settings,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const STORAGE_KEY_PREFIX: &str = "relay_access_token:v1:";
const SETTINGS_TRANSACTION_STORAGE_KEY: &str = "relay_configuration:transaction:v1";
const MAX_TOKEN_LENGTH: usize = 4096;

#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct RelayAccessToken(String);

impl RelayAccessToken {
    pub fn new(mut value: String) -> Result<Self, RelayCredentialsError> {
        if value.is_empty()
            || value.len() > MAX_TOKEN_LENGTH
            || !value.is_ascii()
            || value.chars().any(char::is_control)
        {
            value.zeroize();
            return Err(RelayCredentialsError::InvalidToken);
        }
        Ok(Self(value))
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RelayAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RelayAccessToken([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum RelayProbeCredential {
    Stored,
    None,
    Override(RelayAccessToken),
}

impl fmt::Debug for RelayProbeCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Stored => "stored",
            Self::None => "none",
            Self::Override(_) => "override",
        };
        formatter
            .debug_struct("RelayProbeCredential")
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum RelayCredentialEdit {
    Keep {
        url: String,
    },
    Set {
        url: String,
        access_token: RelayAccessToken,
    },
    Delete {
        url: String,
    },
}

impl RelayCredentialEdit {
    pub(crate) fn url(&self) -> &str {
        match self {
            Self::Keep { url } | Self::Set { url, .. } | Self::Delete { url } => url,
        }
    }

    pub(crate) fn configured_after_save(&self, configured_before_save: bool) -> bool {
        match self {
            Self::Keep { .. } => configured_before_save,
            Self::Set { .. } => true,
            Self::Delete { .. } => false,
        }
    }
}

impl fmt::Debug for RelayCredentialEdit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Keep { .. } => "keep",
            Self::Set { .. } => "set",
            Self::Delete { .. } => "delete",
        };
        formatter
            .debug_struct("RelayCredentialEdit")
            .field("kind", &kind)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RelayCredentialsError {
    #[error("invalid relay URL")]
    InvalidRelayUrl,
    #[error("invalid relay access token")]
    InvalidToken,
    #[error("relay credential URL does not match the saved relay settings")]
    InvalidTarget,
    #[error("relay credential storage failed")]
    Storage(#[source] SecureStorageError),
    #[error("stored relay credential is corrupt")]
    Corrupt,
}

#[derive(Clone)]
pub struct RelayCredentials {
    storage: Arc<dyn SecureStoragePort>,
}

pub(crate) struct RelayCredentialRestorePoint {
    entries: Vec<(String, Option<Zeroizing<Vec<u8>>>)>,
}

#[derive(Serialize, Deserialize)]
struct RelaySettingsTransaction {
    version: u8,
    previous_settings: Settings,
    entries: Vec<RelaySettingsTransactionEntry>,
}

#[derive(Serialize, Deserialize)]
struct RelaySettingsTransactionEntry {
    relay_url: String,
    value: Option<Vec<u8>>,
}

enum RelayCredentialMutation {
    Set(RelayAccessToken),
    Delete,
}

impl RelayCredentials {
    pub fn new(storage: Arc<dyn SecureStoragePort>) -> Self {
        Self { storage }
    }

    pub fn load(&self, relay_url: &str) -> Result<Option<RelayAccessToken>, RelayCredentialsError> {
        let key = storage_key(relay_url)?;
        let Some(bytes) = self
            .storage
            .get(&key)
            .map_err(RelayCredentialsError::Storage)?
        else {
            return Ok(None);
        };
        let value = match String::from_utf8(bytes) {
            Ok(value) => value,
            Err(error) => {
                let mut bytes = error.into_bytes();
                bytes.zeroize();
                return Err(RelayCredentialsError::Corrupt);
            }
        };
        RelayAccessToken::new(value)
            .map(Some)
            .map_err(|_| RelayCredentialsError::Corrupt)
    }

    pub fn is_configured(&self, relay_url: &str) -> Result<bool, RelayCredentialsError> {
        Ok(self.load_raw(relay_url)?.is_some())
    }

    pub fn set(
        &self,
        relay_url: &str,
        token: &RelayAccessToken,
    ) -> Result<(), RelayCredentialsError> {
        let key = storage_key(relay_url)?;
        self.storage
            .set(&key, token.expose_secret().as_bytes())
            .map_err(RelayCredentialsError::Storage)
    }

    pub fn delete(&self, relay_url: &str) -> Result<bool, RelayCredentialsError> {
        let key = storage_key(relay_url)?;
        let mut stored = self
            .storage
            .get(&key)
            .map_err(RelayCredentialsError::Storage)?;
        let existed = stored.is_some();
        if let Some(bytes) = stored.as_mut() {
            bytes.zeroize();
        }
        if existed {
            self.storage
                .delete(&key)
                .map_err(RelayCredentialsError::Storage)?;
        }
        Ok(existed)
    }

    #[cfg(test)]
    pub(crate) fn apply_settings_edit(
        &self,
        previous_urls: &[String],
        current_urls: &[String],
        edit: Option<&RelayCredentialEdit>,
    ) -> Result<RelayCredentialRestorePoint, RelayCredentialsError> {
        let current_keys = current_urls
            .iter()
            .map(|url| storage_key(url))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let previous_keys = previous_urls
            .iter()
            .map(|url| storage_key(url))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut mutations = std::collections::BTreeMap::new();
        for relay_url in previous_urls {
            let key = storage_key(relay_url)?;
            if !current_keys.contains(&key) {
                mutations.insert(key, (relay_url.clone(), RelayCredentialMutation::Delete));
            }
        }

        if let Some(edit) = edit {
            let url = edit.url().to_string();
            let key = storage_key(&url)?;
            let target_is_valid = match edit {
                RelayCredentialEdit::Keep { .. } | RelayCredentialEdit::Set { .. } => {
                    current_keys.contains(&key)
                }
                RelayCredentialEdit::Delete { .. } => {
                    previous_keys.contains(&key) || current_keys.contains(&key)
                }
            };
            if !target_is_valid {
                return Err(RelayCredentialsError::InvalidTarget);
            }
            let mutation = match edit {
                RelayCredentialEdit::Keep { .. } => None,
                RelayCredentialEdit::Set { access_token, .. } => {
                    Some(RelayCredentialMutation::Set(access_token.clone()))
                }
                RelayCredentialEdit::Delete { .. } => Some(RelayCredentialMutation::Delete),
            };
            if let Some(mutation) = mutation {
                mutations.insert(key, (url, mutation));
            }
        }

        let restore_point = RelayCredentialRestorePoint {
            entries: mutations
                .values()
                .map(|(relay_url, _)| {
                    self.load_raw(relay_url)
                        .map(|value| (relay_url.clone(), value))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };

        for (index, (_, (relay_url, mutation))) in mutations.iter().enumerate() {
            let result = match mutation {
                RelayCredentialMutation::Set(token) => self.set(relay_url, token),
                RelayCredentialMutation::Delete => self.delete(relay_url).map(|_| ()),
            };
            if let Err(error) = result {
                self.restore_entries(&restore_point.entries[..=index])?;
                return Err(error);
            }
        }

        Ok(restore_point)
    }

    pub(crate) fn begin_settings_transaction(
        &self,
        previous_settings: &Settings,
        previous_urls: &[String],
        current_urls: &[String],
        edit: Option<&RelayCredentialEdit>,
    ) -> Result<(), RelayCredentialsError> {
        let current_keys = current_urls
            .iter()
            .map(|url| storage_key(url))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let previous_keys = previous_urls
            .iter()
            .map(|url| storage_key(url))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let mut mutations = std::collections::BTreeMap::new();
        for relay_url in previous_urls {
            let key = storage_key(relay_url)?;
            if !current_keys.contains(&key) {
                mutations.insert(key, (relay_url.clone(), RelayCredentialMutation::Delete));
            }
        }

        if let Some(edit) = edit {
            let url = edit.url().to_string();
            let key = storage_key(&url)?;
            let target_is_valid = match edit {
                RelayCredentialEdit::Keep { .. } | RelayCredentialEdit::Set { .. } => {
                    current_keys.contains(&key)
                }
                RelayCredentialEdit::Delete { .. } => {
                    previous_keys.contains(&key) || current_keys.contains(&key)
                }
            };
            if !target_is_valid {
                return Err(RelayCredentialsError::InvalidTarget);
            }
            let mutation = match edit {
                RelayCredentialEdit::Keep { .. } => None,
                RelayCredentialEdit::Set { access_token, .. } => {
                    Some(RelayCredentialMutation::Set(access_token.clone()))
                }
                RelayCredentialEdit::Delete { .. } => Some(RelayCredentialMutation::Delete),
            };
            if let Some(mutation) = mutation {
                mutations.insert(key, (url, mutation));
            }
        }

        let restore_point = RelayCredentialRestorePoint {
            entries: mutations
                .values()
                .map(|(relay_url, _)| {
                    self.load_raw(relay_url)
                        .map(|value| (relay_url.clone(), value))
                })
                .collect::<Result<Vec<_>, _>>()?,
        };
        self.write_settings_transaction(previous_settings, &restore_point)?;

        for (index, (_, (relay_url, mutation))) in mutations.iter().enumerate() {
            let result = match mutation {
                RelayCredentialMutation::Set(token) => self.set(relay_url, token),
                RelayCredentialMutation::Delete => self.delete(relay_url).map(|_| ()),
            };
            if let Err(error) = result {
                self.restore_entries(&restore_point.entries[..=index])?;
                self.complete_settings_transaction()?;
                return Err(error);
            }
        }

        Ok(())
    }

    pub(crate) fn restore_pending_settings_transaction(
        &self,
    ) -> Result<Option<Settings>, RelayCredentialsError> {
        let Some(mut bytes) = self
            .storage
            .get(SETTINGS_TRANSACTION_STORAGE_KEY)
            .map_err(RelayCredentialsError::Storage)?
        else {
            return Ok(None);
        };
        let transaction = serde_json::from_slice::<RelaySettingsTransaction>(&bytes)
            .map_err(|_| RelayCredentialsError::Corrupt);
        bytes.zeroize();
        let transaction = transaction?;
        if transaction.version != 1 {
            return Err(RelayCredentialsError::Corrupt);
        }
        let restore_point = RelayCredentialRestorePoint {
            entries: transaction
                .entries
                .into_iter()
                .map(|entry| (entry.relay_url, entry.value.map(Zeroizing::new)))
                .collect(),
        };
        self.restore_entries(&restore_point.entries)?;
        Ok(Some(transaction.previous_settings))
    }

    pub(crate) fn complete_settings_transaction(&self) -> Result<(), RelayCredentialsError> {
        self.storage
            .delete(SETTINGS_TRANSACTION_STORAGE_KEY)
            .map_err(RelayCredentialsError::Storage)
    }

    fn write_settings_transaction(
        &self,
        previous_settings: &Settings,
        restore_point: &RelayCredentialRestorePoint,
    ) -> Result<(), RelayCredentialsError> {
        let transaction = RelaySettingsTransaction {
            version: 1,
            previous_settings: previous_settings.clone(),
            entries: restore_point
                .entries
                .iter()
                .map(|(relay_url, value)| RelaySettingsTransactionEntry {
                    relay_url: relay_url.clone(),
                    value: value.as_ref().map(|value| value.to_vec()),
                })
                .collect(),
        };
        let mut bytes =
            serde_json::to_vec(&transaction).map_err(|_| RelayCredentialsError::Corrupt)?;
        let result = self
            .storage
            .set(SETTINGS_TRANSACTION_STORAGE_KEY, &bytes)
            .map_err(RelayCredentialsError::Storage);
        bytes.zeroize();
        result
    }

    fn restore_entries(
        &self,
        entries: &[(String, Option<Zeroizing<Vec<u8>>>)],
    ) -> Result<(), RelayCredentialsError> {
        let mut first_error = None;
        for (relay_url, value) in entries.iter().rev() {
            let result = match value {
                Some(value) => self
                    .storage
                    .set(&storage_key(relay_url)?, value.as_slice())
                    .map_err(RelayCredentialsError::Storage),
                None => self.delete(relay_url).map(|_| ()),
            };
            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn load_raw(
        &self,
        relay_url: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, RelayCredentialsError> {
        let key = storage_key(relay_url)?;
        self.storage
            .get(&key)
            .map(|value| value.map(Zeroizing::new))
            .map_err(RelayCredentialsError::Storage)
    }
}

fn storage_key(relay_url: &str) -> Result<String, RelayCredentialsError> {
    let canonical_url = canonical_relay_url(relay_url)?;
    let digest = blake3::hash(canonical_url.as_bytes());
    Ok(format!("{STORAGE_KEY_PREFIX}{}", digest.to_hex()))
}

fn canonical_relay_url(relay_url: &str) -> Result<String, RelayCredentialsError> {
    let url =
        url::Url::parse(relay_url.trim()).map_err(|_| RelayCredentialsError::InvalidRelayUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RelayCredentialsError::InvalidRelayUrl);
    }
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::{storage_key, RelayAccessToken, RelayCredentials, RelayCredentialsError};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

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

    #[test]
    fn credential_is_scoped_to_its_relay_url() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = RelayCredentials::new(storage);
        let token = RelayAccessToken::new(TOKEN.to_string()).expect("valid token");

        credentials
            .set("https://relay-a.example.com", &token)
            .expect("store token");

        let loaded = credentials
            .load("https://relay-a.example.com")
            .expect("load token")
            .expect("configured token");
        assert_eq!(loaded.expose_secret(), TOKEN);
        assert!(credentials
            .load("https://relay-b.example.com")
            .expect("load other relay")
            .is_none());
    }

    #[test]
    fn credential_can_be_replaced_and_deleted() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = RelayCredentials::new(storage);
        let first = RelayAccessToken::new(TOKEN.to_string()).expect("valid token");
        let second = RelayAccessToken::new("replacement-token".to_string()).expect("valid token");

        credentials
            .set("https://relay.example.com", &first)
            .expect("store token");
        credentials
            .set("https://relay.example.com", &second)
            .expect("replace token");

        assert_eq!(
            credentials
                .load("https://relay.example.com")
                .expect("load token")
                .expect("configured token")
                .expose_secret(),
            "replacement-token"
        );
        assert!(credentials
            .delete("https://relay.example.com")
            .expect("delete token"));
        assert!(!credentials
            .delete("https://relay.example.com")
            .expect("delete missing token"));
        assert!(credentials
            .load("https://relay.example.com")
            .expect("load deleted token")
            .is_none());
    }

    #[test]
    fn equivalent_relay_urls_share_one_credential() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = RelayCredentials::new(storage);
        let token = RelayAccessToken::new(TOKEN.to_string()).expect("valid token");

        credentials
            .set(" HTTPS://Relay.Example.COM:443 ", &token)
            .expect("store token");

        assert_eq!(
            credentials
                .load("https://relay.example.com/")
                .expect("load token")
                .expect("configured token")
                .expose_secret(),
            TOKEN
        );
    }

    #[test]
    fn relay_url_rejects_embedded_credentials() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = RelayCredentials::new(storage);

        let error = credentials
            .load("https://login:password@relay.example.com")
            .expect_err("embedded credentials must be rejected");

        assert!(matches!(error, RelayCredentialsError::InvalidRelayUrl));
    }

    #[test]
    fn token_rejects_values_that_cannot_be_sent_as_an_http_header() {
        for value in [
            String::new(),
            "line\nbreak".to_string(),
            "令牌".to_string(),
            "x".repeat(4097),
        ] {
            let error = RelayAccessToken::new(value).expect_err("invalid token");
            assert!(matches!(error, RelayCredentialsError::InvalidToken));
        }
    }

    #[test]
    fn corrupt_stored_credential_is_reported() {
        let storage = Arc::new(InMemorySecureStorage::default());
        storage
            .set(
                &storage_key("https://relay.example.com").expect("valid relay URL"),
                &[0xff, 0xfe],
            )
            .expect("seed corrupt credential");
        let credentials = RelayCredentials::new(storage);

        let error = credentials
            .load("https://relay.example.com")
            .expect_err("corrupt credential must fail");

        assert!(matches!(error, RelayCredentialsError::Corrupt));
    }

    #[test]
    fn token_debug_output_is_redacted() {
        let token = RelayAccessToken::new(TOKEN.to_string()).expect("valid token");
        let rendered = format!("{token:?}");

        assert!(!rendered.contains(TOKEN));
        assert!(rendered.contains("REDACTED"));
    }

    #[test]
    fn credential_edit_rejects_a_url_unrelated_to_the_saved_settings() {
        let storage = Arc::new(InMemorySecureStorage::default());
        let credentials = RelayCredentials::new(storage);
        let edit = super::RelayCredentialEdit::Set {
            url: "https://orphan.example.com".to_string(),
            access_token: RelayAccessToken::new(TOKEN.to_string()).expect("valid token"),
        };

        assert!(credentials
            .apply_settings_edit(&[], &[], Some(&edit))
            .is_err());
        assert!(credentials
            .load("https://orphan.example.com")
            .expect("query orphan credential")
            .is_none());
    }

    #[test]
    fn credential_edit_can_replace_a_corrupt_stored_token() {
        let relay = "https://relay.example.com/";
        let storage = Arc::new(InMemorySecureStorage::default());
        storage
            .set(&storage_key(relay).expect("valid relay URL"), &[0xff, 0xfe])
            .expect("seed corrupt credential");
        let credentials = RelayCredentials::new(storage);
        let edit = super::RelayCredentialEdit::Set {
            url: relay.to_string(),
            access_token: RelayAccessToken::new(TOKEN.to_string()).expect("valid token"),
        };

        credentials
            .apply_settings_edit(&[relay.to_string()], &[relay.to_string()], Some(&edit))
            .expect("replace corrupt credential");

        assert_eq!(
            credentials
                .load(relay)
                .expect("load replacement")
                .expect("configured replacement")
                .expose_secret(),
            TOKEN
        );
    }

    #[test]
    fn credential_edit_can_delete_a_corrupt_stored_token() {
        let relay = "https://relay.example.com/";
        let storage = Arc::new(InMemorySecureStorage::default());
        storage
            .set(&storage_key(relay).expect("valid relay URL"), &[0xff, 0xfe])
            .expect("seed corrupt credential");
        let credentials = RelayCredentials::new(storage);
        let edit = super::RelayCredentialEdit::Delete {
            url: relay.to_string(),
        };

        credentials
            .apply_settings_edit(&[relay.to_string()], &[relay.to_string()], Some(&edit))
            .expect("delete corrupt credential");

        assert!(credentials
            .load(relay)
            .expect("load deleted credential")
            .is_none());
    }

    #[test]
    fn corrupt_stored_credential_still_reports_as_configured() {
        let relay = "https://relay.example.com/";
        let storage = Arc::new(InMemorySecureStorage::default());
        storage
            .set(&storage_key(relay).expect("valid relay URL"), &[0xff, 0xfe])
            .expect("seed corrupt credential");
        let credentials = RelayCredentials::new(storage);

        assert!(credentials
            .is_configured(relay)
            .expect("query credential presence"));
    }
}
