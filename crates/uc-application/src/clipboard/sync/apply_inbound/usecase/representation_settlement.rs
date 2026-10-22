//! 表示内容保存失败后的原尝试结算；不再次获取或保存内容。
use super::{ApplyInboundClipboardUseCase, ApplyInboundError};
use crate::transfer::receive::materialization::ReceiveFailureSettlementError;
use crate::transfer::receive::work::{ReceiveAction, UnsettledRepresentations};
use anyhow::Error as SourceError;
use uc_core::ids::EntryId;
use uc_core::ports::{AttemptState, PartialReceiveTerminal};

pub(super) enum RepresentationPersistenceFailure {
    Failed(SourceError),
    MissingEntry(SourceError),
}

impl ApplyInboundClipboardUseCase {
    pub(super) async fn fail_representation_receive(
        &self,
        action: &ReceiveAction,
        entry: &EntryId,
        attempt: &str,
        cancelled: bool,
        failure: RepresentationPersistenceFailure,
    ) -> ApplyInboundError {
        let (wrap, cause): (fn(SourceError) -> ApplyInboundError, SourceError) = match failure {
            RepresentationPersistenceFailure::Failed(cause) => (ApplyInboundError::Capture, cause),
            RepresentationPersistenceFailure::MissingEntry(cause) => {
                (ApplyInboundError::Internal, cause)
            }
        };
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
}
