use crate::space::membership::MembershipLedgerError;

#[derive(Debug, thiserror::Error)]
pub enum QueryDeviceTrustError {
    #[error("space is locked")]
    Locked,
    #[error("device trust recovery is required")]
    RecoveryRequired {
        #[source]
        source: Option<anyhow::Error>,
    },
    #[error("device trust state is unavailable")]
    Unavailable,
    #[error("device trust dependency failed")]
    Dependency {
        #[source]
        source: anyhow::Error,
    },
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl QueryDeviceTrustError {
    pub fn recovery_required() -> Self {
        Self::RecoveryRequired { source: None }
    }

    pub fn recovery_required_from(source: impl Into<anyhow::Error>) -> Self {
        Self::RecoveryRequired {
            source: Some(source.into()),
        }
    }
}

impl From<MembershipLedgerError> for QueryDeviceTrustError {
    fn from(error: MembershipLedgerError) -> Self {
        match error {
            MembershipLedgerError::Locked => Self::Locked,
            error @ (MembershipLedgerError::Corrupt { .. }
            | MembershipLedgerError::RecoveryRequired) => Self::recovery_required_from(error),
            MembershipLedgerError::Conflict | MembershipLedgerError::Unavailable { .. } => {
                Self::Unavailable
            }
        }
    }
}
