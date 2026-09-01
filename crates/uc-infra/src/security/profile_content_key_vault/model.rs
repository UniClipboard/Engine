use uc_core::membership::{ContentKeyId, GroupEpoch, ProtectionGroupId};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::super::MasterKey;

pub(super) const FORMAT_VERSION_V1: u16 = 1;
pub(super) const MAX_VAULT_PLAINTEXT_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_GROUPS: usize = 128;
pub(super) const MAX_ENTRIES_PER_GROUP: usize = 1024;
pub(super) const MAX_TOTAL_ENTRIES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum ProfileContentKeyVaultError {
    #[error("profile content key secure storage is unavailable")]
    SecureStorage {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile content key vault storage is unavailable")]
    Storage {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile content key material is invalid")]
    InvalidMaterial {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile content key vault is corrupt")]
    Corrupt {
        #[source]
        source: anyhow::Error,
    },
    #[error("profile content key vault conflicts with verified material")]
    Conflict,
    #[error("profile content key was not found")]
    KeyNotFound,
    #[error("profile content key epoch does not match")]
    EpochMismatch,
    #[error("profile content key vault capacity was exceeded")]
    CapacityExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstalledProfileCatalog {
    pub(super) revision: u64,
    pub(super) group_count: usize,
    pub(super) entry_count: usize,
    pub(super) changed: bool,
}

impl InstalledProfileCatalog {
    pub const fn revision(self) -> u64 {
        self.revision
    }

    pub const fn group_count(self) -> usize {
        self.group_count
    }

    pub const fn entry_count(self) -> usize {
        self.entry_count
    }

    pub const fn changed(self) -> bool {
        self.changed
    }
}

pub struct ResolvedProfileContentKey {
    pub(super) protection_group_id: ProtectionGroupId,
    pub(super) content_key_id: ContentKeyId,
    pub(super) epoch: GroupEpoch,
    pub(super) key: MasterKey,
}

/// 搜索模块可使用的 profile 稳定根能力与已安装保护组快照。
///
/// 该类型只在 Infra 内流转，不暴露 vault 外层 AEAD key；搜索模块必须再按
/// 固定 domain 和保护组构造 group ref 与 term tag。
pub(crate) struct ProfileSearchCatalog {
    pub(super) root_key: MasterKey,
    pub(super) protection_groups: Vec<ProtectionGroupId>,
}

impl ProfileSearchCatalog {
    pub(crate) fn root_key(&self) -> &MasterKey {
        &self.root_key
    }

    pub(crate) fn protection_groups(&self) -> &[ProtectionGroupId] {
        &self.protection_groups
    }
}

impl std::fmt::Debug for ProfileSearchCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProfileSearchCatalog")
            .field("root_key", &"[REDACTED]")
            .field("protection_groups", &"[REDACTED]")
            .finish()
    }
}

impl ResolvedProfileContentKey {
    pub fn protection_group_id(&self) -> &ProtectionGroupId {
        &self.protection_group_id
    }

    pub fn content_key_id(&self) -> &ContentKeyId {
        &self.content_key_id
    }

    pub const fn epoch(&self) -> GroupEpoch {
        self.epoch
    }

    pub(crate) fn key(&self) -> &MasterKey {
        &self.key
    }
}

impl std::fmt::Debug for ResolvedProfileContentKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedProfileContentKey")
            .field("identifiers", &"[REDACTED]")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct PersistedVault {
    pub(super) format_version: u16,
    pub(super) revision: u64,
    pub(super) groups: Vec<PersistedGroup>,
}

#[derive(serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct PersistedGroup {
    pub(super) protection_group_id: String,
    pub(super) space_id: String,
    pub(super) entries: Vec<PersistedEntry>,
    pub(super) catalog_digest: [u8; 32],
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct PersistedEntry {
    pub(super) content_key_id: String,
    pub(super) epoch: u64,
    pub(super) key: Vec<u8>,
}

pub(super) fn invalid_material(context: &'static str) -> ProfileContentKeyVaultError {
    ProfileContentKeyVaultError::InvalidMaterial {
        source: anyhow::anyhow!(context),
    }
}

pub(super) fn corrupt(context: &'static str) -> ProfileContentKeyVaultError {
    ProfileContentKeyVaultError::Corrupt {
        source: anyhow::anyhow!(context),
    }
}
