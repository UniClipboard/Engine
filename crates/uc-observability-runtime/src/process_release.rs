//! 进程释放请求的结果与内部等待状态。

use std::collections::HashMap;
#[cfg(feature = "test-process-release")]
use std::error::Error;
use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::engine_activity::{fence_engines, RetainedEngineError};
use crate::local_file::lock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessReleaseResult {
    Pending {
        generation: u64,
    },
    Released {
        generation: u64,
        operation_failed: bool,
    },
    Failed {
        generation: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessResumeResult {
    Pending { generation: u64 },
    Resumed { generation: u64 },
    Failed { generation: u64 },
    Superseded { generation: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessFailureKind {
    DailyFile,
    NetworkFile,
    ReleaseWorker,
    Engine,
    TraceExport,
    LogExport,
    Unconfirmed,
    OtherOperation,
}

#[derive(Debug)]
pub struct ProcessReleaseReport {
    pub result: ProcessReleaseResult,
    pub cleanup_errors: Vec<ProcessFailureKind>,
    pub operation_errors: Vec<ProcessFailureKind>,
    pub explicit_retry_supported: bool,
}

#[derive(Clone, Default)]
pub(crate) struct ProcessReleaseErrors {
    pub(crate) local: Option<Arc<io::Error>>,
    pub(crate) network: Option<Arc<io::Error>>,
    pub(crate) coordinator: Option<Arc<io::Error>>,
    pub(crate) traces: Option<Arc<opentelemetry_sdk::error::OTelSdkError>>,
    pub(crate) logs: Option<Arc<opentelemetry_sdk::error::OTelSdkError>>,
    pub(crate) engines: Vec<RetainedEngineError>,
}

impl ProcessReleaseErrors {
    fn safe_categories(&self) -> Vec<ProcessFailureKind> {
        let mut categories = Vec::new();
        if self.local.is_some() {
            categories.push(ProcessFailureKind::DailyFile);
        }
        if self.network.is_some() {
            categories.push(ProcessFailureKind::NetworkFile);
        }
        if self.coordinator.is_some() {
            categories.push(ProcessFailureKind::ReleaseWorker);
        }
        if !self.engines.is_empty() {
            categories.push(ProcessFailureKind::Engine);
        }
        if self.traces.is_some() {
            categories.push(ProcessFailureKind::TraceExport);
        }
        if self.logs.is_some() {
            categories.push(ProcessFailureKind::LogExport);
        }
        categories
    }
    fn with_prior(self, previous: &Self) -> Self {
        let mut engines = previous.engines.clone();
        for error in self.engines {
            if !engines.iter().any(|prior| prior.same_owner(&error)) {
                engines.push(error);
            }
        }
        Self {
            local: previous.local.clone().or(self.local),
            network: previous.network.clone().or(self.network),
            coordinator: previous.coordinator.clone().or(self.coordinator),
            traces: previous.traces.clone().or(self.traces),
            logs: previous.logs.clone().or(self.logs),
            engines,
        }
    }
}

/// 同一次释放的结果；持有者需使用代次判断它是否仍有当前授权。
pub(crate) struct ReleaseObservation {
    generation: u64,
    result: Mutex<Option<ProcessReleaseResult>>,
    cleanup_errors: Mutex<ProcessReleaseErrors>,
    operation_errors: Mutex<ProcessReleaseErrors>,
    changed: Condvar,
}

impl ReleaseObservation {
    pub(crate) fn safe_report(
        &self,
        result: ProcessReleaseResult,
        retry_supported: bool,
    ) -> ProcessReleaseReport {
        let mut cleanup_errors = if matches!(result, ProcessReleaseResult::Failed { .. }) {
            lock(&self.cleanup_errors).safe_categories()
        } else {
            Vec::new()
        };
        if matches!(result, ProcessReleaseResult::Failed { .. }) && cleanup_errors.is_empty() {
            cleanup_errors.push(ProcessFailureKind::Unconfirmed);
        }
        let mut operation_errors = if matches!(result, ProcessReleaseResult::Pending { .. }) {
            Vec::new()
        } else {
            lock(&self.operation_errors).safe_categories()
        };
        if matches!(
            result,
            ProcessReleaseResult::Released {
                operation_failed: true,
                ..
            }
        ) && operation_errors.is_empty()
        {
            operation_errors.push(ProcessFailureKind::OtherOperation);
        }
        ProcessReleaseReport {
            result,
            cleanup_errors,
            operation_errors,
            explicit_retry_supported: matches!(result, ProcessReleaseResult::Failed { .. })
                && retry_supported,
        }
    }
    fn new(generation: u64) -> Arc<Self> {
        Arc::new(Self {
            generation,
            result: Mutex::new(None),
            cleanup_errors: Mutex::new(ProcessReleaseErrors::default()),
            operation_errors: Mutex::new(ProcessReleaseErrors::default()),
            changed: Condvar::new(),
        })
    }

    fn complete(&self, result: ProcessReleaseResult, cleanup_errors: ProcessReleaseErrors) {
        let mut current = lock(&self.result);
        if current.is_none() {
            *lock(&self.cleanup_errors) = cleanup_errors;
            *current = Some(result);
            self.changed.notify_all();
        }
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_local_error(&self) -> Option<Arc<io::Error>> {
        lock(&self.cleanup_errors).local.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_coordinator_error(&self) -> Option<Arc<io::Error>> {
        lock(&self.cleanup_errors).coordinator.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_operation_error(&self) -> Option<Arc<io::Error>> {
        lock(&self.operation_errors).local.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_network_error(&self) -> Option<Arc<io::Error>> {
        lock(&self.cleanup_errors).network.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_network_operation_error(&self) -> Option<Arc<io::Error>> {
        lock(&self.operation_errors).network.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_trace_operation_error(
        &self,
    ) -> Option<Arc<opentelemetry_sdk::error::OTelSdkError>> {
        lock(&self.operation_errors).traces.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_log_operation_error(
        &self,
    ) -> Option<Arc<opentelemetry_sdk::error::OTelSdkError>> {
        lock(&self.operation_errors).logs.clone()
    }

    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_engine_errors(
        &self,
    ) -> (
        Vec<Arc<dyn Error + Send + Sync>>,
        Vec<Arc<dyn Error + Send + Sync>>,
    ) {
        (
            lock(&self.cleanup_errors)
                .engines
                .iter()
                .map(|error| Arc::clone(&error.source))
                .collect(),
            lock(&self.operation_errors)
                .engines
                .iter()
                .map(|error| Arc::clone(&error.source))
                .collect(),
        )
    }

    pub(crate) fn wait(&self, deadline: Duration) -> ProcessReleaseResult {
        let started = Instant::now();
        let mut result = lock(&self.result);
        loop {
            if let Some(result) = *result {
                return result;
            }
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                return ProcessReleaseResult::Pending {
                    generation: self.generation,
                };
            };
            let (next, _) = self
                .changed
                .wait_timeout(result, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            result = next;
        }
    }
}

pub(crate) struct ResumeObservation {
    generation: u64,
    result: Mutex<Option<ProcessResumeResult>>,
    changed: Condvar,
}

impl ResumeObservation {
    fn new(generation: u64) -> Arc<Self> {
        Arc::new(Self {
            generation,
            result: Mutex::new(None),
            changed: Condvar::new(),
        })
    }

    fn complete(&self, result: ProcessResumeResult) {
        let mut current = lock(&self.result);
        if current.is_none() {
            *current = Some(result);
            self.changed.notify_all();
        }
    }

    pub(crate) fn wait(&self, deadline: Duration) -> ProcessResumeResult {
        let started = Instant::now();
        let mut result = lock(&self.result);
        loop {
            if let Some(result) = *result {
                return result;
            }
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                return ProcessResumeResult::Pending {
                    generation: self.generation,
                };
            };
            let (next, _) = self
                .changed
                .wait_timeout(result, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            result = next;
        }
    }
}

pub(crate) struct ProcessReleaseCoordinator {
    state: Mutex<Phase>,
    releases: Mutex<HashMap<u64, Arc<ReleaseObservation>>>,
    changed: Condvar,
}

enum Phase {
    Active {
        generation: u64,
    },
    Releasing {
        generation: u64,
        fence: Arc<()>,
        prior_operation_failed: bool,
    },
    ReleaseQueued {
        resume_generation: u64,
        resume: Arc<ResumeObservation>,
        generation: u64,
        fence: Arc<()>,
        spawn_failed: bool,
        prior_operation_failed: bool,
    },
    ReleaseAfterQueuedResume {
        old_release_generation: u64,
        resume_generation: u64,
        resume: Arc<ResumeObservation>,
        generation: u64,
        fence: Arc<()>,
        resume_spawn_failed: bool,
        release_spawn_failed: bool,
        prior_operation_failed: bool,
    },
    ResumeQueued {
        release_generation: u64,
        generation: u64,
        resume: Arc<ResumeObservation>,
        fence: Arc<()>,
        spawn_failed: bool,
        prior_operation_failed: bool,
    },
    Released {
        generation: u64,
        fence: Arc<()>,
        operation_failed: bool,
        operation_errors: ProcessReleaseErrors,
    },
    Resuming {
        generation: u64,
        resume: Arc<ResumeObservation>,
        fence: Arc<()>,
    },
    Failed {
        generation: u64,
        fence: Arc<()>,
        resume_failed: bool,
        retryable_files: bool,
        cleanup_errors: ProcessReleaseErrors,
        operation_errors: ProcessReleaseErrors,
    },
}

pub(crate) enum ReleaseStart {
    Started {
        generation: u64,
        fence: Arc<()>,
        observation: Arc<ReleaseObservation>,
    },
    Existing {
        observation: Option<Arc<ReleaseObservation>>,
    },
}

pub(crate) enum ResumeStart {
    Started {
        generation: u64,
        fence: Arc<()>,
        wait_release: bool,
        observation: Arc<ResumeObservation>,
    },
    Existing {
        observation: Option<Arc<ResumeObservation>>,
    },
}

impl Default for ProcessReleaseCoordinator {
    fn default() -> Self {
        Self {
            state: Mutex::new(Phase::Active { generation: 0 }),
            releases: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        }
    }
}

impl ProcessReleaseCoordinator {
    pub(crate) fn current_generation(&self) -> u64 {
        match &*lock(&self.state) {
            Phase::Active { generation }
            | Phase::Releasing { generation, .. }
            | Phase::ReleaseQueued { generation, .. }
            | Phase::ReleaseAfterQueuedResume { generation, .. }
            | Phase::ResumeQueued { generation, .. }
            | Phase::Released { generation, .. }
            | Phase::Resuming { generation, .. }
            | Phase::Failed { generation, .. } => *generation,
        }
    }
    pub(crate) fn retry_is_supported(&self, generation: u64, retry_engines: bool) -> bool {
        generation.checked_add(1).is_some()
            && matches!(&*lock(&self.state), Phase::Failed { generation: current, resume_failed, retryable_files, .. } if *current == generation && (*resume_failed || *retryable_files || retry_engines))
    }
    pub(crate) fn record_operation_errors(&self, generation: u64, errors: ProcessReleaseErrors) {
        if let Some(observation) = lock(&self.releases).get(&generation) {
            let mut previous = lock(&observation.operation_errors);
            *previous = errors.with_prior(&previous);
        }
    }
    /// 仅供端到端工件注入无法实际耗尽的代次。
    #[cfg(feature = "test-process-release")]
    pub(crate) fn probe_active_generation(generation: u64) -> Self {
        Self {
            state: Mutex::new(Phase::Active { generation }),
            releases: Mutex::new(HashMap::new()),
            changed: Condvar::new(),
        }
    }

    pub(crate) fn invalidate_for_engine_retention(&self, pending: bool) -> Option<(u64, Arc<()>)> {
        let mut state = lock(&self.state);
        let Phase::Released {
            generation, fence, ..
        } = &*state
        else {
            return None;
        };
        let generation = generation.saturating_add(1);
        let fence = Arc::clone(fence);
        let restart = if pending {
            self.new_release(generation);
            *state = Phase::Releasing {
                generation,
                fence: Arc::clone(&fence),
                prior_operation_failed: false,
            };
            Some((generation, fence))
        } else {
            *state = Phase::Failed {
                generation,
                fence,
                resume_failed: false,
                retryable_files: false,
                cleanup_errors: ProcessReleaseErrors::default(),
                operation_errors: ProcessReleaseErrors::default(),
            };
            None
        };
        self.changed.notify_all();
        restart
    }

    pub(crate) fn release_is_current(&self, generation: u64) -> bool {
        let state = lock(&self.state);
        matches!(&*state, Phase::Releasing { generation: current, .. } if *current == generation)
            || matches!(&*state, Phase::ResumeQueued { release_generation, .. } if *release_generation == generation)
            || matches!(&*state, Phase::ReleaseAfterQueuedResume { old_release_generation, .. } if *old_release_generation == generation)
    }

    fn new_release(&self, generation: u64) -> Arc<ReleaseObservation> {
        let observation = ReleaseObservation::new(generation);
        lock(&self.releases).insert(generation, Arc::clone(&observation));
        observation
    }

    fn complete_release(
        &self,
        generation: u64,
        result: ProcessReleaseResult,
        cleanup_errors: ProcessReleaseErrors,
    ) {
        if let Some(observation) = lock(&self.releases).remove(&generation) {
            observation.complete(result, cleanup_errors);
        }
    }

    fn pending_cleanup_errors(&self, generation: u64) -> ProcessReleaseErrors {
        lock(&self.releases)
            .get(&generation)
            .map(|observation| lock(&observation.cleanup_errors).clone())
            .unwrap_or_default()
    }

    fn pending_operation_errors(&self, generation: u64) -> ProcessReleaseErrors {
        lock(&self.releases)
            .get(&generation)
            .map(|observation| lock(&observation.operation_errors).clone())
            .unwrap_or_default()
    }

    fn release_observation(&self, state: &Phase) -> Option<Arc<ReleaseObservation>> {
        match state {
            Phase::Releasing { generation, .. }
            | Phase::ReleaseQueued { generation, .. }
            | Phase::ReleaseAfterQueuedResume { generation, .. } => {
                lock(&self.releases).get(generation).cloned()
            }
            Phase::Released {
                generation,
                operation_failed,
                operation_errors,
                ..
            } => {
                let observation = ReleaseObservation::new(*generation);
                *lock(&observation.operation_errors) = operation_errors.clone();
                observation.complete(
                    ProcessReleaseResult::Released {
                        generation: *generation,
                        operation_failed: *operation_failed,
                    },
                    ProcessReleaseErrors::default(),
                );
                Some(observation)
            }
            Phase::Failed {
                generation,
                cleanup_errors,
                operation_errors,
                ..
            } => {
                let observation = ReleaseObservation::new(*generation);
                *lock(&observation.operation_errors) = operation_errors.clone();
                observation.complete(
                    ProcessReleaseResult::Failed {
                        generation: *generation,
                    },
                    cleanup_errors.clone(),
                );
                Some(observation)
            }
            _ => None,
        }
    }

    fn existing_release(&self, state: &Phase) -> ReleaseStart {
        ReleaseStart::Existing {
            observation: self.release_observation(state),
        }
    }

    pub(crate) fn current_release_observation(&self) -> Option<Arc<ReleaseObservation>> {
        let state = lock(&self.state);
        match self.existing_release(&state) {
            ReleaseStart::Existing { observation } => observation,
            ReleaseStart::Started { .. } => None,
        }
    }

    pub(crate) fn ongoing_release_observation(&self) -> Option<Arc<ReleaseObservation>> {
        let state = lock(&self.state);
        if matches!(
            &*state,
            Phase::Releasing { .. }
                | Phase::ReleaseQueued { .. }
                | Phase::ReleaseAfterQueuedResume { .. }
        ) {
            match self.existing_release(&state) {
                ReleaseStart::Existing { observation } => observation,
                ReleaseStart::Started { .. } => None,
            }
        } else {
            None
        }
    }

    pub(crate) fn resume_is_queued(&self) -> bool {
        matches!(&*lock(&self.state), Phase::ResumeQueued { .. })
    }

    pub(crate) fn terminal_resume_result(&self, deadline: Duration) -> ProcessResumeResult {
        let started = Instant::now();
        let mut state = lock(&self.state);
        loop {
            match &*state {
                Phase::ResumeQueued { generation, .. }
                | Phase::Resuming { generation, .. }
                | Phase::ReleaseQueued { generation, .. }
                | Phase::ReleaseAfterQueuedResume { generation, .. } => {
                    let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                        return ProcessResumeResult::Pending {
                            generation: *generation,
                        };
                    };
                    let (next, _) = self
                        .changed
                        .wait_timeout(state, remaining.min(Duration::from_secs(60)))
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state = next;
                }
                Phase::Active { generation }
                | Phase::Releasing { generation, .. }
                | Phase::Released { generation, .. }
                | Phase::Failed { generation, .. } => {
                    return ProcessResumeResult::Failed {
                        generation: *generation,
                    };
                }
            }
        }
    }

    pub(crate) fn begin_release(&self) -> ReleaseStart {
        let mut state = lock(&self.state);
        self.begin_release_locked(&mut state)
    }

    pub(crate) fn begin_release_during_queued_resume(&self) -> Option<ReleaseStart> {
        let mut state = lock(&self.state);
        if matches!(&*state, Phase::ResumeQueued { .. }) {
            Some(self.begin_release_locked(&mut state))
        } else {
            None
        }
    }

    fn begin_release_locked(&self, state: &mut Phase) -> ReleaseStart {
        match &*state {
            Phase::Active { generation } => {
                let Some(generation) = generation.checked_add(1) else {
                    return self.existing_release(&state);
                };
                let fence = fence_engines();
                *state = Phase::Releasing {
                    generation,
                    fence: Arc::clone(&fence),
                    prior_operation_failed: false,
                };
                ReleaseStart::Started {
                    generation,
                    fence,
                    observation: self.new_release(generation),
                }
            }
            Phase::Resuming {
                generation,
                fence,
                resume,
            } => {
                let resume_generation = *generation;
                let Some(generation) = generation.checked_add(1) else {
                    return self.existing_release(&state);
                };
                let fence = Arc::clone(fence);
                let resume = Arc::clone(resume);
                *state = Phase::ReleaseQueued {
                    resume_generation,
                    resume,
                    generation,
                    fence: Arc::clone(&fence),
                    spawn_failed: false,
                    prior_operation_failed: false,
                };
                self.changed.notify_all();
                ReleaseStart::Started {
                    generation,
                    fence,
                    observation: self.new_release(generation),
                }
            }
            Phase::ResumeQueued {
                release_generation,
                generation,
                fence,
                spawn_failed,
                prior_operation_failed,
                resume,
            } => {
                let old_release_generation = *release_generation;
                let resume_generation = *generation;
                let Some(generation) = generation.checked_add(1) else {
                    return self.existing_release(&state);
                };
                let fence = Arc::clone(fence);
                let resume = Arc::clone(resume);
                *state = Phase::ReleaseAfterQueuedResume {
                    old_release_generation,
                    resume_generation,
                    resume,
                    generation,
                    fence: Arc::clone(&fence),
                    resume_spawn_failed: *spawn_failed,
                    release_spawn_failed: false,
                    prior_operation_failed: *prior_operation_failed,
                };
                self.changed.notify_all();
                ReleaseStart::Started {
                    generation,
                    fence,
                    observation: self.new_release(generation),
                }
            }
            _ => self.existing_release(&state),
        }
    }

    pub(crate) fn begin_retry_with_engines(&self, retry_engines: bool) -> ReleaseStart {
        let mut state = lock(&self.state);
        let Phase::Failed {
            generation,
            fence,
            resume_failed,
            retryable_files,
            cleanup_errors,
            operation_errors,
            ..
        } = &*state
        else {
            return self.existing_release(&state);
        };
        if !*resume_failed && !retry_engines && !*retryable_files {
            return self.existing_release(&state);
        }
        let Some(generation) = generation.checked_add(1) else {
            return self.existing_release(&state);
        };
        let fence = Arc::clone(fence);
        let prior_error = cleanup_errors.clone().with_prior(operation_errors);
        *state = Phase::Releasing {
            generation,
            fence: Arc::clone(&fence),
            prior_operation_failed: true,
        };
        let observation = self.new_release(generation);
        *lock(&observation.operation_errors) = prior_error;
        ReleaseStart::Started {
            generation,
            fence,
            observation,
        }
    }

    pub(crate) fn finish_release(
        &self,
        generation: u64,
        success: bool,
        operation_failed: bool,
        retryable_files: bool,
        cleanup_errors: ProcessReleaseErrors,
    ) {
        // 最末代次只表示无法继续分配代次，不能成为可复用的释放凭据。
        let success = success && generation != u64::MAX;
        let mut state = lock(&self.state);
        let accepted = matches!(&*state,
            Phase::Releasing { generation: current, .. } if *current == generation)
            || matches!(&*state, Phase::ResumeQueued { release_generation, .. } if *release_generation == generation)
            || matches!(&*state, Phase::ReleaseAfterQueuedResume { old_release_generation, .. } if *old_release_generation == generation);
        let operation_errors = lock(&self.releases)
            .get(&generation)
            .map(|observation| lock(&observation.operation_errors).clone())
            .unwrap_or_default();
        if accepted {
            let result = if success {
                ProcessReleaseResult::Released {
                    generation,
                    operation_failed,
                }
            } else {
                ProcessReleaseResult::Failed { generation }
            };
            self.complete_release(generation, result, cleanup_errors.clone());
        }
        match &*state {
            Phase::Releasing {
                generation: current,
                fence,
                ..
            } if *current == generation => {
                let fence = Arc::clone(fence);
                *state = if success {
                    Phase::Released {
                        generation,
                        fence,
                        operation_failed,
                        operation_errors: operation_errors.clone(),
                    }
                } else {
                    Phase::Failed {
                        generation,
                        fence,
                        resume_failed: false,
                        retryable_files,
                        cleanup_errors: cleanup_errors.clone(),
                        operation_errors: operation_errors.clone(),
                    }
                };
                self.changed.notify_all();
            }
            Phase::ResumeQueued {
                release_generation,
                generation: next,
                fence,
                spawn_failed,
                resume,
                ..
            } if *release_generation == generation => {
                let next = *next;
                let fence = Arc::clone(fence);
                let resume = Arc::clone(resume);
                *state = if success && !spawn_failed {
                    Phase::Resuming {
                        generation: next,
                        fence,
                        resume,
                    }
                } else {
                    resume.complete(ProcessResumeResult::Failed { generation: next });
                    Phase::Failed {
                        generation: next,
                        fence,
                        resume_failed: false,
                        retryable_files: !success && retryable_files,
                        cleanup_errors: cleanup_errors.clone(),
                        operation_errors: operation_errors.clone(),
                    }
                };
                self.changed.notify_all();
            }
            Phase::ReleaseAfterQueuedResume {
                old_release_generation,
                resume_generation,
                resume,
                generation: next,
                fence,
                resume_spawn_failed,
                release_spawn_failed,
                ..
            } if *old_release_generation == generation => {
                let next = *next;
                let resume_generation = *resume_generation;
                let fence = Arc::clone(fence);
                let resume = Arc::clone(resume);
                let queued_cleanup_errors = self.pending_cleanup_errors(next);
                if let Some(observation) = lock(&self.releases).get(&next) {
                    *lock(&observation.operation_errors) = operation_errors.clone();
                }
                *state = if !success {
                    let queued_cleanup_errors =
                        cleanup_errors.clone().with_prior(&queued_cleanup_errors);
                    resume.complete(ProcessResumeResult::Failed {
                        generation: resume_generation,
                    });
                    self.complete_release(
                        next,
                        ProcessReleaseResult::Failed { generation: next },
                        queued_cleanup_errors.clone(),
                    );
                    Phase::Failed {
                        generation: next,
                        fence,
                        resume_failed: false,
                        retryable_files,
                        cleanup_errors: queued_cleanup_errors,
                        operation_errors: operation_errors.clone(),
                    }
                } else if *resume_spawn_failed && *release_spawn_failed {
                    resume.complete(ProcessResumeResult::Failed {
                        generation: resume_generation,
                    });
                    self.complete_release(
                        next,
                        ProcessReleaseResult::Failed { generation: next },
                        queued_cleanup_errors.clone(),
                    );
                    Phase::Failed {
                        generation: next,
                        fence,
                        resume_failed: false,
                        retryable_files: false,
                        cleanup_errors: queued_cleanup_errors,
                        operation_errors: operation_errors.clone(),
                    }
                } else if *resume_spawn_failed {
                    resume.complete(ProcessResumeResult::Failed {
                        generation: resume_generation,
                    });
                    Phase::Releasing {
                        generation: next,
                        fence,
                        prior_operation_failed: true,
                    }
                } else {
                    Phase::ReleaseQueued {
                        resume_generation,
                        resume,
                        generation: next,
                        fence,
                        spawn_failed: *release_spawn_failed,
                        prior_operation_failed: operation_failed,
                    }
                };
                self.changed.notify_all();
            }
            _ => {}
        }
    }

    pub(crate) fn wait_release(&self, deadline: Duration) -> ProcessReleaseResult {
        self.wait_release_observed(deadline).0
    }

    pub(crate) fn wait_release_observed(
        &self,
        deadline: Duration,
    ) -> (ProcessReleaseResult, Option<Arc<ReleaseObservation>>) {
        let started = Instant::now();
        let mut state = lock(&self.state);
        loop {
            match &*state {
                Phase::Released {
                    generation,
                    operation_failed,
                    ..
                } => {
                    let result = ProcessReleaseResult::Released {
                        generation: *generation,
                        operation_failed: *operation_failed,
                    };
                    return (result, self.release_observation(&state));
                }
                Phase::Failed { generation, .. } => {
                    let result = ProcessReleaseResult::Failed {
                        generation: *generation,
                    };
                    return (result, self.release_observation(&state));
                }
                Phase::Active { generation }
                | Phase::Releasing { generation, .. }
                | Phase::ReleaseQueued { generation, .. }
                | Phase::ReleaseAfterQueuedResume { generation, .. }
                | Phase::ResumeQueued { generation, .. }
                | Phase::Resuming { generation, .. } => {
                    let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                        return (
                            ProcessReleaseResult::Pending {
                                generation: *generation,
                            },
                            None,
                        );
                    };
                    let (next, _) = self
                        .changed
                        .wait_timeout(state, remaining.min(Duration::from_secs(60)))
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state = next;
                }
            }
        }
    }

    pub(crate) fn begin_resume(&self) -> ResumeStart {
        let mut state = lock(&self.state);
        self.begin_resume_locked(&mut state)
    }

    pub(crate) fn begin_resume_during_release(&self) -> Option<ResumeStart> {
        let mut state = lock(&self.state);
        if matches!(
            &*state,
            Phase::Releasing { .. }
                | Phase::ResumeQueued { .. }
                | Phase::ReleaseAfterQueuedResume { .. }
                | Phase::ReleaseQueued { .. }
                | Phase::Resuming { .. }
        ) {
            Some(self.begin_resume_locked(&mut state))
        } else {
            None
        }
    }

    fn begin_resume_locked(&self, state: &mut Phase) -> ResumeStart {
        match &*state {
            Phase::Released {
                generation, fence, ..
            } => {
                let Some(generation) = generation.checked_add(1).filter(|next| *next < u64::MAX)
                else {
                    let observation = ResumeObservation::new(u64::MAX);
                    observation.complete(ProcessResumeResult::Failed {
                        generation: u64::MAX,
                    });
                    return ResumeStart::Existing {
                        observation: Some(observation),
                    };
                };
                let fence = Arc::clone(fence);
                let observation = ResumeObservation::new(generation);
                *state = Phase::Resuming {
                    generation,
                    fence: Arc::clone(&fence),
                    resume: Arc::clone(&observation),
                };
                ResumeStart::Started {
                    generation,
                    fence,
                    wait_release: false,
                    observation,
                }
            }
            Phase::Releasing {
                generation,
                fence,
                prior_operation_failed,
            } => {
                let release_generation = *generation;
                let prior_operation_failed = *prior_operation_failed;
                let Some(generation) = generation.checked_add(1).filter(|next| *next < u64::MAX)
                else {
                    let observation = ResumeObservation::new(u64::MAX);
                    observation.complete(ProcessResumeResult::Failed {
                        generation: u64::MAX,
                    });
                    return ResumeStart::Existing {
                        observation: Some(observation),
                    };
                };
                let fence = Arc::clone(fence);
                let observation = ResumeObservation::new(generation);
                *state = Phase::ResumeQueued {
                    release_generation,
                    generation,
                    resume: Arc::clone(&observation),
                    fence: Arc::clone(&fence),
                    spawn_failed: false,
                    prior_operation_failed,
                };
                self.changed.notify_all();
                ResumeStart::Started {
                    generation,
                    fence,
                    wait_release: true,
                    observation,
                }
            }
            Phase::Resuming { resume, .. }
            | Phase::ResumeQueued { resume, .. }
            | Phase::ReleaseQueued { resume, .. }
            | Phase::ReleaseAfterQueuedResume { resume, .. } => ResumeStart::Existing {
                observation: Some(Arc::clone(resume)),
            },
            _ => ResumeStart::Existing { observation: None },
        }
    }

    pub(crate) fn wait_resume_ready(&self, generation: u64) -> bool {
        let mut state = lock(&self.state);
        loop {
            match &*state {
                Phase::Resuming {
                    generation: current,
                    ..
                } if *current == generation => return true,
                Phase::ReleaseQueued {
                    resume_generation, ..
                } if *resume_generation == generation => return true,
                Phase::ResumeQueued {
                    generation: current,
                    ..
                } if *current == generation => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                Phase::ReleaseAfterQueuedResume {
                    resume_generation, ..
                } if *resume_generation == generation => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                _ => return false,
            }
        }
    }

    pub(crate) fn wait_resume(&self, deadline: Duration) -> ProcessResumeResult {
        let started = Instant::now();
        let mut state = lock(&self.state);
        loop {
            match &*state {
                Phase::Active { generation } => {
                    return ProcessResumeResult::Resumed {
                        generation: *generation,
                    }
                }
                Phase::Failed { generation, .. } => {
                    return ProcessResumeResult::Failed {
                        generation: *generation,
                    }
                }
                Phase::Releasing { generation, .. }
                | Phase::ReleaseQueued { generation, .. }
                | Phase::ReleaseAfterQueuedResume { generation, .. }
                | Phase::ResumeQueued { generation, .. }
                | Phase::Resuming { generation, .. }
                | Phase::Released { generation, .. } => {
                    let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                        return ProcessResumeResult::Pending {
                            generation: *generation,
                        };
                    };
                    let (next, _) = self
                        .changed
                        .wait_timeout(state, remaining.min(Duration::from_secs(60)))
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state = next;
                }
            }
        }
    }

    pub(crate) fn finish_resume(&self, generation: u64, success: bool) {
        let mut state = lock(&self.state);
        if let Phase::ReleaseQueued {
            resume_generation,
            generation: next,
            fence,
            spawn_failed,
            prior_operation_failed,
            resume,
        } = &*state
        {
            if *resume_generation == generation {
                let next = *next;
                let fence = Arc::clone(fence);
                let queued_cleanup_errors = self.pending_cleanup_errors(next);
                let queued_operation_errors = self.pending_operation_errors(next);
                resume.complete(if success {
                    ProcessResumeResult::Superseded { generation }
                } else {
                    ProcessResumeResult::Failed { generation }
                });
                *state = if *spawn_failed {
                    self.complete_release(
                        next,
                        ProcessReleaseResult::Failed { generation: next },
                        queued_cleanup_errors.clone(),
                    );
                    Phase::Failed {
                        generation: next,
                        fence,
                        resume_failed: false,
                        retryable_files: false,
                        cleanup_errors: queued_cleanup_errors,
                        operation_errors: queued_operation_errors,
                    }
                } else {
                    Phase::Releasing {
                        generation: next,
                        fence,
                        prior_operation_failed: *prior_operation_failed || !success,
                    }
                };
                self.changed.notify_all();
            }
            return;
        }
        if let Phase::ReleaseAfterQueuedResume {
            resume_generation,
            resume_spawn_failed,
            ..
        } = &mut *state
        {
            if *resume_generation == generation && !success {
                *resume_spawn_failed = true;
                self.changed.notify_all();
            }
            return;
        }
        if let Phase::ResumeQueued {
            generation: current,
            spawn_failed,
            ..
        } = &mut *state
        {
            if *current == generation && !success {
                *spawn_failed = true;
                self.changed.notify_all();
            }
            return;
        }
        if let Phase::Resuming {
            generation: current,
            fence,
            resume,
        } = &*state
        {
            if *current == generation {
                resume.complete(if success {
                    ProcessResumeResult::Resumed { generation }
                } else {
                    ProcessResumeResult::Failed { generation }
                });
                *state = if success {
                    Phase::Active { generation }
                } else {
                    Phase::Failed {
                        generation,
                        fence: Arc::clone(fence),
                        resume_failed: true,
                        retryable_files: false,
                        cleanup_errors: ProcessReleaseErrors::default(),
                        operation_errors: ProcessReleaseErrors::default(),
                    }
                };
                self.changed.notify_all();
            }
        }
    }

    pub(crate) fn resume_is_current(&self, generation: u64) -> bool {
        matches!(&*lock(&self.state), Phase::Resuming { generation: current, .. } if *current == generation)
    }

    pub(crate) fn wait_release_ready(&self, generation: u64) -> Option<bool> {
        let mut state = lock(&self.state);
        loop {
            match &*state {
                Phase::Releasing {
                    generation: current,
                    prior_operation_failed,
                    ..
                } if *current == generation => return Some(*prior_operation_failed),
                Phase::ResumeQueued {
                    release_generation,
                    prior_operation_failed,
                    ..
                } if *release_generation == generation => return Some(*prior_operation_failed),
                Phase::ReleaseAfterQueuedResume {
                    old_release_generation,
                    prior_operation_failed,
                    ..
                } if *old_release_generation == generation => return Some(*prior_operation_failed),
                Phase::ReleaseQueued {
                    generation: current,
                    ..
                } if *current == generation => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                Phase::ReleaseAfterQueuedResume {
                    generation: current,
                    ..
                } if *current == generation => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                _ => return None,
            }
        }
    }

    pub(crate) fn fail_release_spawn(&self, generation: u64, error: Arc<io::Error>) {
        let mut state = lock(&self.state);
        if let Some(observation) = lock(&self.releases).get(&generation) {
            lock(&observation.cleanup_errors).coordinator = Some(Arc::clone(&error));
        }
        if let Phase::ReleaseAfterQueuedResume {
            generation: current,
            release_spawn_failed,
            ..
        } = &mut *state
        {
            if *current == generation {
                *release_spawn_failed = true;
                return;
            }
        }
        if let Phase::ReleaseQueued {
            generation: current,
            spawn_failed,
            ..
        } = &mut *state
        {
            if *current == generation {
                *spawn_failed = true;
            }
            return;
        }
        drop(state);
        self.finish_release(
            generation,
            false,
            false,
            false,
            ProcessReleaseErrors {
                coordinator: Some(error),
                ..ProcessReleaseErrors::default()
            },
        );
    }
}
