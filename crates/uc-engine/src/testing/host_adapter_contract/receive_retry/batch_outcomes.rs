//! 原SQLite逐项保存失败必须完整到达源端，末项成功不能覆盖前项失败。
mod directory;
mod mixed;
mod retry;
use super::ReceiveCase;
use crate::{
    Operation, OperationResult, OutboundProgressBatchState, OutboundProgressFreshness,
    OutboundProgressInput, OutboundProgressReconciliation, OutboundProgressState,
    OutboundProgressSummary, ReceiveItemFailureReason, ReceiveProgressSummary,
    ReconcileOutboundProgressInput,
};
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use uc_core::file_transfer::{ProgressIdentity, ProgressItemKind, ProgressTerminalOutcome};
use uuid::Uuid;

pub(super) async fn assert_file_failure(
    case: &ReceiveCase,
    received: &ReceiveProgressSummary,
    failed: usize,
) {
    let view = reconciled_view(case, received, OutboundProgressReconciliation::Settled).await;
    assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(view.state, OutboundProgressBatchState::PartiallyFailed);
    assert_eq!(
        (view.items_total, view.items_completed, view.items_failed),
        (3, 2, 1)
    );
    assert_eq!(
        (
            view.items_waiting,
            view.items_running,
            view.items_cancelled,
            view.items_unconfirmed
        ),
        (0, 0, 0, 0)
    );
    for (index, item) in view.items.iter().enumerate() {
        assert_eq!(
            item.state,
            if index == failed {
                OutboundProgressState::Failed
            } else {
                OutboundProgressState::Completed
            }
        );
        assert_eq!(
            item.failure_reason,
            (index == failed).then_some(ReceiveItemFailureReason::StorageUnavailable)
        );
    }
    let source = ProgressIdentity::new(*Uuid::parse_str(&view.entry_id).unwrap().as_bytes());
    let attempt = ProgressIdentity::new(*Uuid::parse_str(&received.attempt_id).unwrap().as_bytes());
    let acks = case.exports.matching_acks(source, attempt, 3).await;
    assert_eq!(acks.len(), 3);
    assert_eq!(
        acks.iter()
            .map(|notice| notice.result.item.index)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([0, 1, 2])
    );
    for notice in acks {
        assert_eq!(notice.result.item.kind, ProgressItemKind::File);
        let index = usize::from(notice.result.item.index);
        assert_eq!(notice.result.round, view.items[index].round.unwrap());
        assert_eq!(Some(notice.result.bytes), view.items[index].completed_bytes);
        assert_eq!(notice.result.total, view.items[index].total_bytes);
        if index == failed {
            assert!(matches!(
                notice.result.outcome,
                ProgressTerminalOutcome::Failed(_)
            ));
        } else {
            assert_eq!(notice.result.outcome, ProgressTerminalOutcome::Completed);
        }
    }
}

async fn reconciled_view(
    case: &ReceiveCase,
    received: &ReceiveProgressSummary,
    expected: OutboundProgressReconciliation,
) -> OutboundProgressSummary {
    let registered = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgressList(views) = case
                .source
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("complete source views")
            };
            if let Some(view) = views
                .into_iter()
                .find(|view| view.attempt_id.as_deref() == Some(received.attempt_id.as_str()))
            {
                return view;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("original receiver attempt is registered");
    reconciled_for_input(
        case,
        received,
        OutboundProgressInput {
            entry_id: registered.entry_id,
            target_device_id: registered.target_device_id,
        },
        expected,
    )
    .await
}

async fn reconciled_for_input(
    case: &ReceiveCase,
    received: &ReceiveProgressSummary,
    input: OutboundProgressInput,
    expected: OutboundProgressReconciliation,
) -> OutboundProgressSummary {
    let original = case
        .source
        .execute(Operation::QueryOutboundProgress(input.clone()))
        .await
        .unwrap();
    assert!(
        matches!(original, OperationResult::OutboundProgress(Some(_))),
        "original intent missing before query: {original:?}"
    );
    let actual = case
        .source
        .execute(Operation::ReconcileOutboundProgress(
            ReconcileOutboundProgressInput {
                entry_id: input.entry_id.clone(),
                target_device_id: input.target_device_id.clone(),
                expected_attempt_id: Some(received.attempt_id.clone()),
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        actual,
        OperationResult::OutboundProgressReconciliation(expected)
    );
    let OperationResult::OutboundProgress(Some(view)) = case
        .source
        .execute(Operation::QueryOutboundProgress(input))
        .await
        .unwrap()
    else {
        panic!("adopted complete receiver facts")
    };
    assert_ne!(view.entry_id, received.entry_id);
    assert_eq!(
        view.attempt_id.as_deref(),
        Some(received.attempt_id.as_str())
    );
    view
}
