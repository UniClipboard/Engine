use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::security::WrappedSpaceAdmissionDataKey;

pub(in crate::space::admission) const SPACE_ADMISSION_REPOSITORY_FORMAT_V1: u16 = 1;

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
    pub(in crate::space::admission) records: BTreeMap<[u8; 32], StoredSpaceAdmissionV1>,
}

impl PersistedSpaceAdmissionRepositoryV1 {
    pub(super) fn fresh(profile_generation: [u8; 16]) -> Self {
        Self {
            format_version: SPACE_ADMISSION_REPOSITORY_FORMAT_V1,
            profile_generation,
            next_local_join_ordinal: 0,
            current_local_join_id: None,
            records: BTreeMap::new(),
        }
    }
}
