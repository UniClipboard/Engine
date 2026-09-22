use std::{
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tokio::{process::Command, time::timeout};

use crate::{
    CleanupStatus, EventRecorder, FailureKind, ResourceReport, ScenarioBudget, ScenarioEvent,
    ScenarioFailure, ScenarioIdentity, ScenarioReport, StageGuard, StageTiming, TcpPortLease,
    TempDirLease,
    budget::{lock, millis},
    event::EventLog,
    process::run_child_process,
    report::{ArtifactPaths, prepare_artifact_directory, write_report},
    resource::{ResourceTracker, record_external},
};

pub struct ScenarioConfig {
    name: &'static str,
    seed: u64,
    budget: ScenarioBudget,
    reproduce: &'static str,
    artifact_root: PathBuf,
}

impl ScenarioConfig {
    pub fn new(
        name: &'static str,
        seed: u64,
        budget: ScenarioBudget,
        reproduce: &'static str,
        artifact_root: impl AsRef<Path>,
    ) -> Self {
        Self {
            name,
            seed,
            budget,
            reproduce,
            artifact_root: artifact_root.as_ref().to_path_buf(),
        }
    }
}

pub struct Scenario {
    identity: ScenarioIdentity,
    budget: ScenarioBudget,
    reproduce: &'static str,
    started_at: Instant,
    artifact_directory: PathBuf,
    events: Arc<EventLog>,
    stages: Arc<Mutex<Vec<StageTiming>>>,
    resources: ResourceTracker,
}

impl Scenario {
    pub fn start(config: ScenarioConfig) -> Result<Self, ScenarioFailure> {
        let identity = ScenarioIdentity::new(config.name, config.seed)?;
        let artifact_directory = prepare_artifact_directory(
            &config.artifact_root,
            identity.artifact_id(),
        )
        .map_err(|_| {
            ScenarioFailure::new(FailureKind::FrameworkArtifact, "artifact-directory-create")
        })?;

        Ok(Self {
            identity,
            budget: config.budget,
            reproduce: config.reproduce,
            started_at: Instant::now(),
            artifact_directory,
            events: EventLog::new(),
            stages: Arc::new(Mutex::new(Vec::new())),
            resources: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn event_recorder(&self) -> EventRecorder {
        EventRecorder::new(Arc::clone(&self.events))
    }

    pub fn record_event(&self, kind: &'static str) {
        self.event_recorder().record(kind);
    }

    pub fn stage(&self, name: &'static str) -> StageGuard {
        StageGuard::new(name, Arc::clone(&self.stages))
    }

    pub async fn wait_for_event(
        &self,
        condition: &'static str,
        predicate: impl Fn(&ScenarioEvent) -> bool,
    ) -> Result<ScenarioEvent, ScenarioFailure> {
        let mut revision = self.events.subscribe();
        loop {
            if let Some(event) = self.events.snapshot().into_iter().find(&predicate) {
                return Ok(event);
            }

            let remaining = self.budget.remaining(self.started_at);
            if remaining.is_zero() {
                return Err(timeout_failure(condition, &self.events));
            }

            match timeout(remaining, revision.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    return Err(ScenarioFailure::new(
                        FailureKind::DriverProtocol,
                        "event-recorder-closed",
                    )
                    .with_last_event(self.events.last()));
                }
                Err(_) => return Err(timeout_failure(condition, &self.events)),
            }
        }
    }

    pub fn temp_dir(&mut self, label: &'static str) -> Result<TempDirLease, ScenarioFailure> {
        TempDirLease::new(Arc::clone(&self.resources), label).map_err(|_| {
            ScenarioFailure::new(FailureKind::EnvironmentUnavailable, "temp-dir-create")
        })
    }

    pub fn tcp_port(&mut self, label: &'static str) -> Result<TcpPortLease, ScenarioFailure> {
        TcpPortLease::new(Arc::clone(&self.resources), label)
            .map_err(|_| ScenarioFailure::new(FailureKind::ResourceCollision, "loopback-port-bind"))
    }

    pub fn record_external_resource(
        &mut self,
        kind: &'static str,
        label: &'static str,
        cleanup: CleanupStatus,
    ) {
        record_external(&self.resources, kind, label, cleanup);
    }

    pub async fn run_child_process(
        &mut self,
        label: &'static str,
        command: &mut Command,
        wait_budget: Duration,
    ) -> Result<ExitStatus, ScenarioFailure> {
        let remaining = self.budget.remaining(self.started_at);
        run_child_process(&self.resources, label, command, wait_budget.min(remaining)).await
    }

    pub fn finish(
        self,
        result: Result<(), ScenarioFailure>,
    ) -> Result<ScenarioCompletion, ScenarioCompletionFailure> {
        let resources = lock(&self.resources).clone();
        let cleanup = cleanup_status(&resources);
        let mut failure = result.err();
        if cleanup != CleanupStatus::Completed {
            match &mut failure {
                Some(primary) => primary.add_secondary(FailureKind::CleanupFailed),
                None => {
                    failure = Some(ScenarioFailure::new(
                        FailureKind::CleanupFailed,
                        "resource-cleanup-incomplete",
                    ));
                }
            }
        }

        let report = ScenarioReport {
            schema_version: 2,
            scenario: self.identity.name().to_owned(),
            artifact_id: self.identity.artifact_id().to_owned(),
            artifact_directory: self
                .artifact_directory
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("artifact-directory-invalid")
                .to_owned(),
            seed: self.identity.seed(),
            outcome: if failure.is_some() {
                "failed"
            } else {
                "passed"
            },
            failure: failure.as_ref().map(ScenarioFailure::report),
            last_event: self.events.last(),
            stages: lock(&self.stages).clone(),
            resources,
            cleanup,
            total_elapsed_ms: millis(self.started_at.elapsed()),
            reproduce: self.reproduce.to_owned(),
        };

        let paths = match write_report(&self.artifact_directory, &report) {
            Ok(paths) => paths,
            Err(_) => {
                let artifact_failure =
                    ScenarioFailure::new(FailureKind::FrameworkArtifact, "artifact-write");
                let preserved_failure = match failure {
                    Some(mut primary) => {
                        primary.add_secondary(FailureKind::FrameworkArtifact);
                        primary
                    }
                    None => artifact_failure,
                };
                return Err(ScenarioCompletionFailure::without_artifacts(
                    preserved_failure,
                    report,
                    self.artifact_directory,
                ));
            }
        };
        let completion = ScenarioCompletion { report, paths };
        match failure {
            Some(failure) => Err(ScenarioCompletionFailure {
                failure,
                completion,
            }),
            None => Ok(completion),
        }
    }
}

fn timeout_failure(condition: &'static str, events: &EventLog) -> ScenarioFailure {
    ScenarioFailure::new(FailureKind::DriverProtocol, condition).with_last_event(events.last())
}

fn cleanup_status(resources: &[ResourceReport]) -> CleanupStatus {
    if resources
        .iter()
        .any(|resource| resource.cleanup == CleanupStatus::Failed)
    {
        CleanupStatus::Failed
    } else if resources
        .iter()
        .any(|resource| resource.cleanup == CleanupStatus::Pending)
    {
        CleanupStatus::Pending
    } else {
        CleanupStatus::Completed
    }
}

#[derive(Debug)]
pub struct ScenarioCompletion {
    report: ScenarioReport,
    paths: ArtifactPaths,
}

impl ScenarioCompletion {
    pub fn report(&self) -> &ScenarioReport {
        &self.report
    }

    pub fn artifact_dir(&self) -> &Path {
        &self.paths.directory
    }

    pub fn result_json(&self) -> &Path {
        &self.paths.result_json
    }

    pub fn summary(&self) -> &Path {
        &self.paths.summary
    }
}

pub struct ScenarioCompletionFailure {
    failure: ScenarioFailure,
    completion: ScenarioCompletion,
}

impl ScenarioCompletionFailure {
    fn without_artifacts(
        failure: ScenarioFailure,
        report: ScenarioReport,
        directory: PathBuf,
    ) -> Self {
        Self {
            failure,
            completion: ScenarioCompletion {
                report,
                paths: ArtifactPaths {
                    result_json: directory.join("result.json"),
                    summary: directory.join("summary.txt"),
                    directory,
                },
            },
        }
    }

    pub fn failure(&self) -> &ScenarioFailure {
        &self.failure
    }

    pub fn report(&self) -> &ScenarioReport {
        self.completion.report()
    }

    pub fn artifact_dir(&self) -> &Path {
        self.completion.artifact_dir()
    }

    pub fn result_json(&self) -> &Path {
        self.completion.result_json()
    }

    pub fn summary(&self) -> &Path {
        self.completion.summary()
    }
}

impl std::fmt::Debug for ScenarioCompletionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScenarioCompletionFailure")
            .field("failure", &self.failure)
            .field("artifact_id", &self.completion.report.artifact_id)
            .finish()
    }
}

impl std::fmt::Display for ScenarioCompletionFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.failure.fmt(formatter)
    }
}

impl std::error::Error for ScenarioCompletionFailure {}
