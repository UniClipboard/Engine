use std::sync::Arc;

use async_trait::async_trait;
use uc_core::membership::{GroupRevocationPort, GroupUpdateDispatchPort, KeyEpochError};

#[async_trait]
pub trait GroupUpdateDeliveryPort: Send + Sync {
    async fn deliver_pending(&self, now_ms: i64) -> Result<usize, KeyEpochError>;
}

pub struct GroupUpdateDelivery {
    outbox: Arc<dyn GroupRevocationPort>,
    dispatch: Arc<dyn GroupUpdateDispatchPort>,
}

impl GroupUpdateDelivery {
    pub fn new(
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
        let mut acknowledgement_error = None;
        for update in updates {
            if self.dispatch.dispatch_group_update(&update).await.is_ok() {
                match self
                    .outbox
                    .acknowledge_space_group_update(update.update_id(), now_ms)
                    .await
                {
                    Ok(true) => delivered += 1,
                    Ok(false) => {}
                    Err(error) => acknowledgement_error = Some(error),
                }
            }
        }
        acknowledgement_error.map_or(Ok(delivered), Err)
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

    #[tokio::test]
    async fn continues_after_acknowledgement_error_and_returns_it_after_the_batch() {
        let first = PendingGroupUpdate::persistent(DeviceId::new("first"), vec![1]);
        let second = PendingGroupUpdate::persistent(DeviceId::new("second"), vec![2]);
        let first_id = first.update_id().to_owned();
        let second_id = second.update_id().to_owned();
        let mut outbox = MockOutbox::new();
        outbox
            .expect_pending_space_group_updates()
            .times(1)
            .return_once(move || Ok(vec![first, second]));
        let mut sequence = mockall::Sequence::new();
        outbox
            .expect_acknowledge_space_group_update()
            .times(1)
            .withf(move |update_id, _| update_id == first_id)
            .in_sequence(&mut sequence)
            .returning(|_, _| Err(KeyEpochError::Repository("first ack failed".into())));
        outbox
            .expect_acknowledge_space_group_update()
            .times(1)
            .withf(move |update_id, _| update_id == second_id)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok(true));
        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch_group_update()
            .times(2)
            .returning(|_| Ok(()));
        let delivery = super::GroupUpdateDelivery::new(
            std::sync::Arc::new(outbox),
            std::sync::Arc::new(dispatch),
        );

        let error = super::GroupUpdateDeliveryPort::deliver_pending(&delivery, 123)
            .await
            .unwrap_err();

        assert!(
            matches!(error, KeyEpochError::Repository(message) if message == "first ack failed")
        );
    }
}
