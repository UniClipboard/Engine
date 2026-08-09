use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    MembershipAppliedSecurityUpdateRepositoryError, MembershipAppliedSecurityUpdateRepositoryPort,
    RelayedSecurityUpdate,
};

use crate::db::ports::DbExecutor;

use super::{EncryptedRelationshipStore, RelationshipStoreError};

pub struct EncryptedMembershipAppliedSecurityUpdateRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> EncryptedMembershipAppliedSecurityUpdateRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<E> MembershipAppliedSecurityUpdateRepositoryPort
    for EncryptedMembershipAppliedSecurityUpdateRepository<E>
where
    E: DbExecutor,
{
    async fn list(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<RelayedSecurityUpdate>, MembershipAppliedSecurityUpdateRepositoryError> {
        self.store
            .list_membership_applied_security_updates(space_id)
            .await
            .map_err(map_store_error)
    }

    async fn save(
        &self,
        space_id: &SpaceId,
        update: &RelayedSecurityUpdate,
    ) -> Result<(), MembershipAppliedSecurityUpdateRepositoryError> {
        self.store
            .save_membership_applied_security_update(space_id, update)
            .await
            .map_err(map_store_error)
    }
}

fn map_store_error(
    error: RelationshipStoreError,
) -> MembershipAppliedSecurityUpdateRepositoryError {
    match error {
        RelationshipStoreError::Locked => MembershipAppliedSecurityUpdateRepositoryError::Locked,
        RelationshipStoreError::InvalidCiphertext => {
            MembershipAppliedSecurityUpdateRepositoryError::Corrupt
        }
        RelationshipStoreError::Storage(message) => {
            MembershipAppliedSecurityUpdateRepositoryError::Repository(message)
        }
    }
}
