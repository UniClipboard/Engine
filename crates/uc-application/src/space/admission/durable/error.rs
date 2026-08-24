#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AdmissionAttemptRepositoryError {
    #[error("admission attempt storage is locked")]
    Locked,
    #[error("admission attempt storage is corrupt")]
    Corrupt,
    #[error("admission attempt already exists")]
    AlreadyExists,
    #[error("admission attempt was not found")]
    NotFound,
    #[error("admission attempt version conflicts with persisted state")]
    VersionConflict,
    #[error("the previous local join cannot be superseded")]
    PreviousJoinCannotBeSuperseded,
    #[error("admission attempt repository failed: {0}")]
    Repository(String),
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("admission outbox delivery is temporarily unavailable")]
pub struct AdmissionOutboxDeliveryError;
