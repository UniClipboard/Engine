use crate::space::lifecycle::SpaceActivityError;

#[derive(Debug, thiserror::Error)]
pub enum RecoverSpaceSessionError {
    #[error("failed to load current Space identity")]
    CurrentSpace(#[source] anyhow::Error),
    #[error("cached master key is not available from secure storage")]
    KeyringMiss,
    #[error("space key material corrupted")]
    CorruptedKeyMaterial,
    #[error(transparent)]
    Activity(#[from] SpaceActivityError),
    /// 本地数据准备暂时不可用（例如 Space 重建提交时的锁争用）；持久进度可续做，稍后重试即可。
    #[error("space session recovery is temporarily unavailable")]
    Unavailable(#[source] anyhow::Error),
    #[error("space session recovery failed")]
    Internal(#[source] anyhow::Error),
}
