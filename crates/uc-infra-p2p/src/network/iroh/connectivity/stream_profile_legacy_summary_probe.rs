//! 只支持旧ALPN的真实对端读取完整批次旧帧；FIN不能变成新版业务采用ACK。
use super::super::progress_probe::progress_budget;
use super::{endpoint, manager, AdmittedConnections, Capture, Directory, Incoming, BUDGET, SOURCE};
use crate::network::iroh::protocol_router::ProtocolRouterBuilder;
use crate::network::iroh::transfer_progress_v2::TransferProgressWire;
use async_trait::async_trait;
use iroh::protocol::Router;
use iroh::Endpoint;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionProtocol, ConnectivityManager, ProgressClaimRefusal, ProgressLiveRoundPort,
    ProgressPageReply, ProgressReceiveError, ProgressSnapshotPage, ProgressSnapshotProviderPort,
    ProgressSnapshotQuery, ProgressSourceTask, ProgressTaskAuthorityPort, ProgressTaskDescriptor,
    ProgressTerminalRoundPort, TransferProgressProducer,
};
use uc_core::file_transfer::{
    OutboundProgressStatus, ProgressActive, ProgressFailureReason, ProgressIdentity, ProgressItem,
    ProgressItemKind, ProgressItemState, ProgressPhase, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::{
    read_frame, ProgressFrame, LEGACY_TRANSFER_PROGRESS_ALPN,
};

pub(in super::super) struct LegacyPair {
    client: Arc<Endpoint>,
    pub(in super::super) outbound: ConnectivityManager,
    inbound: ConnectivityManager,
    router: Router,
    pub(in super::super) incoming: mpsc::Receiver<Incoming>,
    pub(in super::super) peer: DeviceId,
}
impl LegacyPair {
    pub(in super::super) async fn new() -> anyhow::Result<Self> {
        let client = endpoint().await?;
        let server = endpoint().await?;
        let peer = DeviceId::new("progress-source");
        let caller = DeviceId::new("progress-receiver");
        let directory = Arc::new(Directory {
            peers: vec![(peer, server.addr()), (caller, client.addr())],
            allowed: AtomicBool::new(true),
        });
        let outbound = manager(
            client.clone(),
            directory.clone(),
            AdmittedConnections::default(),
        );
        let inbound = manager(server.clone(), directory, AdmittedConnections::default());
        let (tx, incoming) = mpsc::channel(2);
        let router = ProtocolRouterBuilder::new((*server).clone())
            .accept(
                LEGACY_TRANSFER_PROGRESS_ALPN,
                Capture {
                    manager: inbound.clone(),
                    events: tx,
                },
            )
            .spawn();
        Ok(Self {
            client,
            outbound,
            inbound,
            router,
            incoming,
            peer,
        })
    }
    pub(in super::super) async fn close(self) -> anyhow::Result<()> {
        self.outbound.shutdown().await?;
        self.inbound.shutdown().await?;
        self.router.shutdown().await?;
        self.client.close().await;
        Ok(())
    }
}
struct Facts {
    peer: DeviceId,
    items: Vec<ProgressItemState>,
    reads: AtomicUsize,
}
fn item(index: u16) -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index,
    }
}
fn terminal(index: u16, failed: bool) -> ProgressTerminal {
    ProgressTerminal {
        item: item(index),
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 20,
        total: Some(20),
        outcome: if failed {
            ProgressTerminalOutcome::Failed(ProgressFailureReason::StorageUnavailable)
        } else {
            ProgressTerminalOutcome::Completed
        },
    }
}
fn descriptor() -> ProgressTaskDescriptor {
    ProgressTaskDescriptor {
        producer: ProgressIdentity::new([2; 16]),
        revision: 1,
        source: SOURCE,
        attempt: ProgressIdentity::new([3; 16]),
        item_count: 2,
    }
}
impl ProgressTaskAuthorityPort for Facts {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        (*peer == self.peer && source == SOURCE).then(|| ProgressSourceTask {
            descriptor: descriptor(),
            items: Arc::from([item(0), item(1)]),
            notification_closed: Arc::new(HashSet::new()),
        })
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressTaskDescriptor,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("a complete round does not claim a second result");
    }
}
#[async_trait]
impl ProgressSnapshotProviderPort for Facts {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        assert_eq!(*peer, self.peer);
        assert_eq!(query.source, SOURCE);
        assert_eq!(query.attempt, Some(descriptor().attempt));
        assert_eq!(query.offset, 0);
        assert_eq!(query.revision, None);
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(ProgressPageReply::Current(ProgressSnapshotPage {
            descriptor: descriptor(),
            revision: 7,
            offset: 0,
            total: 2,
            items: self.items.clone(),
        }))
    }
}
#[derive(Clone, Copy)]
enum Case {
    FailedThenCompleted,
    ProcessingThenCompleted,
    ActiveThenCompleted,
    AllCompleted,
}
#[derive(Clone, Copy)]
enum Operation {
    Terminal,
    Contact,
    Update,
}
async fn explicit_legacy_uses_complete_facts(
    case: Case,
    operation: Operation,
) -> anyhow::Result<()> {
    timeout(BUDGET * 3, async {
        let mut pair = LegacyPair::new().await?;
        let peer = pair.peer;
        let first = match case {
            Case::FailedThenCompleted => ProgressItemState::Terminal(terminal(0, true)),
            Case::ProcessingThenCompleted => ProgressItemState::Active(ProgressActive {
                item: item(0),
                round: 0,
                sequence: 1,
                bytes: 20,
                total: Some(20),
                phase: ProgressPhase::Processing,
            }),
            Case::ActiveThenCompleted => ProgressItemState::Active(ProgressActive {
                item: item(0),
                round: 0,
                sequence: 1,
                bytes: 8,
                total: Some(20),
                phase: ProgressPhase::Transferring,
            }),
            Case::AllCompleted => ProgressItemState::Terminal(terminal(0, false)),
        };
        let facts = Arc::new(Facts {
            peer,
            items: vec![first, ProgressItemState::Terminal(terminal(1, false))],
            reads: AtomicUsize::new(0),
        });
        let producer = TransferProgressProducer::new(
            pair.outbound.clone(),
            Arc::new(TransferProgressWire),
            facts.clone(),
            facts.clone(),
            Arc::new(SystemElapsedClock),
        );
        let stop = CancellationToken::new();
        let deadline = progress_budget();
        let sending = async {
            match operation {
                Operation::Terminal => {
                    producer
                        .report_settled(&peer, descriptor(), terminal(1, false), &deadline, &stop)
                        .await
                }
                Operation::Contact => {
                    producer
                        .report_contact(&peer, descriptor(), 1, &deadline, &stop)
                        .await
                }
                Operation::Update => {
                    producer
                        .report_live(
                            &peer,
                            descriptor(),
                            ProgressActive {
                                item: item(0),
                                round: 0,
                                sequence: 1,
                                bytes: 3,
                                total: Some(20),
                                phase: ProgressPhase::Transferring,
                            },
                            &deadline,
                            &stop,
                        )
                        .await
                }
            }
        };
        let receiving = async {
            let (lease, _) = timeout(BUDGET, pair.incoming.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("legacy profile was not admitted"))?;
            assert_eq!(lease.protocol(), ConnectionProtocol::TransferProgressV1);
            let mut stream = lease.accept_uni(Instant::now() + BUDGET, &stop).await?;
            let frame = read_frame(&mut stream.stream).await?;
            let mut trailing = [0];
            assert_eq!(stream.stream.read(&mut trailing).await?, 0);
            Ok::<_, anyhow::Error>(frame)
        };
        let (sent, received) = tokio::join!(sending, receiving);
        let reads = facts.reads.load(Ordering::SeqCst);
        pair.close().await?;
        let expected_status = match case {
            Case::FailedThenCompleted => OutboundProgressStatus::Failed,
            Case::AllCompleted => OutboundProgressStatus::Completed,
            _ => OutboundProgressStatus::InProgress,
        };
        let expected_bytes = if matches!(case, Case::ActiveThenCompleted) {
            28
        } else {
            40
        };
        let received = received.expect(
            "a complete legacy batch frame must be delivered through the managed uni stream",
        );
        assert_eq!(
            received,
            ProgressFrame {
                transfer_id_bytes: SOURCE.bytes(),
                bytes_transferred: expected_bytes,
                total_bytes: Some(40),
                status: expected_status
            }
        );
        assert_eq!(
            reads, 1,
            "the old frame must come from the actual full-plan facts"
        );
        match operation {
            Operation::Terminal => assert!(
                matches!(sent, Err(ProgressReceiveError::Unsupported)),
                "legacy transport completion cannot stand in for an Application ACK"
            ),
            Operation::Contact | Operation::Update => assert!(sent.is_ok()),
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}
#[tokio::test]
async fn legacy_terminal_uses_previous_failed_item_even_when_the_last_item_completed(
) -> anyhow::Result<()> {
    explicit_legacy_uses_complete_facts(Case::FailedThenCompleted, Operation::Terminal).await
}
#[tokio::test]
async fn legacy_terminal_keeps_processing_item_active_after_all_network_bytes_arrive(
) -> anyhow::Result<()> {
    explicit_legacy_uses_complete_facts(Case::ProcessingThenCompleted, Operation::Terminal).await
}
#[tokio::test]
async fn legacy_terminal_keeps_sibling_active_and_reports_full_batch_bytes() -> anyhow::Result<()> {
    explicit_legacy_uses_complete_facts(Case::ActiveThenCompleted, Operation::Terminal).await
}
#[tokio::test]
async fn legacy_terminal_reports_completed_only_when_all_actual_items_completed(
) -> anyhow::Result<()> {
    explicit_legacy_uses_complete_facts(Case::AllCompleted, Operation::Terminal).await
}

#[tokio::test]
async fn legacy_contact_reads_the_complete_current_batch_without_a_business_ack(
) -> anyhow::Result<()> {
    explicit_legacy_uses_complete_facts(Case::ActiveThenCompleted, Operation::Contact).await
}
#[tokio::test]
async fn legacy_update_uses_current_batch_facts_instead_of_the_single_callback_value(
) -> anyhow::Result<()> {
    explicit_legacy_uses_complete_facts(Case::ActiveThenCompleted, Operation::Update).await
}
#[path = "stream_profile_legacy_source_probe.rs"]
mod source_probe;

#[path = "stream_profile_legacy_inbound_probe.rs"]
mod inbound_probe;
