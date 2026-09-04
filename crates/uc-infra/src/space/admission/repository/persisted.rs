use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::security::WrappedSpaceAdmissionDataKey;

pub(in crate::space::admission) const SPACE_ADMISSION_REPOSITORY_FORMAT_V1: u16 = 1;
pub(in crate::space::admission) const SPACE_ADMISSION_REPOSITORY_FORMAT_V2: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::space::admission) struct StoredSpaceAdmissionV1 {
    pub(in crate::space::admission) wrapped_data_key: WrappedSpaceAdmissionDataKey,
    pub(in crate::space::admission) encrypted_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::space::admission) struct PersistedSpaceAdmissionRepositoryV1 {
    pub(in crate::space::admission) format_version: u16,
    pub(in crate::space::admission) profile_generation: [u8; 16],
    pub(in crate::space::admission) next_local_join_ordinal: u64,
    pub(in crate::space::admission) current_local_join_id: Option<[u8; 32]>,
    pub(in crate::space::admission) claimed_invitations: BTreeMap<[u8; 32], [u8; 32]>,
    pub(in crate::space::admission) records: BTreeMap<[u8; 32], StoredSpaceAdmissionV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::space::admission) struct PersistedSpaceAdmissionRepositoryV2 {
    pub(in crate::space::admission) format_version: u16,
    pub(in crate::space::admission) profile_generation: [u8; 16],
    pub(in crate::space::admission) next_local_join_ordinal: u64,
    pub(in crate::space::admission) current_local_join_id: Option<[u8; 32]>,
    pub(in crate::space::admission) latest_local_join_id: Option<[u8; 32]>,
    pub(in crate::space::admission) claimed_invitations: BTreeMap<[u8; 32], [u8; 32]>,
    pub(in crate::space::admission) records: BTreeMap<[u8; 32], StoredSpaceAdmissionV1>,
}

impl PersistedSpaceAdmissionRepositoryV2 {
    pub(super) fn fresh(profile_generation: [u8; 16]) -> Self {
        Self {
            format_version: SPACE_ADMISSION_REPOSITORY_FORMAT_V2,
            profile_generation,
            next_local_join_ordinal: 0,
            current_local_join_id: None,
            latest_local_join_id: None,
            claimed_invitations: BTreeMap::new(),
            records: BTreeMap::new(),
        }
    }
}

impl From<PersistedSpaceAdmissionRepositoryV1> for PersistedSpaceAdmissionRepositoryV2 {
    fn from(legacy: PersistedSpaceAdmissionRepositoryV1) -> Self {
        Self {
            format_version: SPACE_ADMISSION_REPOSITORY_FORMAT_V2,
            profile_generation: legacy.profile_generation,
            next_local_join_ordinal: legacy.next_local_join_ordinal,
            current_local_join_id: legacy.current_local_join_id,
            latest_local_join_id: None,
            claimed_invitations: legacy.claimed_invitations,
            records: legacy.records,
        }
    }
}

pub(super) fn decode_repository(bytes: &[u8]) -> Option<PersistedSpaceAdmissionRepositoryV2> {
    if let Ok(current) = postcard::from_bytes::<PersistedSpaceAdmissionRepositoryV2>(bytes) {
        if current.format_version == SPACE_ADMISSION_REPOSITORY_FORMAT_V2 {
            return Some(current);
        }
    }
    let legacy = postcard::from_bytes::<PersistedSpaceAdmissionRepositoryV1>(bytes).ok()?;
    (legacy.format_version == SPACE_ADMISSION_REPOSITORY_FORMAT_V1).then(|| legacy.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct LegacyRepositoryV1 {
        format_version: u16,
        profile_generation: [u8; 16],
        next_local_join_ordinal: u64,
        current_local_join_id: Option<[u8; 32]>,
        claimed_invitations: BTreeMap<[u8; 32], [u8; 32]>,
        records: BTreeMap<[u8; 32], StoredSpaceAdmissionV1>,
    }

    #[test]
    fn current_repository_decodes_legacy_payload_without_latest_join() {
        let legacy = LegacyRepositoryV1 {
            format_version: SPACE_ADMISSION_REPOSITORY_FORMAT_V1,
            profile_generation: [0x41; 16],
            next_local_join_ordinal: 3,
            current_local_join_id: None,
            claimed_invitations: BTreeMap::new(),
            records: BTreeMap::new(),
        };
        let bytes = postcard::to_stdvec(&legacy).unwrap();

        let decoded = decode_repository(&bytes).unwrap();

        assert_eq!(decoded.profile_generation, [0x41; 16]);
        assert_eq!(decoded.next_local_join_ordinal, 3);
        assert_eq!(decoded.latest_local_join_id, None);
        assert!(decoded.claimed_invitations.is_empty());
        assert!(decoded.records.is_empty());
    }
}
