use std::sync::Arc;

use crate::space::lifecycle::CurrentSpaceIdentityPort;
use crate::space::lifecycle::IsSpaceUnlockedPort;

use super::{QuerySpaceAccessStateError, SpaceAccessState};

pub(crate) struct QuerySpaceAccessStateUseCase {
    current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
    is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
}

impl QuerySpaceAccessStateUseCase {
    pub(crate) fn new(
        current_space_identity: Arc<dyn CurrentSpaceIdentityPort>,
        is_unlocked: Arc<dyn IsSpaceUnlockedPort>,
    ) -> Self {
        Self {
            current_space_identity,
            is_unlocked,
        }
    }

    pub(crate) async fn execute(&self) -> Result<SpaceAccessState, QuerySpaceAccessStateError> {
        let Some(space_id) = self.current_space_identity.current_space_id().await? else {
            return Ok(SpaceAccessState {
                initialized: false,
                session_ready: false,
            });
        };

        Ok(SpaceAccessState {
            initialized: true,
            session_ready: self.is_unlocked.is_unlocked(&space_id).await,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use uc_core::ids::SpaceId;

    use super::*;
    use crate::space::lifecycle::{CurrentSpaceIdentityError, CurrentSpaceIdentityPort};

    struct StubCurrentSpace {
        result: Result<Option<SpaceId>, CurrentSpaceIdentityError>,
    }

    #[async_trait]
    impl CurrentSpaceIdentityPort for StubCurrentSpace {
        async fn current_space_id(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError> {
            self.result.clone()
        }
    }

    struct RecordingUnlocked {
        unlocked: bool,
        requested_spaces: Mutex<Vec<SpaceId>>,
    }

    #[async_trait]
    impl IsSpaceUnlockedPort for RecordingUnlocked {
        async fn is_unlocked(&self, space_id: &SpaceId) -> bool {
            self.requested_spaces.lock().unwrap().push(space_id.clone());
            self.unlocked
        }
    }

    fn use_case(
        current_space: Result<Option<SpaceId>, CurrentSpaceIdentityError>,
        unlocked: bool,
    ) -> (QuerySpaceAccessStateUseCase, Arc<RecordingUnlocked>) {
        let session = Arc::new(RecordingUnlocked {
            unlocked,
            requested_spaces: Mutex::new(Vec::new()),
        });
        (
            QuerySpaceAccessStateUseCase::new(
                Arc::new(StubCurrentSpace {
                    result: current_space,
                }),
                session.clone(),
            ),
            session,
        )
    }

    #[tokio::test]
    async fn no_current_space_returns_uninitialized_state() {
        let (query, session) = use_case(Ok(None), true);

        let state = query.execute().await.unwrap();

        assert_eq!(
            state,
            SpaceAccessState {
                initialized: false,
                session_ready: false,
            }
        );
        assert!(session.requested_spaces.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn current_unlocked_space_returns_ready_state() {
        let space_id = SpaceId::from("space-a");
        let (query, session) = use_case(Ok(Some(space_id.clone())), true);

        let state = query.execute().await.unwrap();

        assert_eq!(
            state,
            SpaceAccessState {
                initialized: true,
                session_ready: true,
            }
        );
        assert_eq!(*session.requested_spaces.lock().unwrap(), vec![space_id]);
    }

    #[tokio::test]
    async fn current_space_failure_is_preserved() {
        let (query, session) = use_case(Err(CurrentSpaceIdentityError::Unavailable), true);

        let error = query.execute().await.unwrap_err();

        assert!(matches!(
            error,
            QuerySpaceAccessStateError::CurrentSpace(CurrentSpaceIdentityError::Unavailable)
        ));
        assert!(session.requested_spaces.lock().unwrap().is_empty());
    }
}
