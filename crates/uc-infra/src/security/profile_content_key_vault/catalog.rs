use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ContentKeyId, GroupEpoch, ProtectionGroupId, SpaceKeyMaterial, SpaceSecurityMode,
};

use crate::space::export_admission_content_key_catalog;

use super::super::MasterKey;
use super::model::{
    corrupt, invalid_material, InstalledProfileCatalog, PersistedEntry, PersistedGroup,
    PersistedVault, ProfileContentKeyVaultError, ResolvedProfileContentKey, FORMAT_VERSION_V1,
    MAX_ENTRIES_PER_GROUP, MAX_GROUPS, MAX_TOTAL_ENTRIES,
};

const GROUP_DIGEST_DOMAIN_V1: &[u8] = b"uniclipboard/protected-group-catalog/v1\0";

pub(super) fn empty() -> PersistedVault {
    PersistedVault {
        format_version: FORMAT_VERSION_V1,
        revision: 0,
        groups: Vec::new(),
    }
}

pub(super) fn group_from_verified_material(
    material: &SpaceKeyMaterial,
) -> Result<PersistedGroup, ProfileContentKeyVaultError> {
    if material.state().mode() != SpaceSecurityMode::Ready
        || material.group_state().is_empty()
        || material.state().space_id().as_ref().is_empty()
    {
        return Err(invalid_material("space material is not ready"));
    }
    let protection_group_id = material
        .state()
        .protection_group_id()
        .ok_or_else(|| invalid_material("protection group is missing"))?;
    let catalog = export_admission_content_key_catalog(material).map_err(|source| {
        ProfileContentKeyVaultError::InvalidMaterial {
            source: anyhow::Error::new(source).context("export verified space content key catalog"),
        }
    })?;
    let mut entries = catalog
        .entries
        .into_iter()
        .filter(|entry| entry.content_key_id != "legacy-v1")
        .map(|entry| PersistedEntry {
            content_key_id: entry.content_key_id,
            epoch: entry.epoch,
            key: entry.key,
        })
        .collect::<Vec<_>>();
    entries.sort_by(compare_entries);
    if entries.is_empty() {
        return Err(invalid_material(
            "space material has no non-legacy content key",
        ));
    }
    if entries.len() > MAX_ENTRIES_PER_GROUP {
        return Err(ProfileContentKeyVaultError::CapacityExceeded);
    }
    let mut group = PersistedGroup {
        protection_group_id: protection_group_id.as_str().to_owned(),
        space_id: material.state().space_id().as_ref().to_owned(),
        entries,
        catalog_digest: [0; 32],
    };
    group.catalog_digest = expected_digest(&group);
    Ok(group)
}

pub(super) fn merge(
    vault: &mut PersistedVault,
    incoming: PersistedGroup,
) -> Result<bool, ProfileContentKeyVaultError> {
    let existing_index = vault
        .groups
        .iter()
        .position(|group| group.protection_group_id == incoming.protection_group_id);
    if let Some(index) = existing_index {
        let foreign_key_ids = vault
            .groups
            .iter()
            .enumerate()
            .filter(|(group_index, _)| *group_index != index)
            .flat_map(|(_, group)| group.entries.iter())
            .map(|entry| entry.content_key_id.clone())
            .collect::<BTreeSet<_>>();
        let existing = &mut vault.groups[index];
        if existing.space_id != incoming.space_id {
            return Err(ProfileContentKeyVaultError::Conflict);
        }
        let mut changed = false;
        for candidate in &incoming.entries {
            match existing
                .entries
                .iter()
                .find(|entry| entry.content_key_id == candidate.content_key_id)
            {
                Some(entry) if entry == candidate => {}
                Some(_) => return Err(ProfileContentKeyVaultError::Conflict),
                None if foreign_key_ids.contains(&candidate.content_key_id) => {
                    return Err(ProfileContentKeyVaultError::Conflict);
                }
                None => {
                    existing.entries.push(candidate.clone());
                    changed = true;
                }
            }
        }
        if changed {
            existing.entries.sort_by(compare_entries);
            existing.catalog_digest = expected_digest(existing);
        }
        return Ok(changed);
    }

    if incoming.entries.iter().any(|candidate| {
        vault.groups.iter().any(|group| {
            group
                .entries
                .iter()
                .any(|entry| entry.content_key_id == candidate.content_key_id)
        })
    }) {
        return Err(ProfileContentKeyVaultError::Conflict);
    }
    vault.groups.push(incoming);
    vault.groups.sort_by(|left, right| {
        left.protection_group_id
            .as_bytes()
            .cmp(right.protection_group_id.as_bytes())
    });
    Ok(true)
}

pub(super) fn validate(vault: &PersistedVault) -> Result<(), ProfileContentKeyVaultError> {
    if vault.format_version != FORMAT_VERSION_V1 || vault.revision == 0 {
        return Err(corrupt("vault header is invalid"));
    }
    if vault.groups.len() > MAX_GROUPS {
        return Err(ProfileContentKeyVaultError::CapacityExceeded);
    }
    let mut group_ids = BTreeSet::new();
    let mut key_ids = BTreeSet::new();
    let mut total_entries = 0usize;
    for group in &vault.groups {
        ProtectionGroupId::from_string(group.protection_group_id.clone()).map_err(|source| {
            ProfileContentKeyVaultError::Corrupt {
                source: anyhow::Error::new(source).context("validate stored protection group"),
            }
        })?;
        if group.space_id.is_empty()
            || !group_ids.insert(group.protection_group_id.as_str())
            || group.entries.is_empty()
            || group.entries.len() > MAX_ENTRIES_PER_GROUP
            || group.catalog_digest != expected_digest(group)
        {
            return Err(corrupt("stored group catalog is invalid"));
        }
        let _space_id = SpaceId::from_string(group.space_id.clone());
        let mut previous: Option<&PersistedEntry> = None;
        for entry in &group.entries {
            if entry.content_key_id == "legacy-v1"
                || entry.key.len() != MasterKey::LEN
                || !key_ids.insert(entry.content_key_id.as_str())
                || ContentKeyId::from_string(entry.content_key_id.clone()).is_err()
                || previous.is_some_and(|previous| !compare_entries(previous, entry).is_lt())
            {
                return Err(corrupt("stored content key catalog is invalid"));
            }
            previous = Some(entry);
        }
        total_entries = total_entries
            .checked_add(group.entries.len())
            .ok_or(ProfileContentKeyVaultError::CapacityExceeded)?;
    }
    if total_entries > MAX_TOTAL_ENTRIES {
        return Err(ProfileContentKeyVaultError::CapacityExceeded);
    }
    Ok(())
}

pub(super) fn resolve(
    vault: &PersistedVault,
    content_key_id: &ContentKeyId,
    epoch: GroupEpoch,
) -> Result<ResolvedProfileContentKey, ProfileContentKeyVaultError> {
    for group in &vault.groups {
        if let Some(entry) = group
            .entries
            .iter()
            .find(|entry| entry.content_key_id == content_key_id.as_str())
        {
            if entry.epoch != epoch.value() {
                return Err(ProfileContentKeyVaultError::EpochMismatch);
            }
            let protection_group_id = ProtectionGroupId::from_string(
                group.protection_group_id.clone(),
            )
            .map_err(|source| ProfileContentKeyVaultError::Corrupt {
                source: anyhow::Error::new(source)
                    .context("decode profile content protection group"),
            })?;
            let key = MasterKey::from_bytes(&entry.key).map_err(|source| {
                ProfileContentKeyVaultError::Corrupt {
                    source: anyhow::Error::new(source)
                        .context("decode resolved profile content key"),
                }
            })?;
            return Ok(ResolvedProfileContentKey {
                protection_group_id,
                content_key_id: content_key_id.clone(),
                epoch,
                key,
            });
        }
    }
    Err(ProfileContentKeyVaultError::KeyNotFound)
}

pub(super) fn summary(vault: &PersistedVault, changed: bool) -> InstalledProfileCatalog {
    InstalledProfileCatalog {
        revision: vault.revision,
        group_count: vault.groups.len(),
        entry_count: vault.groups.iter().map(|group| group.entries.len()).sum(),
        changed,
    }
}

fn compare_entries(left: &PersistedEntry, right: &PersistedEntry) -> std::cmp::Ordering {
    left.content_key_id
        .as_bytes()
        .cmp(right.content_key_id.as_bytes())
        .then(left.epoch.cmp(&right.epoch))
}

fn expected_digest(group: &PersistedGroup) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(GROUP_DIGEST_DOMAIN_V1);
    append_digest_field(&mut hasher, group.protection_group_id.as_bytes());
    append_digest_field(&mut hasher, group.space_id.as_bytes());
    hasher.update((group.entries.len() as u64).to_be_bytes());
    for entry in &group.entries {
        append_digest_field(&mut hasher, entry.content_key_id.as_bytes());
        hasher.update(entry.epoch.to_be_bytes());
        append_digest_field(&mut hasher, &entry.key);
    }
    hasher.finalize().into()
}

fn append_digest_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}
