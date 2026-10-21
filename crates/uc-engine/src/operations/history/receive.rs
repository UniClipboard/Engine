//! Shared inbound receive progress and cancellation operations.

use crate::error_codes::*;

use uc_application::facade::{
    AppFacade, BlobTransferError, CancelEntryReceiveError,
    CancelEntryReceiveOutcome as AppEntryReceiveCancellationOutcome,
    InboundCancelOutcome as AppInboundTransferCancellationOutcome, RetryEntryReceiveError,
    RetryEntryReceiveOutcome as AppReceiveRetryOutcome,
};

use crate::{
    CancelEntryReceiveInput, CancelInboundTransferInput, EngineError, EngineErrorCategory,
    EntryReceiveCancellationOutcome, EntryReceiveProgressInput, EntryReceiveRetryOutcome,
    InboundTransferCancellationOutcome, OperationResult, ReceiveItemFailureReason, ReceiveItemKind,
    ReceiveItemSummary, ReceiveProgressSummary, ReceiveRetryUnavailableReason,
    RetryEntryReceiveInput, TransferCancellationReason,
};
use uc_core::ids::EntryId;
use uc_core::ports::{
    ReceiveItemFailureReason as CoreReceiveItemFailureReason,
    ReceiveItemKind as CoreReceiveItemKind,
};
use uc_observability_contract::uc_error;

pub async fn execute_query_entry_receive_progress(
    facade: &AppFacade,
    input: EntryReceiveProgressInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.entry_id)?;
    let progress = facade
        .get_entry_receive_progress(&uc_core::ids::EntryId::from_string(input.entry_id))
        .await
        .map_err(map_receive_error)?
        .map(receive_progress);
    Ok(OperationResult::EntryReceiveProgress(progress))
}

pub async fn execute_list_entry_receive_progress(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let progress = facade
        .list_entry_receive_progress()
        .await
        .map_err(map_receive_error)?
        .into_iter()
        .map(receive_progress)
        .collect();
    Ok(OperationResult::EntryReceiveProgressList(progress))
}

pub async fn execute_retry_entry_receive(
    facade: &AppFacade,
    input: RetryEntryReceiveInput,
) -> Result<OperationResult, EngineError> {
    if input.item_ids.is_empty() {
        return Err(map_retry_error(RetryEntryReceiveError::InvalidInput));
    }
    validate_id(&input.entry_id)?;
    validate_id(&input.expected_attempt_id)?;
    for item in &input.item_ids {
        validate_id(item)?;
    }
    let outcome = facade
        .retry_entry_receive(
            EntryId::from_string(input.entry_id),
            input.expected_attempt_id,
            input.item_ids,
        )
        .await
        .map_err(map_retry_error)?;
    let outcome = match outcome {
        AppReceiveRetryOutcome::Accepted { attempt_id } => {
            EntryReceiveRetryOutcome::Accepted { attempt_id }
        }
        AppReceiveRetryOutcome::AlreadyReceiving => EntryReceiveRetryOutcome::AlreadyReceiving,
        AppReceiveRetryOutcome::Superseded => EntryReceiveRetryOutcome::Superseded,
        AppReceiveRetryOutcome::CancellationIncomplete => {
            EntryReceiveRetryOutcome::CancellationIncomplete
        }
        AppReceiveRetryOutcome::Unavailable => EntryReceiveRetryOutcome::Unavailable {
            reason: ReceiveRetryUnavailableReason::SourceUnavailable,
        },
        AppReceiveRetryOutcome::AccessDenied => EntryReceiveRetryOutcome::Unavailable {
            reason: ReceiveRetryUnavailableReason::AccessDenied,
        },
    };
    Ok(OperationResult::EntryReceiveRetry(outcome))
}

fn map_retry_error(error: RetryEntryReceiveError) -> EngineError {
    match error {
        RetryEntryReceiveError::InvalidInput => EngineError::new(
            RECEIVE_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        // discarded-source[contract-boundary]: 完整重试负责人已记录原始失败分类，边界只映射固定码。
        RetryEntryReceiveError::Unavailable(_) => EngineError::new(
            RECEIVE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
        ),
        RetryEntryReceiveError::Busy => EngineError::new(
            RECEIVE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        // discarded-source[contract-boundary]: 原始链保留在Application结果；禁止向宿主泄露正文或重复记录。
        RetryEntryReceiveError::Source(_)
        | RetryEntryReceiveError::Receive(_)
        | RetryEntryReceiveError::Task(_) => {
            EngineError::new(RECEIVE_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
    }
}

pub async fn execute_cancel_entry_receive(
    facade: &AppFacade,
    input: CancelEntryReceiveInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.entry_id)?;
    validate_id(&input.attempt_id)?;
    let outcome = facade
        .cancel_entry_receive(
            &uc_core::ids::EntryId::from_string(input.entry_id),
            &input.attempt_id,
        )
        .await
        .map_err(map_receive_error)?;
    Ok(OperationResult::EntryReceiveCancellation(
        entry_receive_cancellation(outcome),
    ))
}

pub async fn execute_cancel_inbound_transfer(
    facade: &AppFacade,
    input: CancelInboundTransferInput,
) -> Result<OperationResult, EngineError> {
    validate_id(&input.transfer_id)?;
    let outcome = match facade
        .cancel_inbound_transfer(&input.transfer_id, transfer_reason(input.reason))
        .await
    {
        Ok(outcome) => outcome,
        Err(BlobTransferError::CancellationIncomplete(_)) => {
            // discarded-source[timeout]: 固定业务结果表达仍在收尾，不伪装为取消成功或没有在途任务。
            return Ok(OperationResult::InboundTransferCancellation(
                InboundTransferCancellationOutcome::CancellationIncomplete,
            ));
        }
        Err(error) => return Err(map_transfer_cancel_error(error)),
    };
    Ok(OperationResult::InboundTransferCancellation(
        inbound_transfer_cancellation(outcome),
    ))
}

fn receive_progress(progress: uc_core::ports::EntryReceiveProgress) -> ReceiveProgressSummary {
    ReceiveProgressSummary {
        entry_id: progress.entry_id,
        attempt_id: progress.attempt_id,
        state: progress.state.to_string(),
        total_bytes: progress.total_bytes,
        completed_bytes: progress.completed_bytes,
        items_total: progress.items_total,
        items_completed: progress.items_completed,
        items: progress
            .items
            .into_iter()
            .map(|item| ReceiveItemSummary {
                item_id: item.item_id,
                kind: match item.kind {
                    CoreReceiveItemKind::Unknown => ReceiveItemKind::Unknown,
                    CoreReceiveItemKind::File => ReceiveItemKind::File,
                    CoreReceiveItemKind::Representation => ReceiveItemKind::Representation,
                    CoreReceiveItemKind::DirectoryUnit => ReceiveItemKind::DirectoryUnit,
                },
                state: item.status.to_string(),
                failure_reason: item.failure_reason.map(|reason| match reason {
                    CoreReceiveItemFailureReason::NetworkUnavailable => {
                        ReceiveItemFailureReason::NetworkUnavailable
                    }
                    CoreReceiveItemFailureReason::TimedOut => ReceiveItemFailureReason::TimedOut,
                    CoreReceiveItemFailureReason::AccessDenied => {
                        ReceiveItemFailureReason::AccessDenied
                    }
                    CoreReceiveItemFailureReason::StorageUnavailable => {
                        ReceiveItemFailureReason::StorageUnavailable
                    }
                    CoreReceiveItemFailureReason::IntegrityCheckFailed => {
                        ReceiveItemFailureReason::IntegrityCheckFailed
                    }
                    CoreReceiveItemFailureReason::Interrupted => {
                        ReceiveItemFailureReason::Interrupted
                    }
                    CoreReceiveItemFailureReason::Unknown => ReceiveItemFailureReason::Unknown,
                }),
            })
            .collect(),
    }
}

fn entry_receive_cancellation(
    outcome: AppEntryReceiveCancellationOutcome,
) -> EntryReceiveCancellationOutcome {
    match outcome {
        AppEntryReceiveCancellationOutcome::CancellationIncomplete => {
            EntryReceiveCancellationOutcome::CancellationIncomplete
        }
        AppEntryReceiveCancellationOutcome::Cancelled => EntryReceiveCancellationOutcome::Cancelled,
        AppEntryReceiveCancellationOutcome::NotReceiving => {
            EntryReceiveCancellationOutcome::NotReceiving
        }
        AppEntryReceiveCancellationOutcome::TooLate => EntryReceiveCancellationOutcome::TooLate,
        AppEntryReceiveCancellationOutcome::AlreadyTerminal => {
            EntryReceiveCancellationOutcome::AlreadyTerminal
        }
        AppEntryReceiveCancellationOutcome::Superseded => {
            EntryReceiveCancellationOutcome::Superseded
        }
    }
}

fn inbound_transfer_cancellation(
    outcome: AppInboundTransferCancellationOutcome,
) -> InboundTransferCancellationOutcome {
    match outcome {
        AppInboundTransferCancellationOutcome::Cancelled => {
            InboundTransferCancellationOutcome::Cancelled
        }
        AppInboundTransferCancellationOutcome::NotInflight => {
            InboundTransferCancellationOutcome::NotInflight
        }
    }
}

fn transfer_reason(reason: TransferCancellationReason) -> uc_core::FileTransferCancellationReason {
    match reason {
        TransferCancellationReason::LocalUser => uc_core::FileTransferCancellationReason::LocalUser,
        TransferCancellationReason::RemotePeer => {
            uc_core::FileTransferCancellationReason::RemotePeer
        }
        TransferCancellationReason::Replaced => uc_core::FileTransferCancellationReason::Replaced,
        TransferCancellationReason::Timeout => uc_core::FileTransferCancellationReason::Timeout,
        TransferCancellationReason::Unknown => uc_core::FileTransferCancellationReason::Unknown,
    }
}

fn validate_id(value: &str) -> Result<(), EngineError> {
    if value.trim().is_empty() {
        return Err(EngineError::new(
            RECEIVE_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ));
    }
    Ok(())
}

fn map_receive_error(error: CancelEntryReceiveError) -> EngineError {
    match error {
        CancelEntryReceiveError::Unavailable | CancelEntryReceiveError::Busy(_) => {
            EngineError::new(
                RECEIVE_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                true,
            )
        }
        // discarded-source[contract-boundary]: 完整取消负责人已记录该次失败；重复查询同一结果不重复写业务失败。
        CancelEntryReceiveError::Shared(_) => {
            EngineError::new(RECEIVE_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
        CancelEntryReceiveError::Supervision(_)
        | CancelEntryReceiveError::Attempt(_)
        | CancelEntryReceiveError::PublishLog(_)
        | CancelEntryReceiveError::Transfer(_)
        | CancelEntryReceiveError::Cleanup(_) => {
            uc_error!("entry receive operation failed");
            EngineError::new(RECEIVE_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
    }
}

fn map_transfer_cancel_error(_error: BlobTransferError) -> EngineError {
    uc_error!("inbound transfer cancellation failed");
    EngineError::new(
        TRANSFER_CANCEL_FAILED_CODE,
        EngineErrorCategory::Internal,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_failures_keep_stable_categories_without_details() {
        let unavailable = map_receive_error(CancelEntryReceiveError::Unavailable);
        let internal = map_receive_error(CancelEntryReceiveError::Attempt(anyhow::anyhow!(
            "/private/path/receive.db"
        )));
        let transfer = map_transfer_cancel_error(BlobTransferError::Fetch(anyhow::anyhow!(
            "/private/path/file.bin"
        )));

        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert_eq!(internal.category(), EngineErrorCategory::Internal);
        assert_eq!(transfer.category(), EngineErrorCategory::Internal);
        assert!(!internal.to_string().contains("receive.db"));
        assert!(!transfer.to_string().contains("file.bin"));
    }
}
