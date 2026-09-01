use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hkdf::Hkdf;
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use zeroize::Zeroizing;

use super::super::crypto_model::EncryptedBlob;
use super::super::{v1_aead, MasterKey};
use super::catalog;
use super::model::{PersistedVault, ProfileContentKeyVaultError, MAX_VAULT_PLAINTEXT_BYTES};

const VAULT_FILE: &str = "profile-content-key-vault-v1.json";
pub(in crate::security) const VAULT_KEY_NAME: &str = "profile_content_vault_key:v1";
const VAULT_PURPOSE: &[u8] = b"uniclipboard/profile-content-key-vault/v1\0";
const PROFILE_SEARCH_ROOT_INFO: &[u8] = b"uniclipboard/profile-search-root/v1\0";
const MAX_ENCRYPTED_VAULT_BYTES: usize = 8 * 1024 * 1024;

pub(super) struct VaultPersistence {
    path: PathBuf,
    secure_storage: Arc<dyn SecureStoragePort>,
    profile_generation: [u8; 16],
}

impl VaultPersistence {
    pub(super) fn new(
        directory: PathBuf,
        secure_storage: Arc<dyn SecureStoragePort>,
        profile_generation: [u8; 16],
    ) -> Self {
        Self {
            path: directory.join(VAULT_FILE),
            secure_storage,
            profile_generation,
        }
    }

    pub(super) async fn load(&self) -> Result<PersistedVault, ProfileContentKeyVaultError> {
        self.load_optional()
            .await?
            .ok_or(ProfileContentKeyVaultError::KeyNotFound)
    }

    pub(super) async fn load_optional(
        &self,
    ) -> Result<Option<PersistedVault>, ProfileContentKeyVaultError> {
        let file = match tokio::fs::File::open(&self.path).await {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(ProfileContentKeyVaultError::Storage {
                    source: anyhow::Error::new(source).context("read profile content key vault"),
                });
            }
        };
        let mut ciphertext = Vec::new();
        file.take((MAX_ENCRYPTED_VAULT_BYTES + 1) as u64)
            .read_to_end(&mut ciphertext)
            .await
            .map_err(storage_error)?;
        if ciphertext.len() > MAX_ENCRYPTED_VAULT_BYTES {
            return Err(ProfileContentKeyVaultError::CapacityExceeded);
        }
        let encrypted: EncryptedBlob = serde_json::from_slice(&ciphertext).map_err(|source| {
            ProfileContentKeyVaultError::Corrupt {
                source: anyhow::Error::new(source).context("decode encrypted content key vault"),
            }
        })?;
        let aad = self.aad();
        validate_framing(&encrypted, &aad)?;
        let key = self.load_existing_key()?;
        let plaintext = Zeroizing::new(
            v1_aead::decrypt_blob_xchacha(&key, &encrypted.nonce, &encrypted.ciphertext, &aad)
                .map_err(|source| ProfileContentKeyVaultError::Corrupt {
                    source: anyhow::Error::new(source).context("open profile content key vault"),
                })?,
        );
        if plaintext.len() > MAX_VAULT_PLAINTEXT_BYTES {
            return Err(ProfileContentKeyVaultError::CapacityExceeded);
        }
        let vault: PersistedVault = postcard::from_bytes(&plaintext).map_err(|source| {
            ProfileContentKeyVaultError::Corrupt {
                source: anyhow::Error::new(source).context("decode profile content key vault"),
            }
        })?;
        catalog::validate(&vault)?;
        Ok(Some(vault))
    }

    pub(super) async fn store(
        &self,
        vault: &PersistedVault,
    ) -> Result<(), ProfileContentKeyVaultError> {
        let key = self.load_or_create_key_for_install()?;
        let plaintext = Zeroizing::new(postcard::to_stdvec(vault).map_err(|source| {
            ProfileContentKeyVaultError::InvalidMaterial {
                source: anyhow::Error::new(source).context("encode profile content key vault"),
            }
        })?);
        if plaintext.len() > MAX_VAULT_PLAINTEXT_BYTES {
            return Err(ProfileContentKeyVaultError::CapacityExceeded);
        }
        let encrypted =
            v1_aead::encrypt_blob_xchacha(&key, &plaintext, &self.aad()).map_err(|source| {
                ProfileContentKeyVaultError::Storage {
                    source: anyhow::Error::new(source).context("seal profile content key vault"),
                }
            })?;
        let ciphertext = serde_json::to_vec(&encrypted).map_err(|source| {
            ProfileContentKeyVaultError::Storage {
                source: anyhow::Error::new(source).context("encode encrypted content key vault"),
            }
        })?;
        if ciphertext.len() > MAX_ENCRYPTED_VAULT_BYTES {
            return Err(ProfileContentKeyVaultError::CapacityExceeded);
        }
        write_atomically(&self.path, &ciphertext).await
    }

    pub(super) fn derive_profile_search_root(
        &self,
    ) -> Result<MasterKey, ProfileContentKeyVaultError> {
        let vault_key = self.load_existing_key()?;
        let hkdf = Hkdf::<Sha256>::new(Some(&self.profile_generation), vault_key.as_bytes());
        let mut output = Zeroizing::new([0u8; MasterKey::LEN]);
        hkdf.expand(PROFILE_SEARCH_ROOT_INFO, output.as_mut())
            .map_err(|source| ProfileContentKeyVaultError::Corrupt {
                source: anyhow::Error::new(HkdfExpandError(source))
                    .context("derive profile search root"),
            })?;
        MasterKey::from_bytes(output.as_ref()).map_err(|source| {
            ProfileContentKeyVaultError::Corrupt {
                source: anyhow::Error::new(source).context("decode profile search root"),
            }
        })
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn load_or_create_key_for_install(&self) -> Result<MasterKey, ProfileContentKeyVaultError> {
        if let Some(key) = self.read_key()? {
            return Ok(key);
        }
        let generated =
            MasterKey::generate().map_err(|source| ProfileContentKeyVaultError::SecureStorage {
                source: anyhow::Error::new(source).context("generate profile content vault key"),
            })?;
        self.secure_storage
            .set(VAULT_KEY_NAME, generated.as_bytes())
            .map_err(secure_storage_error)?;
        self.load_existing_key()
    }

    fn load_existing_key(&self) -> Result<MasterKey, ProfileContentKeyVaultError> {
        self.read_key()?
            .ok_or_else(|| ProfileContentKeyVaultError::Corrupt {
                source: anyhow::anyhow!("profile content vault key is missing"),
            })
    }

    fn read_key(&self) -> Result<Option<MasterKey>, ProfileContentKeyVaultError> {
        self.secure_storage
            .get(VAULT_KEY_NAME)
            .map_err(secure_storage_error)?
            .map(|bytes| {
                let bytes = Zeroizing::new(bytes);
                MasterKey::from_bytes(&bytes).map_err(|source| {
                    ProfileContentKeyVaultError::Corrupt {
                        source: anyhow::Error::new(source)
                            .context("decode profile content vault key"),
                    }
                })
            })
            .transpose()
    }

    fn aad(&self) -> Vec<u8> {
        let mut aad = Vec::with_capacity(VAULT_PURPOSE.len() + self.profile_generation.len());
        aad.extend_from_slice(VAULT_PURPOSE);
        aad.extend_from_slice(&self.profile_generation);
        aad
    }
}

#[derive(Debug)]
struct HkdfExpandError(hkdf::InvalidLength);

impl fmt::Display for HkdfExpandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HKDF output length is invalid")
    }
}

impl std::error::Error for HkdfExpandError {}

fn validate_framing(
    encrypted: &EncryptedBlob,
    aad: &[u8],
) -> Result<(), ProfileContentKeyVaultError> {
    encrypted
        .validate_basic()
        .map_err(|source| ProfileContentKeyVaultError::Corrupt {
            source: anyhow::Error::new(source).context("validate encrypted content key vault"),
        })?;
    let aad_digest = blake3::hash(aad);
    let expected = &aad_digest.as_bytes()[..16];
    if encrypted.aad_fingerprint.as_deref() != Some(expected) {
        return Err(ProfileContentKeyVaultError::Corrupt {
            source: anyhow::anyhow!("encrypted content key vault AAD fingerprint is invalid"),
        });
    }
    Ok(())
}

async fn write_atomically(
    path: &Path,
    ciphertext: &[u8],
) -> Result<(), ProfileContentKeyVaultError> {
    let parent = path
        .parent()
        .ok_or_else(|| ProfileContentKeyVaultError::Storage {
            source: anyhow::anyhow!("profile content vault parent is missing"),
        })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(storage_error)?;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await
            .map_err(storage_error)?;
        file.write_all(ciphertext).await.map_err(storage_error)?;
        file.sync_all().await.map_err(storage_error)?;
        drop(file);
        replace_file_atomically(&temporary, path).map_err(storage_error)?;
        sync_parent_directory(parent).map_err(storage_error)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

#[cfg(not(windows))]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_file_atomically(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let wide = |path: &Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &Path) -> std::io::Result<()> {
    Ok(())
}

fn secure_storage_error(source: SecureStorageError) -> ProfileContentKeyVaultError {
    ProfileContentKeyVaultError::SecureStorage {
        source: anyhow::Error::new(source).context("access profile content vault key"),
    }
}

fn storage_error(source: std::io::Error) -> ProfileContentKeyVaultError {
    ProfileContentKeyVaultError::Storage {
        source: anyhow::Error::new(source).context("persist profile content key vault"),
    }
}
