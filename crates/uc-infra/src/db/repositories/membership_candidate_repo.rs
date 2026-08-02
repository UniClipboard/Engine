use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    MembershipCandidateRepositoryError, MembershipCandidateRepositoryPort, SpaceMembershipCandidate,
};

use crate::db::ports::DbExecutor;

use super::{EncryptedRelationshipStore, RelationshipStoreError};

pub struct EncryptedMembershipCandidateRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> EncryptedMembershipCandidateRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<E> MembershipCandidateRepositoryPort for EncryptedMembershipCandidateRepository<E>
where
    E: DbExecutor,
{
    async fn get(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<Option<SpaceMembershipCandidate>, MembershipCandidateRepositoryError> {
        self.store
            .get_candidate(space_id, device_id)
            .await
            .map_err(map_store_error)
    }

    async fn list(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<SpaceMembershipCandidate>, MembershipCandidateRepositoryError> {
        self.store
            .list_candidates(space_id)
            .await
            .map_err(map_store_error)
    }

    async fn save(
        &self,
        candidate: &SpaceMembershipCandidate,
    ) -> Result<(), MembershipCandidateRepositoryError> {
        self.store
            .save_candidate(candidate)
            .await
            .map_err(map_store_error)
    }

    async fn remove(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<bool, MembershipCandidateRepositoryError> {
        self.store
            .remove_candidate(space_id, device_id)
            .await
            .map_err(map_store_error)
    }

    async fn purge_expired(
        &self,
        space_id: &SpaceId,
        now_ms: i64,
    ) -> Result<usize, MembershipCandidateRepositoryError> {
        let candidates = self
            .store
            .list_candidates(space_id)
            .await
            .map_err(map_store_error)?;
        let mut removed = 0usize;
        for candidate in candidates {
            if candidate.expires_at_ms() <= now_ms
                && self
                    .store
                    .remove_candidate(space_id, candidate.device_id())
                    .await
                    .map_err(map_store_error)?
            {
                removed += 1;
            }
        }
        Ok(removed)
    }
}

fn map_store_error(error: RelationshipStoreError) -> MembershipCandidateRepositoryError {
    match error {
        RelationshipStoreError::Locked => MembershipCandidateRepositoryError::Locked,
        RelationshipStoreError::InvalidCiphertext => MembershipCandidateRepositoryError::Corrupt,
        RelationshipStoreError::Storage(message) => {
            MembershipCandidateRepositoryError::Repository(message)
        }
    }
}
