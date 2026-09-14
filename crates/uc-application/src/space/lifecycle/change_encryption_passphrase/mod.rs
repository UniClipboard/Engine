mod error;
mod ports;
mod use_case;

pub use error::ChangeEncryptionPassphraseError;
pub use ports::{
    ApplyEncryptionPassphraseChangePort, ApplyEncryptionPassphraseChangePortError,
    RetirePairingInvitationsPort,
};
pub(crate) use use_case::ChangeEncryptionPassphraseUseCase;
