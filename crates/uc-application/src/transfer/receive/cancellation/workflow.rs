use super::{CancelEntryReceiveError, CancelEntryReceiveOutcome, CancelEntryReceiveUseCase};
use crate::facade::{ClipboardHostEvent, HostEvent};
use crate::transfer::blob::facade::TransferCancellationWait;
use anyhow::Error as SourceError;
use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ids::EntryId;
use uc_core::ports::{
    AttemptState, InboundReceiveSettlement, NoEntryReceiveArtifacts, PartialReceiveTerminal,
    RequestReceiveCancellationOutcome,
};

impl CancelEntryReceiveUseCase {
    pub(super) async fn run(
        &self,
        entry_id: &EntryId,
        expected_attempt_id: &str,
    ) -> Result<CancelEntryReceiveOutcome, CancelEntryReceiveError> {
        let Some(attempt) = self
            .get_attempt
            .get_entry_attempt(entry_id.as_ref())
            .await
            .map_err(|error| {
                CancelEntryReceiveError::Attempt(
                    SourceError::from(error).context("read receive attempt"),
                )
            })?
        else {
            return Ok(CancelEntryReceiveOutcome::NotReceiving);
        };

        if attempt.current_attempt_id != expected_attempt_id {
            return Ok(CancelEntryReceiveOutcome::Superseded);
        }

        match self
            .request_cancel
            .request_receive_cancellation(
                entry_id.as_ref(),
                expected_attempt_id,
                self.clock.now_ms(),
            )
            .await
            .map_err(|error| {
                CancelEntryReceiveError::Attempt(
                    SourceError::from(error).context("request receive cancellation"),
                )
            })? {
            RequestReceiveCancellationOutcome::Requested
            | RequestReceiveCancellationOutcome::AlreadyCancelling => {}
            RequestReceiveCancellationOutcome::TooLate => {
                return Ok(CancelEntryReceiveOutcome::TooLate)
            }
            RequestReceiveCancellationOutcome::Terminal => {
                return Ok(CancelEntryReceiveOutcome::AlreadyTerminal)
            }
            RequestReceiveCancellationOutcome::Superseded => {
                return Ok(CancelEntryReceiveOutcome::Superseded)
            }
        }

        self.host_event_bus.emit_or_warn(HostEvent::Clipboard(
            ClipboardHostEvent::ReceiveAttemptStateChanged {
                entry_id: entry_id.as_ref().to_owned(),
                attempt_id: expected_attempt_id.to_owned(),
                state: "cancelling".to_owned(),
            },
        ));

        let receive = self.receive_work.stop(entry_id, expected_attempt_id);

        if let Err(error) = self
            .transfer_cancellation
            .cancel_inbound_attempt(
                expected_attempt_id,
                FileTransferCancellationReason::LocalUser,
                TransferCancellationWait::Complete,
            )
            .await
        {
            return Err(CancelEntryReceiveError::Transfer(
                SourceError::from(error).context("cancel inbound attempt transfers"),
            ));
        }
        let mut settled_by_receive = false;
        if let Some(receive) = receive {
            receive
                .wait()
                .await
                .map_err(|source| CancelEntryReceiveError::Transfer(SourceError::new(source)))?;
        }
        // 原接收动作负责保留独立成功项及最终 CAS；不能再用 NoEntry 覆盖其结果。
        let current = self
            .get_attempt
            .get_entry_attempt(entry_id.as_ref())
            .await
            .map_err(|source| CancelEntryReceiveError::Attempt(SourceError::new(source)))?;
        let Some(current) = current else {
            return Ok(CancelEntryReceiveOutcome::NotReceiving);
        };
        if current.current_attempt_id != expected_attempt_id {
            return Ok(CancelEntryReceiveOutcome::Superseded);
        }
        match current.state {
            AttemptState::Cancelled => settled_by_receive = true,
            AttemptState::Completed | AttemptState::Committing => {
                return Ok(CancelEntryReceiveOutcome::TooLate)
            }
            AttemptState::Failed => return Ok(CancelEntryReceiveOutcome::AlreadyTerminal),
            AttemptState::Receiving | AttemptState::Cancelling | AttemptState::Failing => {}
        }
        if let Err(error) = self
            .cancel_projection
            .cancel_attempt_transfers(
                entry_id.as_ref(),
                expected_attempt_id,
                FileTransferCancellationReason::LocalUser,
                self.clock.now_ms(),
            )
            .await
        {
            return Err(CancelEntryReceiveError::Transfer(
                SourceError::from(error).context("cancel directory attempt transfers"),
            ));
        }

        if settled_by_receive {
            return Ok(CancelEntryReceiveOutcome::Cancelled);
        }

        let record = self
            .get_publish
            .get_publish_record(entry_id.as_ref(), expected_attempt_id)
            .await
            .map_err(CancelEntryReceiveError::PublishLog)?;
        if let Some(record) = record {
            let staged_roots = record
                .root_map
                .into_iter()
                .map(|(staged, _)| staged)
                .collect::<Vec<_>>();
            self.staging_cleanup
                .cleanup_staging_roots(&staged_roots)
                .await
                .map_err(CancelEntryReceiveError::Cleanup)?;
        }

        self.commit_inbound
            .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
                entry_id: entry_id.clone(),
                attempt_id: expected_attempt_id.to_owned(),
                terminal: PartialReceiveTerminal::Cancelled,
                artifacts: NoEntryReceiveArtifacts::None,
                now_ms: self.clock.now_ms(),
            })
            .await
            .map_err(|error| {
                CancelEntryReceiveError::Attempt(
                    SourceError::from(error).context("commit cancelled receive"),
                )
            })?;
        self.host_event_bus.emit_or_warn(HostEvent::Clipboard(
            ClipboardHostEvent::ReceiveAttemptStateChanged {
                entry_id: entry_id.as_ref().to_owned(),
                attempt_id: expected_attempt_id.to_owned(),
                state: "cancelled".to_owned(),
            },
        ));
        Ok(CancelEntryReceiveOutcome::Cancelled)
    }
}
