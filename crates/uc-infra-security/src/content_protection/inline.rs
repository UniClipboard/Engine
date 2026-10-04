use std::sync::Arc;

use async_trait::async_trait;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ports::security::{BlobCipherError, BlobCipherPort};

use super::{ContentProtection, ContentProtectionError};

/// 将 Core 持久 inline cipher port 收口到 V3 `ContentProtection` 的薄 adapter。
pub struct V3InlinePayloadCipher {
    protection: Arc<ContentProtection>,
}

impl V3InlinePayloadCipher {
    pub fn new(protection: Arc<ContentProtection>) -> Self {
        Self { protection }
    }
}

#[async_trait]
impl BlobCipherPort for V3InlinePayloadCipher {
    async fn encrypt(
        &self,
        plaintext: &Plaintext,
        aad: &Aad,
    ) -> Result<Ciphertext, BlobCipherError> {
        self.protection
            .seal_for_active(plaintext, aad)
            .await
            .map_err(map_error)
    }

    async fn decrypt(
        &self,
        ciphertext: &Ciphertext,
        aad: &Aad,
    ) -> Result<Plaintext, BlobCipherError> {
        self.protection
            .open(ciphertext, aad)
            .await
            .map_err(map_error)
    }
}

fn map_error(error: ContentProtectionError) -> BlobCipherError {
    match error {
        error @ ContentProtectionError::NotActive { .. } => BlobCipherError::not_unlocked(
            anyhow::Error::new(error).context("V3 inline protection is not active"),
        ),
        error @ (ContentProtectionError::InvalidCiphertext { .. }
        | ContentProtectionError::KeyUnavailable { .. }) => BlobCipherError::invalid_ciphertext(
            anyhow::Error::new(error).context("V3 inline ciphertext cannot be opened"),
        ),
        error @ (ContentProtectionError::InvalidActiveContext { .. }
        | ContentProtectionError::Cryptography { .. }) => BlobCipherError::internal(
            anyhow::Error::new(error).context("V3 inline content protection failed"),
        ),
    }
}
