//! 同一真实文件动作对照SQLite接收事实、源端采用视图与精确ACK。
use super::*;
use crate::{OutboundProgressFreshness, OutboundProgressState};
use uc_core::file_transfer::{ProgressIdentity, ProgressItemKind, ProgressTerminalOutcome};
use uuid::Uuid;

pub(super) async fn assert_chain(
    source: &Engine,
    source_entry: &str,
    received: &ReceiveProgressSummary,
    exports: &ReceiveExports,
    settled: bool,
) {
    let source_id = ProgressIdentity::new(*Uuid::parse_str(source_entry).unwrap().as_bytes());
    let attempt = ProgressIdentity::new(*Uuid::parse_str(&received.attempt_id).unwrap().as_bytes());
    let acknowledgements = exports
        .matching_acks(source_id, attempt, if settled { 3 } else { 2 })
        .await;
    assert_eq!(acknowledgements.len(), if settled { 3 } else { 2 });
    let expected = [
        OutboundProgressState::Completed,
        OutboundProgressState::Failed,
        if settled {
            OutboundProgressState::Completed
        } else {
            OutboundProgressState::Processing
        },
    ];
    let view = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgressList(views) = source
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("complete source projection")
            };
            if let Some(view) = views.into_iter().find(|view| {
                view.entry_id == source_entry
                    && view.attempt_id.as_deref() == Some(received.attempt_id.as_str())
                    && view.items.iter().map(|item| item.state).eq(expected)
            }) {
                return view;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("source adopts exactly the actual file outcomes");
    assert_ne!(view.entry_id, received.entry_id);
    assert_eq!(view.items_completed, if settled { 2 } else { 1 });
    assert_eq!(view.items_failed, 1);
    assert_eq!(view.items_running, if settled { 0 } else { 1 });
    assert_eq!(view.items_unconfirmed, 0);
    assert_eq!(
        view.items[1].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    if settled {
        assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
    }
    let mut indices = Vec::new();
    for notice in acknowledgements {
        assert_eq!(notice.result.item.kind, ProgressItemKind::File);
        let index = usize::from(notice.result.item.index);
        indices.push(index);
        let item = &view.items[index];
        assert_eq!(item.round, Some(notice.result.round));
        assert_eq!(item.completed_bytes, Some(notice.result.bytes));
        assert_eq!(item.total_bytes, notice.result.total);
        match notice.result.outcome {
            ProgressTerminalOutcome::Completed => {
                assert!(index == 0 || (settled && index == 2));
                assert_eq!(notice.result.total, Some(notice.result.bytes));
            }
            ProgressTerminalOutcome::Failed(_) => assert_eq!(index, 1),
            ProgressTerminalOutcome::Cancelled(_) => panic!("no cancellation was requested"),
        }
    }
    indices.sort_unstable();
    assert_eq!(indices, if settled { vec![0, 1, 2] } else { vec![0, 1] });
}
