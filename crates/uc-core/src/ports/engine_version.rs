//! Persisted cursor for the Engine version that completed its migrations.

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum EngineVersionStateError {
    #[error("failed to read the stored Engine version")]
    Read(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("stored Engine version is invalid")]
    Invalid(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("failed to record the Engine version")]
    Write(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait]
pub trait EngineVersionStatePort: Send + Sync {
    async fn read(&self) -> Result<Option<String>, EngineVersionStateError>;
    async fn write(&self, version: &str) -> Result<(), EngineVersionStateError>;
}
