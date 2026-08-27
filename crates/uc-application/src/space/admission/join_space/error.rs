#[derive(Debug, thiserror::Error)]
pub enum JoinSpaceError {
    #[error("device name is required")]
    DeviceNameRequired,

    #[error("failed to save device name: {0}")]
    Settings(String),

    #[error("space admission is locked")]
    Locked,

    #[error("space admission state changed")]
    StateChanged,

    #[error("space admission requires recovery")]
    RecoveryRequired,

    #[error("space admission is unavailable")]
    Unavailable,

    #[error("the invitation cannot start a new admission")]
    InvalidInvitation,

    #[error("the previous local join cannot be superseded")]
    PreviousJoinCannotBeSuperseded,

    #[error("the generated join material is invalid")]
    InvalidStartMaterial,
}
