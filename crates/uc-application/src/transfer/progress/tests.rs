use super::{
    CurrentProgressTaskPort, ProgressReceiveError, ProgressReceiverPort, ProgressTaskDescriptor,
    ProgressTerminalNotice, TransferProgressReceiver,
};
use async_trait::async_trait;
use std::error::Error;
use std::io;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal, ProgressTerminalOutcome,
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
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()]).unwrap();
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
    assert_eq!(owner.snapshot().items[0].1, Some(notice.result));
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
    assert_eq!(owner.snapshot().items[0].1, Some(notice.result));
    assert_eq!(owner.snapshot().version, version + 1);
}

#[tokio::test]
async fn source_peer_plan_attempt_revision_and_binding_all_fence_adoption() {
    let peer = DeviceId::new("receiver");
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()]).unwrap();
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
        TransferProgressReceiver::new(peer, id(1), vec![item(), item()]),
        Err(ProgressReceiveError::InvalidPlan)
    ));
    let owner = TransferProgressReceiver::new(peer, id(1), vec![item()]).unwrap();
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
    let owner = Arc::new(TransferProgressReceiver::new(peer, id(1), vec![item()]).unwrap());
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
