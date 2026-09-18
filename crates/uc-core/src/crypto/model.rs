//! Security / Encryption domain models.
//!
//! 本模块最终保留的跨 crate 领域符号:
//! - `Passphrase`: 用户提供的解锁口令;uc-application / cli 等领域输入类型
//! - `EncryptionError`: 跨 crate 错误类型(uc-infra `KeySlotStore` port 等返回)
//!
//! 其余符号已物理下沉到 `uc-infra/src/security/`:
//! - `Kek` / `MasterKey`  → `secrets.rs`(Slice 4 B.4.5)
//! - `KdfParams` / `KdfParamsV1` / `KeySlot` / `WrappedMasterKey` /
//!   `EncryptedBlob` / `KeySlotFile` / `KeySlotConvertError` / `KeyScope`
//!   → `crypto_model.rs`(Slice 6-7)
//!
//! `KeyScopePort` → `CurrentProfilePort`(Slice 7 U7 候选 B),返回
//! `uc_core::ids::ProfileId` 值对象。

use anyhow::Error;
use std::fmt;

use crate::ports::SecureStorageError;

/// Passphrase provided by user. Only used to derive KEK inside use cases.
/// Avoid storing this beyond the unlock/initialize flow.
#[derive(Clone)]
pub struct Passphrase(pub String);

impl fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Passphrase([REDACTED])")
    }
}

impl Passphrase {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

#[derive(thiserror::Error)]
pub enum EncryptionError {
    #[error("encryption is not initialized")]
    NotInitialized,

    #[error("encryption is locked")]
    Locked,

    #[error("wrong passphrase")]
    WrongPassphrase,

    #[error("unsupported keyslot version")]
    UnsupportedKeySlotVersion,

    #[error("unsupported blob format version")]
    UnsupportedBlobVersion,

    #[error("corrupted keyslot data")]
    CorruptedKeySlot,

    #[error("corrupted encrypted blob")]
    CorruptedBlob,

    #[error("internal crypto failure")]
    CryptoFailure,

    #[error("invalid key")]
    InvalidKey,

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),

    #[error("KDF operation failed")]
    KdfFailed,

    #[error("unsupported KDF algorithm")]
    UnsupportedKdfAlgorithm,

    #[error("encryption failed")]
    EncryptFailed,
    /// Keyring / Key Material errors

    #[error("key material not found")]
    KeyNotFound, // keyring 或 keyslot 缺失

    #[error("key material is corrupt")]
    KeyMaterialCorrupt, // keyslot 或 keyring 内容损坏/长度不对/反序列化失败

    #[error("other encryption error: {0}")]
    KeyringError(String),

    #[error("permission denied for key material access")]
    PermissionDenied, // keyring 权限/系统拒绝

    #[error("I/O failure during key material access")]
    IoFailure, // 文件/DB IO

    #[error("unsupported version for key material")]
    UnsupportedVersion, // keyslot/blob 版本不支持

    #[error("key material access failed")]
    KeyMaterialAccessFailed {
        #[source]
        source: Error,
    },
}

impl fmt::Debug for EncryptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 底层访问失败可能携带宿主信息，不通过默认 Debug 展开来源。
        fmt::Display::fmt(self, formatter)
    }
}

impl From<SecureStorageError> for EncryptionError {
    fn from(source: SecureStorageError) -> Self {
        match source {
            SecureStorageError::PermissionDenied(_) => Self::PermissionDenied,
            SecureStorageError::Corrupt(_) => Self::KeyMaterialCorrupt,
            SecureStorageError::Unavailable(message) | SecureStorageError::Other(message) => {
                Self::KeyringError(message)
            }
            SecureStorageError::AccessFailed(failure) => Self::KeyMaterialAccessFailed {
                source: failure.into_source(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io;

    use crate::ports::{SecureStorageAccessFailure, SecureStorageError};

    use super::EncryptionError;

    #[test]
    fn key_material_access_failure_retains_source_without_displaying_it() {
        let error = EncryptionError::KeyMaterialAccessFailed {
            source: io::Error::other("private host payload").into(),
        };
        assert!(error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .is_some());
        assert!(!format!("{error:?}").contains("private"));
        assert!(!format!("{error}").contains("private"));
    }

    #[test]
    fn secure_storage_failures_use_stable_encryption_classifications() {
        assert!(matches!(
            EncryptionError::from(SecureStorageError::PermissionDenied("private".into())),
            EncryptionError::PermissionDenied
        ));
        assert!(matches!(
            EncryptionError::from(SecureStorageError::Corrupt("private".into())),
            EncryptionError::KeyMaterialCorrupt
        ));

        let error = EncryptionError::from(SecureStorageError::AccessFailed(
            SecureStorageAccessFailure::new(io::Error::other("private host payload")),
        ));
        assert!(error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .is_some());
        assert!(!format!("{error:?}").contains("private"));
    }
}
