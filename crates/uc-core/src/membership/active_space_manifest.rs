use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const ACTIVE_SPACE_MANIFEST_FORMAT_V2: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveSpaceManifestV2 {
    pub format_version: u16,
    pub space_id: String,
    pub keyslot_generation: [u8; 16],
    pub database_generation: [u8; 16],
    pub security_generation: [u8; 16],
    pub manifest_digest: [u8; 32],
}

impl ActiveSpaceManifestV2 {
    pub fn new(
        space_id: String,
        keyslot_generation: [u8; 16],
        database_generation: [u8; 16],
        security_generation: [u8; 16],
    ) -> Option<Self> {
        if space_id.is_empty() {
            return None;
        }
        let mut manifest = Self {
            format_version: ACTIVE_SPACE_MANIFEST_FORMAT_V2,
            space_id,
            keyslot_generation,
            database_generation,
            security_generation,
            manifest_digest: [0; 32],
        };
        manifest.manifest_digest = manifest.expected_digest();
        Some(manifest)
    }

    pub fn validate(&self) -> bool {
        self.format_version == ACTIVE_SPACE_MANIFEST_FORMAT_V2
            && !self.space_id.is_empty()
            && self.manifest_digest == self.expected_digest()
    }

    fn expected_digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/active-space-manifest/v2\0");
        hasher.update(self.format_version.to_be_bytes());
        hasher.update((self.space_id.len() as u64).to_be_bytes());
        hasher.update(self.space_id.as_bytes());
        hasher.update(self.keyslot_generation);
        hasher.update(self.database_generation);
        hasher.update(self.security_generation);
        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_binds_every_active_generation() {
        let manifest =
            ActiveSpaceManifestV2::new("space-a".to_owned(), [0x11; 16], [0x12; 16], [0x13; 16])
                .unwrap();
        assert!(manifest.validate());

        let mut changed = manifest.clone();
        changed.database_generation = [0x14; 16];
        assert!(!changed.validate());

        changed = manifest.clone();
        changed.space_id = "space-b".to_owned();
        assert!(!changed.validate());
    }
}
