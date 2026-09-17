use async_trait::async_trait;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tracing::warn;
use uc_application::deps::{LifecycleError, StopProfileRuntimePort};
use uc_core::TaskRegistry;

use super::session_supervisor::{lifecycle_error, SessionSupervisor};
use super::task_shutdown::shutdown_tasks;
use super::ProductionRuntime;
use crate::EngineError;

#[derive(Default)]
struct ShutdownOutcome {
    errors: Vec<anyhow::Error>,
    session_stopped: bool,
    file_transfers_closed: bool,
    process_tasks_stopped: bool,
}

impl ShutdownOutcome {
    fn resources_can_close(&self) -> bool {
        self.session_stopped && self.file_transfers_closed && self.process_tasks_stopped
    }
}

#[async_trait]
trait ShutdownActions: Send + Sync {
    async fn stop_session(&self, deadline: Option<Instant>) -> anyhow::Result<()>;
    async fn close_file_transfers(&self) -> anyhow::Result<()>;
    async fn stop_process_tasks(&self, deadline: Option<Instant>) -> anyhow::Result<()>;
    fn close_local_resources(&self);
}

struct ProductionShutdownActions<'a>(&'a ProductionRuntime);

#[async_trait]
impl ShutdownActions for ProductionShutdownActions<'_> {
    async fn stop_session(&self, deadline: Option<Instant>) -> anyhow::Result<()> {
        self.0
            .session_supervisor
            .stop(deadline)
            .await
            .map_err(Into::into)
    }

    async fn close_file_transfers(&self) -> anyhow::Result<()> {
        self.0
            .session_supervisor
            .close_file_transfers()
            .await
            .map_err(Into::into)
    }

    async fn stop_process_tasks(&self, deadline: Option<Instant>) -> anyhow::Result<()> {
        shutdown_tasks(&self.0.task_registry, deadline)
            .await
            .into_result()
            .map_err(Into::into)
    }

    fn close_local_resources(&self) {
        self.0.security_lifecycle.close_security_session();
        self.0.session_supervisor.clear_factory();
        if let Err(error) = std::fs::remove_dir_all(&self.0.clipboard_import_root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn!(error = %error, "failed to remove host clipboard imports");
            }
        }
    }
}

pub(super) async fn shutdown(
    runtime: &ProductionRuntime,
    deadline: Option<Instant>,
) -> Result<(), EngineError> {
    let outcome = stop_runtime_resource_users(
        async {
            runtime
                .network_recovery
                .shutdown()
                .await
                .map_err(anyhow::Error::new)
        },
        &ProductionShutdownActions(runtime),
        deadline,
    )
    .await;
    LifecycleError::from_errors(outcome.errors).map_err(lifecycle_error)
}

pub(super) async fn shutdown_failed_start(
    network_recovery: &uc_application::facade::NetworkRecoveryFacade,
    runtime: &ProfileRuntimeStopper,
) -> Result<(), LifecycleError> {
    let outcome = stop_runtime_resource_users(
        async {
            network_recovery
                .shutdown()
                .await
                .map_err(anyhow::Error::new)
        },
        runtime,
        None,
    )
    .await;
    LifecycleError::from_errors(outcome.errors)
}

async fn stop_runtime_resource_users<Recovery>(
    stop_network_recovery: Recovery,
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome
where
    Recovery: Future<Output = anyhow::Result<()>>,
{
    let (recovery, mut outcome) = tokio::join!(
        stop_network_recovery,
        stop_resource_users(actions, deadline),
    );
    if outcome.resources_can_close() {
        actions.close_local_resources();
    }
    if let Err(error) = recovery {
        outcome.errors.insert(0, error);
    }
    outcome
}

async fn stop_resource_users_and_close(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome {
    let outcome = stop_resource_users(actions, deadline).await;
    if outcome.resources_can_close() {
        actions.close_local_resources();
    }
    outcome
}

async fn stop_resource_users(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome {
    let (mut outcome, process_tasks) = tokio::join!(
        stop_session_and_transfers(actions, deadline),
        actions.stop_process_tasks(deadline),
    );
    outcome.process_tasks_stopped = true;
    if let Err(error) = process_tasks {
        outcome.errors.push(error);
    }
    outcome
}

async fn stop_session_and_transfers(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> ShutdownOutcome {
    let mut outcome = ShutdownOutcome::default();
    match actions.stop_session(deadline).await {
        Ok(()) => outcome.session_stopped = true,
        Err(error) => outcome.errors.push(error),
    }
    match actions.close_file_transfers().await {
        Ok(()) => outcome.file_transfers_closed = true,
        Err(error) => outcome.errors.push(error),
    }
    outcome
}

pub(super) struct ProfileRuntimeStopper {
    security_lifecycle: Arc<uc_infra::space::RuntimeSpaceAccessAdapter>,
    session_supervisor: Arc<SessionSupervisor>,
    tasks: Arc<TaskRegistry>,
}

impl ProfileRuntimeStopper {
    pub(super) fn new(
        security_lifecycle: Arc<uc_infra::space::RuntimeSpaceAccessAdapter>,
        session_supervisor: Arc<SessionSupervisor>,
        tasks: Arc<TaskRegistry>,
    ) -> Self {
        Self {
            security_lifecycle,
            session_supervisor,
            tasks,
        }
    }
}

#[async_trait]
impl ShutdownActions for ProfileRuntimeStopper {
    async fn stop_session(&self, _deadline: Option<Instant>) -> anyhow::Result<()> {
        self.session_supervisor.stop(None).await.map_err(Into::into)
    }

    async fn close_file_transfers(&self) -> anyhow::Result<()> {
        self.session_supervisor
            .close_file_transfers()
            .await
            .map_err(Into::into)
    }

    async fn stop_process_tasks(&self, _deadline: Option<Instant>) -> anyhow::Result<()> {
        let deadline = Instant::now().checked_add(Duration::from_millis(500));
        shutdown_tasks(&self.tasks, deadline)
            .await
            .into_result()
            .map_err(Into::into)
    }

    fn close_local_resources(&self) {
        self.security_lifecycle.close_security_session();
        self.session_supervisor.clear_factory();
    }
}

#[async_trait]
impl StopProfileRuntimePort for ProfileRuntimeStopper {
    async fn stop_profile_runtime(&self) -> Result<(), LifecycleError> {
        let outcome = stop_resource_users_and_close(self, None).await;
        LifecycleError::from_errors(outcome.errors)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingActions {
        calls: Mutex<Vec<&'static str>>,
        fail_session: bool,
        fail_transfers: bool,
        fail_tasks: bool,
    }

    impl RecordingActions {
        fn result(&self, fail: bool, name: &'static str) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push(name);
            if fail {
                Err(anyhow::anyhow!("{name} private failure"))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl ShutdownActions for RecordingActions {
        async fn stop_session(&self, _deadline: Option<Instant>) -> anyhow::Result<()> {
            self.result(self.fail_session, "session")
        }

        async fn close_file_transfers(&self) -> anyhow::Result<()> {
            self.result(self.fail_transfers, "transfers")
        }

        async fn stop_process_tasks(&self, _deadline: Option<Instant>) -> anyhow::Result<()> {
            self.result(self.fail_tasks, "tasks")
        }

        fn close_local_resources(&self) {
            self.calls.lock().unwrap().push("resources");
        }
    }

    #[test]
    fn local_resources_require_every_dependent_owner_to_stop() {
        for (session_stopped, file_transfers_closed, process_tasks_stopped) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let outcome = ShutdownOutcome {
                session_stopped,
                file_transfers_closed,
                process_tasks_stopped,
                ..ShutdownOutcome::default()
            };
            assert!(!outcome.resources_can_close());
        }
        assert!(ShutdownOutcome {
            session_stopped: true,
            file_transfers_closed: true,
            process_tasks_stopped: true,
            ..ShutdownOutcome::default()
        }
        .resources_can_close());
    }

    #[test]
    fn shutdown_outcome_retains_every_failure() {
        let outcome = ShutdownOutcome {
            errors: vec![
                anyhow::anyhow!("first private failure"),
                anyhow::anyhow!("second private failure"),
                anyhow::anyhow!("third private failure"),
            ],
            ..ShutdownOutcome::default()
        };
        let error = LifecycleError::from_errors(outcome.errors).unwrap_err();

        assert_eq!(error.additional.len(), 2);
        assert_eq!(error.to_string(), "runtime lifecycle transition incomplete");
        assert!(!format!("{error:?}").contains("private failure"));
    }

    struct BlockingSessionActions {
        calls: Mutex<Vec<&'static str>>,
        session_started: tokio::sync::Notify,
        session_release: tokio::sync::Notify,
    }

    #[async_trait]
    impl ShutdownActions for BlockingSessionActions {
        async fn stop_session(&self, _deadline: Option<Instant>) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("session");
            self.session_started.notify_one();
            self.session_release.notified().await;
            Ok(())
        }

        async fn close_file_transfers(&self) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("transfers");
            Ok(())
        }

        async fn stop_process_tasks(&self, _deadline: Option<Instant>) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push("tasks");
            Ok(())
        }

        fn close_local_resources(&self) {
            self.calls.lock().unwrap().push("resources");
        }
    }

    #[tokio::test]
    async fn slow_session_does_not_delay_process_task_stop() {
        let actions = Arc::new(BlockingSessionActions {
            calls: Mutex::new(Vec::new()),
            session_started: tokio::sync::Notify::new(),
            session_release: tokio::sync::Notify::new(),
        });
        let stopping = tokio::spawn({
            let actions = Arc::clone(&actions);
            async move { stop_resource_users_and_close(actions.as_ref(), None).await }
        });

        actions.session_started.notified().await;
        tokio::task::yield_now().await;
        assert_eq!(*actions.calls.lock().unwrap(), vec!["session", "tasks"]);
        assert!(!stopping.is_finished());
        actions.session_release.notify_one();
        assert!(stopping.await.unwrap().errors.is_empty());
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "tasks", "transfers", "resources"]
        );
    }

    #[tokio::test]
    async fn completed_network_recovery_failure_does_not_block_local_resource_close() {
        let actions = RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_session: false,
            fail_transfers: false,
            fail_tasks: false,
        };
        let outcome = stop_runtime_resource_users(
            async { Err(anyhow::anyhow!("recovery stop failed")) },
            &actions,
            None,
        )
        .await;

        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "transfers", "tasks", "resources"]
        );
    }

    #[tokio::test]
    async fn slow_network_recovery_does_not_delay_other_stop_notifications() {
        let actions = Arc::new(RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_session: false,
            fail_transfers: false,
            fail_tasks: false,
        });
        let recovery_started = Arc::new(tokio::sync::Notify::new());
        let recovery_release = Arc::new(tokio::sync::Notify::new());
        let stopping = tokio::spawn({
            let actions = Arc::clone(&actions);
            let recovery_started = Arc::clone(&recovery_started);
            let recovery_release = Arc::clone(&recovery_release);
            async move {
                stop_runtime_resource_users(
                    async move {
                        recovery_started.notify_one();
                        recovery_release.notified().await;
                        Ok(())
                    },
                    actions.as_ref(),
                    None,
                )
                .await
            }
        });

        recovery_started.notified().await;
        tokio::task::yield_now().await;
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "transfers", "tasks"]
        );
        assert!(!stopping.is_finished());
        recovery_release.notify_one();
        assert!(stopping.await.unwrap().errors.is_empty());
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "transfers", "tasks", "resources"]
        );
    }

    #[tokio::test]
    async fn every_resource_user_stops_after_earlier_failures() {
        let actions = RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_session: true,
            fail_transfers: true,
            fail_tasks: true,
        };

        let outcome = stop_resource_users(&actions, None).await;
        let error = LifecycleError::from_errors(outcome.errors).unwrap_err();

        assert_eq!(error.additional.len(), 2);
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "transfers", "tasks"]
        );
    }

    #[tokio::test]
    async fn completed_task_stop_failure_does_not_block_local_resource_close() {
        let actions = RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_session: false,
            fail_transfers: false,
            fail_tasks: true,
        };

        let outcome = stop_resource_users_and_close(&actions, None).await;
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "transfers", "tasks", "resources"]
        );
    }

    #[tokio::test]
    async fn local_resources_close_after_every_dependent_owner_stops() {
        let actions = RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_session: false,
            fail_transfers: false,
            fail_tasks: false,
        };

        let outcome = stop_resource_users_and_close(&actions, None).await;
        assert!(outcome.errors.is_empty());
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["session", "transfers", "tasks", "resources"]
        );
    }
}
