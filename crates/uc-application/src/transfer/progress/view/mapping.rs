use super::super::{ProgressProjectionKey, ProgressProjectionSnapshot};
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{
    FileTransferCancellationReason, ProgressFreshnessView, ProgressItemState, ProgressPhase,
    ProgressTerminalOutcome, TransferProgressItemStatus, TransferProgressItemView,
    TransferProgressView,
};
use uc_core::ids::EntryId;
use uc_observability_contract::uc_debug;
use uuid::Uuid;

pub(super) fn present(
    key: ProgressProjectionKey,
    snapshot: ProgressProjectionSnapshot,
) -> TransferProgressView {
    let freshness_available = snapshot.freshness.is_ok();
    let freshness = match snapshot.freshness {
        Ok(freshness) => freshness,
        Err(source) => {
            // 展示边界保留已知成果；本机计时失败只令活动事实未确认，不伪造成远端离线。
            uc_debug!(
                error_class = source.class(),
                error = source.as_ref(),
                "transfer progress display clock unavailable"
            );
            ProgressFreshnessView::Unconfirmed
        }
    };
    let confirmed = freshness != ProgressFreshnessView::Unconfirmed;
    let items = snapshot
        .items
        .into_iter()
        .map(|(item, value)| {
            let mut view = TransferProgressItemView {
                item,
                status: if confirmed {
                    TransferProgressItemStatus::Waiting
                } else {
                    TransferProgressItemStatus::Unconfirmed
                },
                last_known_phase: None,
                round: None,
                bytes: None,
                total: None,
                failure: None,
                cancellation: None,
            };
            match value {
                Some(ProgressItemState::Active(active)) => {
                    view.round = Some(active.round);
                    view.bytes = Some(active.bytes);
                    view.total = active.total;
                    view.last_known_phase = Some(active.phase);
                    if confirmed {
                        view.status = match active.phase {
                            ProgressPhase::Waiting => TransferProgressItemStatus::Waiting,
                            ProgressPhase::Transferring => TransferProgressItemStatus::Transferring,
                            ProgressPhase::Processing => TransferProgressItemStatus::Processing,
                            ProgressPhase::Retrying => TransferProgressItemStatus::Retrying,
                        };
                    }
                }
                Some(ProgressItemState::Terminal(result)) => {
                    view.round = Some(result.round);
                    view.bytes = Some(result.bytes);
                    view.total = result.total;
                    view.status = match result.outcome {
                        ProgressTerminalOutcome::Completed => TransferProgressItemStatus::Completed,
                        ProgressTerminalOutcome::Failed(reason) => {
                            view.failure = Some(reason);
                            TransferProgressItemStatus::Failed
                        }
                        ProgressTerminalOutcome::Cancelled(reason) => {
                            view.cancellation = Some(match reason {
                                FileTransferCancellationReason::LocalUser => {
                                    FileTransferCancellationReason::RemotePeer
                                }
                                FileTransferCancellationReason::RemotePeer => {
                                    FileTransferCancellationReason::LocalUser
                                }
                                other => other,
                            });
                            TransferProgressItemStatus::Cancelled
                        }
                    };
                }
                None => {}
            }
            view
        })
        .collect();
    TransferProgressView {
        peer: key.peer,
        source_entry: EntryId::from(Uuid::from_bytes(key.source.bytes()).to_string()),
        attempt: snapshot
            .task
            .map(|task| Uuid::from_bytes(task.attempt.bytes()).to_string()),
        revision: snapshot.version,
        freshness,
        freshness_available,
        summary: snapshot.summary,
        items,
    }
}
