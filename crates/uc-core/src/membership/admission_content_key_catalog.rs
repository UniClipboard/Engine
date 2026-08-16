use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::MembershipHistoryV2Error;

pub const ADMISSION_CONTENT_KEY_CATALOG_FORMAT_V1: u16 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionContentKeyEntryV1 {
    pub content_key_id: String,
    pub epoch: u64,
    pub key: Vec<u8>,
}

impl AdmissionContentKeyEntryV1 {
    pub fn new(
        content_key_id: impl Into<String>,
        epoch: u64,
        key: Vec<u8>,
    ) -> Result<Self, MembershipHistoryV2Error> {
        let entry = Self {
            content_key_id: content_key_id.into(),
            epoch,
            key,
        };
        entry.validate()?;
        Ok(entry)
    }

    fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.content_key_id.is_empty()
            || self.content_key_id.len() > 128
            || !self.content_key_id.is_ascii()
            || self.key.len() != 32
        {
            return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
        }
        Ok(())
    }
}

impl std::fmt::Debug for AdmissionContentKeyEntryV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionContentKeyEntryV1")
            .field("content_key_id", &self.content_key_id)
            .field("epoch", &self.epoch)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionContentKeyCatalogV1 {
    pub format_version: u16,
    pub current_content_key_id: String,
    pub target_epoch: u64,
    pub entries: Vec<AdmissionContentKeyEntryV1>,
}

impl AdmissionContentKeyCatalogV1 {
    pub fn new(
        current_content_key_id: impl Into<String>,
        target_epoch: u64,
        mut entries: Vec<AdmissionContentKeyEntryV1>,
    ) -> Result<Self, MembershipHistoryV2Error> {
        entries.sort_by(|left, right| {
            left.content_key_id
                .as_bytes()
                .cmp(right.content_key_id.as_bytes())
                .then(left.epoch.cmp(&right.epoch))
        });
        let catalog = Self {
            format_version: ADMISSION_CONTENT_KEY_CATALOG_FORMAT_V1,
            current_content_key_id: current_content_key_id.into(),
            target_epoch,
            entries,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.format_version != ADMISSION_CONTENT_KEY_CATALOG_FORMAT_V1
            || self.current_content_key_id.is_empty()
            || self.current_content_key_id.len() > 128
            || !self.current_content_key_id.is_ascii()
            || self.entries.is_empty()
        {
            return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
        }
        let mut ids = BTreeSet::new();
        let mut epochs = BTreeSet::new();
        let mut previous: Option<&AdmissionContentKeyEntryV1> = None;
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(entry.content_key_id.as_str()) || !epochs.insert(entry.epoch) {
                return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
            }
            if let Some(previous) = previous {
                let order = previous
                    .content_key_id
                    .as_bytes()
                    .cmp(entry.content_key_id.as_bytes())
                    .then(previous.epoch.cmp(&entry.epoch));
                if !order.is_lt() {
                    return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
                }
            }
            previous = Some(entry);
        }
        if !self
            .entries
            .iter()
            .any(|entry| entry.content_key_id == "legacy-v1" && entry.epoch == 0)
            || !self.entries.iter().any(|entry| {
                entry.content_key_id == self.current_content_key_id
                    && entry.epoch == self.target_epoch
            })
        {
            return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
        }
        Ok(())
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/admission-content-key-catalog/v1\0");
        hasher.update(self.format_version.to_be_bytes());
        append_field(&mut hasher, self.current_content_key_id.as_bytes());
        hasher.update(self.target_epoch.to_be_bytes());
        hasher.update((self.entries.len() as u64).to_be_bytes());
        for entry in &self.entries {
            append_field(&mut hasher, entry.content_key_id.as_bytes());
            hasher.update(entry.epoch.to_be_bytes());
            append_field(&mut hasher, &entry.key);
        }
        hasher.finalize().into()
    }

    pub fn encode(&self) -> Result<Vec<u8>, MembershipHistoryV2Error> {
        self.validate()?;
        postcard::to_stdvec(self).map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MembershipHistoryV2Error> {
        let catalog: Self = postcard::from_bytes(bytes)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        catalog.validate()?;
        Ok(catalog)
    }
}

impl std::fmt::Debug for AdmissionContentKeyCatalogV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionContentKeyCatalogV1")
            .field("format_version", &self.format_version)
            .field("current_content_key_id", &self.current_content_key_id)
            .field("target_epoch", &self.target_epoch)
            .field("entry_count", &self.entries.len())
            .finish()
    }
}

fn append_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}
