use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    DeviceAnnouncement, MembershipAnnouncementRepositoryError, MembershipAnnouncementRepositoryPort,
};

use crate::db::ports::DbExecutor;

use super::{EncryptedRelationshipStore, RelationshipStoreError};

pub struct EncryptedMembershipAnnouncementRepository<E> {
    store: Arc<EncryptedRelationshipStore<E>>,
}

impl<E> EncryptedMembershipAnnouncementRepository<E> {
    pub fn new(store: Arc<EncryptedRelationshipStore<E>>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<E> MembershipAnnouncementRepositoryPort for EncryptedMembershipAnnouncementRepository<E>
where
    E: DbExecutor,
{
    async fn get(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<Option<DeviceAnnouncement>, MembershipAnnouncementRepositoryError> {
        self.store
            .get_membership_announcement(space_id, device_id)
            .await
            .map_err(map_store_error)
    }

    async fn list(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<DeviceAnnouncement>, MembershipAnnouncementRepositoryError> {
        self.store
            .list_membership_announcements(space_id)
            .await
            .map_err(map_store_error)
    }

    async fn save(
        &self,
        announcement: &DeviceAnnouncement,
    ) -> Result<(), MembershipAnnouncementRepositoryError> {
        self.store
            .save_membership_announcement(announcement)
            .await
            .map_err(map_store_error)
    }

    async fn remove(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<bool, MembershipAnnouncementRepositoryError> {
        self.store
            .remove_membership_announcement(space_id, device_id)
            .await
            .map_err(map_store_error)
    }
}

fn map_store_error(error: RelationshipStoreError) -> MembershipAnnouncementRepositoryError {
    match error {
        RelationshipStoreError::Locked => MembershipAnnouncementRepositoryError::Locked,
        RelationshipStoreError::InvalidCiphertext => MembershipAnnouncementRepositoryError::Corrupt,
        RelationshipStoreError::Storage(message) => {
            MembershipAnnouncementRepositoryError::Repository(message)
        }
    }
}
