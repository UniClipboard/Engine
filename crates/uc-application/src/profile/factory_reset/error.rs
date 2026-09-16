#[derive(Debug, thiserror::Error)]
pub enum ProfileFactoryResetError {
    #[error(transparent)]
    Lifecycle(#[from] ProfileLifecycleError),
    #[error(transparent)]
    Repository(#[from] ProfileLifecycleRepositoryError),
    #[error("profile lifecycle state is missing")]
    LifecycleMissing,
    #[error("profile runtime could not be stopped")]
    StopRuntime,
    #[error("profile keys could not be wiped")]
    WipeKeys,
    #[error("profile state could not be cleared")]
    ClearState,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProfileLifecycleError {
    #[error("profile generation does not match the active lifecycle")]
    GenerationConflict,
    #[error("profile lifecycle transition is not allowed from the current state")]
    InvalidTransition,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProfileLifecycleRepositoryError {
    #[error("profile lifecycle storage is unavailable")]
    Unavailable,
    #[error("profile lifecycle record is corrupt")]
    Corrupt,
    #[error("profile lifecycle record changed before it could be saved")]
    Conflict,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("profile factory reset capability failed")]
pub struct ProfileFactoryResetCapabilityError;
