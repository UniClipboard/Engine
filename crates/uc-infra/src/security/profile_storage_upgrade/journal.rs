use rand::RngCore;
use uc_core::ids::SpaceId;
use uc_core::membership::{ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2};
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::ProfileStorageUpgradeError;
use crate::security::ActiveRuntimeManifestV3;

pub(super) const JOURNAL_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Zeroize)]
pub(super) enum UpgradePhaseV1 {
    Detected,
    TargetStaged,
    StoresSeparated,
    PrimaryPayloadsConverted,
    PayloadsConverted,
    Verified,
    Promoted,
    CleanupPending,
}

#[derive(serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
struct UpgradeSourceV1 {
    space_id: String,
    manifest_digest: [u8; 32],
    keyslot_generation: [u8; 16],
    database_generation: [u8; 16],
    security_generation: [u8; 16],
}

#[derive(serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
pub(super) struct UpgradeJournalV1 {
    format_version: u16,
    phase: UpgradePhaseV1,
    source: Option<UpgradeSourceV1>,
    target_profile_data_generation: [u8; 16],
    target_space_control_generation: [u8; 16],
    source_snapshot_digest: Option<[u8; 32]>,
    source_database_revision: Option<u64>,
    profile_database_digest: Option<[u8; 32]>,
    control_database_digest: Option<[u8; 32]>,
    primary_profile_database_digest: Option<[u8; 32]>,
    primary_blob_tree_digest: Option<[u8; 32]>,
    converted_inline_count: Option<u64>,
    converted_blob_count: Option<u64>,
    payload_profile_database_digest: Option<[u8; 32]>,
    payload_blob_tree_digest: Option<[u8; 32]>,
    converted_derived_count: Option<u64>,
    converted_search_document_count: Option<u64>,
    verified_profile_schema_digest: Option<[u8; 32]>,
    verified_control_schema_digest: Option<[u8; 32]>,
}

impl UpgradeJournalV1 {
    pub(super) fn detected(source: Option<&ActiveSpaceGenerationManifestV2>) -> Self {
        let reserved = source
            .map(|manifest| {
                [
                    manifest.keyslot_generation,
                    manifest.database_generation,
                    manifest.security_generation,
                ]
            })
            .unwrap_or([[0; 16]; 3]);
        let target_profile_data_generation = random_generation_excluding(&reserved);
        let target_space_control_generation = random_generation_excluding(&[
            reserved[0],
            reserved[1],
            reserved[2],
            target_profile_data_generation,
        ]);
        Self {
            format_version: JOURNAL_FORMAT_V1,
            phase: UpgradePhaseV1::Detected,
            source: source.map(UpgradeSourceV1::from),
            target_profile_data_generation,
            target_space_control_generation,
            source_snapshot_digest: None,
            source_database_revision: None,
            profile_database_digest: None,
            control_database_digest: None,
            primary_profile_database_digest: None,
            primary_blob_tree_digest: None,
            converted_inline_count: None,
            converted_blob_count: None,
            payload_profile_database_digest: None,
            payload_blob_tree_digest: None,
            converted_derived_count: None,
            converted_search_document_count: None,
            verified_profile_schema_digest: None,
            verified_control_schema_digest: None,
        }
    }

    pub(super) fn validate(&self) -> Result<(), ProfileStorageUpgradeError> {
        if self.format_version != JOURNAL_FORMAT_V1
            || self.target_profile_data_generation == [0; 16]
            || self.target_space_control_generation == [0; 16]
            || self.target_profile_data_generation == self.target_space_control_generation
            || self.source.as_ref().is_some_and(|source| {
                source.space_id.is_empty()
                    || [
                        source.keyslot_generation,
                        source.database_generation,
                        source.security_generation,
                    ]
                    .contains(&self.target_profile_data_generation)
                    || [
                        source.keyslot_generation,
                        source.database_generation,
                        source.security_generation,
                    ]
                    .contains(&self.target_space_control_generation)
            })
            || (self.phase == UpgradePhaseV1::Detected
                && (self.source_snapshot_digest.is_some()
                    || self.source_database_revision.is_some()))
            || (self.phase != UpgradePhaseV1::Detected
                && (self.source_snapshot_digest.is_none()
                    || self.source_database_revision.is_none()))
            || (matches!(
                self.phase,
                UpgradePhaseV1::Detected | UpgradePhaseV1::TargetStaged
            ) && (self.profile_database_digest.is_some()
                || self.control_database_digest.is_some()))
            || (!matches!(
                self.phase,
                UpgradePhaseV1::Detected | UpgradePhaseV1::TargetStaged
            ) && (self.profile_database_digest.is_none()
                || self.control_database_digest.is_none()))
            || (matches!(
                self.phase,
                UpgradePhaseV1::Detected
                    | UpgradePhaseV1::TargetStaged
                    | UpgradePhaseV1::StoresSeparated
            ) && (self.primary_profile_database_digest.is_some()
                || self.primary_blob_tree_digest.is_some()
                || self.converted_inline_count.is_some()
                || self.converted_blob_count.is_some()))
            || (!matches!(
                self.phase,
                UpgradePhaseV1::Detected
                    | UpgradePhaseV1::TargetStaged
                    | UpgradePhaseV1::StoresSeparated
            ) && (self.primary_profile_database_digest.is_none()
                || self.primary_blob_tree_digest.is_none()
                || self.converted_inline_count.is_none()
                || self.converted_blob_count.is_none()))
            || (matches!(
                self.phase,
                UpgradePhaseV1::Detected
                    | UpgradePhaseV1::TargetStaged
                    | UpgradePhaseV1::StoresSeparated
                    | UpgradePhaseV1::PrimaryPayloadsConverted
            ) && (self.payload_profile_database_digest.is_some()
                || self.payload_blob_tree_digest.is_some()
                || self.converted_derived_count.is_some()
                || self.converted_search_document_count.is_some()))
            || (!matches!(
                self.phase,
                UpgradePhaseV1::Detected
                    | UpgradePhaseV1::TargetStaged
                    | UpgradePhaseV1::StoresSeparated
                    | UpgradePhaseV1::PrimaryPayloadsConverted
            ) && (self.payload_profile_database_digest.is_none()
                || self.payload_blob_tree_digest.is_none()
                || self.converted_derived_count.is_none()
                || self.converted_search_document_count.is_none()))
            || (matches!(
                self.phase,
                UpgradePhaseV1::Detected
                    | UpgradePhaseV1::TargetStaged
                    | UpgradePhaseV1::StoresSeparated
                    | UpgradePhaseV1::PrimaryPayloadsConverted
                    | UpgradePhaseV1::PayloadsConverted
            ) && (self.verified_profile_schema_digest.is_some()
                || self.verified_control_schema_digest.is_some()))
            || (!matches!(
                self.phase,
                UpgradePhaseV1::Detected
                    | UpgradePhaseV1::TargetStaged
                    | UpgradePhaseV1::StoresSeparated
                    | UpgradePhaseV1::PrimaryPayloadsConverted
                    | UpgradePhaseV1::PayloadsConverted
            ) && (self.verified_profile_schema_digest.is_none()
                || self.verified_control_schema_digest.is_none()))
        {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile storage upgrade journal invariants are invalid"),
            });
        }
        Ok(())
    }

    pub(super) fn matches_source(&self, source: Option<&ActiveSpaceGenerationManifestV2>) -> bool {
        match (&self.source, source) {
            (None, None) => true,
            (Some(persisted), Some(current)) => persisted.matches(current),
            _ => false,
        }
    }

    pub(super) const fn phase(&self) -> UpgradePhaseV1 {
        self.phase
    }

    pub(super) const fn target_profile_data_generation(&self) -> &[u8; 16] {
        &self.target_profile_data_generation
    }

    pub(super) const fn target_space_control_generation(&self) -> &[u8; 16] {
        &self.target_space_control_generation
    }

    pub(super) fn source_space_id(&self) -> Option<&str> {
        self.source.as_ref().map(|source| source.space_id.as_str())
    }

    pub(super) fn source_database_generation(&self) -> Option<&[u8; 16]> {
        self.source
            .as_ref()
            .map(|source| &source.database_generation)
    }

    pub(super) const fn source_snapshot_digest(&self) -> Option<[u8; 32]> {
        self.source_snapshot_digest
    }

    pub(super) const fn source_database_revision(&self) -> Option<u64> {
        self.source_database_revision
    }

    pub(super) const fn profile_database_digest(&self) -> Option<[u8; 32]> {
        self.profile_database_digest
    }

    pub(super) const fn control_database_digest(&self) -> Option<[u8; 32]> {
        self.control_database_digest
    }

    pub(super) const fn primary_profile_database_digest(&self) -> Option<[u8; 32]> {
        self.primary_profile_database_digest
    }

    pub(super) const fn primary_blob_tree_digest(&self) -> Option<[u8; 32]> {
        self.primary_blob_tree_digest
    }

    pub(super) const fn converted_inline_count(&self) -> Option<u64> {
        self.converted_inline_count
    }

    pub(super) const fn converted_blob_count(&self) -> Option<u64> {
        self.converted_blob_count
    }

    pub(super) const fn payload_profile_database_digest(&self) -> Option<[u8; 32]> {
        self.payload_profile_database_digest
    }

    pub(super) const fn payload_blob_tree_digest(&self) -> Option<[u8; 32]> {
        self.payload_blob_tree_digest
    }

    pub(super) const fn converted_derived_count(&self) -> Option<u64> {
        self.converted_derived_count
    }

    pub(super) const fn converted_search_document_count(&self) -> Option<u64> {
        self.converted_search_document_count
    }

    pub(super) const fn verified_profile_schema_digest(&self) -> Option<[u8; 32]> {
        self.verified_profile_schema_digest
    }

    pub(super) const fn verified_control_schema_digest(&self) -> Option<[u8; 32]> {
        self.verified_control_schema_digest
    }

    pub(super) fn target_manifest(
        &self,
        source: &ActiveSpaceGenerationManifestV2,
    ) -> Result<ActiveRuntimeManifestV3, ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::Verified || !self.matches_source(Some(source)) {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade promotion binding is invalid"),
            });
        }
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(source.space_id.clone()),
            self.target_profile_data_generation,
            self.target_space_control_generation,
        )
        .map_err(|source| ProfileStorageUpgradeError::Corrupt {
            source: anyhow::Error::new(source)
                .context("construct profile upgrade target runtime layout"),
        })?;
        ActiveRuntimeManifestV3::new(layout, source.keyslot_generation).ok_or_else(|| {
            ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade target keyslot generation is invalid"),
            }
        })
    }

    pub(super) fn matches_target(&self, target: &ActiveRuntimeManifestV3) -> bool {
        self.target_profile_data_generation == *target.layout().profile_data_generation()
            && self.target_space_control_generation == *target.layout().space_control_generation()
            && self.source.as_ref().is_none_or(|source| {
                source.space_id == target.layout().space_id().as_ref()
                    && source.keyslot_generation == *target.keyslot_generation()
            })
    }

    pub(super) fn matches_activated_fresh_profile(&self, target: &ActiveRuntimeManifestV3) -> bool {
        self.phase == UpgradePhaseV1::Verified
            && self.source.is_none()
            && self.target_profile_data_generation == *target.layout().profile_data_generation()
    }

    pub(super) fn mark_target_staged(
        &mut self,
        source_snapshot_digest: [u8; 32],
        source_database_revision: u64,
    ) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::Detected || source_snapshot_digest == [0; 32] {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade target staging transition is invalid"),
            });
        }
        self.source_snapshot_digest = Some(source_snapshot_digest);
        self.source_database_revision = Some(source_database_revision);
        self.phase = UpgradePhaseV1::TargetStaged;
        self.validate()
    }

    pub(super) fn mark_stores_separated(
        &mut self,
        profile_database_digest: [u8; 32],
        control_database_digest: [u8; 32],
    ) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::TargetStaged
            || profile_database_digest == [0; 32]
            || control_database_digest == [0; 32]
        {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade store separation transition is invalid"),
            });
        }
        self.profile_database_digest = Some(profile_database_digest);
        self.control_database_digest = Some(control_database_digest);
        self.phase = UpgradePhaseV1::StoresSeparated;
        self.validate()
    }

    pub(super) fn mark_primary_payloads_converted(
        &mut self,
        profile_database_digest: [u8; 32],
        blob_tree_digest: [u8; 32],
        inline_count: u64,
        blob_count: u64,
    ) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::StoresSeparated
            || profile_database_digest == [0; 32]
            || blob_tree_digest == [0; 32]
        {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade primary conversion transition is invalid"),
            });
        }
        self.primary_profile_database_digest = Some(profile_database_digest);
        self.primary_blob_tree_digest = Some(blob_tree_digest);
        self.converted_inline_count = Some(inline_count);
        self.converted_blob_count = Some(blob_count);
        self.phase = UpgradePhaseV1::PrimaryPayloadsConverted;
        self.validate()
    }

    pub(super) fn mark_payloads_converted(
        &mut self,
        profile_database_digest: [u8; 32],
        blob_tree_digest: [u8; 32],
        derived_count: u64,
        search_document_count: u64,
    ) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::PrimaryPayloadsConverted
            || profile_database_digest == [0; 32]
            || blob_tree_digest == [0; 32]
        {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade payload conversion transition is invalid"),
            });
        }
        self.payload_profile_database_digest = Some(profile_database_digest);
        self.payload_blob_tree_digest = Some(blob_tree_digest);
        self.converted_derived_count = Some(derived_count);
        self.converted_search_document_count = Some(search_document_count);
        self.phase = UpgradePhaseV1::PayloadsConverted;
        self.validate()
    }

    pub(super) fn mark_verified(
        &mut self,
        profile_schema_digest: [u8; 32],
        control_schema_digest: [u8; 32],
    ) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::PayloadsConverted
            || profile_schema_digest == [0; 32]
            || control_schema_digest == [0; 32]
        {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade verification transition is invalid"),
            });
        }
        self.verified_profile_schema_digest = Some(profile_schema_digest);
        self.verified_control_schema_digest = Some(control_schema_digest);
        self.phase = UpgradePhaseV1::Verified;
        self.validate()
    }

    pub(super) fn mark_promoted(&mut self) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::Verified {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade promotion transition is invalid"),
            });
        }
        self.phase = UpgradePhaseV1::Promoted;
        self.validate()
    }

    pub(super) fn mark_cleanup_pending(&mut self) -> Result<(), ProfileStorageUpgradeError> {
        if self.phase != UpgradePhaseV1::Promoted {
            return Err(ProfileStorageUpgradeError::Corrupt {
                source: anyhow::anyhow!("profile upgrade cleanup transition is invalid"),
            });
        }
        self.phase = UpgradePhaseV1::CleanupPending;
        self.validate()
    }
}

impl From<&ActiveSpaceGenerationManifestV2> for UpgradeSourceV1 {
    fn from(manifest: &ActiveSpaceGenerationManifestV2) -> Self {
        Self {
            space_id: manifest.space_id.clone(),
            manifest_digest: manifest.manifest_digest,
            keyslot_generation: manifest.keyslot_generation,
            database_generation: manifest.database_generation,
            security_generation: manifest.security_generation,
        }
    }
}

impl UpgradeSourceV1 {
    fn matches(&self, manifest: &ActiveSpaceGenerationManifestV2) -> bool {
        self.space_id == manifest.space_id
            && self.manifest_digest == manifest.manifest_digest
            && self.keyslot_generation == manifest.keyslot_generation
            && self.database_generation == manifest.database_generation
            && self.security_generation == manifest.security_generation
    }
}

#[cfg(test)]
mod tests {
    use uc_core::ids::SpaceId;
    use uc_core::membership::ActiveRuntimeLayout;

    use super::UpgradeJournalV1;
    use crate::security::ActiveRuntimeManifestV3;

    fn verified_fresh_journal() -> UpgradeJournalV1 {
        let mut journal = UpgradeJournalV1::detected(None);
        journal.mark_target_staged([0x11; 32], 1).unwrap();
        journal
            .mark_stores_separated([0x12; 32], [0x13; 32])
            .unwrap();
        journal
            .mark_primary_payloads_converted([0x14; 32], [0x15; 32], 0, 0)
            .unwrap();
        journal
            .mark_payloads_converted([0x16; 32], [0x17; 32], 0, 0)
            .unwrap();
        journal.mark_verified([0x18; 32], [0x19; 32]).unwrap();
        journal
    }

    fn manifest(profile: [u8; 16], control: [u8; 16]) -> ActiveRuntimeManifestV3 {
        ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("space-a"), profile, control).unwrap(),
            [0x31; 16],
        )
        .unwrap()
    }

    #[test]
    fn verified_fresh_journal_follows_the_profile_across_control_generation_changes() {
        let journal = verified_fresh_journal();
        let changed_control = manifest(*journal.target_profile_data_generation(), [0x41; 16]);
        let changed_profile = manifest([0x42; 16], [0x43; 16]);

        assert!(journal.matches_activated_fresh_profile(&changed_control));
        assert!(!journal.matches_activated_fresh_profile(&changed_profile));
    }
}

fn random_generation_excluding(reserved: &[[u8; 16]]) -> [u8; 16] {
    loop {
        let mut generation = [0; 16];
        rand::rng().fill_bytes(&mut generation);
        if generation != [0; 16] && !reserved.contains(&generation) {
            return generation;
        }
    }
}
