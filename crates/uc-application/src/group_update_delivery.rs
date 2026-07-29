use std::sync::Arc;

use async_trait::async_trait;
use uc_core::membership::{GroupRevocationPort, GroupUpdateDispatchPort, KeyEpochError};

#[async_trait]
pub(crate) trait GroupUpdateDeliveryPort: Send + Sync {
    async fn deliver_pending(&self, now_ms: i64) -> Result<usize, KeyEpochError>;
}

pub(crate) struct GroupUpdateDelivery {
    outbox: Arc<dyn GroupRevocationPort>,
    dispatch: Arc<dyn GroupUpdateDispatchPort>,
}

impl GroupUpdateDelivery {
    pub(crate) fn new(
        outbox: Arc<dyn GroupRevocationPort>,
        dispatch: Arc<dyn GroupUpdateDispatchPort>,
    ) -> Self {
        Self { outbox, dispatch }
    }
}

#[async_trait]
impl GroupUpdateDeliveryPort for GroupUpdateDelivery {
    async fn deliver_pending(&self, now_ms: i64) -> Result<usize, KeyEpochError> {
        let updates = self.outbox.pending_space_group_updates().await?;
        let mut delivered = 0;
        for update in updates {
            if self.dispatch.dispatch_group_update(&update).await.is_ok()
                && self
                    .outbox
                    .acknowledge_space_group_update(update.update_id(), now_ms)
                    .await?
            {
                delivered += 1;
            }
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use uc_core::membership::{
        GroupEpoch, GroupRevocationPort, GroupRevocationResult, GroupUpdateDispatchError,
        GroupUpdateDispatchPort, KeyEpochError, PendingGroupUpdate, RevocationId,
    };
    use uc_core::DeviceId;

    mockall::mock! {
        Outbox {}

        #[async_trait]
        impl GroupRevocationPort for Outbox {
            async fn revoke_group_member(
                &self,
                target: &DeviceId,
                retained_recipients: &[DeviceId],
                now_ms: i64,
            ) -> Result<GroupRevocationResult, KeyEpochError>;
            async fn acknowledge_group_update(
                &self,
                revocation_id: &RevocationId,
                recipient: &DeviceId,
                now_ms: i64,
            ) -> Result<GroupRevocationResult, KeyEpochError>;
            async fn apply_group_epoch_update(
                &self,
                payload: &[u8],
            ) -> Result<GroupEpoch, KeyEpochError>;
            async fn pending_group_updates(
                &self,
                revocation_id: &RevocationId,
            ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;
            async fn query_group_revocation(
                &self,
                revocation_id: &RevocationId,
            ) -> Result<Option<GroupRevocationResult>, KeyEpochError>;
            async fn resume_group_revocations(
                &self,
                now_ms: i64,
            ) -> Result<Vec<GroupRevocationResult>, KeyEpochError>;
            async fn pending_space_group_updates(
                &self,
            ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;
            async fn acknowledge_space_group_update(
                &self,
                update_id: &str,
                now_ms: i64,
            ) -> Result<bool, KeyEpochError>;
        }
    }

    mockall::mock! {
        Dispatch {}

        #[async_trait]
        impl GroupUpdateDispatchPort for Dispatch {
            async fn dispatch_group_update(
                &self,
                update: &PendingGroupUpdate,
            ) -> Result<(), GroupUpdateDispatchError>;
        }
    }

    fn delivery_mocks(
        delivered: PendingGroupUpdate,
        retained: PendingGroupUpdate,
    ) -> (MockOutbox, MockDispatch) {
        let delivered_id = delivered.update_id().to_string();
        let mut outbox = MockOutbox::new();
        outbox
            .expect_pending_space_group_updates()
            .times(1)
            .return_once(move || Ok(vec![delivered, retained]));
        outbox
            .expect_acknowledge_space_group_update()
            .times(1)
            .withf(move |update_id, now_ms| update_id == delivered_id && *now_ms == 123)
            .returning(|_, _| Ok(true));

        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch_group_update()
            .times(2)
            .returning(|update| {
                if update.recipient() == &DeviceId::new("offline") {
                    Err(GroupUpdateDispatchError::Offline)
                } else {
                    Ok(())
                }
            });
        (outbox, dispatch)
    }

    #[tokio::test]
    async fn acknowledges_only_updates_that_were_delivered() {
        let delivered = PendingGroupUpdate::persistent(DeviceId::new("online"), vec![1]);
        let retained = PendingGroupUpdate::persistent(DeviceId::new("offline"), vec![2]);
        let (outbox, dispatch) = delivery_mocks(delivered, retained);

        let delivery = super::GroupUpdateDelivery::new(
            std::sync::Arc::new(outbox),
            std::sync::Arc::new(dispatch),
        );
        let count = super::GroupUpdateDeliveryPort::deliver_pending(&delivery, 123)
            .await
            .unwrap();

        assert_eq!(count, 1);
    }
}
