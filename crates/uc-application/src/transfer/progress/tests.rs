use super::{
    CurrentProgressTaskPort, ProgressContactNotice, ProgressReceiveError, ProgressReceiverPort,
    ProgressTaskDescriptor, ProgressTerminalNotice, ProgressUpdateNotice, TransferProgressReceiver,
};
use super::{ElapsedClockError, ElapsedClockPort};
use async_trait::async_trait;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::ProgressItemState;
use uc_core::file_transfer::{
    ProgressActive, ProgressBatchState, ProgressFailureReason, ProgressFreshnessView,
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressPhase, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;

fn id(value: u8) -> ProgressIdentity {
    ProgressIdentity::new([value; 16])
}
fn item() -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index: 0,
    }
}
fn descriptor() -> ProgressTaskDescriptor {
    ProgressTaskDescriptor {
        producer: id(2),
        revision: 8,
        source: id(1),
        attempt: id(3),
        item_count: 1,
    }
}
struct Current(ProgressTaskDescriptor);
#[async_trait]
impl CurrentProgressTaskPort for Current {
    async fn challenge(
        &mut self,
        _: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
        Ok(self.0)
    }
}
fn terminal(binding: ProgressIdentity) -> ProgressTerminalNotice {
    ProgressTerminalNotice {
        binding,
        source: id(1),
        attempt: id(3),
        revision: 8,
        result: ProgressTerminal {
            item: item(),
            round: 0,
            sequence: 1,
            result_revision: 1,
            bytes: 8,
            total: Some(8),
            outcome: ProgressTerminalOutcome::Completed,
        },
    }
}

#[tokio::test]
async fn adoption_updates_complete_snapshot_before_exact_ack_and_duplicates_do_not_publish_changes()
{
    let peer = DeviceId::new("receiver");
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()], test_clock()).unwrap();
    assert!(owner.snapshot().items[0].1.is_none());
    let bound = owner
        .register(&peer, 1, id(1), &mut Current(descriptor()))
        .await
        .unwrap();
    let notice = terminal(bound.binding);
    let version = owner.snapshot().version;
    assert_eq!(
        owner.adopt_terminal(&peer, 1, notice).await.unwrap(),
        notice
    );
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Terminal(notice.result))
    );
    assert_eq!(owner.snapshot().version, version + 1);
    assert_eq!(
        owner.adopt_terminal(&peer, 1, notice).await.unwrap(),
        notice
    );
    assert_eq!(owner.snapshot().version, version + 1);
    for conflict in [
        ProgressTerminalNotice {
            result: ProgressTerminal {
                bytes: 7,
                ..notice.result
            },
            ..notice
        },
        ProgressTerminalNotice {
            result: ProgressTerminal {
                result_revision: 2,
                ..notice.result
            },
            ..notice
        },
    ] {
        assert!(matches!(
            owner.adopt_terminal(&peer, 1, conflict).await,
            Err(ProgressReceiveError::Conflict)
        ));
    }
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Terminal(notice.result))
    );
    assert_eq!(owner.snapshot().version, version + 1);
}

#[tokio::test]
async fn source_peer_plan_attempt_revision_and_binding_all_fence_adoption() {
    let peer = DeviceId::new("receiver");
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()], test_clock()).unwrap();
    let bound = owner
        .register(&peer, 1, id(1), &mut Current(descriptor()))
        .await
        .unwrap();
    let notice = terminal(bound.binding);
    assert!(matches!(
        owner
            .adopt_terminal(&DeviceId::new("other"), 1, notice)
            .await,
        Err(ProgressReceiveError::UnknownTask)
    ));
    for wrong in [
        ProgressTerminalNotice {
            binding: id(9),
            ..notice
        },
        ProgressTerminalNotice {
            attempt: id(9),
            ..notice
        },
        ProgressTerminalNotice {
            revision: 7,
            ..notice
        },
    ] {
        assert!(matches!(
            owner.adopt_terminal(&peer, 1, wrong).await,
            Err(ProgressReceiveError::Stale)
        ));
    }
    assert!(matches!(
        owner
            .adopt_terminal(
                &peer,
                1,
                ProgressTerminalNotice {
                    result: ProgressTerminal {
                        item: ProgressItem { index: 1, ..item() },
                        ..notice.result
                    },
                    ..notice
                }
            )
            .await,
        Err(ProgressReceiveError::InvalidPlan)
    ));
    assert!(owner.snapshot().items[0].1.is_none());
    let rebound = owner
        .register(&peer, 2, id(1), &mut Current(descriptor()))
        .await
        .unwrap();
    assert_ne!(bound.binding, rebound.binding);
    assert!(matches!(
        owner.adopt_terminal(&peer, 1, notice).await,
        Err(ProgressReceiveError::Stale)
    ));
    assert!(owner
        .adopt_terminal(&peer, 2, terminal(rebound.binding))
        .await
        .is_ok());
}

#[tokio::test]
async fn invalid_registration_cannot_authorize_unexpected_items_or_attempts() {
    let peer = DeviceId::new("receiver");
    assert!(matches!(
        TransferProgressReceiver::new(peer, id(1), vec![item(), item()], test_clock()),
        Err(ProgressReceiveError::InvalidPlan)
    ));
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()], test_clock()).unwrap();
    assert!(matches!(
        owner
            .register(
                &peer,
                1,
                id(1),
                &mut Current(ProgressTaskDescriptor {
                    item_count: 2,
                    ..descriptor()
                })
            )
            .await,
        Err(ProgressReceiveError::InvalidPlan)
    ));
    assert!(owner.snapshot().bound.is_none());
    assert!(matches!(
        owner.adopt_terminal(&peer, 1, terminal(id(9))).await,
        Err(ProgressReceiveError::Stale)
    ));
}

#[test]
fn exchange_errors_preserve_source_but_do_not_format_sensitive_details() {
    let error = ProgressReceiveError::Exchange(Box::new(io::Error::other("PRIVATE_PATH")));
    assert!(error.source().unwrap().is::<io::Error>());
    assert_eq!(error.class(), "progress_exchange_failed");
    assert!(!format!("{error:?} {error}").contains("PRIVATE"));
}

#[tokio::test]
async fn delayed_authoritative_reply_cannot_replace_newer_registration() {
    use std::sync::Arc;
    use tokio::sync::{Notify, Semaphore};
    struct Delayed {
        entered: Arc<Notify>,
        release: Arc<Semaphore>,
    }
    #[async_trait]
    impl CurrentProgressTaskPort for Delayed {
        async fn challenge(
            &mut self,
            _: ProgressIdentity,
        ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            Ok(descriptor())
        }
    }
    let peer = DeviceId::new("receiver");
    let owner =
        Arc::new(TransferProgressReceiver::new(peer, id(1), vec![item()], test_clock()).unwrap());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let slow = owner.clone();
    let mut exchange = Delayed {
        entered: entered.clone(),
        release: release.clone(),
    };
    let delayed = tokio::spawn(async move { slow.register(&peer, 1, id(1), &mut exchange).await });
    entered.notified().await;
    let current = owner
        .register(
            &peer,
            1,
            id(1),
            &mut Current(ProgressTaskDescriptor {
                revision: 9,
                attempt: id(4),
                ..descriptor()
            }),
        )
        .await
        .unwrap();
    release.add_permits(1);
    assert!(matches!(
        delayed.await.unwrap(),
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(owner.snapshot().bound.unwrap().binding, current.binding);
    assert_eq!(owner.snapshot().bound.unwrap().descriptor.attempt, id(4));
}

#[tokio::test]
async fn updates_share_registration_fences_and_publish_only_new_valid_activity() {
    let peer = DeviceId::new("receiver");
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()], test_clock()).unwrap();
    let bound = owner
        .register(&peer, 1, id(1), &mut Current(descriptor()))
        .await
        .unwrap();
    let notice = ProgressUpdateNotice {
        binding: bound.binding,
        source: id(1),
        attempt: id(3),
        revision: 8,
        update: ProgressActive {
            item: item(),
            round: 0,
            sequence: 10,
            bytes: 5,
            total: Some(8),
            phase: ProgressPhase::Transferring,
        },
    };
    let version = owner.snapshot().version;
    for invalid in [
        ProgressUpdateNotice {
            binding: id(9),
            ..notice
        },
        ProgressUpdateNotice {
            attempt: id(9),
            ..notice
        },
        ProgressUpdateNotice {
            revision: 7,
            ..notice
        },
    ] {
        assert!(matches!(
            owner.adopt_update(&peer, 1, invalid).await,
            Err(ProgressReceiveError::Stale)
        ));
    }
    assert!(matches!(
        owner.adopt_update(&DeviceId::new("other"), 1, notice).await,
        Err(ProgressReceiveError::UnknownTask)
    ));
    assert!(matches!(
        owner.adopt_update(&peer, 2, notice).await,
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(owner.snapshot().version, version);
    owner.adopt_update(&peer, 1, notice).await.unwrap();
    owner.adopt_update(&peer, 1, notice).await.unwrap();
    assert_eq!(owner.snapshot().version, version + 1);
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Active(notice.update))
    );
    owner
        .adopt_terminal(&peer, 1, terminal(bound.binding))
        .await
        .unwrap();
    assert!(matches!(
        owner.adopt_update(&peer, 1, notice).await,
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(owner.snapshot().version, version + 2);
}

struct TestClock(Instant);
impl ElapsedClockPort for TestClock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(self.0.elapsed())
    }
}
fn test_clock() -> Arc<dyn ElapsedClockPort> {
    Arc::new(TestClock(Instant::now()))
}

struct ManualClock(AtomicU64);
impl ElapsedClockPort for ManualClock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
fn contact(binding: ProgressIdentity, sequence: u64) -> ProgressContactNotice {
    ProgressContactNotice {
        binding,
        source: id(1),
        attempt: id(3),
        revision: 8,
        sequence,
    }
}
#[tokio::test]
async fn contact_and_new_activity_renew_only_the_original_intents_sixty_second_confirmation() {
    let peer = DeviceId::new("receiver");
    let clock = Arc::new(ManualClock(AtomicU64::new(0)));
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()], clock.clone()).unwrap();
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Waiting
    );
    clock.0.store(60, Ordering::SeqCst);
    let bound = owner
        .register(&peer, 1, id(1), &mut Current(descriptor()))
        .await
        .unwrap();
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Unconfirmed
    );
    owner
        .adopt_contact(&peer, 1, contact(bound.binding, 5))
        .await
        .unwrap();
    assert!(owner.snapshot().items[0].1.is_none());
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Live
    );
    clock.0.store(119, Ordering::SeqCst);
    for seq in [1, 4, 5] {
        owner
            .adopt_contact(&peer, 1, contact(bound.binding, seq))
            .await
            .unwrap();
    }
    clock.0.store(120, Ordering::SeqCst);
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Unconfirmed
    );
    let update = ProgressUpdateNotice {
        binding: bound.binding,
        source: id(1),
        attempt: id(3),
        revision: 8,
        update: ProgressActive {
            item: item(),
            round: 0,
            sequence: 1,
            bytes: 0,
            total: None,
            phase: ProgressPhase::Waiting,
        },
    };
    owner.adopt_update(&peer, 1, update).await.unwrap();
    clock.0.store(180, Ordering::SeqCst);
    owner.adopt_update(&peer, 1, update).await.unwrap();
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Unconfirmed
    );
    owner
        .adopt_terminal(&peer, 1, terminal(bound.binding))
        .await
        .unwrap();
    clock.0.store(1000, Ordering::SeqCst);
    assert!(matches!(
        owner
            .adopt_contact(&peer, 1, contact(bound.binding, 6))
            .await,
        Err(ProgressReceiveError::Stale)
    ));
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Settled
    );
}
#[tokio::test]
async fn clock_fault_keeps_source_and_duplicate_contact_cannot_restore_confirmed_display() {
    let peer = DeviceId::new("receiver");
    let clock = Arc::new(ManualClock(AtomicU64::new(100)));
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()], clock.clone()).unwrap();
    let bound = owner
        .register(&peer, 1, id(1), &mut Current(descriptor()))
        .await
        .unwrap();
    clock.0.store(110, Ordering::SeqCst);
    owner
        .adopt_contact(&peer, 1, contact(bound.binding, 1))
        .await
        .unwrap();
    clock.0.store(109, Ordering::SeqCst);
    assert_eq!(
        owner.snapshot().summary.state,
        ProgressBatchState::Unconfirmed
    );
    let fault = owner.snapshot().freshness.unwrap_err();
    assert!(fault.source().is_some());
    assert!(owner.snapshot().freshness.is_err());
    owner
        .adopt_contact(&peer, 1, contact(bound.binding, 1))
        .await
        .unwrap();
    assert!(owner.snapshot().freshness.is_err());
    owner
        .adopt_contact(&peer, 1, contact(bound.binding, 2))
        .await
        .unwrap();
    assert_eq!(
        owner.snapshot().freshness.unwrap(),
        ProgressFreshnessView::Live
    );
}

#[tokio::test]
async fn complete_snapshot_summarizes_partial_unknown_then_real_settlement() {
    let peer = DeviceId::new("receiver");
    let clock = Arc::new(ManualClock(AtomicU64::new(0)));
    let second = ProgressItem { index: 1, ..item() };
    let owner =
        TransferProgressReceiver::new(peer, id(1), vec![item(), second], clock.clone()).unwrap();
    let initial = owner.snapshot();
    assert_eq!(initial.summary.state, ProgressBatchState::Waiting);
    assert_eq!(initial.summary.counts.waiting, 2);
    let bound = owner
        .register(
            &peer,
            1,
            id(1),
            &mut Current(ProgressTaskDescriptor {
                item_count: 2,
                ..descriptor()
            }),
        )
        .await
        .unwrap();
    owner
        .adopt_terminal(&peer, 1, terminal(bound.binding))
        .await
        .unwrap();
    let partial = owner.snapshot();
    assert_eq!(partial.summary.counts.completed, 1);
    assert_eq!(partial.summary.counts.waiting, 1);
    assert_eq!(partial.summary.state, ProgressBatchState::Waiting);
    clock.0.store(60, Ordering::SeqCst);
    let unknown = owner.snapshot();
    assert_eq!(
        unknown.summary.state,
        ProgressBatchState::PartiallyUnconfirmed
    );
    assert_eq!(unknown.summary.counts.completed, 1);
    assert_eq!(unknown.summary.counts.unconfirmed, 1);
    assert_eq!(unknown.summary.counts.failed, 0);
    let mut failure = terminal(bound.binding);
    failure.result.item = second;
    failure.result.outcome =
        ProgressTerminalOutcome::Failed(ProgressFailureReason::StorageUnavailable);
    owner.adopt_terminal(&peer, 1, failure).await.unwrap();
    let settled = owner.snapshot();
    assert_eq!(settled.freshness.unwrap(), ProgressFreshnessView::Settled);
    assert_eq!(settled.summary.state, ProgressBatchState::PartiallyFailed);
    assert_eq!(settled.summary.counts.completed, 1);
    assert_eq!(settled.summary.counts.failed, 1);
    clock.0.store(0, Ordering::SeqCst);
    assert_eq!(owner.snapshot().summary, settled.summary);
    owner.retire();
    assert_eq!(owner.snapshot().summary.state, ProgressBatchState::NoItems);
}
