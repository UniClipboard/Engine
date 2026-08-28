use super::persisted::PersistedSpaceAdmissionRepositoryV1;
use sha2::{Digest, Sha256};
use uc_core::membership::AdmissionRecordPersistence;

pub(in crate::space::admission) fn joiner_start_token<R: AdmissionRecordPersistence>(
    state: &PersistedSpaceAdmissionRepositoryV1,
    current: Option<&R>,
    source_snapshot: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/joiner-start-token/v1\0");
    hasher.update(state.profile_generation);
    hasher.update(state.next_local_join_ordinal.to_be_bytes());
    append_optional_id(&mut hasher, state.current_local_join_id);
    append_optional_version(
        &mut hasher,
        current.map(AdmissionRecordPersistence::record_version),
    );
    hasher.update((source_snapshot.len() as u64).to_be_bytes());
    hasher.update(source_snapshot);
    hasher.finalize().into()
}

pub(in crate::space::admission) fn recovery_token<R: AdmissionRecordPersistence>(
    profile_generation: [u8; 16],
    aggregate: &R,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/recovery-token/v1\0");
    hasher.update(profile_generation);
    hasher.update(aggregate.admission_id().as_bytes());
    hasher.update(aggregate.record_version().to_be_bytes());
    hasher.finalize().into()
}

pub(in crate::space::admission) fn joiner_activation_token<R: AdmissionRecordPersistence>(
    profile_generation: [u8; 16],
    admission: &R,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/joiner-activation-token/v1\0");
    hasher.update(profile_generation);
    hasher.update(admission.admission_id().as_bytes());
    hasher.update(admission.record_version().to_be_bytes());
    hasher.finalize().into()
}

pub(in crate::space::admission) fn sponsor_existing_token<R: AdmissionRecordPersistence>(
    profile_generation: [u8; 16],
    aggregate: &R,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/sponsor-existing-token/v1\0");
    hasher.update(profile_generation);
    hasher.update(aggregate.admission_id().as_bytes());
    hasher.update(aggregate.record_version().to_be_bytes());
    hasher.finalize().into()
}

pub(in crate::space::admission) fn sponsor_fresh_token(
    state: &PersistedSpaceAdmissionRepositoryV1,
    admission_id: [u8; 32],
    invitation_id: [u8; 32],
    base_snapshot: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/space-admission/sponsor-fresh-token/v1\0");
    hasher.update(state.profile_generation);
    hasher.update(admission_id);
    hasher.update(invitation_id);
    hasher.update((base_snapshot.len() as u64).to_be_bytes());
    hasher.update(base_snapshot);
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
