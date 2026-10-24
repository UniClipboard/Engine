use std::fmt;
use std::sync::Arc;

use anyhow::Error as SourceError;

use thiserror::Error;
use tokio::task::JoinError;
use uc_core::error_class::ErrorClass;
use uc_core::TaskShutdownReport;

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
    pub primary: SourceError,
    pub additional: Vec<SourceError>,
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
        } else if self.is_deadline_elapsed() {
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

    /// 全部失败都只是共同期限结束了等待，没有参与者报告实际失败。
    pub fn is_deadline_elapsed(&self) -> bool {
        is_deadline_source(&self.primary) && self.additional.iter().all(is_deadline_source)
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

    pub fn from_errors(mut errors: Vec<SourceError>) -> Result<(), Self> {
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

// 汇总和固定 context 不改变期限性质；任一真实参与者失败都使整个报告保持失败。
fn is_deadline_source(source: &SourceError) -> bool {
    source.is::<LifecycleDeadlineElapsed>()
        || source
            .downcast_ref::<TaskShutdownReport>()
            .is_some_and(|report| report.timed_out_count > 0 && report.join_error_count == 0)
        || source
            .downcast_ref::<LifecycleError>()
            .is_some_and(LifecycleError::is_deadline_elapsed)
        || source
            .downcast_ref::<Arc<LifecycleError>>()
            .is_some_and(|error| error.is_deadline_elapsed())
}

pub(super) fn sanitize_task_failure(source: JoinError) -> SourceError {
    if source.is_panic() {
        LifecycleTaskFailure::Panicked.into()
    } else {
        LifecycleTaskFailure::Cancelled.into()
    }
}

pub(super) fn deadline_elapsed() -> SourceError {
    LifecycleDeadlineElapsed.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error as IoError, ErrorKind};
    use std::time::Duration;
    use uc_core::TaskRegistry;

    #[test]
    fn nested_deadlines_keep_timeout_classification_without_hiding_real_failure() {
        let inner = LifecycleError {
            primary: super::deadline_elapsed(),
            additional: vec![super::deadline_elapsed()],
        };
        let nested = LifecycleError {
            primary: SourceError::new(inner).context("stop session work"),
            additional: vec![SourceError::new(Arc::new(LifecycleError {
                primary: super::deadline_elapsed(),
                additional: Vec::new(),
            }))
            .context("stop application work")],
        };
        assert!(nested.is_deadline_elapsed());
        assert_eq!(nested.class(), "deadline_elapsed");
        let mixed = LifecycleError {
            primary: SourceError::new(nested),
            additional: vec![IoError::from(ErrorKind::StorageFull).into()],
        };
        assert!(!mixed.is_deadline_elapsed());
        assert_eq!(mixed.class(), "incomplete");
        let wrapped = LifecycleError {
            primary: SourceError::new(mixed).context("stop runtime work"),
            additional: Vec::new(),
        };
        assert!(
            !wrapped.is_deadline_elapsed(),
            "嵌套报告中的实际失败不能被期限遮住"
        );
        let mixed = wrapped.primary.downcast_ref::<LifecycleError>().unwrap();
        assert_eq!(
            mixed.additional[0]
                .downcast_ref::<IoError>()
                .unwrap()
                .kind(),
            ErrorKind::StorageFull
        );
    }

    #[tokio::test]
    async fn additional_task_timeouts_are_deadlines_without_hiding_real_failures() {
        let tasks = TaskRegistry::new();
        assert!(tasks.spawn(|_| std::future::pending()).await);
        let report = tasks
            .shutdown(Duration::ZERO)
            .await
            .into_result()
            .unwrap_err();
        assert_eq!(report.timed_out_count, 1);
        assert_eq!(report.join_error_count, 0);
        let error = LifecycleError {
            primary: super::deadline_elapsed(),
            additional: vec![SourceError::new(report).context("stop process tasks")],
        };
        assert!(error.is_deadline_elapsed());
        assert_eq!(error.class(), "deadline_elapsed");
        let error = LifecycleError {
            primary: SourceError::new(error),
            additional: vec![IoError::from(ErrorKind::PermissionDenied).into()],
        };
        assert!(!error.is_deadline_elapsed());
        assert_eq!(error.class(), "incomplete");
    }

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
