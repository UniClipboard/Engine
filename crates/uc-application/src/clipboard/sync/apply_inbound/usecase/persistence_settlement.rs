//! 表示内容保存失败后的原尝试结算；不再次获取或保存内容。
use super::{ApplyInboundClipboardUseCase, ApplyInboundError};
use crate::transfer::receive::materialization::ReceiveFailureSettlementError;
use crate::transfer::receive::work::{ReceiveAction, UnsettledDirectory, UnsettledRepresentations};
use anyhow::Error as SourceError;
use uc_core::ids::EntryId;
use uc_core::ports::{AttemptState, PartialReceiveTerminal};

pub(super) enum ReceivePersistenceFailure {
    Failed(SourceError),
    MissingEntry(SourceError),
}

impl ReceivePersistenceFailure {
    fn into_parts(self) -> (fn(SourceError) -> ApplyInboundError, SourceError) {
        match self {
            Self::Failed(cause) => (ApplyInboundError::Capture, cause),
            Self::MissingEntry(cause) => (ApplyInboundError::Internal, cause),
        }
    }
}

impl ApplyInboundClipboardUseCase {
    pub(super) async fn fail_representation_receive(
        &self,
        action: &ReceiveAction,
        entry: &EntryId,
        attempt: &str,
        cancelled: bool,
        failure: ReceivePersistenceFailure,
    ) -> ApplyInboundError {
        let (wrap, cause) = failure.into_parts();
        match self
            .repair_failure_settlement(entry, attempt, cancelled, &[], false)
            .await
        {
            Ok(terminal) => {
                action.batch.settled(match terminal {
                    PartialReceiveTerminal::Failed => AttemptState::Failed,
                    PartialReceiveTerminal::Cancelled => AttemptState::Cancelled,
                });
                wrap(cause)
            }
            Err(settlement) => {
                action.defer_representation_settlement(UnsettledRepresentations {
                    entry: entry.clone(),
                    attempt: attempt.to_owned(),
                    cancelled,
                });
                wrap(SourceError::new(ReceiveFailureSettlementError {
                    cause,
                    settlement: SourceError::new(settlement),
                }))
            }
        }
    }
    pub(super) async fn fail_directory_receive(
        &self,
        action: &ReceiveAction,
        mut pending: UnsettledDirectory,
        failure: ReceivePersistenceFailure,
    ) -> ApplyInboundError {
        let (wrap, cause) = failure.into_parts();
        match self.repair_directory_settlement(&mut pending).await {
            Ok(terminal) => {
                action.batch.settled(match terminal {
                    PartialReceiveTerminal::Failed => AttemptState::Failed,
                    PartialReceiveTerminal::Cancelled => AttemptState::Cancelled,
                });
                wrap(cause)
            }
            Err(settlement) => {
                action.defer_directory_settlement(pending);
                wrap(SourceError::new(ReceiveFailureSettlementError {
                    cause,
                    settlement: SourceError::new(settlement),
                }))
            }
        }
    }

    pub(super) async fn repair_directory_settlement(
        &self,
        pending: &mut UnsettledDirectory,
    ) -> Result<PartialReceiveTerminal, ApplyInboundError> {
        if let Some(attempt) = pending.attempt.as_deref() {
            self.claim_failure_terminal(&pending.entry, attempt, pending.cancelled)
                .await?;
        }
        pending
            .publication
            .rollback()
            .await
            .map_err(|source| ApplyInboundError::Internal(SourceError::new(source)))?;
        if let Some(attempt) = pending.attempt.as_deref() {
            self.repair_failure_settlement(&pending.entry, attempt, pending.cancelled, &[], false)
                .await
        } else {
            Ok(if pending.cancelled {
                PartialReceiveTerminal::Cancelled
            } else {
                PartialReceiveTerminal::Failed
            })
        }
    }
}
