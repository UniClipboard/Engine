//! 通知消费原接收负责人的canonical快照；实际保存与重试不等待本观察者。
use super::super::{ProgressExchangeService, ProgressReceiveError, ProgressSubmission};
use crate::transfer::receive::work::{ReceiveProgressChange, ReceiveWorkRegistry};
use std::error::Error;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::ProgressItemState;
use uc_observability_contract::uc_debug;

pub(super) async fn observe_receive(
    registry: Arc<ReceiveWorkRegistry>,
    exchanges: ProgressExchangeService,
    cancel: CancellationToken,
) {
    while let Some(changed) = registry.next_progress_changes(&cancel).await {
        for changed in changed {
            submit_change(&exchanges, changed);
        }
    }
}
fn submit_change(exchanges: &ProgressExchangeService, changed: ReceiveProgressChange) {
    let snapshot = match changed.snapshot {
        Ok(snapshot) => snapshot,
        Err(source) => {
            record_error(source);
            return;
        }
    };
    match exchanges.submit_activity(changed.peer, snapshot.descriptor, None) {
        Ok(ProgressSubmission::Closed | ProgressSubmission::Refused(_)) => return,
        Ok(_) => {}
        Err(source) => {
            record_error(source);
            return;
        }
    }
    for value in &snapshot.items {
        let submitted = match value {
            ProgressItemState::Active(value) => {
                exchanges.submit_activity(changed.peer, snapshot.descriptor, Some(*value))
            }
            ProgressItemState::Terminal(value) => {
                exchanges.submit_settled(changed.peer, snapshot.descriptor, *value)
            }
        };
        if let Err(source) = submitted {
            record_error(source);
        }
    }
}

fn record_error(source: ProgressReceiveError) {
    uc_debug!(
        error_class = source.class(),
        error = &source as &dyn Error,
        "receive facts progress handoff failed"
    );
}
