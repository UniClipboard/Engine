use std::fmt;

use thiserror::Error;
use tokio::task::JoinError;
use uc_core::error_class::ErrorClass;

#[derive(Debug, Error)]
#[error("runtime lifecycle has been stopped")]
struct LifecycleStopped;

#[derive(Debug, Error)]
#[error("runtime lifecycle request has been superseded")]
struct LifecycleSuperseded;

#[derive(Debug, Error, PartialEq, Eq)]
pub(super) enum LifecycleTaskFailure {
    #[error("runtime lifecycle task panicked")]
    Panicked,
    #[error("runtime lifecycle task was cancelled")]
    Cancelled,
}

#[derive(Debug, Error)]
#[error("runtime lifecycle deadline elapsed")]
pub(super) struct LifecycleDeadlineElapsed;

/// 标准 source 指向首项，其余原因保存在同一报告内，不丢失其他失败。
#[derive(Error)]
#[error("runtime lifecycle transition incomplete")]
pub struct LifecycleError {
    #[source]
    pub primary: anyhow::Error,
    pub additional: Vec<anyhow::Error>,
}

impl fmt::Debug for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LifecycleError")
            .field("failure_count", &(1 + self.additional.len()))
            .finish()
    }
}

impl ErrorClass for LifecycleError {
    fn class(&self) -> &'static str {
        if self.primary.is::<LifecycleStopped>() {
            "stopped"
        } else if self.primary.is::<LifecycleSuperseded>() {
            "superseded"
        } else if self.primary.is::<LifecycleDeadlineElapsed>() {
            "deadline_elapsed"
        } else {
            match self.primary.downcast_ref::<LifecycleTaskFailure>() {
                Some(LifecycleTaskFailure::Panicked) => "task_panicked",
                Some(LifecycleTaskFailure::Cancelled) => "task_cancelled",
                None => "incomplete",
            }
        }
    }
}

impl LifecycleError {
    pub fn is_stopped(&self) -> bool {
        self.primary.is::<LifecycleStopped>()
    }

    pub fn is_superseded(&self) -> bool {
        self.primary.is::<LifecycleSuperseded>()
    }

    pub(super) fn superseded() -> Self {
        Self {
            primary: LifecycleSuperseded.into(),
            additional: Vec::new(),
        }
    }

    pub(super) fn stopped() -> Self {
        Self {
            primary: LifecycleStopped.into(),
            additional: Vec::new(),
        }
    }

    pub(super) fn from_task_failure(source: JoinError) -> Self {
        Self {
            primary: sanitize_task_failure(source),
            additional: Vec::new(),
        }
    }

    pub fn from_errors(mut errors: Vec<anyhow::Error>) -> Result<(), Self> {
        if errors.is_empty() {
            return Ok(());
        }
        let primary = errors.remove(0);
        Err(Self {
            primary,
            additional: errors,
        })
    }
}

pub(super) fn sanitize_task_failure(source: JoinError) -> anyhow::Error {
    if source.is_panic() {
        LifecycleTaskFailure::Panicked.into()
    } else {
        LifecycleTaskFailure::Cancelled.into()
    }
}

pub(super) fn deadline_elapsed() -> anyhow::Error {
    LifecycleDeadlineElapsed.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_errors_have_a_fixed_class_per_primary_cause() {
        assert_eq!(LifecycleError::stopped().class(), "stopped");
        assert_eq!(LifecycleError::superseded().class(), "superseded");
        let other = LifecycleError {
            primary: anyhow::anyhow!("PRIVATE_DETAIL"),
            additional: Vec::new(),
        };
        assert_eq!(other.class(), "incomplete");
    }
}
