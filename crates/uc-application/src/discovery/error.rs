use anyhow::Error;
use uc_core::error_class::ErrorClass;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("discovery provider failed")]
    Provider {
        #[source]
        source: Error,
    },
    #[error("discovery directory lock is poisoned")]
    DirectoryPoisoned,
    #[error("discovery observation is invalid or exceeds limits")]
    InvalidObservation,
    #[error("duplicate discovery provider identifier")]
    DuplicateProvider,
    #[error("discovery provider stopped unexpectedly")]
    ProviderStopped,
    #[error("discovery manager is shut down")]
    Stopped,
    #[error("discovery manager is already initialized")]
    AlreadyInitialized,
    #[error("discovery manager was not initialized")]
    NotInitialized,
}

impl ErrorClass for DiscoveryError {
    fn class(&self) -> &'static str {
        match self {
            Self::Provider { .. } => "discovery_provider_failed",
            Self::DirectoryPoisoned => "discovery_directory_poisoned",
            Self::InvalidObservation => "discovery_invalid_observation",
            Self::DuplicateProvider => "discovery_duplicate_provider",
            Self::ProviderStopped => "discovery_provider_stopped",
            Self::Stopped => "discovery_stopped",
            Self::AlreadyInitialized => "discovery_already_initialized",
            Self::NotInitialized => "discovery_not_initialized",
        }
    }
}
