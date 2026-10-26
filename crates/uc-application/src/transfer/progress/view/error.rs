use crate::deps::CurrentSpaceMemberScopeError;
use crate::transfer::progress::ProgressQueryResult;
use std::error::Error;
use std::fmt;
use uc_core::error_class::ErrorClass;

#[derive(Debug, thiserror::Error)]
pub enum TransferProgressQueryError {
    #[error("transfer progress input is invalid")]
    InvalidInput(#[source] Box<dyn Error + Send + Sync>),
    #[error("transfer progress access is denied")]
    AccessDenied(#[source] Box<dyn Error + Send + Sync>),
    #[error("transfer progress data is invalid")]
    InvalidData(#[source] Box<dyn Error + Send + Sync>),
    #[error("transfer progress is unavailable")]
    Unavailable(#[source] Box<dyn Error + Send + Sync>),
}
impl ErrorClass for TransferProgressQueryError {
    fn class(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "progress_query_invalid_input",
            Self::AccessDenied(_) => "progress_query_access_denied",
            Self::Unavailable(_) => "progress_query_unavailable",
            Self::InvalidData(_) => "progress_query_invalid_data",
        }
    }
}
impl From<CurrentSpaceMemberScopeError> for TransferProgressQueryError {
    fn from(source: CurrentSpaceMemberScopeError) -> Self {
        match source {
            CurrentSpaceMemberScopeError::NoCurrentSpace | CurrentSpaceMemberScopeError::Locked => {
                Self::AccessDenied(Box::new(source))
            }
            CurrentSpaceMemberScopeError::RecoveryRequired { .. }
            | CurrentSpaceMemberScopeError::Unavailable => Self::Unavailable(Box::new(source)),
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("transfer progress refused: peer is outside current usable membership")]
pub(super) struct NotCurrentMember;

/// 同一不可变查询结果保存完整失败来源，不克隆或字符串化下层错误。
pub(super) struct SharedQueryFailure(pub ProgressQueryResult);
impl fmt::Display for SharedQueryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("transfer progress reconciliation failed")
    }
}
impl fmt::Debug for SharedQueryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}
impl Error for SharedQueryFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0
            .as_ref()
            .as_ref()
            .err()
            .map(|source| source as &dyn Error)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("transfer progress refused: remote task scope is not authorized")]
pub(super) struct RemoteScopeDenied;
