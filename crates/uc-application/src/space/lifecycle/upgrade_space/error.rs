use thiserror::Error;

use crate::space::lifecycle::RebuildSpaceError;

use uc_core::ports::EngineVersionStateError;

#[derive(Debug, Error)]
pub(crate) enum UpgradeSpaceError {
    #[error("failed to read the previous Engine version")]
    ReadVersion(#[source] EngineVersionStateError),

    #[error("stored Engine version is invalid")]
    InvalidVersion(#[source] semver::Error),

    #[error("failed to read Space setup state during Engine upgrade")]
    ReadSetupState(#[source] anyhow::Error),

    #[error("failed to rebuild space during Engine upgrade")]
    Rebuild(#[source] RebuildSpaceError),

    #[error("Engine upgrade completed but recording its version failed")]
    RecordVersion(#[source] EngineVersionStateError),
}

impl UpgradeSpaceError {
    /// 升级中的 Space 重建暂时不可用，已持久的进度可在下一次尝试中续做。
    pub(crate) fn is_temporarily_unavailable(&self) -> bool {
        matches!(self, Self::Rebuild(error) if error.is_temporarily_unavailable())
    }
}
