use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::security::WrappedSpaceAdmissionDataKey;

pub(in crate::space::admission) const SPACE_ADMISSION_REPOSITORY_FORMAT_V1: u16 = 1;
pub(in crate::space::admission) const SPACE_ADMISSION_REPOSITORY_FORMAT_V2: u16 = 2;
pub(super) const SPACE_ADMISSION_REPOSITORY_FORMAT_V3: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::space::admission) struct StoredSpaceAdmissionV1 {
    pub(in crate::space::admission) wrapped_data_key: WrappedSpaceAdmissionDataKey,
    // rust-style: allow-qualified-path -- 可见性限定要求模块路径，不能使用导入别名
    pub(in crate::space::admission) encrypted_payload: Arc<Vec<u8>>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedSpaceAdmissionMetadataV3 {
    pub(super) format_version: u16,
    pub(super) profile_generation: [u8; 16],
    pub(super) next_local_join_ordinal: u64,
    pub(super) current_local_join_id: Option<[u8; 32]>,
    pub(super) latest_local_join_id: Option<[u8; 32]>,
    pub(super) claimed_invitations: BTreeMap<[u8; 32], [u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedSpaceAdmissionRecordV3 {
    pub(super) admission_id: [u8; 32],
    pub(super) stored: StoredSpaceAdmissionV1,
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

impl From<&PersistedSpaceAdmissionRepositoryV2> for PersistedSpaceAdmissionMetadataV3 {
    fn from(state: &PersistedSpaceAdmissionRepositoryV2) -> Self {
        Self {
            format_version: SPACE_ADMISSION_REPOSITORY_FORMAT_V3,
            profile_generation: state.profile_generation,
            next_local_join_ordinal: state.next_local_join_ordinal,
            current_local_join_id: state.current_local_join_id,
            latest_local_join_id: state.latest_local_join_id,
            claimed_invitations: state.claimed_invitations.clone(),
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
