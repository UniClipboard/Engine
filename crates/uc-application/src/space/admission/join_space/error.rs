use crate::facade::space_setup::RedeemPairingInvitationError;

#[derive(Debug, thiserror::Error)]
pub enum JoinSpaceError {
    #[error("device name is required")]
    DeviceNameRequired,

    #[error("failed to save device name: {0}")]
    Settings(String),

    #[error(transparent)]
    Admission(#[from] RedeemPairingInvitationError),

    #[error("saved join state is unavailable: {0}")]
    SavedState(String),
}
