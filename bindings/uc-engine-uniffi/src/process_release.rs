//! 进程观测释放与恢复的完整动作及其代次观察。
use std::sync::Arc;
use std::time::Duration;

use uc_engine::observability::{
    ProcessAdmissionError, ProcessFailureKind, ProcessReleaseObservation, ProcessReleaseReport,
    ProcessReleaseResult, ProcessResumeObservation, ProcessResumeResult,
};

use crate::{observability, BindingError, BindingErrorCategory};

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BindingProcessReleaseState {
    Pending,
    Released,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BindingProcessFailure {
    DailyFile,
    NetworkFile,
    ReleaseWorker,
    Engine,
    TraceExport,
    LogExport,
    Unconfirmed,
    OtherOperation,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct BindingProcessReleaseReport {
    pub state: BindingProcessReleaseState,
    pub generation: u64,
    pub cleanup_errors: Vec<BindingProcessFailure>,
    pub operation_errors: Vec<BindingProcessFailure>,
    pub explicit_retry_supported: bool,
}

impl From<ProcessFailureKind> for BindingProcessFailure {
    fn from(value: ProcessFailureKind) -> Self {
        match value {
            ProcessFailureKind::DailyFile => Self::DailyFile,
            ProcessFailureKind::NetworkFile => Self::NetworkFile,
            ProcessFailureKind::ReleaseWorker => Self::ReleaseWorker,
            ProcessFailureKind::Engine => Self::Engine,
            ProcessFailureKind::TraceExport => Self::TraceExport,
            ProcessFailureKind::LogExport => Self::LogExport,
            ProcessFailureKind::Unconfirmed => Self::Unconfirmed,
            ProcessFailureKind::OtherOperation => Self::OtherOperation,
        }
    }
}

impl From<ProcessReleaseReport> for BindingProcessReleaseReport {
    fn from(value: ProcessReleaseReport) -> Self {
        let (state, generation) = match value.result {
            ProcessReleaseResult::Pending { generation } => {
                (BindingProcessReleaseState::Pending, generation)
            }
            ProcessReleaseResult::Released { generation, .. } => {
                (BindingProcessReleaseState::Released, generation)
            }
            ProcessReleaseResult::Failed { generation } => {
                (BindingProcessReleaseState::Failed, generation)
            }
        };
        Self {
            state,
            generation,
            cleanup_errors: value.cleanup_errors.into_iter().map(Into::into).collect(),
            operation_errors: value.operation_errors.into_iter().map(Into::into).collect(),
            explicit_retry_supported: value.explicit_retry_supported,
        }
    }
}

#[derive(uniffi::Object)]
pub struct BindingProcessReleaseObservation {
    inner: ProcessReleaseObservation,
}

#[uniffi::export]
impl BindingProcessReleaseObservation {
    pub fn wait(&self, deadline_ms: u64) -> BindingProcessReleaseReport {
        self.inner.wait(Duration::from_millis(deadline_ms)).into()
    }
}

#[derive(uniffi::Record)]
pub struct BindingProcessReleaseRequest {
    pub report: BindingProcessReleaseReport,
    pub observation: Option<Arc<BindingProcessReleaseObservation>>,
}

fn map_release(
    result: (ProcessReleaseReport, Option<ProcessReleaseObservation>),
) -> BindingProcessReleaseRequest {
    BindingProcessReleaseRequest {
        report: result.0.into(),
        observation: result
            .1
            .map(|inner| Arc::new(BindingProcessReleaseObservation { inner })),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BindingProcessResumeState {
    Pending,
    Resumed,
    Failed,
    Superseded,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct BindingProcessResumeReport {
    pub state: BindingProcessResumeState,
    pub generation: u64,
}

impl From<ProcessResumeResult> for BindingProcessResumeReport {
    fn from(value: ProcessResumeResult) -> Self {
        let (state, generation) = match value {
            ProcessResumeResult::Pending { generation } => {
                (BindingProcessResumeState::Pending, generation)
            }
            ProcessResumeResult::Resumed { generation } => {
                (BindingProcessResumeState::Resumed, generation)
            }
            ProcessResumeResult::Failed { generation } => {
                (BindingProcessResumeState::Failed, generation)
            }
            ProcessResumeResult::Superseded { generation } => {
                (BindingProcessResumeState::Superseded, generation)
            }
        };
        Self { state, generation }
    }
}

#[derive(uniffi::Object)]
pub struct BindingProcessResumeObservation {
    inner: ProcessResumeObservation,
}

#[uniffi::export]
impl BindingProcessResumeObservation {
    pub fn wait(&self, deadline_ms: u64) -> BindingProcessResumeReport {
        self.inner.wait(Duration::from_millis(deadline_ms)).into()
    }
}

#[derive(uniffi::Record)]
pub struct BindingProcessResumeRequest {
    pub report: BindingProcessResumeReport,
    pub observation: Option<Arc<BindingProcessResumeObservation>>,
}

fn map_admission_error(error: ProcessAdmissionError) -> BindingError {
    match error {
        ProcessAdmissionError::DeadlineExceeded => BindingError::Engine {
            code: 1002,
            category: BindingErrorCategory::DeadlineExceeded,
            retryable: true,
        },
    }
}

#[uniffi::export]
pub fn release_process_observability(
    deadline_ms: u64,
) -> Result<BindingProcessReleaseRequest, BindingError> {
    let handle = observability::process_handle().ok_or(BindingError::ObservabilityNotInstalled)?;
    Ok(map_release(
        handle
            .release_process(Duration::from_millis(deadline_ms))
            .map_err(map_admission_error)?,
    ))
}

#[uniffi::export]
pub fn retry_process_observability_release(
    deadline_ms: u64,
) -> Result<BindingProcessReleaseRequest, BindingError> {
    let handle = observability::process_handle().ok_or(BindingError::ObservabilityNotInstalled)?;
    Ok(map_release(
        handle
            .retry_process_release(Duration::from_millis(deadline_ms))
            .map_err(map_admission_error)?,
    ))
}

#[uniffi::export]
pub fn resume_process_observability(
    deadline_ms: u64,
) -> Result<BindingProcessResumeRequest, BindingError> {
    let handle = observability::process_handle().ok_or(BindingError::ObservabilityNotInstalled)?;
    let (report, observation) = handle
        .resume_process(Duration::from_millis(deadline_ms))
        .map_err(map_admission_error)?;
    Ok(BindingProcessResumeRequest {
        report: report.into(),
        observation: observation.map(|inner| Arc::new(BindingProcessResumeObservation { inner })),
    })
}
