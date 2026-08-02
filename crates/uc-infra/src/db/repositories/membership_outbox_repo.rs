use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    MembershipOutboxRepositoryError, MembershipOutboxRepositoryPort, PendingMembershipBatch,
};

use crate::db::ports::DbExecutor;

use super::{EncryptedRelationshipStore, RelationshipStoreError};

pub struct EncryptedMembershipOutboxRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> EncryptedMembershipOutboxRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<E> MembershipOutboxRepositoryPort for EncryptedMembershipOutboxRepository<E>
where
    E: DbExecutor,
{
    async fn get(
        &self,
        space_id: &SpaceId,
        recipient_device_id: &DeviceId,
        batch_id: &[u8; 32],
    ) -> Result<Option<PendingMembershipBatch>, MembershipOutboxRepositoryError> {
        self.store
            .get_membership_outbox(space_id, recipient_device_id, batch_id)
            .await
            .map_err(map_store_error)
    }

    async fn list_pending(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<PendingMembershipBatch>, MembershipOutboxRepositoryError> {
        self.store
            .list_membership_outbox(space_id)
            .await
            .map_err(map_store_error)
    }

    async fn save(
        &self,
        pending: &PendingMembershipBatch,
    ) -> Result<(), MembershipOutboxRepositoryError> {
        self.store
            .save_membership_outbox(pending)
            .await
            .map_err(map_store_error)
    }

    async fn remove(
        &self,
        space_id: &SpaceId,
        recipient_device_id: &DeviceId,
        batch_id: &[u8; 32],
    ) -> Result<bool, MembershipOutboxRepositoryError> {
        self.store
            .remove_membership_outbox(space_id, recipient_device_id, batch_id)
            .await
            .map_err(map_store_error)
    }
}

fn map_store_error(error: RelationshipStoreError) -> MembershipOutboxRepositoryError {
    match error {
        RelationshipStoreError::Locked => MembershipOutboxRepositoryError::Locked,
        RelationshipStoreError::InvalidCiphertext => MembershipOutboxRepositoryError::Corrupt,
        RelationshipStoreError::Storage(message) => {
            MembershipOutboxRepositoryError::Repository(message)
        }
    }
}
