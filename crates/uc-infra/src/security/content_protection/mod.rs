mod blob_store;
mod context;
mod envelope;
mod inline;

pub use blob_store::V3EncryptedBlobStore;
pub use inline::V3InlinePayloadCipher;

use std::sync::Arc;

use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::membership::{ContentKeyId, ContentKeyPurpose};

use crate::security::v1_aead::{decrypt_xchacha_raw, encrypt_xchacha_raw};
use crate::security::ProfileContentKeyVault;
use crate::space::InMemorySession;

use context::ProtectionContextV1;

/// 本机 V3 持久业务负载的唯一加解密入口。
///
/// purpose 在构造时固定；写入上下文来自活动 session，读取上下文则只由密文
/// 引用和 profile vault 重建，因此切换活动 Space 不会改变历史密文的读法。
pub struct ContentProtection {
    session: Arc<InMemorySession>,
    vault: Arc<ProfileContentKeyVault>,
    purpose: ContentKeyPurpose,
}

#[derive(Debug, thiserror::Error)]
pub enum ContentProtectionError {
    #[error("no V3 content protection context is active")]
    NotActive {
        #[source]
        source: anyhow::Error,
    },
    #[error("the active V3 content protection context is invalid")]
    InvalidActiveContext {
        #[source]
        source: anyhow::Error,
    },
    #[error("the V3 ciphertext is invalid")]
    InvalidCiphertext {
        #[source]
        source: anyhow::Error,
    },
    #[error("the V3 content key is unavailable")]
    KeyUnavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("V3 content cryptography failed")]
    Cryptography {
        #[source]
        source: anyhow::Error,
    },
}

impl ContentProtection {
    pub fn for_content(session: Arc<InMemorySession>, vault: Arc<ProfileContentKeyVault>) -> Self {
        Self::new(session, vault, ContentKeyPurpose::Content)
    }

    pub fn for_search(session: Arc<InMemorySession>, vault: Arc<ProfileContentKeyVault>) -> Self {
        Self::new(session, vault, ContentKeyPurpose::Search)
    }

    fn new(
        session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
        purpose: ContentKeyPurpose,
    ) -> Self {
        Self {
            session,
            vault,
            purpose,
        }
    }

    pub async fn seal_for_active(
        &self,
        plaintext: &Plaintext,
        aad: &Aad,
    ) -> Result<Ciphertext, ContentProtectionError> {
        let session_was_ready = self.session.is_ready();
        let active = self
            .session
            .current_content_protection_key()
            .map_err(|source| {
                let source =
                    anyhow::Error::new(source).context("active content key was unavailable");
                if session_was_ready {
                    ContentProtectionError::InvalidActiveContext { source }
                } else {
                    ContentProtectionError::NotActive { source }
                }
            })?;
        if active.content_key_id() == &ContentKeyId::legacy_v1() {
            return Err(ContentProtectionError::InvalidActiveContext {
                source: anyhow::anyhow!("legacy content key cannot protect V3 ciphertext"),
            });
        }
        let context = ProtectionContextV1::new(
            active.protection_group_id().clone(),
            active.content_key_id().clone(),
            active.epoch(),
            self.purpose,
        );
        let purpose_key = context.derive_purpose_key(active.key())?;
        let canonical_aad = context.canonical_aad(aad)?;
        let (nonce, encrypted) = encrypt_xchacha_raw(
            purpose_key.as_bytes(),
            plaintext.as_bytes(),
            canonical_aad.as_slice(),
        )
        .map_err(|source| ContentProtectionError::Cryptography {
            source: anyhow::Error::new(source).context("V3 content encryption failed"),
        })?;
        let encoded = envelope::encode(
            context.content_key_id(),
            context.group_epoch(),
            nonce,
            encrypted,
        )?;
        Ok(Ciphertext::new(encoded))
    }

    pub async fn open(
        &self,
        ciphertext: &Ciphertext,
        aad: &Aad,
    ) -> Result<Plaintext, ContentProtectionError> {
        let envelope = envelope::decode(ciphertext.as_bytes())?;
        let resolved = self
            .vault
            .resolve(&envelope.content_key_id, envelope.group_epoch)
            .await
            .map_err(|source| ContentProtectionError::KeyUnavailable {
                source: anyhow::Error::new(source).context("V3 content key resolution failed"),
            })?;
        let context = ProtectionContextV1::new(
            resolved.protection_group_id().clone(),
            resolved.content_key_id().clone(),
            resolved.epoch(),
            self.purpose,
        );
        let purpose_key = context.derive_purpose_key(resolved.key())?;
        let canonical_aad = context.canonical_aad(aad)?;
        let plaintext = decrypt_xchacha_raw(
            purpose_key.as_bytes(),
            &envelope.nonce,
            &envelope.ciphertext,
            canonical_aad.as_slice(),
        )
        .map_err(|source| ContentProtectionError::InvalidCiphertext {
            source: anyhow::Error::new(source).context("V3 ciphertext authentication failed"),
        })?;
        Ok(Plaintext::new(plaintext))
    }
}

#[cfg(test)]
mod tests;
