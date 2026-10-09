//! Profile 口令恢复的 port 定义——住在真正的消费者（security）这一侧。
//!
//! 实现（`ProfileKeyRecoveryStore`）位于 `uc-infra-profile`（`security::profile_key_recovery`），
//! 因为它需要 app 目录布局等 profile 专属上下文；但 trait 定义和其错误类型
//! 必须住在消费者这一侧,否则 security 就要反向依赖 profile。
//! 见 Engine issue #144 第 5 节"依赖切断"第 1 条。
use uc_core::crypto::model::EncryptionError;
use uc_core::ports::SecureStorageError;
use uc_infra_crypto::v1_aead;

#[derive(Debug, thiserror::Error)]
pub enum ProfileKeyRecoveryError {
    #[error("profile recovery passphrase was rejected")]
    WrongPassphrase,
    #[error("profile recovery data is corrupt")]
    Corrupt,
    #[error("profile recovery format is unsupported")]
    Unsupported,
    #[error("profile recovery storage is unavailable")]
    Storage(#[source] anyhow::Error),
}

impl ProfileKeyRecoveryError {
    /// 诊断用固定分类；不包含下层错误正文。
    pub fn diagnostic_reason(&self) -> &'static str {
        match self {
            Self::WrongPassphrase => "key_mismatch",
            Self::Corrupt => "corrupt",
            Self::Unsupported => "unsupported_version",
            Self::Storage(_) => "storage_unavailable",
        }
    }
}

impl From<SecureStorageError> for ProfileKeyRecoveryError {
    fn from(source: SecureStorageError) -> Self {
        Self::Storage(source.into())
    }
}

impl From<std::io::Error> for ProfileKeyRecoveryError {
    fn from(source: std::io::Error) -> Self {
        Self::Storage(source.into())
    }
}

impl From<EncryptionError> for ProfileKeyRecoveryError {
    fn from(source: EncryptionError) -> Self {
        match source {
            EncryptionError::WrongPassphrase => Self::WrongPassphrase,
            EncryptionError::UnsupportedKeySlotVersion
            | EncryptionError::UnsupportedBlobVersion
            | EncryptionError::UnsupportedVersion
            | EncryptionError::UnsupportedKdfAlgorithm => Self::Unsupported,
            EncryptionError::CorruptedKeySlot
            | EncryptionError::CorruptedBlob
            | EncryptionError::KeyMaterialCorrupt { .. }
            | EncryptionError::InvalidKey => Self::Corrupt,
            other => Self::Storage(other.into()),
        }
    }
}

impl From<v1_aead::AeadError> for ProfileKeyRecoveryError {
    fn from(source: v1_aead::AeadError) -> Self {
        Self::Storage(anyhow::Error::new(source))
    }
}

pub trait ProfilePassphraseRecoveryPort: Send + Sync {
    fn prepare_passphrase_change(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError>;
    fn finish_passphrase_change(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError>;
    /// 资料 KEK 被切换目标的访问材料替换前调用；vault 可能尚未创建或已随运行期挂起。
    fn prepare_kek_replacement(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError>;
    /// 新 KEK 写入安全存储后调用。
    fn finish_kek_replacement(&self, kek: &[u8]) -> Result<(), ProfileKeyRecoveryError>;
}
