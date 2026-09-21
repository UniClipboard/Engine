use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{FailureReport, ScenarioEvent, StageTiming};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupStatus {
    Completed,
    Pending,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceReport {
    pub kind: String,
    pub label: String,
    pub cleanup: CleanupStatus,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScenarioReport {
    pub schema_version: u32,
    pub scenario: String,
    pub artifact_id: String,
    pub seed: u64,
    pub outcome: &'static str,
    pub failure: Option<FailureReport>,
    pub last_event: Option<ScenarioEvent>,
    pub stages: Vec<StageTiming>,
    pub resources: Vec<ResourceReport>,
    pub cleanup: CleanupStatus,
    pub total_elapsed_ms: u64,
    pub reproduce: String,
}

#[derive(Debug)]
pub(crate) struct ArtifactPaths {
    pub directory: PathBuf,
    pub result_json: PathBuf,
    pub summary: PathBuf,
}

pub(crate) fn prepare_artifact_directory(root: &Path, artifact_id: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(root)?;
    let directory = root.join(artifact_id);
    fs::create_dir(&directory)?;
    Ok(directory)
}

pub(crate) fn write_report(directory: &Path, report: &ScenarioReport) -> io::Result<ArtifactPaths> {
    let result_json = directory.join("result.json");
    let summary = directory.join("summary.txt");
    let json = serde_json::to_vec_pretty(report).map_err(io::Error::other)?;
    write_new_file(&result_json, &json)?;
    write_new_file(&summary, human_summary(report).as_bytes())?;

    Ok(ArtifactPaths {
        directory: directory.to_path_buf(),
        result_json,
        summary,
    })
}

fn write_new_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid artifact name"))?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)
}

fn human_summary(report: &ScenarioReport) -> String {
    let mut summary = format!(
        "scenario: {}\noutcome: {}\nelapsed: {} ms\nartifact: {}\nreproduce: {}\n",
        report.scenario,
        report.outcome,
        report.total_elapsed_ms,
        report.artifact_id,
        report.reproduce,
    );
    if let Some(failure) = &report.failure {
        summary.push_str(&format!("failure: {}\n", failure.kind));
        if let Some(condition) = &failure.condition {
            summary.push_str(&format!("condition: {condition}\n"));
        }
    }
    if let Some(event) = &report.last_event {
        summary.push_str(&format!("last event: {}\n", event.kind));
    }
    for stage in &report.stages {
        summary.push_str(&format!("stage {}: {} ms\n", stage.name, stage.elapsed_ms));
    }
    summary
}
