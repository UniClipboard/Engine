use rand::RngCore;
use uc_core::membership::ActiveSpaceGenerationManifestV2;
use zeroize::{Zeroize, ZeroizeOnDrop};

use super::ProfileStorageUpgradeError;

pub(super) const JOURNAL_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Zeroize)]
pub(super) enum UpgradePhaseV1 {
    Detected,
    TargetStaged,
    PayloadsConverted,
    Verified,
    Promoted,
    CleanupPending,
}

#[derive(serde::Serialize, serde::Deserialize, Zeroize, ZeroizeOnDrop)]
struct UpgradeSourceV1 {
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
        }
    }

    pub(super) fn validate(&self) -> Result<(), ProfileStorageUpgradeError> {
        if self.format_version != JOURNAL_FORMAT_V1
            || self.target_profile_data_generation == [0; 16]
            || self.target_space_control_generation == [0; 16]
            || self.target_profile_data_generation == self.target_space_control_generation
            || self.source.as_ref().is_some_and(|source| {
                [
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

    pub(super) const fn source_snapshot_digest(&self) -> Option<[u8; 32]> {
        self.source_snapshot_digest
    }

    pub(super) const fn source_database_revision(&self) -> Option<u64> {
        self.source_database_revision
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
}

impl From<&ActiveSpaceGenerationManifestV2> for UpgradeSourceV1 {
    fn from(manifest: &ActiveSpaceGenerationManifestV2) -> Self {
        Self {
            manifest_digest: manifest.manifest_digest,
            keyslot_generation: manifest.keyslot_generation,
            database_generation: manifest.database_generation,
            security_generation: manifest.security_generation,
        }
    }
}

impl UpgradeSourceV1 {
    fn matches(&self, manifest: &ActiveSpaceGenerationManifestV2) -> bool {
        self.manifest_digest == manifest.manifest_digest
            && self.keyslot_generation == manifest.keyslot_generation
            && self.database_generation == manifest.database_generation
            && self.security_generation == manifest.security_generation
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
