use crate::transfer::blob::transport::BlobExchangeError;
use anyhow::Error as SourceError;
use std::sync::Arc;
use uc_core::error_class::ErrorClass;
use uc_core::ports::{DirectoryStagingCleanupError, PublishLogError};

#[derive(Debug, thiserror::Error)]
pub enum CancelEntryReceiveError {
    #[error("entry receive cancellation is unavailable")]
    Unavailable,
    #[error("receive cancellation capacity exhausted")]
    Busy(#[source] BlobExchangeError),
    #[error("receive cancellation supervision failed")]
    Supervision(#[source] BlobExchangeError),
    #[error("receive cancellation failed")]
    Shared(#[source] Arc<CancelEntryReceiveError>),
    #[error("receive attempt state is unavailable")]
    Attempt(#[source] SourceError),
    #[error("receive publication metadata is unavailable")]
    PublishLog(#[source] PublishLogError),
    #[error("receive transfer cancellation failed")]
    Transfer(#[source] SourceError),
    #[error("receive staging cleanup failed")]
    Cleanup(#[source] DirectoryStagingCleanupError),
}

impl ErrorClass for CancelEntryReceiveError {
    fn class(&self) -> &'static str {
        match self {
            Self::Unavailable => "receive_cancel_unavailable",
            Self::Busy(_) => "receive_cancel_busy",
            Self::Supervision(_) => "receive_cancel_supervision_failed",
            Self::Shared(source) => source.class(),
            Self::Attempt(_) => "receive_cancel_authority_failed",
            Self::PublishLog(_) => "receive_cancel_publication_read_failed",
            Self::Transfer(_) => "receive_cancel_stop_failed",
            Self::Cleanup(_) => "receive_cancel_cleanup_failed",
        }
    }
}
