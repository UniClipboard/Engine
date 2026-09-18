use anyhow::Error;
use uc_core::ports::SecureStoragePort;
use zeroize::Zeroizing;

use crate::security::SecureStorageAccess;

use super::{MasterKey, ProfileContentKeyVaultError};

#[cfg(test)]
mod tests;

pub(in super::super) const VAULT_KEY_NAME: &str = "profile_content_vault_key:v1";

pub(super) async fn load_existing(
    storage: SecureStorageAccess,
) -> Result<MasterKey, ProfileContentKeyVaultError> {
    storage.execute(required_key).await
}

pub(super) async fn load_or_create(
    storage: SecureStorageAccess,
) -> Result<MasterKey, ProfileContentKeyVaultError> {
    // 检查、生成、写入和复读作为一次完整访问，由持有目录租约的调用方等待。
    storage
        .execute(|storage| {
            if let Some(key) = read_key(storage)? {
                return Ok(key);
            }
            let generated = match MasterKey::generate() {
                Ok(generated) => generated,
                Err(source) => {
                    return Err(ProfileContentKeyVaultError::InvalidMaterial {
                        source: Error::new(source).context("generate profile content vault key"),
                    });
                }
            };
            storage.set(VAULT_KEY_NAME, generated.as_bytes())?;
            required_key(storage)
        })
        .await
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
        .get(VAULT_KEY_NAME)?
        .map(|bytes| {
            let bytes = Zeroizing::new(bytes);
            MasterKey::from_bytes(&bytes).map_err(|source| ProfileContentKeyVaultError::Corrupt {
                source: Error::new(source).context("decode profile content vault key"),
            })
        })
        .transpose()
}
