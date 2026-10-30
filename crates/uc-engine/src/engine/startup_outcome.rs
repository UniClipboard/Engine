//! 启动尝试的私有结果；资源和关闭责任仍由启动负责人持有。
use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::EngineError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartupOutcomeSnapshot {
    NotStarted,
    InputDropped {
        startup_error: EngineError,
    },
    Constructing {
        startup_error: Option<EngineError>,
    },
    Cleaning {
        startup_error: Option<EngineError>,
    },
    Delivered,
    Released {
        startup_error: Option<EngineError>,
        operation_error: Option<EngineError>,
    },
    Failed {
        startup_error: Option<EngineError>,
        cleanup_error: EngineError,
        operation_error: Option<EngineError>,
        explicit_retry_supported: bool,
    },
}

#[derive(Default)]
pub(super) struct StartupOutcome {
    phase: Phase,
    primary: Option<EngineError>,
    operation_error: Option<EngineError>,
}

#[derive(Default)]
enum Phase {
    #[default]
    NotStarted,
    InputDropped(EngineError),
    Constructing,
    Cleaning,
    Delivered,
    Released(Option<EngineError>),
    Failed {
        error: EngineError,
        explicit_retry_supported: bool,
    },
}

impl StartupOutcome {
    pub(super) fn begin(&mut self) {
        if matches!(self.phase, Phase::NotStarted) {
            self.phase = Phase::Constructing;
        }
    }

    /// 只记录尚未交给 Engine 的输入；已接受的尝试保持原状态。
    pub(super) fn input_dropped(&mut self, error: EngineError) {
        if matches!(self.phase, Phase::NotStarted) {
            self.phase = Phase::InputDropped(error);
        }
    }

    pub(super) fn record_primary(&mut self, error: EngineError) {
        if matches!(self.phase, Phase::Constructing | Phase::Cleaning) {
            self.primary.get_or_insert(error);
        }
    }

    pub(super) fn record_operation_error(&mut self, error: EngineError) {
        if matches!(self.phase, Phase::Constructing | Phase::Cleaning) {
            self.operation_error.get_or_insert(error);
        }
    }

    pub(super) fn begin_cleanup(&mut self) {
        if matches!(self.phase, Phase::Constructing) {
            self.phase = Phase::Cleaning;
        }
    }

    /// 显式重试只从已结算的失败开始，并保留先前的关闭错误。
    pub(super) fn retry_cleanup(&mut self) -> bool {
        let Phase::Failed {
            error,
            explicit_retry_supported: true,
        } = &self.phase
        else {
            return false;
        };
        self.operation_error.get_or_insert_with(|| error.clone());
        self.phase = Phase::Cleaning;
        true
    }

    pub(super) fn released(&mut self, operation_error: Option<EngineError>) {
        if matches!(self.phase, Phase::Constructing | Phase::Cleaning) {
            self.phase = Phase::Released(self.operation_error.clone().or(operation_error));
        }
    }

    pub(super) fn failed(&mut self, error: EngineError, explicit_retry_supported: bool) {
        if matches!(self.phase, Phase::Constructing | Phase::Cleaning) {
            self.phase = Phase::Failed {
                error,
                explicit_retry_supported,
            };
        }
    }

    pub(super) fn delivered(&mut self) {
        if matches!(self.phase, Phase::Constructing) {
            self.phase = Phase::Delivered;
        }
    }

    pub(super) fn snapshot(&self) -> StartupOutcomeSnapshot {
        match &self.phase {
            Phase::NotStarted => StartupOutcomeSnapshot::NotStarted,
            Phase::InputDropped(error) => StartupOutcomeSnapshot::InputDropped {
                startup_error: error.clone(),
            },
            Phase::Constructing => StartupOutcomeSnapshot::Constructing {
                startup_error: self.primary.clone(),
            },
            Phase::Cleaning => StartupOutcomeSnapshot::Cleaning {
                startup_error: self.primary.clone(),
            },
            Phase::Delivered => StartupOutcomeSnapshot::Delivered,
            Phase::Released(operation_error) => StartupOutcomeSnapshot::Released {
                startup_error: self.primary.clone(),
                operation_error: operation_error.clone(),
            },
            Phase::Failed {
                error,
                explicit_retry_supported,
            } => StartupOutcomeSnapshot::Failed {
                startup_error: self.primary.clone(),
                cleanup_error: error.clone(),
                operation_error: self.operation_error.clone(),
                explicit_retry_supported: *explicit_retry_supported,
            },
        }
    }

    pub(super) fn can_complete(&self) -> bool {
        matches!(self.phase, Phase::Constructing | Phase::Cleaning)
    }
}

/// 为同一次启动保存结果，并让各等待者使用各自的期限。
#[derive(Default)]
pub(super) struct StartupOutcomeCell {
    state: Mutex<StartupOutcome>,
    changed: Condvar,
}

pub(super) struct StartupOutcomeGuard<'a> {
    state: MutexGuard<'a, StartupOutcome>,
    changed: &'a Condvar,
}

impl Deref for StartupOutcomeGuard<'_> {
    type Target = StartupOutcome;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for StartupOutcomeGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Drop for StartupOutcomeGuard<'_> {
    fn drop(&mut self) {
        self.changed.notify_all();
    }
}

impl StartupOutcomeCell {
    pub(super) fn lock(&self) -> StartupOutcomeGuard<'_> {
        StartupOutcomeGuard {
            state: self.state.lock().unwrap_or_else(PoisonError::into_inner),
            changed: &self.changed,
        }
    }

    pub(super) fn update(&self, change: impl FnOnce(&mut StartupOutcome)) {
        change(&mut self.lock());
    }

    /// 私有等待入口；完整启动与绑定范围验收前不向宿主开放。
    pub(super) fn wait(&self, deadline: Duration) -> StartupOutcomeSnapshot {
        let started = Instant::now();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let snapshot = state.snapshot();
            if !matches!(
                snapshot,
                StartupOutcomeSnapshot::Constructing { .. }
                    | StartupOutcomeSnapshot::Cleaning { .. }
            ) {
                return snapshot;
            }
            let Some(remaining) = deadline.checked_sub(started.elapsed()) else {
                return snapshot;
            };
            let (next, _) = self
                .changed
                .wait_timeout(state, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
        }
    }
}
