#[derive(Debug, thiserror::Error)]
pub enum DecideDeviceTrustChangeError {
    #[error("space is locked")]
    Locked,
    #[error("device trust recovery is required")]
    RecoveryRequired {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("device trust decision is unavailable")]
    Unavailable {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("device trust state changed")]
    StateChanged,
    #[error("device trust decision was committed but follow-up is pending")]
    CommittedButPending {
        #[source]
        source: Option<anyhow::Error>,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl DecideDeviceTrustChangeError {
    pub fn unavailable() -> Self {
        Self::Unavailable { source: None }
    }

    pub fn unavailable_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Unavailable {
            source: Some(source.into()),
        }
    }

    pub fn committed_but_pending() -> Self {
        Self::CommittedButPending { source: None }
    }

    pub fn committed_but_pending_from(source: impl Into<anyhow::Error>) -> Self {
        Self::CommittedButPending {
            source: Some(source.into()),
        }
    }

    pub fn recovery_required() -> Self {
        Self::RecoveryRequired { source: None }
    }

    pub fn recovery_required_from(source: impl Into<anyhow::Error>) -> Self {
        Self::RecoveryRequired {
            source: Some(source.into()),
        }
    }
}
