//! 同一组合测试中读取实际接收use case进程的当前权威，再通过真实Iroh流登记。
use super::progress_probe::{progress_budget, Fixture};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use anyhow::ensure;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant as WallInstant};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::sync::{Notify, Semaphore};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    CurrentProgressTaskPort, ProgressBound, ProgressClaimRefusal, ProgressExchangeDeadline,
    ProgressReceiveError, ProgressReceiverPort, ProgressSourceTask, ProgressSubmission,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalDelivery,
    ProgressTerminalNotice, ProgressTerminalRoundPort, TransferProgressProducer,
    TransferProgressReceiver,
};
use uc_core::file_transfer::{
    ProgressFailureReason, ProgressIdentity, ProgressItem, ProgressItemKind, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, Envelope, ItemKind, LogicalItem, Message, TaskScope, TerminalOutcome,
    TerminalResult, WireId, TRANSFER_PROGRESS_ALPN,
};
use uc_sync_protocol::transfer_progress::LEGACY_TRANSFER_PROGRESS_ALPN;

const BUDGET: Duration = Duration::from_secs(5);
const SOURCE: ProgressIdentity = ProgressIdentity::new([1; 16]);
struct AuthorityProcess {
    process: Child,
    authority: Arc<Authority>,
    _directory: TempDir,
}
impl AuthorityProcess {
    fn start() -> anyhow::Result<Self> {
        let executable = std::env::var_os("UC_APPLICATION_TEST_BINARY").ok_or_else(|| {
            anyhow::anyhow!("proof driver must supply current Application binary")
        })?;
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("authority-address");
        let process=Command::new(executable).args(["--ignored","--exact","clipboard::sync::apply_inbound::tests::progress_authority::registration_authority_worker","--nocapture"])
            .env("UC_PROGRESS_AUTHORITY_WORKER_MARKER",&marker).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn()?;
        let mut process = Self {
            process,
            authority: Arc::new(Authority("127.0.0.1:1".parse()?)),
            _directory: directory,
        };
        let deadline = WallInstant::now() + BUDGET;
        loop {
            if let Ok(address) = std::fs::read_to_string(&marker) {
                if let Ok(address) = address.parse() {
                    process.authority = Arc::new(Authority(address));
                    break;
                }
            }
            ensure!(
                process.process.try_wait()?.is_none(),
                "Application worker terminated before readiness"
            );
            ensure!(
                WallInstant::now() < deadline,
                "Application worker readiness expired"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        ensure!(
            process
                .authority
                .current_task(&DeviceId::new("progress-source"), SOURCE)
                .is_none(),
            "fresh process cannot restore prior association"
        );
        Ok(process)
    }
    fn accept(&self, directory: bool) -> anyhow::Result<ProgressSourceTask> {
        ensure!(
            self.authority
                .request(json!({"op":"accept","directory":directory}))?["accepted"]
                == true,
            "actual receive fixture must accept"
        );
        self.authority
            .current_task(&DeviceId::new("progress-source"), SOURCE)
            .ok_or_else(|| anyhow::anyhow!("accepted receive must supply current authority"))
    }
    fn stop(&mut self) -> anyhow::Result<()> {
        self.process.kill()?;
        self.process.wait()?;
        Ok(())
    }
}
impl Drop for AuthorityProcess {
    fn drop(&mut self) {
        if self.process.try_wait().ok().flatten().is_none() {
            let _ = self.process.kill();
        }
        let _ = self.process.wait();
    }
}
struct Authority(SocketAddr);
impl Authority {
    fn request(&self, request: Value) -> anyhow::Result<Value> {
        let mut connection = TcpStream::connect_timeout(&self.0, BUDGET)?;
        connection.set_read_timeout(Some(BUDGET))?;
        connection.set_write_timeout(Some(BUDGET))?;
        serde_json::to_writer(&mut connection, &request)?;
        connection.write_all(b"\n")?;
        let mut response = String::new();
        BufReader::new(connection).read_line(&mut response)?;
        Ok(serde_json::from_str(&response)?)
    }
}
fn decode_task(value: Value) -> Option<ProgressSourceTask> {
    if value.is_null() {
        return None;
    }
    let id =
        |field: &str| ProgressIdentity::new(serde_json::from_value(value[field].clone()).unwrap());
    Some(ProgressSourceTask {
        descriptor: ProgressTaskDescriptor {
            producer: id("producer"),
            revision: value["revision"].as_u64().unwrap(),
            source: id("source"),
            attempt: id("attempt"),
            item_count: value["item_count"].as_u64().unwrap() as u16,
        },
        items: value["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| ProgressItem {
                kind: match item["kind"].as_str().unwrap() {
                    "file" => ProgressItemKind::File,
                    "representation" => ProgressItemKind::Representation,
                    "directory" => ProgressItemKind::DirectoryUnit,
                    _ => panic!("invalid fixture kind"),
                },
                index: item["index"].as_u64().unwrap() as u16,
            })
            .collect::<Vec<_>>()
            .into(),
    })
}
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        decode_task(
            self.request(json!({"op":"current","peer":peer.as_str(),"source":source.bytes()}))
                .unwrap(),
        )
    }
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
        attempt: ProgressIdentity,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        let kind = match item.kind {
            ProgressItemKind::File => "file",
            ProgressItemKind::Representation => "representation",
            ProgressItemKind::DirectoryUnit => "directory",
        };
        let reply=self.request(json!({"op":"claim","peer":peer.as_str(),"source":source.bytes(),"attempt":attempt.bytes(),"kind":kind,"index":item.index})).unwrap();
        if !reply["task"].is_null() {
            return Ok(decode_task(reply["task"].clone()).unwrap());
        }
        Err(match reply["refusal"].as_str().unwrap() {
            "AlreadyObserved" => ProgressClaimRefusal::AlreadyObserved,
            "Superseded" => ProgressClaimRefusal::Superseded,
            "InvalidItem" => ProgressClaimRefusal::InvalidItem,
            "NotRetained" => ProgressClaimRefusal::NotRetained,
            _ => panic!("invalid synthetic claim outcome"),
        })
    }
}
struct Gate {
    before: bool,
    owner: Arc<TransferProgressReceiver>,
    armed: AtomicBool,
    entered: Notify,
    release: Semaphore,
}
struct HeldDescriptor<'a> {
    wire: &'a mut dyn CurrentProgressTaskPort,
    gate: &'a Gate,
}
#[async_trait]
impl CurrentProgressTaskPort for HeldDescriptor<'_> {
    async fn challenge(
        &mut self,
        nonce: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
        if self.gate.before {
            self.gate.entered.notify_one();
            self.gate.release.acquire().await.unwrap().forget();
        }
        let descriptor = self.wire.challenge(nonce).await?;
        if !self.gate.before {
            self.gate.entered.notify_one();
            self.gate.release.acquire().await.unwrap().forget();
        }
        Ok(descriptor)
    }
}
#[async_trait]
impl ProgressReceiverPort for Gate {
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.owner
                .register(
                    peer,
                    generation,
                    source,
                    &mut HeldDescriptor {
                        wire: exchange,
                        gate: self,
                    },
                )
                .await
        } else {
            self.owner
                .register(peer, generation, source, exchange)
                .await
        }
    }
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        self.owner.adopt_terminal(peer, generation, notice).await
    }
}
fn producer(fixture: &Fixture, authority: Arc<Authority>) -> Arc<TransferProgressProducer> {
    Arc::new(TransferProgressProducer::new(
        fixture.outbound.clone(),
        Arc::new(TransferProgressWire),
        authority,
        Arc::new(SystemElapsedClock),
    ))
}
async fn registration(
    fixture: &Fixture,
    producer: &TransferProgressProducer,
    owner: &dyn ProgressReceiverPort,
    generation: u64,
) -> anyhow::Result<ProgressBound> {
    let cancel = CancellationToken::new();
    let receive = async {
        let stream = fixture.receiving.accept_bi().await?;
        TransferProgressInbound::serve(
            stream,
            &fixture.receiving,
            generation,
            progress_budget(),
            &cancel,
            owner,
        )
        .await
    };
    let peer = fixture.sending.peer();
    let (registered, served) =
        tokio::join!(producer.register_current(&peer, SOURCE, &cancel), receive);
    served?;
    Ok(registered?)
}
async fn stale_terminal(
    fixture: &Fixture,
    owner: &TransferProgressReceiver,
    bound: ProgressBound,
    generation: u64,
) -> anyhow::Result<()> {
    let mut stream = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
    write_frame(
        &mut stream.send,
        &Envelope {
            message: Message::Terminal {
                binding: WireId(bound.binding.bytes()),
                scope: TaskScope {
                    source_entry: WireId(SOURCE.bytes()),
                    attempt: WireId(bound.descriptor.attempt.bytes()),
                },
                owner_revision: bound.descriptor.revision,
                result: TerminalResult {
                    item: LogicalItem {
                        kind: ItemKind::File,
                        index: 0,
                    },
                    round: 0,
                    sequence: 1,
                    result_revision: 1,
                    bytes: 10,
                    total: Some(10),
                    outcome: TerminalOutcome::Completed,
                },
            },
            trace_context: None,
        },
    )
    .await?;
    stream.send.finish()?;
    let incoming = fixture.receiving.accept_bi().await?;
    let cancel = CancellationToken::new();
    let (served, reply) = tokio::join!(
        TransferProgressInbound::serve(
            incoming,
            &fixture.receiving,
            generation,
            progress_budget(),
            &cancel,
            owner
        ),
        read_frame(&mut stream.receive)
    );
    ensure!(
        matches!(served, Err(ProgressReceiveError::Stale)),
        "old identity must be refused"
    );
    ensure!(reply.is_err(), "stale terminal cannot ACK");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn current_receive_authority_binds_over_real_iroh_and_old_descriptors_cannot_roll_back(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for directory in [false, true] {
            let worker = AuthorityProcess::start()?;
            let first = worker.accept(directory)?;
            let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
            let client = producer(&fixture, worker.authority.clone());
            let owner = Arc::new(TransferProgressReceiver::new(
                fixture.receiving.peer(),
                SOURCE,
                first.items.to_vec(),
            )?);
            let gate = Arc::new(Gate {
                before: false,
                owner: owner.clone(),
                armed: AtomicBool::new(false),
                entered: Notify::new(),
                release: Semaphore::new(0),
            });
            let old = registration(&fixture, client.as_ref(), gate.as_ref(), 1).await?;
            gate.armed.store(true, Ordering::SeqCst);
            let calling = client.clone();
            let peer = fixture.sending.peer();
            let old_call = tokio::spawn(async move {
                calling
                    .register_current(&peer, SOURCE, &CancellationToken::new())
                    .await
            });
            let incoming = fixture.receiving.accept_bi().await?;
            let connection = fixture.receiving.clone();
            let held = gate.clone();
            let old_serve = tokio::spawn(async move {
                TransferProgressInbound::serve(
                    incoming,
                    &connection,
                    1,
                    progress_budget(),
                    &CancellationToken::new(),
                    held.as_ref(),
                )
                .await
            });
            timeout(BUDGET, gate.entered.notified()).await?;
            let next = worker.accept(directory)?;
            ensure!(
                next.descriptor.attempt != first.descriptor.attempt
                    && next.descriptor.revision > first.descriptor.revision,
                "actual accepted attempt must advance authority"
            );
            let current = registration(&fixture, client.as_ref(), gate.as_ref(), 1).await?;
            ensure!(
                current.descriptor == next.descriptor,
                "wire must bind current actual authority"
            );
            gate.release.add_permits(1);
            ensure!(
                timeout(BUDGET, old_serve).await??.is_err(),
                "old descriptor cannot adopt"
            );
            ensure!(
                timeout(BUDGET, old_call).await??.is_err(),
                "old binding cannot activate"
            );
            if !directory {
                stale_terminal(&fixture, owner.as_ref(), old, 1).await?;
            }
            ensure!(
                owner.snapshot().bound.unwrap().binding == current.binding,
                "old exchange cannot replace current projection"
            );
            fixture.close().await?;
        }
        Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn actual_receiver_restart_new_connection_retires_old_producer_and_pending_stream(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let mut worker = AuthorityProcess::start()?;
        worker.accept(false)?;
        let first = worker.accept(false)?;
        let original = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let client = producer(&original, worker.authority.clone());
        let owner = Arc::new(TransferProgressReceiver::new(
            original.receiving.peer(),
            SOURCE,
            first.items.to_vec(),
        )?);
        let gate = Arc::new(Gate {
            before: false,
            owner: owner.clone(),
            armed: AtomicBool::new(false),
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        let old = registration(&original, client.as_ref(), gate.as_ref(), 1).await?;
        gate.armed.store(true, Ordering::SeqCst);
        let calling = client.clone();
        let peer = original.sending.peer();
        let old_call = tokio::spawn(async move {
            calling
                .register_current(&peer, SOURCE, &CancellationToken::new())
                .await
        });
        let incoming = original.receiving.accept_bi().await?;
        let connection = original.receiving.clone();
        let held = gate.clone();
        let old_serve = tokio::spawn(async move {
            TransferProgressInbound::serve(
                incoming,
                &connection,
                1,
                progress_budget(),
                &CancellationToken::new(),
                held.as_ref(),
            )
            .await
        });
        timeout(BUDGET, gate.entered.notified()).await?;
        worker.stop()?;
        let restarted = AuthorityProcess::start()?;
        let next = restarted.accept(false)?;
        ensure!(
            next.descriptor.producer != first.descriptor.producer
                && next.descriptor.revision == 1
                && next.descriptor.revision < first.descriptor.revision,
            "real restarted process must use a new producer"
        );
        let reconnect = Fixture::connect(
            original.client.clone(),
            original.server.clone(),
            4,
            4,
            TRANSFER_PROGRESS_ALPN,
        )
        .await?;
        let new_client = producer(&reconnect, restarted.authority.clone());
        let current = registration(&reconnect, new_client.as_ref(), gate.as_ref(), 2).await?;
        ensure!(
            current.descriptor == next.descriptor,
            "new connection must confirm restarted current owner"
        );
        gate.release.add_permits(1);
        ensure!(
            timeout(BUDGET, old_serve).await??.is_err(),
            "old connection descriptor retired"
        );
        ensure!(
            timeout(BUDGET, old_call).await??.is_err(),
            "old exchange cannot activate"
        );
        stale_terminal(&original, owner.as_ref(), old, 1).await?;
        ensure!(
            owner.snapshot().bound.unwrap().binding == current.binding,
            "late old producer cannot replace current binding"
        );
        reconnect.close().await?;
        original.close().await?;
        Ok(())
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn explicit_legacy_is_unsupported_but_corrupt_current_frame_keeps_exchange_failure(
) -> anyhow::Result<()> {
    let worker = AuthorityProcess::start()?;
    worker.accept(false)?;
    let legacy = Fixture::new(2, 2, LEGACY_TRANSFER_PROGRESS_ALPN).await?;
    let client = producer(&legacy, worker.authority.clone());
    ensure!(
        matches!(
            client
                .register_current(&legacy.sending.peer(), SOURCE, &CancellationToken::new())
                .await,
            Err(ProgressReceiveError::Unsupported)
        ),
        "only explicit legacy capability is unsupported"
    );
    legacy.close().await?;
    let current = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    let client = producer(&current, worker.authority.clone());
    let peer = current.sending.peer();
    let call = tokio::spawn(async move {
        client
            .register_current(&peer, SOURCE, &CancellationToken::new())
            .await
    });
    let mut incoming = current.receiving.accept_bi().await?;
    ensure!(
        matches!(
            read_frame(&mut incoming.receive).await?.message,
            Message::Register { .. }
        ),
        "register request required"
    );
    incoming.send.write_all(&[0; 8]).await?;
    incoming.send.finish()?;
    let failure = timeout(BUDGET, call)
        .await??
        .err()
        .ok_or_else(|| anyhow::anyhow!("corrupt current frame must fail"))?;
    ensure!(
        matches!(failure, ProgressReceiveError::Exchange(_)) && failure.source().is_some(),
        "malformed current protocol is not unsupported or offline"
    );
    drop(incoming);
    current.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn fresh_challenge_reads_current_attempt_and_late_bound_is_checked_again(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for before in [true, false] {
            let worker = AuthorityProcess::start()?;
            let first = worker.accept(false)?;
            let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
            let client = producer(&fixture, worker.authority.clone());
            let owner = Arc::new(TransferProgressReceiver::new(
                fixture.receiving.peer(),
                SOURCE,
                first.items.to_vec(),
            )?);
            let gate = Arc::new(Gate {
                before,
                owner: owner.clone(),
                armed: AtomicBool::new(true),
                entered: Notify::new(),
                release: Semaphore::new(0),
            });
            let calling = client.clone();
            let peer = fixture.sending.peer();
            let call = tokio::spawn(async move {
                calling
                    .register_current(&peer, SOURCE, &CancellationToken::new())
                    .await
            });
            let incoming = fixture.receiving.accept_bi().await?;
            let connection = fixture.receiving.clone();
            let held = gate.clone();
            let serving = tokio::spawn(async move {
                TransferProgressInbound::serve(
                    incoming,
                    &connection,
                    1,
                    progress_budget(),
                    &CancellationToken::new(),
                    held.as_ref(),
                )
                .await
            });
            timeout(BUDGET, gate.entered.notified()).await?;
            let next = worker.accept(false)?;
            gate.release.add_permits(1);
            timeout(BUDGET, serving).await???;
            let registered = timeout(BUDGET, call).await??;
            if before {
                ensure!(
                    registered?.descriptor == next.descriptor,
                    "pre-dial snapshot cannot replace fresh challenged authority"
                );
            } else {
                ensure!(
                    matches!(registered, Err(ProgressReceiveError::Stale)),
                    "Bound cannot activate if actual attempt changed while awaiting response"
                );
            }
            let current = registration(&fixture, client.as_ref(), gate.as_ref(), 1).await?;
            ensure!(
                current.descriptor == next.descriptor,
                "subsequent exchange must confirm actual current attempt"
            );
            fixture.close().await?;
        }
        Ok(())
    })
    .await?
}

struct AcknowledgedRound {
    producer: Arc<TransferProgressProducer>,
    acknowledged: Arc<Notify>,
}
#[async_trait]
impl ProgressTerminalRoundPort for AcknowledgedRound {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        self.producer
            .report_settled(peer, expected, result, deadline, cancel)
            .await?;
        self.acknowledged.notify_one();
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn live_receive_attempt_reports_its_failed_materialization_through_the_complete_action(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let worker = AuthorityProcess::start()?;
        // 子进程真实use case在CAS后交接来源，再在受控materializer处返回IO失败。
        let accepted = worker.accept(false)?;
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let client = producer(&fixture, worker.authority.clone());
        let owner = TransferProgressReceiver::new(
            fixture.receiving.peer(),
            SOURCE,
            accepted.items.to_vec(),
        )?;
        let result = ProgressTerminal {
            item: accepted.items[0],
            round: 0,
            sequence: 1,
            result_revision: 1,
            bytes: 0,
            total: None,
            outcome: ProgressTerminalOutcome::Failed(ProgressFailureReason::Unknown),
        };
        let stop = CancellationToken::new();
        let peer = fixture.sending.peer();
        let receive = async {
            for _ in 0..2 {
                TransferProgressInbound::serve(
                    fixture.receiving.accept_bi().await?,
                    &fixture.receiving,
                    1,
                    progress_budget(),
                    &stop,
                    &owner,
                )
                .await?;
            }
            anyhow::Ok(())
        };
        let acknowledged = Arc::new(Notify::new());
        let (delivery, supervisor) = ProgressTerminalDelivery::new(
            Arc::new(SystemElapsedClock),
            worker.authority.clone(),
            Arc::new(AcknowledgedRound {
                producer: client,
                acknowledged: acknowledged.clone(),
            }),
        );
        assert_eq!(
            delivery.submit_settled(peer, SOURCE, accepted.descriptor.attempt, result)?,
            ProgressSubmission::Retained
        );
        let supervise_stop = CancellationToken::new();
        let supervised = tokio::spawn(supervisor.run(supervise_stop.clone()));
        let ((), served) = tokio::join!(acknowledged.notified(), receive);
        served?;
        assert_eq!(
            delivery.submit_settled(peer, SOURCE, accepted.descriptor.attempt, result)?,
            ProgressSubmission::Refused(ProgressClaimRefusal::AlreadyObserved)
        );
        supervise_stop.cancel();
        supervised.await?;
        assert_eq!(owner.snapshot().items[0].1, Some(result));
        fixture.close().await?;
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
