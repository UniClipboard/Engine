//! 完整进度能力的合同映射；授权、核对与采用均由Application负责。
use crate::assembly::observability::observe_progress_reconciliation;
use crate::error_codes::{
    OUTBOUND_PROGRESS_ACCESS_DENIED_CODE, OUTBOUND_PROGRESS_INVALID_DATA_CODE,
    OUTBOUND_PROGRESS_INVALID_INPUT_CODE, OUTBOUND_PROGRESS_UNAVAILABLE_CODE,
};
use crate::{
    EngineError, EngineErrorCategory, OperationResult, OutboundProgressBatchState,
    OutboundProgressCancellationReason, OutboundProgressFreshness, OutboundProgressInput,
    OutboundProgressItemSummary, OutboundProgressPhase, OutboundProgressReconciliation,
    OutboundProgressState, OutboundProgressSummary, ReceiveItemFailureReason, ReceiveItemKind,
    ReconcileOutboundProgressInput,
};
use std::error::Error;
use tokio_util::sync::CancellationToken;
use uc_application::facade::{
    AppFacade, TransferProgressQueryError, TransferProgressReconciliation,
};
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{
    FileTransferCancellationReason, ProgressBatchState, ProgressFailureReason,
    ProgressFreshnessView, ProgressItemKind, ProgressPhase, TransferProgressItemStatus,
    TransferProgressView,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_observability_contract::uc_debug;

pub(crate) async fn execute_query_outbound_progress(
    facade: &AppFacade,
    input: OutboundProgressInput,
) -> Result<OperationResult, EngineError> {
    let value = facade
        .get_transfer_progress(
            DeviceId::new(input.target_device_id),
            &EntryId::from(input.entry_id),
        )
        .await
        .map_err(map_error)?;
    Ok(OperationResult::OutboundProgress(value.map(present)))
}
pub(crate) async fn execute_list_outbound_progress(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let values = facade.list_transfer_progress().await.map_err(map_error)?;
    Ok(OperationResult::OutboundProgressList(
        values.into_iter().map(present).collect(),
    ))
}
pub(crate) async fn execute_reconcile_outbound_progress(
    facade: &AppFacade,
    input: ReconcileOutboundProgressInput,
) -> Result<OperationResult, EngineError> {
    let value = observe_progress_reconciliation(Box::pin(facade.reconcile_transfer_progress(
        DeviceId::new(input.target_device_id),
        &EntryId::from(input.entry_id),
        input.expected_attempt_id.as_deref(),
        &CancellationToken::new(),
    )))
    .await
    .map_err(map_error)?;
    Ok(OperationResult::OutboundProgressReconciliation(
        reconciliation(value),
    ))
}
pub(super) fn map_error(source: TransferProgressQueryError) -> EngineError {
    uc_debug!(
        error_class = source.class(),
        error = &source as &dyn Error,
        "outbound progress action failed"
    );
    // discarded-source[contract-boundary]: 完整来源已在上方交给观测边界，稳定宿主错误只包含固定码。
    let (code, category, retryable) = match source {
        TransferProgressQueryError::InvalidInput(_) => (
            OUTBOUND_PROGRESS_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        TransferProgressQueryError::AccessDenied(_) => (
            OUTBOUND_PROGRESS_ACCESS_DENIED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        TransferProgressQueryError::InvalidData(_) => (
            OUTBOUND_PROGRESS_INVALID_DATA_CODE,
            EngineErrorCategory::Internal,
            false,
        ),
        TransferProgressQueryError::Unavailable(_) => (
            OUTBOUND_PROGRESS_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
    };
    EngineError::new(code, category, retryable)
}
pub(super) fn present(view: TransferProgressView) -> OutboundProgressSummary {
    let counts = view.summary.counts;
    OutboundProgressSummary {
        entry_id: view.source_entry.as_ref().to_owned(),
        target_device_id: view.peer.as_str().to_owned(),
        attempt_id: view.attempt,
        revision: view.revision,
        freshness: freshness(view.freshness),
        freshness_available: view.freshness_available,
        state: batch_state(view.summary.state),
        // 原始计划上限1024，计数在进入稳定合同前已经有界。
        items_total: counts.total as u32,
        items_waiting: counts.waiting as u32,
        items_running: counts.running as u32,
        items_completed: counts.completed as u32,
        items_failed: counts.failed as u32,
        items_cancelled: counts.cancelled as u32,
        items_unconfirmed: counts.unconfirmed as u32,
        items: view
            .items
            .into_iter()
            .map(|item| OutboundProgressItemSummary {
                kind: match item.item.kind {
                    ProgressItemKind::File => ReceiveItemKind::File,
                    ProgressItemKind::Representation => ReceiveItemKind::Representation,
                    ProgressItemKind::DirectoryUnit => ReceiveItemKind::DirectoryUnit,
                },
                index: item.item.index,
                state: item_state(item.status),
                last_known_phase: item.last_known_phase.map(phase),
                round: item.round,
                completed_bytes: item.bytes,
                total_bytes: item.total,
                failure_reason: item.failure.map(failure),
                cancellation_reason: item.cancellation.map(cancellation),
            })
            .collect(),
    }
}

fn item_state(value: TransferProgressItemStatus) -> OutboundProgressState {
    match value {
        TransferProgressItemStatus::Waiting => OutboundProgressState::Waiting,
        TransferProgressItemStatus::Transferring => OutboundProgressState::Transferring,
        TransferProgressItemStatus::Processing => OutboundProgressState::Processing,
        TransferProgressItemStatus::Retrying => OutboundProgressState::Retrying,
        TransferProgressItemStatus::Completed => OutboundProgressState::Completed,
        TransferProgressItemStatus::Failed => OutboundProgressState::Failed,
        TransferProgressItemStatus::Cancelled => OutboundProgressState::Cancelled,
        TransferProgressItemStatus::Unconfirmed => OutboundProgressState::Unconfirmed,
    }
}

fn phase(value: ProgressPhase) -> OutboundProgressPhase {
    match value {
        ProgressPhase::Waiting => OutboundProgressPhase::Waiting,
        ProgressPhase::Transferring => OutboundProgressPhase::Transferring,
        ProgressPhase::Processing => OutboundProgressPhase::Processing,
        ProgressPhase::Retrying => OutboundProgressPhase::Retrying,
    }
}

fn freshness(value: ProgressFreshnessView) -> OutboundProgressFreshness {
    match value {
        ProgressFreshnessView::Waiting => OutboundProgressFreshness::Waiting,
        ProgressFreshnessView::Live => OutboundProgressFreshness::Live,
        ProgressFreshnessView::Unconfirmed => OutboundProgressFreshness::Unconfirmed,
        ProgressFreshnessView::Settled => OutboundProgressFreshness::Settled,
    }
}

fn batch_state(value: ProgressBatchState) -> OutboundProgressBatchState {
    match value {
        ProgressBatchState::NoItems => OutboundProgressBatchState::NoItems,
        ProgressBatchState::Waiting => OutboundProgressBatchState::Waiting,
        ProgressBatchState::InProgress => OutboundProgressBatchState::InProgress,
        ProgressBatchState::Unconfirmed => OutboundProgressBatchState::Unconfirmed,
        ProgressBatchState::PartiallyUnconfirmed => {
            OutboundProgressBatchState::PartiallyUnconfirmed
        }
        ProgressBatchState::Completed => OutboundProgressBatchState::Completed,
        ProgressBatchState::Failed => OutboundProgressBatchState::Failed,
        ProgressBatchState::PartiallyFailed => OutboundProgressBatchState::PartiallyFailed,
        ProgressBatchState::Cancelled => OutboundProgressBatchState::Cancelled,
        ProgressBatchState::PartiallyCancelled => OutboundProgressBatchState::PartiallyCancelled,
    }
}

pub(super) fn reconciliation(
    value: TransferProgressReconciliation,
) -> OutboundProgressReconciliation {
    match value {
        TransferProgressReconciliation::Running => OutboundProgressReconciliation::Running,
        TransferProgressReconciliation::Settled => OutboundProgressReconciliation::Settled,
        TransferProgressReconciliation::Superseded => OutboundProgressReconciliation::Superseded,
        TransferProgressReconciliation::NotRetained => OutboundProgressReconciliation::NotRetained,
        TransferProgressReconciliation::Unsupported => OutboundProgressReconciliation::Unsupported,
        TransferProgressReconciliation::Unreachable => OutboundProgressReconciliation::Unreachable,
        TransferProgressReconciliation::Busy => OutboundProgressReconciliation::Busy,
    }
}

fn failure(value: ProgressFailureReason) -> ReceiveItemFailureReason {
    match value {
        ProgressFailureReason::NetworkUnavailable => ReceiveItemFailureReason::NetworkUnavailable,
        ProgressFailureReason::TimedOut => ReceiveItemFailureReason::TimedOut,
        ProgressFailureReason::AccessDenied => ReceiveItemFailureReason::AccessDenied,
        ProgressFailureReason::StorageUnavailable => ReceiveItemFailureReason::StorageUnavailable,
        ProgressFailureReason::IntegrityCheckFailed => {
            ReceiveItemFailureReason::IntegrityCheckFailed
        }
        ProgressFailureReason::Interrupted => ReceiveItemFailureReason::Interrupted,
        ProgressFailureReason::Unknown => ReceiveItemFailureReason::Unknown,
    }
}

fn cancellation(value: FileTransferCancellationReason) -> OutboundProgressCancellationReason {
    match value {
        FileTransferCancellationReason::LocalUser => OutboundProgressCancellationReason::LocalUser,
        FileTransferCancellationReason::RemotePeer => {
            OutboundProgressCancellationReason::RemotePeer
        }
        FileTransferCancellationReason::Replaced => OutboundProgressCancellationReason::Replaced,
        FileTransferCancellationReason::Timeout => OutboundProgressCancellationReason::Timeout,
        FileTransferCancellationReason::ConnectivityRecovery => {
            OutboundProgressCancellationReason::ConnectivityRecovery
        }
        FileTransferCancellationReason::Unknown => OutboundProgressCancellationReason::Unknown,
    }
}
