use std::sync::Arc;

use uc_core::error_class::ErrorClass;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

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

    /// 失败原样返回，同时在这里记录一次固定分类（公开契约边界会丢弃来源）。
    pub(crate) async fn execute(&self) -> Result<SpaceAccessState, QuerySpaceAccessStateError> {
        let result = self.query().await;
        if let Err(error) = &result {
            record_failure(error);
        }
        result
    }

    async fn query(&self) -> Result<SpaceAccessState, QuerySpaceAccessStateError> {
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

fn record_failure(error: &QuerySpaceAccessStateError) {
    let QuerySpaceAccessStateError::CurrentSpace(source) = error;
    uc_warn!(
        operation = "query_space_access_state",
        outcome = "failed",
        error_kind = "current_space_identity",
        error_class = error.class(),
        source_class = source.class(),
        io_error_kind = io_error_kind(error),
        "space access state query failed"
    );
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
            match &self.result {
                Ok(space_id) => Ok(space_id.clone()),
                Err(CurrentSpaceIdentityError::Unavailable { .. }) => {
                    Err(CurrentSpaceIdentityError::unavailable())
                }
                Err(CurrentSpaceIdentityError::Inconsistent { .. }) => {
                    Err(CurrentSpaceIdentityError::inconsistent())
                }
            }
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

    struct IoFailingCurrentSpace;

    #[async_trait]
    impl CurrentSpaceIdentityPort for IoFailingCurrentSpace {
        async fn current_space_id(&self) -> Result<Option<SpaceId>, CurrentSpaceIdentityError> {
            Err(CurrentSpaceIdentityError::unavailable_from(
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, "SECRET_SPACE_PATH"),
            ))
        }
    }

    #[tokio::test]
    async fn a_failed_query_records_only_fixed_classification() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let query = QuerySpaceAccessStateUseCase::new(
            Arc::new(IoFailingCurrentSpace),
            Arc::new(RecordingUnlocked {
                unlocked: true,
                requested_spaces: Mutex::new(Vec::new()),
            }),
        );

        let error = query.execute().await.unwrap_err();

        assert!(matches!(error, QuerySpaceAccessStateError::CurrentSpace(_)));
        let output = logs.output();
        assert_eq!(logs.count("space access state query failed"), 1, "{output}");
        assert!(
            output.contains("error_kind=\"current_space_identity\""),
            "{output}"
        );
        assert!(output.contains("error_class=\"current_space\""), "{output}");
        assert!(output.contains("source_class=\"unavailable\""), "{output}");
        assert!(
            output.contains("io_error_kind=PermissionDenied"),
            "{output}"
        );
        assert!(!output.contains("SECRET_SPACE_PATH"), "{output}");
    }

    #[tokio::test]
    async fn a_successful_query_writes_no_record() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let (query, _) = use_case(Ok(None), true);

        query.execute().await.unwrap();

        assert_eq!(logs.output(), "");
    }

    #[tokio::test]
    async fn current_space_failure_is_preserved() {
        let (query, session) = use_case(Err(CurrentSpaceIdentityError::unavailable()), true);

        let error = query.execute().await.unwrap_err();

        assert!(matches!(
            error,
            QuerySpaceAccessStateError::CurrentSpace(CurrentSpaceIdentityError::Unavailable { .. })
        ));
        assert!(session.requested_spaces.lock().unwrap().is_empty());
    }
}
