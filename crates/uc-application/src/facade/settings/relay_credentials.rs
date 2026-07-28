use std::{fmt, sync::Arc};

use uc_core::ports::{SecureStorageError, SecureStoragePort};
use zeroize::{Zeroize, ZeroizeOnDrop};

const STORAGE_KEY_PREFIX: &str = "relay_access_token:v1:";
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

#[derive(Debug, thiserror::Error)]
pub enum RelayCredentialsError {
    #[error("invalid relay URL")]
    InvalidRelayUrl,
    #[error("invalid relay access token")]
    InvalidToken,
    #[error("relay credential storage failed")]
    Storage(#[source] SecureStorageError),
    #[error("stored relay credential is corrupt")]
    Corrupt,
}

#[derive(Clone)]
pub struct RelayCredentials {
    storage: Arc<dyn SecureStoragePort>,
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
}
