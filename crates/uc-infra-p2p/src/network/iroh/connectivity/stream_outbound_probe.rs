//! Application一次完整终态动作：真实接收/输出后发送，错误ACK和旧attempt不得成功。
use super::progress_probe::{progress_budget, Fixture};
use crate::network::iroh::blob_exchange::IrohBlobExchangeAdapter;
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use anyhow::{bail, ensure};
use async_trait::async_trait;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::ALPN as BLOBS_ALPN;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangePort, BlobReadStorePort, BlobValidatedProgress, ConnectionLease,
    ConnectivityError, ElapsedClockError, ElapsedClockPort, ProgressBound, ProgressClaimRefusal,
    ProgressContactNotice, ProgressExchangeDeadline, ProgressReceiveError, ProgressReceiverPort,
    ProgressSourceTask, ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalNotice,
    ProgressTerminalRoundPort, ProgressUpdateNotice, ProgressWirePort, StreamLease,
    TransferProgressProducer, TransferProgressReceiver,
};
use uc_core::file_transfer::ProgressItemState;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_core::ports::blob::BlobDigest;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, Envelope, Message, TRANSFER_PROGRESS_ALPN,
};

const SOURCE: ProgressIdentity = ProgressIdentity::new([1; 16]);
const ATTEMPT: ProgressIdentity = ProgressIdentity::new([3; 16]);
const BUDGET: Duration = Duration::from_secs(5);
struct Authority(AtomicU64, Mutex<HashSet<(ProgressIdentity, ProgressItem)>>);
impl Default for Authority {
    fn default() -> Self {
        Self(AtomicU64::new(1), Mutex::new(HashSet::new()))
    }
}
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(&self, _: &DeviceId, source: ProgressIdentity) -> Option<ProgressSourceTask> {
        if source != SOURCE {
            return None;
        }
        let revision = self.0.load(Ordering::SeqCst);
        let attempt = if revision == 1 {
            ATTEMPT
        } else {
            ProgressIdentity::new([4; 16])
        };
        Some(ProgressSourceTask {
            descriptor: ProgressTaskDescriptor {
                producer: ProgressIdentity::new([2; 16]),
                revision,
                source: SOURCE,
                attempt,
                item_count: 1,
            },
            items: Arc::from([item()]),
            notification_closed: Arc::new(
                self.1
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(owned, _)| *owned == attempt)
                    .map(|(_, item)| *item)
                    .collect(),
            ),
        })
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        let task = self
            .current_task(peer, source)
            .ok_or(ProgressClaimRefusal::NotRetained)?;
        if task.descriptor.attempt != attempt {
            return Err(ProgressClaimRefusal::Superseded);
        }
        if !task.items.contains(&item) {
            return Err(ProgressClaimRefusal::InvalidItem);
        }
        if !self.1.lock().unwrap().insert((attempt, item)) {
            return Err(ProgressClaimRefusal::AlreadyObserved);
        }
        Ok(task)
    }
}
fn item() -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index: 0,
    }
}
fn terminal(bytes: u64) -> ProgressTerminal {
    ProgressTerminal {
        item: item(),
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes,
        total: Some(bytes),
        outcome: ProgressTerminalOutcome::Completed,
    }
}
fn producer(fixture: &Fixture) -> TransferProgressProducer {
    TransferProgressProducer::new(
        fixture.outbound.clone(),
        Arc::new(TransferProgressWire),
        Arc::new(Authority::default()),
        Arc::new(SystemElapsedClock),
    )
}
fn owner(fixture: &Fixture) -> anyhow::Result<TransferProgressReceiver> {
    Ok(TransferProgressReceiver::new(
        fixture.receiving.peer(),
        SOURCE,
        vec![item()],
        Arc::new(SystemElapsedClock),
    )?)
}
async fn serve_registration(
    fixture: &Fixture,
    owner: &TransferProgressReceiver,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    TransferProgressInbound::serve(
        fixture.receiving.accept_bi().await?,
        &fixture.receiving,
        1,
        progress_budget(),
        cancel,
        owner,
    )
    .await?;
    Ok(())
}
struct Validated;
impl BlobValidatedProgress for Validated {
    fn validated_through(&self, _: u64, _: u64) -> Result<(), BlobExchangeError> {
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_file_single_action_registers_sends_and_observes_exact_adopted_ack(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let temp = tempfile::tempdir()?;
        let source = FsStore::load(temp.path().join("source")).await?;
        let destination = FsStore::load(temp.path().join("destination")).await?;
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let blobs = Fixture::connect(
            fixture.client.clone(),
            fixture.server.clone(),
            4,
            4,
            BLOBS_ALPN,
        )
        .await?;
        let bytes = vec![82; 256 * 1024];
        let tag = source.blobs().add_bytes(bytes.clone()).await?;
        let digest = BlobDigest::from_bytes(*tag.hash.as_bytes());
        let store = IrohBlobExchangeAdapter::new(destination.clone());
        let provider = IrohBlobExchangeAdapter::new(source.clone());
        let stop = CancellationToken::new();
        let get = store.receive(
            blobs.sending.open_bi(Instant::now() + BUDGET).await?,
            digest,
            stop.clone(),
            Arc::new(Validated),
        );
        let provide = async {
            provider
                .provide(
                    blobs.receiving.accept_bi().await?,
                    Instant::now() + BUDGET,
                    stop.clone(),
                )
                .await
        };
        let (received, provided) = tokio::join!(get, provide);
        provided?;
        let received = received?;
        let output = temp.path().join("output");
        let exported = store.export(received.digest, &output).await?;
        assert_eq!(tokio::fs::read(&output).await?, bytes);
        let owner = owner(&fixture)?;
        let producer = producer(&fixture);
        let peer = fixture.sending.peer();
        let budget = progress_budget();
        let serve = async {
            serve_registration(&fixture, &owner, &stop).await?;
            TransferProgressInbound::serve(
                fixture.receiving.accept_bi().await?,
                &fixture.receiving,
                1,
                progress_budget(),
                &stop,
                &owner,
            )
            .await?;
            anyhow::Ok(())
        };
        let (reported, served) = tokio::join!(
            producer.report_settled(
                &peer,
                ProgressTaskDescriptor {
                    attempt: ATTEMPT,
                    ..Authority::default()
                        .current_task(&peer, SOURCE)
                        .unwrap()
                        .descriptor
                },
                terminal(exported),
                &budget,
                &stop
            ),
            serve
        );
        served?;
        reported?;
        assert_eq!(
            owner.snapshot().items[0].1,
            Some(ProgressItemState::Terminal(terminal(exported)))
        );
        assert_eq!(tokio::fs::read(&output).await?, bytes);
        drop(received);
        drop(tag);
        blobs.close().await?;
        fixture.close().await?;
        source.shutdown().await?;
        destination.shutdown().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_attempt_is_refused_before_any_registration_stream() -> anyhow::Result<()> {
    let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
    let producer = producer(&fixture);
    let budget = progress_budget();
    let result = producer
        .report_settled(
            &fixture.sending.peer(),
            ProgressTaskDescriptor {
                attempt: ProgressIdentity::new([9; 16]),
                ..Authority::default()
                    .current_task(&fixture.sending.peer(), SOURCE)
                    .unwrap()
                    .descriptor
            },
            terminal(10),
            &budget,
            &CancellationToken::new(),
        )
        .await;
    assert!(matches!(result, Err(ProgressReceiveError::Stale)));
    assert!(
        timeout(Duration::from_millis(30), fixture.receiving.accept_bi())
            .await
            .is_err()
    );
    fixture.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_ack_or_trailing_bytes_do_not_report_success() -> anyhow::Result<()> {
    for trailing in [false, true] {
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let owner = owner(&fixture)?;
        let producer = producer(&fixture);
        let stop = CancellationToken::new();
        let peer = fixture.sending.peer();
        let budget = progress_budget();
        let serve = async {
            serve_registration(&fixture, &owner, &stop).await?;
            let mut stream = fixture.receiving.accept_bi().await?;
            let Message::Terminal {
                binding,
                scope,
                owner_revision,
                mut result,
            } = read_frame(&mut stream.receive).await?.message
            else {
                bail!("expected terminal")
            };
            let adopted = owner
                .adopt_terminal(
                    &fixture.receiving.peer(),
                    1,
                    ProgressTerminalNotice {
                        binding: ProgressIdentity::new(binding.0),
                        source: ProgressIdentity::new(scope.source_entry.0),
                        attempt: ProgressIdentity::new(scope.attempt.0),
                        revision: owner_revision,
                        result: terminal(10),
                    },
                )
                .await?;
            ensure!(
                adopted.result.bytes == result.bytes,
                "actual terminal adopted"
            );
            if !trailing {
                result.result_revision += 1;
            }
            write_frame(
                &mut stream.send,
                &Envelope {
                    message: Message::TerminalAck {
                        binding,
                        scope,
                        owner_revision,
                        result,
                    },
                    trace_context: None,
                },
            )
            .await?;
            if trailing {
                stream.send.write_all(&[0]).await?;
            }
            stream.send.finish()?;
            anyhow::Ok(())
        };
        let (reported, served) = timeout(Duration::from_secs(15), async {
            tokio::join!(
                producer.report_settled(
                    &peer,
                    ProgressTaskDescriptor {
                        attempt: ATTEMPT,
                        ..Authority::default()
                            .current_task(&peer, SOURCE)
                            .unwrap()
                            .descriptor
                    },
                    terminal(10),
                    &budget,
                    &stop
                ),
                serve
            )
        })
        .await?;
        served?;
        if trailing {
            assert!(matches!(reported, Err(ProgressReceiveError::Exchange(_))));
        } else {
            assert!(matches!(reported, Err(ProgressReceiveError::Conflict)));
        }
        assert_eq!(
            owner.snapshot().items[0].1,
            Some(ProgressItemState::Terminal(terminal(10)))
        );
        fixture.close().await?;
    }
    Ok(())
}

struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
struct ConsumedBudget {
    clock: Arc<Clock>,
}
#[async_trait]
impl ProgressWirePort for ConsumedBudget {
    async fn contact(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressContactNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        TransferProgressWire
            .contact(stream, connection, notice, deadline, cancel)
            .await
    }
    async fn update(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressUpdateNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        TransferProgressWire
            .update(stream, connection, notice, deadline, cancel)
            .await
    }
    async fn register(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        authority: &dyn ProgressTaskAuthorityPort,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let bound = TransferProgressWire
            .register(stream, connection, source, authority, deadline, cancel)
            .await?;
        self.clock.0.store(4, Ordering::SeqCst);
        Ok(bound)
    }
    async fn terminal(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressTerminalNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        // 登记已消耗4秒，恢复后到第6秒；不能为终态重建另一个五秒预算。
        self.clock.0.store(6, Ordering::SeqCst);
        TransferProgressWire
            .terminal(stream, connection, notice, deadline, cancel)
            .await
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_and_terminal_share_original_elapsed_budget() -> anyhow::Result<()> {
    let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
    let owner = owner(&fixture)?;
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let producer = TransferProgressProducer::new(
        fixture.outbound.clone(),
        Arc::new(ConsumedBudget {
            clock: clock.clone(),
        }),
        Arc::new(Authority::default()),
        clock.clone(),
    );
    let deadline = ProgressExchangeDeadline::new(clock.clone(), BUDGET)?;
    let stop = CancellationToken::new();
    let peer = fixture.sending.peer();
    let (reported, served) = timeout(Duration::from_secs(15), async {
        tokio::join!(
            producer.report_settled(
                &peer,
                ProgressTaskDescriptor {
                    attempt: ATTEMPT,
                    ..Authority::default()
                        .current_task(&peer, SOURCE)
                        .unwrap()
                        .descriptor
                },
                terminal(10),
                &deadline,
                &stop
            ),
            serve_registration(&fixture, &owner, &stop)
        )
    })
    .await?;
    served?;
    assert!(matches!(
        reported,
        Err(ProgressReceiveError::Connection(
            ConnectivityError::DeadlineExpired
        ))
    ));
    assert!(owner.snapshot().items[0].1.is_none());
    // 已打开的流可因reset被对端接受；没有合法终态帧才是业务未发送的证据。
    if let Ok(incoming) = timeout(Duration::from_millis(30), fixture.receiving.accept_bi()).await {
        let mut incoming = incoming?;
        assert!(timeout(BUDGET, read_frame(&mut incoming.receive))
            .await?
            .is_err());
    }

    fixture.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_attempt_during_challenge_cannot_relabel_an_old_settled_result() -> anyhow::Result<()> {
    let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
    let authority = Arc::new(Authority::default());
    let owner = owner(&fixture)?;
    let producer = TransferProgressProducer::new(
        fixture.outbound.clone(),
        Arc::new(TransferProgressWire),
        authority.clone(),
        Arc::new(SystemElapsedClock),
    );
    let stop = CancellationToken::new();
    let peer = fixture.sending.peer();
    let budget = progress_budget();
    let serving = async {
        let incoming = fixture.receiving.accept_bi().await?;
        // 原动作预检已经发生；Challenge读取后来接受的尝试。
        authority.0.store(2, Ordering::SeqCst);
        TransferProgressInbound::serve(
            incoming,
            &fixture.receiving,
            1,
            progress_budget(),
            &stop,
            &owner,
        )
        .await?;
        anyhow::Ok(())
    };
    let (reported, served) = timeout(Duration::from_secs(15), async {
        tokio::join!(
            producer.report_settled(
                &peer,
                ProgressTaskDescriptor {
                    attempt: ATTEMPT,
                    ..Authority::default()
                        .current_task(&peer, SOURCE)
                        .unwrap()
                        .descriptor
                },
                terminal(10),
                &budget,
                &stop
            ),
            serving
        )
    })
    .await?;
    served?;
    assert!(matches!(reported, Err(ProgressReceiveError::Stale)));
    let snapshot = owner.snapshot();
    assert_eq!(
        snapshot.bound.unwrap().descriptor.attempt,
        ProgressIdentity::new([4; 16])
    );
    assert!(snapshot.items[0].1.is_none());
    assert!(
        timeout(Duration::from_millis(30), fixture.receiving.accept_bi())
            .await
            .is_err()
    );
    fixture.close().await?;
    Ok(())
}

#[path = "stream_live_probe.rs"]
mod live_probe;

#[path = "stream_projection_probe.rs"]
mod projection_probe;
