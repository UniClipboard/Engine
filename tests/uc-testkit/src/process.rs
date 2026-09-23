use std::{process::ExitStatus, time::Duration};

use tokio::{process::Command, time::timeout};

use crate::{
    CleanupStatus, FailureKind, ScenarioFailure,
    resource::{ResourceTracker, complete, register},
};

pub(crate) async fn run_child_process(
    tracker: &ResourceTracker,
    label: &'static str,
    command: &mut Command,
    wait_budget: Duration,
) -> Result<ExitStatus, ScenarioFailure> {
    let resource = register(tracker, "child-process", label);
    command.kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            complete(tracker, resource, CleanupStatus::Failed);
            return Err(ScenarioFailure::new(
                FailureKind::EnvironmentUnavailable,
                "child-process-spawn",
            ));
        }
    };

    match timeout(wait_budget, child.wait()).await {
        Ok(Ok(status)) => {
            complete(tracker, resource, CleanupStatus::Completed);
            Ok(status)
        }
        Ok(Err(_)) => {
            complete(tracker, resource, CleanupStatus::Failed);
            Err(ScenarioFailure::new(
                FailureKind::EnvironmentUnavailable,
                "child-process-wait",
            ))
        }
        Err(_) => {
            let _ = child.start_kill();
            let cleanup = if child.wait().await.is_ok() {
                CleanupStatus::Completed
            } else {
                CleanupStatus::Failed
            };
            complete(tracker, resource, cleanup);
            Err(ScenarioFailure::new(
                FailureKind::DriverProtocol,
                "child-process-timeout",
            ))
        }
    }
}
