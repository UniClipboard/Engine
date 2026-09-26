#[derive(Debug, thiserror::Error)]
pub enum HandleMembershipHistoryMessageError {
    #[error("space pairing is still in progress")]
    PairingInProgress,
    #[error("space is locked")]
    Locked,
    #[error("membership history recovery is required")]
    RecoveryRequired {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("membership history message is rejected")]
    Rejected {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("membership history handling is unavailable")]
    Unavailable,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl HandleMembershipHistoryMessageError {
    pub fn recovery_required() -> Self {
        Self::RecoveryRequired { source: None }
    }

    pub fn recovery_required_from(source: impl Into<anyhow::Error>) -> Self {
        Self::RecoveryRequired {
            source: Some(source.into()),
        }
    }

    pub fn rejected() -> Self {
        Self::Rejected { source: None }
    }

    pub fn rejected_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Rejected {
            source: Some(source.into()),
        }
    }
}
