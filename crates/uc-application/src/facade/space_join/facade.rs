use std::sync::Arc;

use tokio::sync::broadcast;

use crate::space::admission::cancel_space_join::{CancelSpaceJoinError, CancelSpaceJoinUseCase};
use crate::space::admission::query_space_join_status::{
    QuerySpaceJoinStatusError, QuerySpaceJoinStatusUseCase,
};
use crate::space::admission::recover_space_join_completion::{
    PendingJoinerCompleteAck, RecoverSpaceJoinCompletionError, RecoverSpaceJoinCompletionUseCase,
};
use crate::space::admission::CurrentJoinStatus;

pub struct SpaceJoinFacade {
    query_status: QuerySpaceJoinStatusUseCase,
    cancel: CancelSpaceJoinUseCase,
    recover_completion: RecoverSpaceJoinCompletionUseCase,
    events: broadcast::Sender<u64>,
}

impl SpaceJoinFacade {
    pub fn new(
        admission_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            query_status: QuerySpaceJoinStatusUseCase::new(Arc::clone(&admission_attempts)),
            cancel: CancelSpaceJoinUseCase::new(Arc::clone(&admission_attempts), events.clone()),
            recover_completion: RecoverSpaceJoinCompletionUseCase::new(admission_attempts),
            events,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.events.subscribe()
    }

    pub async fn query_status(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, QuerySpaceJoinStatusError> {
        self.query_status.execute().await
    }

    pub async fn cancel(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        self.cancel.execute(join_id).await
    }

    pub async fn recover_completion(
        &self,
    ) -> Result<Option<PendingJoinerCompleteAck>, RecoverSpaceJoinCompletionError> {
        self.recover_completion.execute().await
    }
}
