use anyhow::Context;
use hkdf::Hkdf;
use sha2::Sha256;
use std::fmt;
use uc_core::crypto::domain::Aad;
use uc_core::membership::{ContentKeyId, ContentKeyPurpose, GroupEpoch, ProtectionGroupId};
use zeroize::Zeroizing;

use crate::security::MasterKey;

use super::envelope::{AEAD_XCHACHA20_POLY1305, FORMAT_VERSION_V3};
use super::ContentProtectionError;

const KEY_DOMAIN: &[u8] = b"uniclipboard/content-protection/key/v1";
const AAD_DOMAIN: &[u8] = b"uniclipboard/content-protection/aad/v1";

pub(super) struct ProtectionContextV1 {
    protection_group_id: ProtectionGroupId,
    content_key_id: ContentKeyId,
    group_epoch: GroupEpoch,
    purpose: ContentKeyPurpose,
}

impl ProtectionContextV1 {
    pub(super) fn new(
        protection_group_id: ProtectionGroupId,
        content_key_id: ContentKeyId,
        group_epoch: GroupEpoch,
        purpose: ContentKeyPurpose,
    ) -> Self {
        Self {
            protection_group_id,
            content_key_id,
            group_epoch,
            purpose,
        }
    }

    pub(super) fn content_key_id(&self) -> &ContentKeyId {
        &self.content_key_id
    }

    pub(super) const fn group_epoch(&self) -> GroupEpoch {
        self.group_epoch
    }

    pub(super) fn derive_purpose_key(
        &self,
        raw_key: &MasterKey,
    ) -> Result<MasterKey, ContentProtectionError> {
        let mut info = Vec::new();
        append_field(&mut info, KEY_DOMAIN).map_err(cryptography)?;
        append_field(&mut info, self.purpose.as_str().as_bytes()).map_err(cryptography)?;
        let hkdf = Hkdf::<Sha256>::new(
            Some(self.protection_group_id.as_str().as_bytes()),
            raw_key.as_bytes(),
        );
        let mut output = Zeroizing::new([0u8; MasterKey::LEN]);
        hkdf.expand(&info, output.as_mut())
            .map_err(|source| cryptography(anyhow::Error::new(HkdfExpandError(source))))?;
        MasterKey::from_bytes(output.as_ref()).map_err(|source| {
            cryptography(anyhow::Error::new(source).context("derived content key was invalid"))
        })
    }

    pub(super) fn canonical_aad(
        &self,
        business_aad: &Aad,
    ) -> Result<Vec<u8>, ContentProtectionError> {
        let mut output = Vec::new();
        append_field(&mut output, AAD_DOMAIN).map_err(cryptography)?;
        append_field(&mut output, &FORMAT_VERSION_V3.to_be_bytes()).map_err(cryptography)?;
        append_field(&mut output, AEAD_XCHACHA20_POLY1305.as_bytes()).map_err(cryptography)?;
        append_field(&mut output, self.protection_group_id.as_str().as_bytes())
            .map_err(cryptography)?;
        append_field(&mut output, self.content_key_id.as_str().as_bytes()).map_err(cryptography)?;
        append_field(&mut output, &self.group_epoch.value().to_be_bytes()).map_err(cryptography)?;
        append_field(&mut output, self.purpose.as_str().as_bytes()).map_err(cryptography)?;
        append_field(&mut output, business_aad.as_bytes()).map_err(cryptography)?;
        Ok(output)
    }
}

fn append_field(target: &mut Vec<u8>, value: &[u8]) -> anyhow::Result<()> {
    let length = u64::try_from(value.len()).context("protection context field is too large")?;
    target.extend_from_slice(&length.to_be_bytes());
    target.extend_from_slice(value);
    Ok(())
}

fn cryptography(source: anyhow::Error) -> ContentProtectionError {
    ContentProtectionError::Cryptography {
        source: source.context("content protection context construction failed"),
    }
}

#[derive(Debug)]
struct HkdfExpandError(hkdf::InvalidLength);

impl fmt::Display for HkdfExpandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HKDF output length is invalid")
    }
}

impl std::error::Error for HkdfExpandError {}
