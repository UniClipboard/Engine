//! 同一监督器的实际查询/终态交换；不可中断成员读取暂停后仍保留真实工作寿命。
use super::progress_probe::UnusedSnapshotProvider;
use super::progress_probe::{progress_budget, Fixture, UnusedLive};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use async_trait::async_trait;
use iroh::{Endpoint, RelayMode};
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectivityError, ElapsedClockError, ElapsedClockPort, ProgressClaimRefusal,
    ProgressExchangeDeadline, ProgressExchangeService, ProgressReceiveError, ProgressSnapshotRead,
    ProgressSnapshotRound, ProgressSnapshotRoundPort, ProgressSnapshotTarget, ProgressSourceTask,
    ProgressSubmission, ProgressTaskAuthorityPort, ProgressTaskDescriptor,
    ProgressTerminalRoundPort, TransferProgressProducer, TransferProgressReceiver,
    TransferProgressSnapshotReader,
};
use uc_core::file_transfer::ProgressItemState;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, ActiveProgress, Envelope, ItemKind, ItemState, LogicalItem, Message,
    Phase, ProducerDescriptor, QueryResult, SnapshotPage, TaskScope, WireId,
    TRANSFER_PROGRESS_ALPN,
};

const SOURCE: ProgressIdentity = ProgressIdentity::new([1; 16]);
fn task() -> ProgressSourceTask {
    ProgressSourceTask {
        notification_closed: Default::default(),
        descriptor: ProgressTaskDescriptor {
            producer: ProgressIdentity::new([2; 16]),
            revision: 1,
            source: SOURCE,
            attempt: ProgressIdentity::new([3; 16]),
            item_count: 1,
        },
        items: Arc::from([ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        }]),
    }
}
struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
struct Authority {
    peer: DeviceId,
    claimed: AtomicBool,
}
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        (*peer == self.peer && source == SOURCE).then(|| {
            let mut current = task();
            if self.claimed.load(Ordering::SeqCst) {
                current.notification_closed = Arc::new(current.items.iter().copied().collect());
            }
            current
        })
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        let current = self
            .current_task(peer, source)
            .ok_or(ProgressClaimRefusal::NotRetained)?;
        if current.descriptor.attempt != attempt {
            return Err(ProgressClaimRefusal::Superseded);
        }
        if !current.items.contains(&item) {
            return Err(ProgressClaimRefusal::InvalidItem);
        }
        if self.claimed.swap(true, Ordering::SeqCst) {
            return Err(ProgressClaimRefusal::AlreadyObserved);
        }
        Ok(current)
    }
}
struct Counts {
    active: Mutex<HashSet<DeviceId>>,
    peak: AtomicUsize,
    reads: AtomicUsize,
    ack: Notify,
}
impl Counts {
    fn enter(&self, peer: DeviceId) {
        let mut active = self.active.lock().unwrap();
        assert!(active.insert(peer));
        assert!(active.len() <= 4);
        self.peak.fetch_max(active.len(), Ordering::SeqCst);
    }
    fn end(&self, peer: DeviceId) {
        assert!(self.active.lock().unwrap().remove(&peer));
    }
}
struct Snapshot {
    reader: Arc<TransferProgressSnapshotReader>,
    counts: Arc<Counts>,
}
impl ProgressSnapshotRoundPort for Snapshot {
    fn read_snapshot(
        self: Arc<Self>,
        peer: DeviceId,
        expected: ProgressSnapshotTarget,
        deadline: Arc<ProgressExchangeDeadline>,
        cancel: CancellationToken,
    ) -> ProgressSnapshotRound {
        let round = self
            .reader
            .clone()
            .read_snapshot(peer, expected, deadline, cancel);
        Box::pin(async move {
            self.counts.enter(peer);
            self.counts.reads.fetch_add(1, Ordering::SeqCst);
            let result = round.await;
            self.counts.end(peer);
            result
        })
    }
}
struct Terminal {
    producer: TransferProgressProducer,
    counts: Arc<Counts>,
}
#[async_trait]
impl ProgressTerminalRoundPort for Terminal {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.counts.enter(*peer);
        let reported = self
            .producer
            .report_settled(peer, expected, result, deadline, cancel)
            .await;
        self.counts.end(*peer);
        if reported.is_ok() {
            self.counts.ack.notify_one();
        }
        reported
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Complete,
    Expired,
    Cancelled,
}
async fn scenario(mode: Mode) -> anyhow::Result<()> {
    let a = DeviceId::new("snapshot-source-a");
    let b = DeviceId::new("terminal-source-b");
    let first = Fixture::new_as(a, 4, 4, TRANSFER_PROGRESS_ALPN).await?;
    let remote = Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![TRANSFER_PROGRESS_ALPN.to_vec()])
        .bind()
        .await?;
    let second = Fixture::connect_as(
        first.client.clone(),
        remote,
        4,
        4,
        TRANSFER_PROGRESS_ALPN,
        b,
    )
    .await?;
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let authority = Arc::new(Authority {
        peer: b,
        claimed: AtomicBool::new(false),
    });
    let counts = Arc::new(Counts {
        active: Mutex::new(HashSet::new()),
        peak: AtomicUsize::new(0),
        reads: AtomicUsize::new(0),
        ack: Notify::new(),
    });
    let (service, worker) = ProgressExchangeService::new(
        clock.clone(),
        authority.clone(),
        Arc::new(Terminal {
            producer: TransferProgressProducer::new(
                second.outbound.clone(),
                Arc::new(TransferProgressWire),
                authority,
                Arc::new(UnusedSnapshotProvider),
                clock.clone(),
            ),
            counts: counts.clone(),
        }),
        Arc::new(Snapshot {
            reader: Arc::new(TransferProgressSnapshotReader::new(
                first.outbound.clone(),
                Arc::new(TransferProgressWire),
            )),
            counts: counts.clone(),
        }),
        Arc::new(UnusedLive),
    );
    let owner = Arc::new(TransferProgressReceiver::new(
        second.receiving.peer(),
        SOURCE,
        task().items.to_vec(),
        Arc::new(SystemElapsedClock),
    )?);
    let stop = CancellationToken::new();
    let running = tokio::spawn(worker.run(stop.clone()));
    let query_cancel = CancellationToken::new();
    let query = {
        let service = service.clone();
        let cancel = query_cancel.clone();
        tokio::spawn(async move {
            service
                .read_snapshot(a, task().into(), SOURCE, &cancel)
                .await
        })
    };
    let mut incoming = first.receiving.accept_bi().await?;
    let Message::Query {
        challenge,
        source_entry,
        expected_attempt,
        offset,
        snapshot_revision,
    } = read_frame(&mut incoming.receive).await?.message
    else {
        panic!("expected actual shared query");
    };
    assert_eq!(source_entry.0, SOURCE.bytes());
    assert_eq!(
        expected_attempt,
        Some(WireId(task().descriptor.attempt.bytes()))
    );
    assert_eq!(offset, 0);
    assert_eq!(snapshot_revision, None);
    assert!(
        matches!(incoming.receive.read_u8().await,Err(error) if error.kind()==std::io::ErrorKind::UnexpectedEof)
    );
    // 暂停真实成员准入port，不是延迟native IO future或服务层fake读取。
    if mode != Mode::Complete {
        first
            .membership
            .read_gate
            .armed
            .store(true, Ordering::SeqCst);
    }
    let descriptor = task().descriptor;
    write_frame(
        &mut incoming.send,
        &Envelope {
            trace_context: None,
            message: Message::QueryResult {
                challenge,
                result: QueryResult::Current {
                    descriptor: ProducerDescriptor {
                        producer: WireId(descriptor.producer.bytes()),
                        owner_revision: descriptor.revision,
                        scope: TaskScope {
                            source_entry: WireId(SOURCE.bytes()),
                            attempt: WireId(descriptor.attempt.bytes()),
                        },
                        item_count: 1,
                    },
                    page: SnapshotPage {
                        snapshot_revision: 9,
                        offset: 0,
                        total: 1,
                        items: vec![ItemState::Active(ActiveProgress {
                            item: LogicalItem {
                                kind: ItemKind::File,
                                index: 0,
                            },
                            round: 0,
                            sequence: 1,
                            bytes: 0,
                            total: None,
                            phase: Phase::Waiting,
                        })],
                    },
                },
            },
        },
    )
    .await?;
    incoming.send.finish()?;
    if mode != Mode::Complete {
        first.membership.read_gate.entered.notified().await;
        match mode {
            Mode::Expired => clock.0.store(6, Ordering::SeqCst),
            Mode::Cancelled => query_cancel.cancel(),
            _ => {}
        }
    }
    let reply = timeout(Duration::from_secs(2), query).await??;
    match mode {
        Mode::Complete => assert!(
            matches!(&*reply,Ok(ProgressSnapshotRead::Complete(view)) if view.items.len()==1 && view.revision==9)
        ),
        Mode::Expired => assert!(matches!(
            &*reply,
            Err(ProgressReceiveError::Connection(
                ConnectivityError::DeadlineExpired
            ))
        )),
        Mode::Cancelled => assert!(matches!(
            &*reply,
            Err(ProgressReceiveError::Connection(ConnectivityError::Stopped))
        )),
    }
    if mode != Mode::Complete {
        assert!(counts.active.lock().unwrap().contains(&a));
        let repeated = timeout(
            Duration::from_secs(2),
            service.read_snapshot(a, task().into(), SOURCE, &CancellationToken::new()),
        )
        .await?;
        assert!(matches!(&*repeated, Err(_)));
        assert_eq!(counts.reads.load(Ordering::SeqCst), 1);
    }
    let receive = {
        let connection = second.receiving.clone();
        let owner = owner.clone();
        tokio::spawn(async move {
            for _ in 0..2 {
                TransferProgressInbound::serve(
                    connection.accept_bi().await?,
                    &connection,
                    1,
                    progress_budget(),
                    &CancellationToken::new(),
                    owner.as_ref(),
                    &UnusedSnapshotProvider,
                )
                .await?;
            }
            Ok::<_, ProgressReceiveError>(())
        })
    };
    let settled = ProgressTerminal {
        item: task().items[0],
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 0,
        total: Some(0),
        outcome: ProgressTerminalOutcome::Completed,
    };
    assert!(matches!(
        service.submit_settled(b, SOURCE, descriptor.attempt, settled)?,
        ProgressSubmission::Retained
    ));
    timeout(Duration::from_secs(2), counts.ack.notified()).await?;
    receive.await??;
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Terminal(settled))
    );
    assert_eq!(counts.reads.load(Ordering::SeqCst), 1);
    stop.cancel();
    if mode != Mode::Complete {
        assert!(!running.is_finished());
        assert_eq!(counts.active.lock().unwrap().len(), 1);
        first.membership.read_gate.released.add_permits(1);
    }
    timeout(Duration::from_secs(2), running).await??;
    assert!(counts.active.lock().unwrap().is_empty());
    assert!(counts.peak.load(Ordering::SeqCst) <= 4);
    if mode != Mode::Complete {
        assert_eq!(counts.peak.load(Ordering::SeqCst), 2);
        assert!(matches!(&*reply, Err(_)));
    }
    drop(incoming);
    first.close().await?;
    second.close().await?;

    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn real_shared_query_completion_expiry_and_cancel_keep_actual_admission_work_until_it_ends(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for mode in [Mode::Complete, Mode::Expired, Mode::Cancelled] {
            scenario(mode).await?;
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
