//! 本地准备或保存失败后的原尝试结算；不再次获取或保存内容。
use super::{ApplyInboundClipboardUseCase, ApplyInboundError};
use crate::facade::host_event::{HostEvent, TransferHostEvent};
use crate::transfer::receive::materialization::ReceiveFailureSettlementError;
use crate::transfer::receive::work::{ReceiveAction, UnsettledDirectory, UnsettledRepresentations};
use anyhow::Error as SourceError;
use uc_core::ids::EntryId;
use uc_core::ports::{AttemptState, PartialReceiveTerminal};

pub(super) enum ReceiveFailure {
    Capture(SourceError),
    Internal(SourceError),
}

impl ReceiveFailure {
    fn into_parts(self) -> (fn(SourceError) -> ApplyInboundError, SourceError) {
        match self {
            Self::Capture(cause) => (ApplyInboundError::Capture, cause),
            Self::Internal(cause) => (ApplyInboundError::Internal, cause),
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
        failure: ReceiveFailure,
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
        failure: ReceiveFailure,
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
        let terminal = if let Some(attempt) = pending.attempt.as_deref() {
            self.repair_failure_settlement(&pending.entry, attempt, pending.cancelled, &[], false)
                .await?
        } else if pending.cancelled {
            PartialReceiveTerminal::Cancelled
        } else {
            PartialReceiveTerminal::Failed
        };
        if pending.notify_failure {
            let cancelled = terminal == PartialReceiveTerminal::Cancelled;
            self.emit_host_event(HostEvent::Transfer(TransferHostEvent::StatusChanged {
                transfer_id: pending.entry.as_ref().to_owned(),
                entry_id: Some(pending.entry.as_ref().to_owned()),
                attempt_id: pending.attempt.clone(),
                status: if cancelled { "cancelled" } else { "failed" }.to_owned(),
                // 原错误仍由调用结果保留；公共通知不复制底层错误正文或路径。
                reason: cancelled.then(|| "local_user".to_owned()),
            }));
        }
        Ok(terminal)
    }
}
