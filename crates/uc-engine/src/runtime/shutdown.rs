use async_trait::async_trait;
use tokio::time::Instant;
use tracing::warn;
use uc_application::facade::LifecycleError;

use super::session_supervisor::lifecycle_error;
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
    async fn stop_network_recovery(&self) -> anyhow::Result<()>;
    async fn stop_session(&self, deadline: Option<Instant>) -> anyhow::Result<()>;
    async fn close_file_transfers(&self) -> anyhow::Result<()>;
    async fn stop_process_tasks(&self, deadline: Option<Instant>) -> anyhow::Result<()>;
    fn close_local_resources(&self);
}

struct ProductionShutdownActions<'a>(&'a ProductionRuntime);

#[async_trait]
impl ShutdownActions for ProductionShutdownActions<'_> {
    async fn stop_network_recovery(&self) -> anyhow::Result<()> {
        self.0.network_recovery.shutdown().await.map_err(Into::into)
    }

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
    run(&ProductionShutdownActions(runtime), deadline)
        .await
        .map_err(lifecycle_error)
}

async fn run(
    actions: &dyn ShutdownActions,
    deadline: Option<Instant>,
) -> Result<(), LifecycleError> {
    let mut outcome = ShutdownOutcome::default();

    if let Err(error) = actions.stop_network_recovery().await {
        outcome.errors.push(error);
    }

    match actions.stop_session(deadline).await {
        Ok(()) => outcome.session_stopped = true,
        Err(error) => outcome.errors.push(error),
    }
    match actions.close_file_transfers().await {
        Ok(()) => outcome.file_transfers_closed = true,
        Err(error) => outcome.errors.push(error),
    }
    match actions.stop_process_tasks(deadline).await {
        Ok(()) => outcome.process_tasks_stopped = true,
        Err(error) => outcome.errors.push(error),
    }

    if outcome.resources_can_close() {
        actions.close_local_resources();
    }

    LifecycleError::from_errors(outcome.errors)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct RecordingActions {
        calls: Mutex<Vec<&'static str>>,
        fail_network: bool,
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
        async fn stop_network_recovery(&self) -> anyhow::Result<()> {
            self.result(self.fail_network, "network")
        }

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

    #[tokio::test]
    async fn every_shutdown_action_runs_after_earlier_failures() {
        let actions = RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_network: true,
            fail_session: true,
            fail_transfers: true,
            fail_tasks: true,
        };

        let error = run(&actions, None).await.unwrap_err();

        assert_eq!(error.additional.len(), 3);
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["network", "session", "transfers", "tasks"]
        );
    }

    #[tokio::test]
    async fn local_resources_close_after_every_dependent_owner_stops() {
        let actions = RecordingActions {
            calls: Mutex::new(Vec::new()),
            fail_network: true,
            fail_session: false,
            fail_transfers: false,
            fail_tasks: false,
        };

        assert!(run(&actions, None).await.is_err());
        assert_eq!(
            *actions.calls.lock().unwrap(),
            vec!["network", "session", "transfers", "tasks", "resources"]
        );
    }
}
