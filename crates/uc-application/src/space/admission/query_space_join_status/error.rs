use crate::deps::AdmissionAttemptRepositoryError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuerySpaceJoinStatusErrorKind {
    Locked,
    Corrupt,
    Failed,
}

#[derive(Debug, thiserror::Error)]
#[error("failed to query Space join status: {detail}")]
pub struct QuerySpaceJoinStatusError {
    kind: QuerySpaceJoinStatusErrorKind,
    detail: String,
}

impl QuerySpaceJoinStatusError {
    pub(crate) fn invalid_state(detail: impl Into<String>) -> Self {
        Self {
            kind: QuerySpaceJoinStatusErrorKind::Failed,
            detail: detail.into(),
        }
    }

    pub(crate) fn repository(error: AdmissionAttemptRepositoryError) -> Self {
        let kind = match &error {
            AdmissionAttemptRepositoryError::Locked => QuerySpaceJoinStatusErrorKind::Locked,
            AdmissionAttemptRepositoryError::Corrupt => QuerySpaceJoinStatusErrorKind::Corrupt,
            _ => QuerySpaceJoinStatusErrorKind::Failed,
        };
        Self {
            kind,
            detail: error.to_string(),
        }
    }

    pub(crate) fn is_locked(&self) -> bool {
        self.kind == QuerySpaceJoinStatusErrorKind::Locked
    }

    pub(crate) fn is_corrupt(&self) -> bool {
        self.kind == QuerySpaceJoinStatusErrorKind::Corrupt
    }
}
