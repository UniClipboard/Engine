use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum LegacyMigrationRecoveryError {
    #[error("legacy migration requires manual recovery")]
    RecoveryRequired {
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    #[error("legacy migration recovery failed")]
    Internal(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl LegacyMigrationRecoveryError {
    pub fn recovery_required() -> Self {
        Self::RecoveryRequired { source: None }
    }

    pub fn recovery_required_from(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::RecoveryRequired {
            source: Some(Box::new(source)),
        }
    }
}

#[async_trait]
pub trait LegacyMigrationRecoveryPort: Send + Sync {
    async fn recover(&self) -> Result<(), LegacyMigrationRecoveryError>;
}
