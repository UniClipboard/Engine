use super::{descriptor, item, key as projection_key, Clock, Current};
use crate::transfer::progress::{
    ProgressProjectionEvent, ProgressProjectionRegistry, ProgressReceiveError,
    ProgressReceiverPort, ProgressSnapshot, ProgressSnapshotRead, ProgressTerminalNotice,
    ProgressUnattributedReceiverPort, ProgressUpdateNotice,
};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tokio::sync::broadcast::error::TryRecvError;
use uc_core::file_transfer::{
    ProgressActive, ProgressBatchState, ProgressFreshnessView, ProgressItemState, ProgressPhase,
    ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::EntryId;
use uuid::Uuid;

#[tokio::test]
async fn unattributed_notice_marks_only_the_original_intent_unknown_and_publishes_once() {
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let key = projection_key(1, 1);
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key).source.bytes()).to_string(),
        ))
        .accept(key, vec![item(0), item(1)])
        .unwrap();
    let before = registry.snapshot(key).unwrap();
    let mut events = registry.subscribe();
    registry
        .observe_unattributed(&key.peer, key.source)
        .await
        .unwrap();
    let after = registry.snapshot(key).unwrap();
    assert_eq!(after.freshness.unwrap(), ProgressFreshnessView::Unconfirmed);
    assert_eq!(after.summary.state, ProgressBatchState::Unconfirmed);
    assert_eq!(after.summary.counts.unconfirmed, 2);
    assert!(after.bound.is_none() && after.task.is_none());
    assert!(after.items.iter().all(|(_, value)| value.is_none()));
    assert_eq!(after.version, before.version + 1);
    assert_eq!(
        events.try_recv().unwrap(),
        ProgressProjectionEvent::Changed(key)
    );
    registry
        .observe_unattributed(&key.peer, key.source)
        .await
        .unwrap();
    assert_eq!(registry.snapshot(key).unwrap().version, after.version);
    assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
}

fn fact(terminal: bool) -> ProgressItemState {
    if terminal {
        ProgressItemState::Terminal(ProgressTerminal {
            item: item(0),
            round: 0,
            sequence: 1,
            result_revision: 1,
            bytes: 8,
            total: Some(8),
            outcome: ProgressTerminalOutcome::Completed,
        })
    } else {
        ProgressItemState::Active(ProgressActive {
            item: item(0),
            round: 0,
            sequence: 1,
            bytes: 4,
            total: Some(8),
            phase: ProgressPhase::Transferring,
        })
    }
}

#[tokio::test]
async fn unattributed_notice_preserves_both_registered_and_query_confirmed_facts() {
    for queried in [false, true] {
        for terminal in [false, true] {
            let (registry, _worker) =
                ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
            let key = projection_key(1, 1);
            registry
                .begin_intent(EntryId::from(
                    Uuid::from_bytes((key).source.bytes()).to_string(),
                ))
                .accept(key, vec![item(0)])
                .unwrap();
            let task = descriptor(key, 1);
            let value = fact(terminal);
            if queried {
                let receiver = registry.receiver(key).unwrap();
                receiver
                    .finish_query(
                        None,
                        &ProgressSnapshotRead::Complete(ProgressSnapshot {
                            descriptor: task,
                            revision: 1,
                            items: vec![value],
                        }),
                    )
                    .unwrap();
                assert!(registry.publish(key, &receiver));
            } else {
                let bound = registry
                    .register(&key.peer, 1, key.source, &mut Current(task))
                    .await
                    .unwrap();
                match value {
                    ProgressItemState::Active(update) => registry
                        .adopt_update(
                            &key.peer,
                            1,
                            ProgressUpdateNotice {
                                binding: bound.binding,
                                source: key.source,
                                attempt: task.attempt,
                                revision: task.revision,
                                update,
                            },
                        )
                        .await
                        .unwrap(),
                    ProgressItemState::Terminal(result) => {
                        registry
                            .adopt_terminal(
                                &key.peer,
                                1,
                                ProgressTerminalNotice {
                                    binding: bound.binding,
                                    source: key.source,
                                    attempt: task.attempt,
                                    revision: task.revision,
                                    result,
                                },
                            )
                            .await
                            .unwrap();
                    }
                }
            }
            let before = registry.snapshot(key).unwrap();
            let mut events = registry.subscribe();
            assert!(matches!(
                registry.observe_unattributed(&key.peer, key.source).await,
                Err(ProgressReceiveError::Stale)
            ));
            let after = registry.snapshot(key).unwrap();
            assert_eq!(after.items, before.items);
            assert_eq!(after.summary, before.summary);
            assert_eq!(after.freshness.unwrap(), before.freshness.unwrap());
            assert!(after.bound == before.bound && after.task == before.task);
            assert_eq!(after.version, before.version);
            assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
        }
    }
}

#[tokio::test]
async fn unattributed_notice_never_creates_deleted_unknown_or_closed_intents() {
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let key = projection_key(1, 1);
    assert!(matches!(
        registry.observe_unattributed(&key.peer, key.source).await,
        Err(ProgressReceiveError::UnknownTask)
    ));
    assert!(registry.snapshots().is_empty());
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key).source.bytes()).to_string(),
        ))
        .accept(key, vec![item(0)])
        .unwrap();
    let old = registry.receiver(key).unwrap();
    registry.remove(key);
    assert!(matches!(
        old.observe_unattributed(&key.peer, key.source).await,
        Err(ProgressReceiveError::UnknownTask)
    ));
    assert!(matches!(
        registry.observe_unattributed(&key.peer, key.source).await,
        Err(ProgressReceiveError::UnknownTask)
    ));
    assert!(registry.snapshots().is_empty());
    let other = projection_key(2, 2);
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((other).source.bytes()).to_string(),
        ))
        .accept(other, vec![item(0)])
        .unwrap();
    registry.close();
    assert!(matches!(
        registry
            .observe_unattributed(&other.peer, other.source)
            .await,
        Err(ProgressReceiveError::UnknownTask)
    ));
    assert!(registry.snapshots().is_empty());
}

#[tokio::test]
async fn unattributed_notice_does_not_prevent_a_later_verified_current_update() {
    let (registry, _worker) = ProgressProjectionRegistry::new(Arc::new(Clock(AtomicU64::new(0))));
    let key = projection_key(1, 1);
    registry
        .begin_intent(EntryId::from(
            Uuid::from_bytes((key).source.bytes()).to_string(),
        ))
        .accept(key, vec![item(0)])
        .unwrap();
    registry
        .observe_unattributed(&key.peer, key.source)
        .await
        .unwrap();
    let task = descriptor(key, 1);
    let bound = registry
        .register(&key.peer, 1, key.source, &mut Current(task))
        .await
        .unwrap();
    let update = ProgressActive {
        item: item(0),
        round: 0,
        sequence: 1,
        bytes: 4,
        total: Some(8),
        phase: ProgressPhase::Transferring,
    };
    registry
        .adopt_update(
            &key.peer,
            1,
            ProgressUpdateNotice {
                binding: bound.binding,
                source: key.source,
                attempt: task.attempt,
                revision: task.revision,
                update,
            },
        )
        .await
        .unwrap();
    let current = registry.snapshot(key).unwrap();
    assert_eq!(current.freshness.unwrap(), ProgressFreshnessView::Live);
    assert_eq!(current.items[0].1, Some(ProgressItemState::Active(update)));
    assert!(current.task == Some(task) && current.bound == Some(bound));
    assert!(matches!(
        registry.observe_unattributed(&key.peer, key.source).await,
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(registry.snapshot(key).unwrap().version, current.version);
}
