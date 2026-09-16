use async_trait::async_trait;
use uc_core::crypto::domain::Passphrase;

#[derive(Debug, thiserror::Error)]
pub enum ApplyEncryptionPassphraseChangePortError {
    #[error("passphrase change requires recovery")]
    RecoveryRequired {
        #[source]
        source: anyhow::Error,
    },
    #[error("passphrase change is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait ApplyEncryptionPassphraseChangePort: Send + Sync {
    /// 完成上次可能中断的替换；没有待恢复替换时不产生变化。
    async fn ensure_encryption_passphrase_change_ready(
        &self,
    ) -> Result<(), ApplyEncryptionPassphraseChangePortError>;

    /// 保留当前 MasterKey，只替换本机解锁保护与配对认证资料。
    async fn apply_encryption_passphrase_change(
        &self,
        passphrase: &Passphrase,
    ) -> Result<(), ApplyEncryptionPassphraseChangePortError>;
}

#[async_trait]
pub trait RetirePairingInvitationsPort: Send + Sync {
    /// 撤销所有实际已签发邀请；没有邀请时也成功。
    async fn retire_all(&self) -> Result<(), anyhow::Error>;
}
