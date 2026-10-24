//! 实际 BAO 接收、文件导出、Application 采用与业务 ACK；生产接收交接仍另验。
use super::progress_probe::{progress_budget, Fixture};
use crate::network::iroh::blob_exchange::IrohBlobExchangeAdapter;
use crate::network::iroh::transfer_progress_v2::TransferProgressInbound;
use anyhow::{bail, ensure};
use async_trait::async_trait;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::ALPN as BLOBS_ALPN;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, Semaphore};
use tokio::time::{sleep_until, timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    BlobExchangeError, BlobExchangePort, BlobReadStorePort, BlobValidatedProgress,
    CurrentProgressTaskPort, ProgressBound, ProgressContactNotice, ProgressReceiveError,
    ProgressReceiverPort, ProgressTaskDescriptor, ProgressTerminalNotice, ProgressUpdateNotice,
    StreamLease, TransferProgressReceiver,
};
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemKind};
use uc_core::ids::DeviceId;
use uc_core::ports::blob::BlobDigest;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, Envelope, ItemKind, LogicalItem, Message, ProducerDescriptor,
    TaskScope, TerminalOutcome, TerminalResult, WireId, TRANSFER_PROGRESS_ALPN,
};

const BUDGET: Duration = Duration::from_secs(5);
const SOURCE: WireId = WireId([1; 16]);
struct Validated;
impl BlobValidatedProgress for Validated {
    fn validated_through(&self, _: u64, _: u64) -> Result<(), BlobExchangeError> {
        Ok(())
    }
}
struct PausedAdoption {
    owner: Arc<TransferProgressReceiver>,
    entered: Notify,
    release: Semaphore,
}
#[async_trait]
impl ProgressReceiverPort for PausedAdoption {
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.owner.adopt_contact(peer, generation, notice).await
    }
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError> {
        self.owner.adopt_update(peer, generation, notice).await
    }
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        self.owner
            .register(peer, generation, source, exchange)
            .await
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        self.entered.notify_one();
        self.release
            .acquire()
            .await
            .expect("test releases actual adoption")
            .forget();
        self.owner.adopt_terminal(peer, generation, notice).await
    }
}

async fn register(
    fixture: &Fixture,
    owner: &dyn ProgressReceiverPort,
) -> anyhow::Result<ProgressBound> {
    let mut sending = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
    write_frame(
        &mut sending.send,
        &Envelope {
            message: Message::Register {
                source_entry: SOURCE,
            },
            trace_context: None,
        },
    )
    .await?;
    let incoming = fixture.receiving.accept_bi().await?;
    let client = async {
        let Message::Challenge { nonce } = read_frame(&mut sending.receive).await?.message else {
            bail!("expected challenge");
        };
        write_frame(
            &mut sending.send,
            &Envelope {
                message: Message::CurrentDescriptor {
                    challenge: nonce,
                    descriptor: ProducerDescriptor {
                        producer: WireId([2; 16]),
                        owner_revision: 8,
                        scope: TaskScope {
                            source_entry: SOURCE,
                            attempt: WireId([3; 16]),
                        },
                        item_count: 1,
                    },
                },
                trace_context: None,
            },
        )
        .await?;
        sending.send.finish()?;
        let Message::Bound {
            challenge,
            binding,
            descriptor,
        } = read_frame(&mut sending.receive).await?.message
        else {
            bail!("expected bound");
        };
        ensure!(challenge == nonce, "challenge must match");
        Ok::<_, anyhow::Error>(ProgressBound {
            challenge: ProgressIdentity::new(challenge.0),
            binding: ProgressIdentity::new(binding.0),
            descriptor: ProgressTaskDescriptor {
                producer: ProgressIdentity::new(descriptor.producer.0),
                revision: descriptor.owner_revision,
                source: ProgressIdentity::new(descriptor.scope.source_entry.0),
                attempt: ProgressIdentity::new(descriptor.scope.attempt.0),
                item_count: descriptor.item_count,
            },
        })
    };
    let cancel = CancellationToken::new();
    let (bound, served) = tokio::join!(
        client,
        TransferProgressInbound::serve(
            incoming,
            &fixture.receiving,
            1,
            progress_budget(),
            &cancel,
            owner
        )
    );
    served?;
    bound
}
fn result(bound: ProgressBound, bytes: u64) -> Message {
    Message::Terminal {
        binding: WireId(bound.binding.bytes()),
        scope: TaskScope {
            source_entry: SOURCE,
            attempt: WireId(bound.descriptor.attempt.bytes()),
        },
        owner_revision: bound.descriptor.revision,
        result: TerminalResult {
            item: LogicalItem {
                kind: ItemKind::File,
                index: 0,
            },
            round: 0,
            sequence: 2,
            result_revision: 1,
            bytes,
            total: Some(bytes),
            outcome: TerminalOutcome::Completed,
        },
    }
}
fn expected_ack(message: &Message) -> Message {
    let Message::Terminal {
        binding,
        scope,
        owner_revision,
        result,
    } = message
    else {
        panic!("test uses terminal");
    };
    Message::TerminalAck {
        binding: *binding,
        scope: scope.clone(),
        owner_revision: *owner_revision,
        result: result.clone(),
    }
}
async fn send(fixture: &Fixture, message: Message) -> anyhow::Result<(StreamLease, StreamLease)> {
    let mut stream = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
    write_frame(
        &mut stream.send,
        &Envelope {
            message,
            trace_context: None,
        },
    )
    .await?;
    stream.send.finish()?;
    let incoming = fixture.receiving.accept_bi().await?;
    Ok((stream, incoming))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exported_file_terminal_waits_for_application_adoption_before_exact_ack(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), prove(WaitEnd::None)).await?
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_ack_wait_preserves_actual_adoption_and_duplicate_can_ack_later(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), prove(WaitEnd::Cancel)).await?
}
enum WaitEnd {
    None,
    Cancel,
    Deadline,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_network_deadline_waits_for_actual_adoption_and_does_not_roll_back_result(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), prove(WaitEnd::Deadline)).await?
}

async fn prove(interruption: WaitEnd) -> anyhow::Result<()> {
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
    let bytes = vec![91; 256 * 1024];
    let tag = source.blobs().add_bytes(bytes.clone()).await?;
    let digest = BlobDigest::from_bytes(*tag.hash.as_bytes());
    let downloading = blobs.sending.open_bi(Instant::now() + BUDGET).await?;
    let store = IrohBlobExchangeAdapter::new(destination.clone());
    let provider = IrohBlobExchangeAdapter::new(source.clone());
    let stop = CancellationToken::new();
    let get = store.receive(downloading, digest, stop.clone(), Arc::new(Validated));
    let provide = async {
        provider
            .provide(
                blobs.receiving.accept_bi().await?,
                Instant::now() + BUDGET,
                stop,
            )
            .await
    };
    let (received, provided) = tokio::join!(get, provide);
    provided?;
    let received = received?;
    let owner = Arc::new(TransferProgressReceiver::new(
        fixture.receiving.peer(),
        ProgressIdentity::new(SOURCE.0),
        vec![ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        }],
        Arc::new(SystemElapsedClock),
    )?);
    let gate = Arc::new(PausedAdoption {
        owner: owner.clone(),
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    let bound = register(&fixture, gate.as_ref()).await?;
    assert!(owner.snapshot().items[0].1.is_none()); // 网络接收完成仍不是输出成果。
    let output = temp.path().join("output");
    let exported = store.export(received.digest, &output).await?;
    assert_eq!(exported, bytes.len() as u64);
    assert_eq!(tokio::fs::read(&output).await?, bytes);
    let mut message = result(bound, exported);
    let ack = expected_ack(&message);
    let (mut sending, incoming) = send(&fixture, message.clone()).await?;
    let connection = fixture.receiving.clone();
    let pause = gate.clone();
    let cancellation = CancellationToken::new();
    let stop_ack = cancellation.clone();
    let budget = progress_budget();
    let deadline = budget.instant();
    let serving = tokio::spawn(async move {
        TransferProgressInbound::serve(incoming, &connection, 1, budget, &stop_ack, pause.as_ref())
            .await
    });
    timeout(BUDGET, gate.entered.notified()).await?;
    let mut reading = Box::pin(read_frame(&mut sending.receive));
    assert!(timeout(Duration::from_millis(30), &mut reading)
        .await
        .is_err());
    assert!(owner.snapshot().items[0].1.is_none()); // FIN 已到，但尚未采用，不得 ACK。
    if matches!(interruption, WaitEnd::Cancel) {
        cancellation.cancel();
    }
    if matches!(interruption, WaitEnd::Deadline) {
        sleep_until(deadline).await;
    }
    assert!(!serving.is_finished());
    gate.release.add_permits(1);
    if !matches!(interruption, WaitEnd::None) {
        assert!(timeout(BUDGET, serving).await??.is_err());
        assert!(timeout(BUDGET, reading).await?.is_err());
    } else {
        assert_eq!(timeout(BUDGET, reading).await??.message, ack);
        timeout(BUDGET, serving).await???;
    }
    assert!(owner.snapshot().items[0].1.is_some());
    let version = owner.snapshot().version;
    let (mut duplicate, incoming) = send(&fixture, message.clone()).await?;
    let stop = CancellationToken::new();
    let (served, reply) = tokio::join!(
        TransferProgressInbound::serve(
            incoming,
            &fixture.receiving,
            1,
            progress_budget(),
            &stop,
            owner.as_ref()
        ),
        read_frame(&mut duplicate.receive)
    );
    served?;
    assert_eq!(reply?.message, ack);
    assert_eq!(owner.snapshot().version, version);
    drop(duplicate);
    drop(sending);
    // 合法帧后的额外字节必须在采用前拒收；不能先 ACK 再发现尾随输入。
    let mut trailing = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
    write_frame(
        &mut trailing.send,
        &Envelope {
            message: message.clone(),
            trace_context: None,
        },
    )
    .await?;
    trailing.send.write_all(&[0]).await?;
    trailing.send.finish()?;
    let incoming = fixture.receiving.accept_bi().await?;
    let (served, reply) = tokio::join!(
        TransferProgressInbound::serve(
            incoming,
            &fixture.receiving,
            1,
            progress_budget(),
            &stop,
            owner.as_ref()
        ),
        read_frame(&mut trailing.receive)
    );
    assert!(matches!(served, Err(ProgressReceiveError::Exchange(_))));
    assert!(reply.is_err());
    assert_eq!(owner.snapshot().version, version);
    drop(trailing);
    // 冲突终态不得回 ACK，也不能改变已采用事实或成功文件。
    let Message::Terminal {
        result: conflicting,
        ..
    } = &mut message
    else {
        bail!("terminal expected");
    };
    conflicting.bytes -= 1;
    let (mut rejected, incoming) = send(&fixture, message).await?;
    let (served, reply) = tokio::join!(
        TransferProgressInbound::serve(
            incoming,
            &fixture.receiving,
            1,
            progress_budget(),
            &stop,
            owner.as_ref()
        ),
        read_frame(&mut rejected.receive)
    );
    assert!(matches!(served, Err(ProgressReceiveError::Conflict)));
    assert!(reply.is_err());
    assert_eq!(owner.snapshot().version, version);
    assert_eq!(tokio::fs::read(&output).await?, bytes);
    drop(rejected);
    drop(received);
    drop(tag);
    blobs.close().await?;
    fixture.close().await?;
    source.shutdown().await?;
    destination.shutdown().await?;
    Ok(())
}
