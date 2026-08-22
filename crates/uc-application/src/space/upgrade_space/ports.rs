use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum EngineVersionStateError {
    #[error("failed to read the stored Engine version: {0}")]
    Read(String),

    #[error("stored Engine version is invalid: {0}")]
    Invalid(String),

    #[error("failed to record the Engine version: {0}")]
    Write(String),
}

#[async_trait]
pub trait EngineVersionStatePort: Send + Sync {
    async fn read(&self) -> Result<Option<String>, EngineVersionStateError>;
    async fn write(&self, version: &str) -> Result<(), EngineVersionStateError>;
}
