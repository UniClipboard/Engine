use std::sync::Arc;

use async_trait::async_trait;
use tracing::warn;

use crate::clipboard_write::MobileConsumableBackfill;
use crate::facade::search::SearchFacade;
use crate::facade::space_setup::{
    FactoryResetError, InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult,
    SpaceFacade, TryResumeSessionError, UnlockSpaceError, UnlockSpaceInput, UnlockSpaceResult,
};
use crate::membership::MembershipConvergenceActivityPort;
use crate::receive_reconciliation::EnsureReceiveReadyPort;
use uc_core::ids::SpaceId;
use uc_core::ports::setup::SetupStatusPort;
use uc_core::ports::space::{LockSpacePort, ResumeSpaceSessionPort};

#[async_trait]
pub(crate) trait SearchSessionActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), String>;
    async fn resume(&self) -> Result<(), String>;
}

#[async_trait]
pub(crate) trait SpaceSessionAccessPort: Send + Sync {
    async fn initialize_space(
        &self,
        input: InitializeSpaceInput,
    ) -> Result<InitializeSpaceResult, InitializeSpaceError>;
    async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError>;
    async fn unlock_secure_storage(&self) -> Result<bool, SpaceSessionAccessError>;
    async fn resume_space_session(&self) -> Result<bool, SpaceSessionAccessError>;
    async fn refresh_presence(&self) -> Result<(), String>;
    async fn lock_space(&self) -> Result<(), SpaceSessionAccessError>;
    async fn factory_reset(&self) -> Result<(), FactoryResetError>;
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SpaceSessionAccessError {
    #[error("space session recovery failed: {0}")]
    Recovery(String),
    #[error("space lock failed")]
    LockFailed,
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceSessionError {
    #[error(transparent)]
    Activity(#[from] SpaceActivityError),
    #[error("space session recovery failed: {0}")]
    Recovery(String),
    #[error("space lock failed")]
    LockFailed,
    #[error("space lock failed and activity recovery was incomplete: {0}")]
    RecoveryFailed(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceActivityError {
    #[error("search session activation failed: {0}")]
    Search(String),
    #[error("receive activation failed: {0}")]
    Receive(String),
    #[error("membership activation failed: {0}")]
    Membership(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoverSpaceSessionResult {
    pub unlocked: bool,
    pub resumed: bool,
}

pub(crate) struct SpaceActivityCoordinator {
    membership: Arc<dyn MembershipConvergenceActivityPort>,
    receive: Arc<dyn EnsureReceiveReadyPort>,
    search: Arc<dyn SearchSessionActivityPort>,
}

impl SpaceActivityCoordinator {
    pub(crate) fn new(
        membership: Arc<dyn MembershipConvergenceActivityPort>,
        receive: Arc<dyn EnsureReceiveReadyPort>,
        search: Arc<dyn SearchSessionActivityPort>,
    ) -> Self {
        Self {
            membership,
            receive,
            search,
        }
    }

    pub(crate) async fn resume_after_session_ready(&self) -> Result<(), SpaceSessionError> {
        self.search
            .resume()
            .await
            .map_err(SpaceActivityError::Search)?;
        self.receive
            .ensure_receive_ready()
            .await
            .map_err(|error| SpaceActivityError::Receive(error.to_string()))?;
        self.membership
            .resume()
            .await
            .map_err(SpaceActivityError::Membership)?;
        Ok(())
    }

    async fn pause_for_lock(&self) -> Result<(), SpaceSessionError> {
        self.membership
            .pause()
            .await
            .map_err(SpaceActivityError::Membership)?;
        self.receive.close_receive_gate();
        self.search
            .pause()
            .await
            .map_err(SpaceActivityError::Search)?;
        Ok(())
    }

    async fn restore_after_failed_lock(&self) -> Result<(), SpaceSessionError> {
        let search = self.search.resume().await;
        let receive = self.receive.ensure_receive_ready().await;
        let membership = self.membership.resume().await;
        match (search, receive, membership) {
            (Ok(()), Ok(()), Ok(())) => Ok(()),
            (search, receive, membership) => Err(SpaceSessionError::RecoveryFailed(format!(
                "search={}, receive={}, membership={}",
                search.err().unwrap_or_else(|| "restored".to_string()),
                receive
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "restored".to_string()),
                membership.err().unwrap_or_else(|| "restored".to_string()),
            ))),
        }
    }
}

pub(crate) struct SpaceSessionCoordinator {
    session_access: Arc<dyn SpaceSessionAccessPort>,
    activities: SpaceActivityCoordinator,
}

pub struct SpaceSessionActivityDeps {
    pub membership: crate::membership::MembershipConvergenceActivity,
    pub receive: Arc<dyn EnsureReceiveReadyPort>,
}

pub struct SpaceSessionAccessDeps {
    pub setup_status: Arc<dyn SetupStatusPort>,
    pub resume_session: Arc<dyn ResumeSpaceSessionPort>,
    pub lock: Arc<dyn LockSpacePort>,
    pub mobile_consumable_backfill: Arc<dyn MobileConsumableBackfill>,
}

pub(crate) fn build_space_session_coordinator(
    space: Arc<SpaceFacade>,
    search: Arc<SearchFacade>,
    activities: SpaceSessionActivityDeps,
    access: SpaceSessionAccessDeps,
) -> Arc<SpaceSessionCoordinator> {
    Arc::new(SpaceSessionCoordinator::new(
        Arc::new(DefaultSpaceSessionAccess::new(space, access)),
        SpaceActivityCoordinator::new(Arc::new(activities.membership), activities.receive, search),
    ))
}

impl SpaceSessionCoordinator {
    pub(crate) fn new(
        session_access: Arc<dyn SpaceSessionAccessPort>,
        activities: SpaceActivityCoordinator,
    ) -> Self {
        Self {
            session_access,
            activities,
        }
    }

    pub(crate) async fn lock_space(&self) -> Result<(), SpaceSessionError> {
        self.activities.pause_for_lock().await?;
        if self.session_access.lock_space().await.is_ok() {
            return Ok(());
        }
        self.activities.restore_after_failed_lock().await?;
        Err(SpaceSessionError::LockFailed)
    }

    pub(crate) async fn initialize_space(
        &self,
        input: InitializeSpaceInput,
    ) -> Result<InitializeSpaceResult, InitializeSpaceError> {
        let result = self.session_access.initialize_space(input).await?;
        self.activities
            .resume_after_session_ready()
            .await
            .map_err(|error| InitializeSpaceError::Internal(error.to_string()))?;
        Ok(result)
    }

    pub(crate) async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError> {
        let result = self.session_access.unlock_space(input).await?;
        self.activities
            .resume_after_session_ready()
            .await
            .map_err(|error| UnlockSpaceError::Internal(error.to_string()))?;
        Ok(result)
    }

    pub(crate) async fn recover_session(
        &self,
        allow_secure_storage_unlock: bool,
    ) -> Result<RecoverSpaceSessionResult, SpaceSessionError> {
        if !allow_secure_storage_unlock {
            return Ok(RecoverSpaceSessionResult {
                unlocked: false,
                resumed: false,
            });
        }

        let unlocked = self
            .session_access
            .unlock_secure_storage()
            .await
            .map_err(|error| SpaceSessionError::Recovery(error.to_string()))?;
        if !unlocked {
            return Ok(RecoverSpaceSessionResult {
                unlocked: false,
                resumed: false,
            });
        }

        let resumed = self
            .session_access
            .resume_space_session()
            .await
            .map_err(|error| SpaceSessionError::Recovery(error.to_string()))?;
        if resumed {
            if let Err(error) = self.session_access.refresh_presence().await {
                warn!(error = %error, "presence refresh failed after session recovery");
            }
        }
        self.activities.resume_after_session_ready().await?;
        Ok(RecoverSpaceSessionResult {
            unlocked: true,
            resumed,
        })
    }

    pub(crate) async fn resume_after_space_change(&self) -> Result<(), SpaceSessionError> {
        self.activities.resume_after_session_ready().await
    }

    pub(crate) async fn factory_reset(&self) -> Result<(), FactoryResetError> {
        self.activities
            .pause_for_lock()
            .await
            .map_err(|error| FactoryResetError::Internal(error.to_string()))?;
        if let Err(error) = self.session_access.factory_reset().await {
            self.activities
                .restore_after_failed_lock()
                .await
                .map_err(|recovery| FactoryResetError::Internal(recovery.to_string()))?;
            return Err(error);
        }
        Ok(())
    }
}

pub(crate) struct DefaultSpaceSessionAccess {
    space: Arc<SpaceFacade>,
    deps: SpaceSessionAccessDeps,
}

impl DefaultSpaceSessionAccess {
    pub(crate) fn new(space: Arc<SpaceFacade>, deps: SpaceSessionAccessDeps) -> Self {
        Self { space, deps }
    }
}

#[async_trait]
impl SpaceSessionAccessPort for DefaultSpaceSessionAccess {
    async fn initialize_space(
        &self,
        input: InitializeSpaceInput,
    ) -> Result<InitializeSpaceResult, InitializeSpaceError> {
        self.space.initialize_space(input).await
    }

    async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError> {
        self.space.unlock_space(input).await
    }

    async fn unlock_secure_storage(&self) -> Result<bool, SpaceSessionAccessError> {
        let status = self
            .deps
            .setup_status
            .get_status()
            .await
            .map_err(|error| SpaceSessionAccessError::Recovery(error.to_string()))?;
        let space_id = status.space_id.unwrap_or_else(|| SpaceId::from("space"));
        match self.deps.resume_session.try_resume_session(&space_id).await {
            Ok(Some(_)) => {
                self.deps
                    .mobile_consumable_backfill
                    .backfill_best_effort()
                    .await;
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(error) => Err(SpaceSessionAccessError::Recovery(error.to_string())),
        }
    }

    async fn resume_space_session(&self) -> Result<bool, SpaceSessionAccessError> {
        self.space
            .try_resume_session()
            .await
            .map_err(|error: TryResumeSessionError| {
                SpaceSessionAccessError::Recovery(error.to_string())
            })
    }

    async fn refresh_presence(&self) -> Result<(), String> {
        self.space
            .refresh_presence()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn lock_space(&self) -> Result<(), SpaceSessionAccessError> {
        self.deps
            .lock
            .lock(&SpaceId::from("space"))
            .await
            .map_err(|_| SpaceSessionAccessError::LockFailed)
    }

    async fn factory_reset(&self) -> Result<(), FactoryResetError> {
        self.space.factory_reset().await
    }
}

#[async_trait]
impl SearchSessionActivityPort for SearchFacade {
    async fn pause(&self) -> Result<(), String> {
        self.pause_background_activity().await;
        Ok(())
    }

    async fn resume(&self) -> Result<(), String> {
        self.on_session_ready().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

    use crate::receive_reconciliation::{
        EnsureReceiveReadyPort, ReceiveReadinessError, ReceiveReadinessStatus,
    };

    use super::{
        SearchSessionActivityPort, SpaceActivityCoordinator, SpaceSessionAccessError,
        SpaceSessionAccessPort, SpaceSessionCoordinator, SpaceSessionError,
    };
    use crate::facade::{
        FactoryResetError, InitializeSpaceError, InitializeSpaceInput, InitializeSpaceResult,
        UnlockSpaceInput, UnlockSpaceResult,
    };
    use crate::membership::MembershipConvergenceActivityPort;
    use uc_core::ids::SpaceId;

    #[derive(Clone)]
    struct CallLog(Arc<Mutex<Vec<&'static str>>>);

    impl CallLog {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }

        fn push(&self, call: &'static str) {
            self.0.lock().expect("call log lock").push(call);
        }

        fn calls(&self) -> Vec<&'static str> {
            self.0.lock().expect("call log lock").clone()
        }
    }

    struct RecordingMembershipActivity {
        calls: CallLog,
    }

    #[async_trait]
    impl MembershipConvergenceActivityPort for RecordingMembershipActivity {
        async fn pause(&self) -> Result<(), String> {
            self.calls.push("membership.pause");
            Ok(())
        }

        async fn resume(&self) -> Result<(), String> {
            self.calls.push("membership.resume");
            Ok(())
        }
    }

    struct RecordingReceiveActivity {
        calls: CallLog,
    }

    struct RecordingSearchActivity {
        calls: CallLog,
    }

    #[async_trait]
    impl SearchSessionActivityPort for RecordingSearchActivity {
        async fn pause(&self) -> Result<(), String> {
            self.calls.push("search.pause");
            Ok(())
        }

        async fn resume(&self) -> Result<(), String> {
            self.calls.push("search.resume");
            Ok(())
        }
    }

    #[async_trait]
    impl EnsureReceiveReadyPort for RecordingReceiveActivity {
        async fn ensure_receive_ready(&self) -> Result<(), ReceiveReadinessError> {
            self.calls.push("receive.resume");
            Ok(())
        }

        fn close_receive_gate(&self) {
            self.calls.push("receive.pause");
        }

        fn receive_readiness_status(&self) -> ReceiveReadinessStatus {
            ReceiveReadinessStatus {
                ready: true,
                degraded_reason: None,
            }
        }
    }

    struct FailingSessionAccess {
        calls: CallLog,
    }

    #[async_trait]
    impl SpaceSessionAccessPort for FailingSessionAccess {
        async fn initialize_space(
            &self,
            _input: InitializeSpaceInput,
        ) -> Result<InitializeSpaceResult, InitializeSpaceError> {
            Err(InitializeSpaceError::Internal("not used".to_string()))
        }

        async fn unlock_space(
            &self,
            _input: UnlockSpaceInput,
        ) -> Result<UnlockSpaceResult, crate::facade::UnlockSpaceError> {
            self.calls.push("session.unlock");
            Ok(UnlockSpaceResult {
                space_id: SpaceId::from("space-a"),
            })
        }

        async fn unlock_secure_storage(&self) -> Result<bool, SpaceSessionAccessError> {
            self.calls.push("session.unlock_secure_storage");
            Ok(false)
        }

        async fn resume_space_session(&self) -> Result<bool, SpaceSessionAccessError> {
            self.calls.push("session.resume");
            Ok(false)
        }

        async fn refresh_presence(&self) -> Result<(), String> {
            self.calls.push("presence.refresh");
            Ok(())
        }

        async fn lock_space(&self) -> Result<(), SpaceSessionAccessError> {
            self.calls.push("session.lock");
            Err(SpaceSessionAccessError::LockFailed)
        }

        async fn factory_reset(&self) -> Result<(), FactoryResetError> {
            self.calls.push("session.factory_reset");
            Ok(())
        }
    }

    #[tokio::test]
    async fn lock_failure_restores_every_paused_activity_in_reverse_order() {
        let calls = CallLog::new();
        let activities = SpaceActivityCoordinator::new(
            Arc::new(RecordingMembershipActivity {
                calls: calls.clone(),
            }),
            Arc::new(RecordingReceiveActivity {
                calls: calls.clone(),
            }),
            Arc::new(RecordingSearchActivity {
                calls: calls.clone(),
            }),
        );
        let coordinator = SpaceSessionCoordinator::new(
            Arc::new(FailingSessionAccess {
                calls: calls.clone(),
            }),
            activities,
        );

        let error = coordinator
            .lock_space()
            .await
            .expect_err("lock failure must be returned");

        assert!(matches!(error, SpaceSessionError::LockFailed));
        assert_eq!(
            calls.calls(),
            vec![
                "membership.pause",
                "receive.pause",
                "search.pause",
                "session.lock",
                "search.resume",
                "receive.resume",
                "membership.resume",
            ]
        );
    }

    #[tokio::test]
    async fn session_activation_resumes_search_receive_and_membership_in_order() {
        let calls = CallLog::new();
        let activities = SpaceActivityCoordinator::new(
            Arc::new(RecordingMembershipActivity {
                calls: calls.clone(),
            }),
            Arc::new(RecordingReceiveActivity {
                calls: calls.clone(),
            }),
            Arc::new(RecordingSearchActivity {
                calls: calls.clone(),
            }),
        );

        activities
            .resume_after_session_ready()
            .await
            .expect("session activities resume");

        assert_eq!(
            calls.calls(),
            vec!["search.resume", "receive.resume", "membership.resume"]
        );
    }

    #[tokio::test]
    async fn unlock_resumes_all_session_activities_before_returning() {
        let calls = CallLog::new();
        let coordinator = SpaceSessionCoordinator::new(
            Arc::new(FailingSessionAccess {
                calls: calls.clone(),
            }),
            SpaceActivityCoordinator::new(
                Arc::new(RecordingMembershipActivity {
                    calls: calls.clone(),
                }),
                Arc::new(RecordingReceiveActivity {
                    calls: calls.clone(),
                }),
                Arc::new(RecordingSearchActivity {
                    calls: calls.clone(),
                }),
            ),
        );

        let result = coordinator
            .unlock_space(UnlockSpaceInput {
                passphrase: "correct horse battery staple".to_string(),
            })
            .await
            .expect("unlock succeeds");

        assert_eq!(result.space_id, SpaceId::from("space-a"));
        assert_eq!(
            calls.calls(),
            vec![
                "session.unlock",
                "search.resume",
                "receive.resume",
                "membership.resume",
            ]
        );
    }

    #[tokio::test]
    async fn recovery_without_an_unlocked_session_leaves_activities_paused() {
        let calls = CallLog::new();
        let coordinator = SpaceSessionCoordinator::new(
            Arc::new(FailingSessionAccess {
                calls: calls.clone(),
            }),
            SpaceActivityCoordinator::new(
                Arc::new(RecordingMembershipActivity {
                    calls: calls.clone(),
                }),
                Arc::new(RecordingReceiveActivity {
                    calls: calls.clone(),
                }),
                Arc::new(RecordingSearchActivity {
                    calls: calls.clone(),
                }),
            ),
        );

        let result = coordinator
            .recover_session(true)
            .await
            .expect("missing secure-storage session is not an error");

        assert!(!result.unlocked);
        assert!(!result.resumed);
        assert_eq!(calls.calls(), vec!["session.unlock_secure_storage"]);
    }
}
