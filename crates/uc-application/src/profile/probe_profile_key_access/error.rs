#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProbeProfileKeyAccessError {
    #[error("profile key access is not initialized")]
    NotInitialized,
    #[error(transparent)]
    Probe(#[from] ProfileKeyAccessProbePortError),
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("profile key access probe failed")]
pub struct ProfileKeyAccessProbePortError;
