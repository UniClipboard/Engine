use std::fmt;

use thiserror::Error;

#[derive(Debug, Error)]
#[error("runtime lifecycle has been stopped")]
struct LifecycleStopped;

#[derive(Debug, Error)]
#[error("runtime lifecycle request has been superseded")]
struct LifecycleSuperseded;

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
