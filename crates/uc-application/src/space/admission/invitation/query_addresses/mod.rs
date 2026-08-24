mod error;
mod use_case;

pub use error::QueryPairingInvitationAddressesError;
pub use uc_core::ports::pairing_invitation::PairingInvitationAddressCandidate;
pub(crate) use use_case::QueryPairingInvitationAddressesUseCase;
