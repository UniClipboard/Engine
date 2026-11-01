//! Classify at the typed adapter boundary; observability does not inspect security internals.

use std::io;
use uc_core::crypto::model::EncryptionError;
use uc_core::ports::security::BlobCipherError;
use uc_infra_security::ContentProtectionError;

pub(super) fn cipher_error(error: BlobCipherError) -> io::Error {
    let kind = match &error {
        BlobCipherError::NotUnlocked { .. } => io::ErrorKind::WouldBlock,
        BlobCipherError::Internal { .. } => io::ErrorKind::Other,
        BlobCipherError::InvalidCiphertext { source } => {
            match source.downcast_ref::<ContentProtectionError>() {
                Some(
                    ContentProtectionError::KeyUnavailable { .. }
                    | ContentProtectionError::NotActive { .. },
                ) => io::ErrorKind::WouldBlock,
                Some(ContentProtectionError::InvalidCiphertext { .. }) => {
                    io::ErrorKind::InvalidData
                }
                Some(
                    ContentProtectionError::InvalidActiveContext { .. }
                    | ContentProtectionError::Cryptography { .. },
                ) => io::ErrorKind::Other,
                // Legacy key resolution can also be wrapped as InvalidCiphertext. Retry conservatively.
                None if source.downcast_ref::<EncryptionError>().is_some() => {
                    io::ErrorKind::WouldBlock
                }
                None => io::ErrorKind::InvalidData,
            }
        }
    };
    io::Error::new(kind, error)
}
