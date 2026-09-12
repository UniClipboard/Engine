use std::sync::Arc;

use anyhow::Error;
use tokio::task::spawn_blocking;
use uc_core::ports::SecureStoragePort;
use zeroize::Zeroizing;

use super::{MasterKey, ProfileContentKeyVaultError};

#[cfg(test)]
mod tests;

pub(in super::super) const VAULT_KEY_NAME: &str = "profile_content_vault_key:v1";

pub(super) async fn load_existing(
    storage: Arc<dyn SecureStoragePort>,
) -> Result<MasterKey, ProfileContentKeyVaultError> {
    spawn_blocking(move || required_key(storage.as_ref()))
        .await
        .map_err(access_error)?
}

pub(super) async fn load_or_create(
    storage: Arc<dyn SecureStoragePort>,
) -> Result<MasterKey, ProfileContentKeyVaultError> {
    // 检查、生成、写入和复读作为一次完整访问，由持有目录租约的调用方等待。
    spawn_blocking(move || {
        if let Some(key) = read_key(storage.as_ref())? {
            return Ok(key);
        }
        let generated = MasterKey::generate().map_err(access_error)?;
        storage
            .set(VAULT_KEY_NAME, generated.as_bytes())
            .map_err(access_error)?;
        required_key(storage.as_ref())
    })
    .await
    .map_err(access_error)?
}

fn required_key(storage: &dyn SecureStoragePort) -> Result<MasterKey, ProfileContentKeyVaultError> {
    read_key(storage)?.ok_or_else(|| ProfileContentKeyVaultError::Corrupt {
        source: anyhow::anyhow!("profile content vault key is missing"),
    })
}

fn read_key(
    storage: &dyn SecureStoragePort,
) -> Result<Option<MasterKey>, ProfileContentKeyVaultError> {
    storage
        .get(VAULT_KEY_NAME)
        .map_err(access_error)?
        .map(|bytes| {
            let bytes = Zeroizing::new(bytes);
            MasterKey::from_bytes(&bytes).map_err(|source| ProfileContentKeyVaultError::Corrupt {
                source: Error::new(source).context("decode profile content vault key"),
            })
        })
        .transpose()
}

fn access_error(source: impl Into<Error>) -> ProfileContentKeyVaultError {
    ProfileContentKeyVaultError::SecureStorage {
        source: source.into().context("access profile content vault key"),
    }
}
