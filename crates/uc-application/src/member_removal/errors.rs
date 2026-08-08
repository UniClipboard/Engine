//! 成员移除协调器的错误类型。

use uc_core::membership::{
    CurrentMemberSignatureError, RemovalExchangeError, RemovalIntentRejection,
    RemovalIntentRepositoryError, RemovalIntentVerificationError, RemovalRecoveryError,
};

#[derive(Debug, thiserror::Error)]
pub enum RemovalCoordinatorError {
    #[error("member removal space lineage mismatch")]
    SpaceMismatch,
    #[error("local device is not a current member of the space")]
    NotAMember,
    #[error("local device has observed its own removal and cannot create new intents")]
    OwnInstanceRemoved,
    #[error("cannot remove the local member instance")]
    SelfTarget,
    #[error("target device is not a member of the current causal view")]
    UnknownTarget,
    #[error("removal intent content is invalid: {0}")]
    InvalidIntent(#[from] RemovalIntentRejection),
    #[error("removal intent verification failed: {0}")]
    Verification(#[from] RemovalIntentVerificationError),
    #[error("removal intent is valid but its causal history is unavailable locally")]
    MissingCausalHistory,
    #[error("removal intent repository failed: {0}")]
    Repository(#[from] RemovalIntentRepositoryError),
    #[error("removal recovery failed: {0}")]
    Recovery(#[from] RemovalRecoveryError),
    #[error("removal exchange failed: {0}")]
    Exchange(#[from] RemovalExchangeError),
    #[error("current member signature failed: {0}")]
    Signature(#[from] CurrentMemberSignatureError),
    #[error("member repository failed: {0}")]
    Membership(String),
    #[error("no executor can be determined for the current convergence state")]
    NoExecutor,
}

impl From<uc_core::membership::MembershipError> for RemovalCoordinatorError {
    fn from(error: uc_core::membership::MembershipError) -> Self {
        Self::Membership(error.to_string())
    }
}

impl From<uc_core::membership::MembershipCandidateRepositoryError> for RemovalCoordinatorError {
    fn from(error: uc_core::membership::MembershipCandidateRepositoryError) -> Self {
        Self::Membership(error.to_string())
    }
}

impl From<uc_core::membership::MembershipAnnouncementRepositoryError> for RemovalCoordinatorError {
    fn from(error: uc_core::membership::MembershipAnnouncementRepositoryError) -> Self {
        Self::Membership(error.to_string())
    }
}
