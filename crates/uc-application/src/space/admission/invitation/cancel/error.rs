#[derive(Debug, thiserror::Error)]
pub enum CancelInvitationError {
    #[error("no in-flight invitation to cancel")]
    NotIssued,

    #[error("internal error: {0}")]
    Internal(String),
}
