use thiserror::Error;

#[derive(Debug, Error)]
#[error("runtime lifecycle has been stopped")]
struct LifecycleStopped;

/// 标准 source 指向首项，其余原因保存在同一报告内，不丢失其他失败。
#[derive(Debug, Error)]
#[error("runtime lifecycle transition incomplete")]
pub struct LifecycleError {
    #[source]
    pub primary: anyhow::Error,
    pub additional: Vec<anyhow::Error>,
}

impl LifecycleError {
    pub fn is_stopped(&self) -> bool {
        self.primary.is::<LifecycleStopped>()
    }

    pub(super) fn stopped() -> Self {
        Self {
            primary: LifecycleStopped.into(),
            additional: Vec::new(),
        }
    }

    pub(super) fn from_errors(mut errors: Vec<anyhow::Error>) -> Result<(), Self> {
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
