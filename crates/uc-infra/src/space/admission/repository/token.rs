use sha2::{Digest, Sha256};
use uc_core::membership::SpaceAdmissionAggregate;

use super::persisted::PersistedSpaceAdmissionRepositoryV1;

pub(in crate::space::admission) fn joiner_start_token(
    state: &PersistedSpaceAdmissionRepositoryV1,
    current: Option<&SpaceAdmissionAggregate>,
    source_snapshot: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/joiner-start-token/v1\0");
    hasher.update(state.profile_generation);
    hasher.update(state.next_local_join_ordinal.to_be_bytes());
    append_optional_id(&mut hasher, state.current_local_join_id);
    append_optional_version(
        &mut hasher,
        current.map(SpaceAdmissionAggregate::record_version),
    );
    hasher.update((source_snapshot.len() as u64).to_be_bytes());
    hasher.update(source_snapshot);
    hasher.finalize().into()
}

pub(in crate::space::admission) fn recovery_token(
    profile_generation: [u8; 16],
    aggregate: &SpaceAdmissionAggregate,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/recovery-token/v1\0");
    hasher.update(profile_generation);
    hasher.update(aggregate.admission_id().as_bytes());
    hasher.update(aggregate.record_version().to_be_bytes());
    hasher.finalize().into()
}

fn append_optional_id(hasher: &mut Sha256, value: Option<[u8; 32]>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value);
        }
        None => hasher.update([0]),
    }
}

fn append_optional_version(hasher: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_be_bytes());
        }
        None => hasher.update([0]),
    }
}
