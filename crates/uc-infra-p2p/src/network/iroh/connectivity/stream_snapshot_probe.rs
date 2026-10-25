//! 最大原始CAS计划与真实分页；状态载荷为合成fixture，不冒充1024文件实际交付。
use super::super::profile_probe::ProfilePair;
use super::super::progress_probe::{progress_budget, Fixture};
use super::{AuthorityProcess, SOURCE};
use crate::network::iroh::transfer_progress_v2::TransferProgressWire;
use anyhow::ensure;
use std::error::Error;
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ConnectivityError, ConnectivityManager, ElapsedClockError, ElapsedClockPort,
    ProgressExchangeDeadline, ProgressReceiveError, ProgressSnapshotRead,
    ProgressSnapshotRoundPort, ProgressSnapshotTarget, ProgressSourceTask, ProgressTaskDescriptor,
    TransferProgressSnapshotReader,
};
use uc_core::file_transfer::{
    FileTransferCancellationReason, ProgressActive, ProgressFailureReason, ProgressIdentity,
    ProgressItem, ProgressItemKind, ProgressItemState, ProgressPhase, ProgressSnapshotRefusal,
    ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_sync_protocol::transfer_progress::v2::{
    encode_frame, read_frame, ActiveProgress, CancelReason, Envelope, FailureReason, FrameRefusal,
    ItemKind, ItemState, LogicalItem, Message, Phase, ProducerDescriptor, QueryResult,
    RejectionReason, SnapshotPage, TaskScope, TerminalOutcome, TerminalResult, WireError, WireId,
    MAX_FRAME_BYTES, TRANSFER_PROGRESS_ALPN,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Good,
    DuplicatePage,
    SkippedPage,
    MixedRevision,
    MixedAttempt,
    DuplicateItem,
    MissingPage,
    WrongChallenge,
    Trailing,
    Oversized,
    Deadline,
    NotRetained,
    Unsupported,
    AccessDenied,
    Superseded,
    SupersededRegressed,
    SupersededConflict,
    SupersededRestart,
    SupersededChangedPlan,
    Busy,
    RejectedStale,
    RejectedUnknownTask,
    RejectedInvalidScope,
    ForeignSource,
    ForeignPlan,
}
impl Mode {
    fn count(self) -> usize {
        if matches!(
            self,
            Self::RejectedStale
                | Self::RejectedUnknownTask
                | Self::RejectedInvalidScope
                | Self::ForeignSource
                | Self::ForeignPlan
        ) {
            return 1;
        }
        if self == Self::Good {
            16
        } else if matches!(
            self,
            Self::NotRetained
                | Self::Unsupported
                | Self::AccessDenied
                | Self::Superseded
                | Self::SupersededRegressed
                | Self::SupersededConflict
                | Self::SupersededRestart
                | Self::SupersededChangedPlan
                | Self::Busy
        ) {
            1
        } else {
            2
        }
    }
}
struct Clock(AtomicU64);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Duration::from_secs(self.0.load(Ordering::SeqCst)))
    }
}
fn task() -> ProgressSourceTask {
    ProgressSourceTask {
        notification_closed: Default::default(),
        descriptor: ProgressTaskDescriptor {
            producer: ProgressIdentity::new([2; 16]),
            revision: 4,
            source: SOURCE,
            attempt: ProgressIdentity::new([3; 16]),
            item_count: 1024,
        },
        items: (0..1024)
            .map(|index| ProgressItem {
                kind: ProgressItemKind::File,
                index,
            })
            .collect::<Vec<_>>()
            .into(),
    }
}
fn record(index: u16) -> (ItemState, ProgressItemState) {
    let item = LogicalItem {
        kind: ItemKind::File,
        index,
    };
    let logical = ProgressItem {
        kind: ProgressItemKind::File,
        index,
    };
    let round = u32::MAX;
    let sequence = u64::MAX;
    match index % 7 {
        n @ 0..=3 => {
            let (phase, semantic) = match n {
                0 => (Phase::Waiting, ProgressPhase::Waiting),
                1 => (Phase::Transferring, ProgressPhase::Transferring),
                2 => (Phase::Processing, ProgressPhase::Processing),
                _ => (Phase::Retrying, ProgressPhase::Retrying),
            };
            let (bytes, total) = match n {
                0 => (0, None),
                1 | 2 => (u64::MAX, Some(u64::MAX)),
                _ => (0, Some(0)),
            };
            (
                ItemState::Active(ActiveProgress {
                    item,
                    round,
                    sequence,
                    bytes,
                    total,
                    phase,
                }),
                ProgressItemState::Active(ProgressActive {
                    item: logical,
                    round,
                    sequence,
                    bytes,
                    total,
                    phase: semantic,
                }),
            )
        }
        n => {
            let (outcome, semantic) = match n {
                4 => (
                    TerminalOutcome::Completed,
                    ProgressTerminalOutcome::Completed,
                ),
                5 => (
                    TerminalOutcome::Failed(FailureReason::StorageUnavailable),
                    ProgressTerminalOutcome::Failed(ProgressFailureReason::StorageUnavailable),
                ),
                _ => (
                    TerminalOutcome::Cancelled(CancelReason::LocalUser),
                    ProgressTerminalOutcome::Cancelled(FileTransferCancellationReason::LocalUser),
                ),
            };
            (
                ItemState::Terminal(TerminalResult {
                    item,
                    round,
                    sequence,
                    result_revision: 17,
                    bytes: 0,
                    total: Some(0),
                    outcome,
                }),
                ProgressItemState::Terminal(ProgressTerminal {
                    item: logical,
                    round,
                    sequence,
                    result_revision: 17,
                    bytes: 0,
                    total: Some(0),
                    outcome: semantic,
                }),
            )
        }
    }
}
fn descriptor(value: ProgressTaskDescriptor) -> ProducerDescriptor {
    ProducerDescriptor {
        producer: WireId(value.producer.bytes()),
        owner_revision: value.revision,
        scope: TaskScope {
            source_entry: WireId(value.source.bytes()),
            attempt: WireId(value.attempt.bytes()),
        },
        item_count: value.item_count,
    }
}
enum Network {
    Baseline(Fixture),
    Progress(ProfilePair),
}
impl Network {
    fn outbound(&self) -> ConnectivityManager {
        match self {
            Self::Baseline(f) => f.outbound.clone(),
            Self::Progress(f) => f.outbound.clone(),
        }
    }
    fn receiving(&self) -> &ConnectionLease {
        match self {
            Self::Baseline(f) => &f.receiving,
            Self::Progress(f) => &f.receiving,
        }
    }
    fn peer(&self) -> DeviceId {
        match self {
            Self::Baseline(f) => f.sending.peer(),
            Self::Progress(f) => f.peer,
        }
    }
    fn profile(&self) -> &'static str {
        match self {
            Self::Baseline(_) => "clipboard_fixture",
            Self::Progress(_) => "transfer_progress",
        }
    }
    async fn close(self) -> anyhow::Result<()> {
        match self {
            Self::Baseline(f) => f.close().await,
            Self::Progress(f) => f.close().await,
        }
    }
}
struct Evidence {
    profile: &'static str,
    requests: usize,
    largest: usize,
    rss_before: Option<u64>,
    rss_after: Option<u64>,
}
fn resident_bytes() -> Option<u64> {
    // 主机描述性采样；非Linux或读取不可用登记为空，不替代阶段5的1GiB资源验收。
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let value = line
            .strip_prefix("VmRSS:")?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?;
        value.checked_mul(1024)
    })
}
async fn exchange(
    expected: ProgressSourceTask,
    mode: Mode,
    discover: bool,
) -> anyhow::Result<(Result<ProgressSnapshotRead, ProgressReceiveError>, Evidence)> {
    let rss_before = resident_bytes();
    let fixture = if matches!(
        mode,
        Mode::Good
            | Mode::SupersededRegressed
            | Mode::SupersededConflict
            | Mode::SupersededRestart
            | Mode::SupersededChangedPlan
    ) {
        Network::Progress(ProfilePair::new().await?)
    } else {
        Network::Baseline(Fixture::new(1, 1, TRANSFER_PROGRESS_ALPN).await?)
    };
    let reader =
        TransferProgressSnapshotReader::new(fixture.outbound(), Arc::new(TransferProgressWire));
    let clock = Arc::new(Clock(AtomicU64::new(0)));
    let budget = if mode == Mode::Deadline {
        ProgressExchangeDeadline::new(clock.clone(), Duration::from_secs(5))?
    } else {
        progress_budget()
    };
    let stop = CancellationToken::new();
    let requests = AtomicUsize::new(0);
    let largest = AtomicUsize::new(0);
    let serve = async {
        for number in 0..mode.count() {
            let mut stream = fixture.receiving().accept_bi().await?;
            fixture.receiving().check_current().await?;
            let request = read_frame(&mut stream.receive).await?.message;
            assert!(
                matches!(stream.receive.read_u8().await,Err(error) if error.kind()==std::io::ErrorKind::UnexpectedEof)
            );
            let Message::Query {
                challenge,
                source_entry,
                expected_attempt,
                offset,
                snapshot_revision,
            } = request
            else {
                panic!("expected actual query");
            };
            requests.fetch_add(1, Ordering::SeqCst);
            assert_eq!(source_entry.0, expected.descriptor.source.bytes());
            assert_eq!(
                expected_attempt,
                if discover && number == 0 {
                    None
                } else {
                    Some(WireId(expected.descriptor.attempt.bytes()))
                }
            );
            assert_eq!(usize::from(offset), number * 64);
            assert_eq!(snapshot_revision, if number == 0 { None } else { Some(9) });
            if mode == Mode::MissingPage && number == 1 {
                stream.send.finish()?;
                continue;
            }
            let mut current = descriptor(expected.descriptor);
            if mode == Mode::ForeignSource {
                current.scope.source_entry = WireId([9; 16]);
            }
            if mode == Mode::ForeignPlan {
                current.item_count = 1023;
            }
            if mode == Mode::SupersededChangedPlan {
                current.item_count = 128;
            }
            let mut page_offset = offset;
            if number == 1 {
                match mode {
                    Mode::DuplicatePage => page_offset = 0,
                    Mode::SkippedPage => page_offset = 128,
                    Mode::MixedAttempt => current.scope.attempt = WireId([8; 16]),
                    _ => {}
                }
            }
            let mut items: Vec<_> = (page_offset..page_offset + 64)
                .map(|index| record(index).0)
                .collect();
            if mode == Mode::DuplicateItem && number == 1 {
                items[0] = record(0).0;
            }
            let page = SnapshotPage {
                snapshot_revision: if mode == Mode::MixedRevision && number == 1 {
                    10
                } else {
                    9
                },
                offset: page_offset,
                total: current.item_count,
                items,
            };
            let result = match mode {
                Mode::NotRetained => QueryResult::NotRetained,
                Mode::Unsupported => QueryResult::Unsupported,
                Mode::AccessDenied => QueryResult::AccessDenied,
                Mode::Superseded
                | Mode::SupersededRegressed
                | Mode::SupersededConflict
                | Mode::SupersededRestart
                | Mode::SupersededChangedPlan => {
                    current.scope.attempt = WireId([8; 16]);
                    current.owner_revision = match mode {
                        Mode::SupersededRegressed | Mode::SupersededRestart => {
                            current.owner_revision - 1
                        }
                        Mode::SupersededConflict => current.owner_revision,
                        _ => current.owner_revision + 1,
                    };
                    if mode == Mode::SupersededRestart {
                        current.producer = WireId([7; 16]);
                    }
                    QueryResult::Superseded {
                        descriptor: current,
                        page,
                    }
                }
                _ => QueryResult::Current {
                    descriptor: current,
                    page,
                },
            };
            let challenge = if mode == Mode::WrongChallenge && number == 1 {
                WireId([9; 16])
            } else {
                challenge
            };
            let response = Envelope {
                message: if matches!(
                    mode,
                    Mode::Busy
                        | Mode::RejectedStale
                        | Mode::RejectedUnknownTask
                        | Mode::RejectedInvalidScope
                ) {
                    Message::Rejected {
                        challenge,
                        reason: match mode {
                            Mode::RejectedStale => RejectionReason::Stale,
                            Mode::RejectedUnknownTask => RejectionReason::UnknownTask,
                            Mode::RejectedInvalidScope => RejectionReason::InvalidScope,
                            _ => RejectionReason::Busy,
                        },
                    }
                } else {
                    Message::QueryResult { challenge, result }
                },
                trace_context: None,
            };
            let mut bytes = encode_frame(&response)?;
            largest.fetch_max(bytes.len(), Ordering::SeqCst);
            if mode == Mode::Oversized && number == 1 {
                bytes[4..8].copy_from_slice(&((MAX_FRAME_BYTES - 7) as u32).to_be_bytes());
                bytes.truncate(8);
            }
            if mode == Mode::Trailing && number == 1 {
                bytes.push(0);
            }
            stream.send.write_all(&bytes).await?;
            if mode == Mode::Deadline {
                clock
                    .0
                    .store(if number == 0 { 4 } else { 6 }, Ordering::SeqCst);
            }
            stream.send.finish()?;
        }
        anyhow::Ok(())
    };
    let peer = fixture.peer();
    let (outcome, served) = tokio::join!(
        reader.read_snapshot(
            &peer,
            ProgressSnapshotTarget {
                source: expected.descriptor.source,
                expected: (!discover).then_some(expected.descriptor),
                items: expected.items.clone()
            },
            &budget,
            &stop
        ),
        serve
    );
    served?;
    let evidence = Evidence {
        profile: fixture.profile(),
        requests: requests.load(Ordering::SeqCst),
        largest: largest.load(Ordering::SeqCst),
        rss_before,
        rss_after: resident_bytes(),
    };
    fixture.close().await?;
    Ok((outcome, evidence))
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn actual_maximum_v3_plan_survives_sixteen_real_snapshot_pages() -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        let process = AuthorityProcess::start()?;
        let expected = process.accept(false, 1024)?;
        assert_eq!(expected.descriptor.item_count, 1024);
        assert_eq!(expected.items.len(), 1024);
        for (index, item) in expected.items.iter().enumerate() {
            assert_eq!(
                *item,
                ProgressItem {
                    kind: ProgressItemKind::File,
                    index: index as u16
                }
            );
        }
        let (outcome, evidence) = exchange(expected.clone(), Mode::Good, false).await?;
        let ProgressSnapshotRead::Complete(snapshot) = outcome? else {
            panic!("all real pages must complete");
        };
        assert!(snapshot.descriptor == expected.descriptor);
        assert_eq!(snapshot.revision, 9);
        assert_eq!(
            snapshot.items,
            (0..1024).map(|index| record(index).1).collect::<Vec<_>>()
        );
        assert_eq!(evidence.requests, 16);
        assert!(evidence.largest <= MAX_FRAME_BYTES);
        assert_eq!(snapshot.items.capacity(),1024);
        if let Some(path) = std::env::var_os("UC_PROGRESS_SNAPSHOT_REPORT") {
            let report=serde_json::json!({
                "plan_items":expected.items.len(),"snapshot_items":snapshot.items.len(),"pages":evidence.requests,
                "largest_frame_bytes":evidence.largest,"frame_limit_bytes":MAX_FRAME_BYTES,"page_item_limit":64,
                "staged_capacity_items":snapshot.items.capacity(),"staged_item_storage_bytes":snapshot.items.capacity()*std::mem::size_of::<ProgressItemState>(),
                "rss_before_bytes":evidence.rss_before,"rss_after_bytes":evidence.rss_after,
                "state_payload":"synthetic","plan_authority":"actual_application_cas_v3",
                "connection_profile":evidence.profile,
            });
            std::fs::write(path,serde_json::to_vec_pretty(&report)?)?;
        }

        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_pagination_faults_never_return_partial_or_falsely_complete_snapshot(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for (mode, reason) in [
            (Mode::DuplicatePage, ProgressSnapshotRefusal::WrongOffset),
            (Mode::SkippedPage, ProgressSnapshotRefusal::WrongOffset),
            (
                Mode::MixedRevision,
                ProgressSnapshotRefusal::RevisionChanged,
            ),
            (
                Mode::MixedAttempt,
                ProgressSnapshotRefusal::DescriptorChanged,
            ),
            (Mode::DuplicateItem, ProgressSnapshotRefusal::WrongItem),
        ] {
            let (outcome, evidence) = exchange(task(), mode, false).await?;
            assert!(
                matches!(outcome,Err(ProgressReceiveError::Snapshot(actual)) if actual==reason),
                "{mode:?}"
            );
            assert_eq!(evidence.requests, 2);
        }
        for mode in [
            Mode::MissingPage,
            Mode::Trailing,
            Mode::Oversized,
            Mode::WrongChallenge,
            Mode::Deadline,
        ] {
            let (outcome, evidence) = exchange(task(), mode, false).await?;
            let error = match outcome {
                Err(error) => error,
                Ok(_) => panic!("{mode:?} cannot return a view or absence"),
            };
            match mode {
                Mode::Deadline => assert!(matches!(
                    error,
                    ProgressReceiveError::Connection(ConnectivityError::DeadlineExpired)
                )),
                Mode::WrongChallenge => assert!(matches!(error, ProgressReceiveError::Stale)),
                Mode::Oversized => assert!(matches!(
                    error.source().unwrap().downcast_ref::<WireError>(),
                    Some(WireError::Frame(FrameRefusal::Length))
                )),
                Mode::Trailing => assert!(matches!(error, ProgressReceiveError::Malformed(_))),
                _ => assert!(matches!(error, ProgressReceiveError::Exchange(_))),
            }
            assert_eq!(evidence.requests, 2);
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseded_current_scope_cannot_regress_or_conflict_with_the_known_producer_revision(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for mode in [Mode::SupersededRegressed, Mode::SupersededConflict] {
            let (outcome, evidence) = exchange(task(), mode, false).await?;
            assert!(
                matches!(outcome, Err(ProgressReceiveError::Stale)),
                "{mode:?}"
            );
            assert_eq!(evidence.profile, "transfer_progress");
            assert_eq!(evidence.requests, 1);
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn superseded_current_scope_allows_a_new_plan_and_never_orders_cross_producer_revisions(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for mode in [Mode::SupersededRestart, Mode::SupersededChangedPlan] {
            let (outcome, evidence) = exchange(task(), mode, false).await?;
            let ProgressSnapshotRead::Superseded(current) = outcome? else {
                panic!("expected a current association without adopting the new plan");
            };
            assert_eq!(current.attempt, ProgressIdentity::new([8; 16]));
            if mode == Mode::SupersededRestart {
                assert_eq!(current.producer, ProgressIdentity::new([7; 16]));
                assert_eq!(current.revision, 3);
            } else {
                assert_eq!(current.item_count, 128);
                assert_eq!(current.revision, 5);
            }
            assert_eq!(evidence.profile, "transfer_progress");
            assert_eq!(evidence.requests, 1);
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_remote_query_outcomes_remain_separate_from_exchange_failures(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for mode in [
            Mode::NotRetained,
            Mode::Unsupported,
            Mode::AccessDenied,
            Mode::Superseded,
            Mode::Busy,
        ] {
            let (outcome, evidence) = exchange(task(), mode, false).await?;
            match (mode, outcome?) {
                (Mode::NotRetained, ProgressSnapshotRead::NotRetained)
                | (Mode::Unsupported, ProgressSnapshotRead::Unsupported)
                | (Mode::AccessDenied, ProgressSnapshotRead::AccessDenied)
                | (Mode::Busy, ProgressSnapshotRead::Busy) => {}
                (Mode::Superseded, ProgressSnapshotRead::Superseded(current)) => {
                    assert_eq!(current.attempt, ProgressIdentity::new([8; 16]))
                }
                _ => panic!("explicit remote classification changed"),
            }
            assert_eq!(evidence.requests, 1);
        }
        for mode in [
            Mode::RejectedStale,
            Mode::RejectedUnknownTask,
            Mode::RejectedInvalidScope,
        ] {
            let (outcome, evidence) = exchange(task(), mode, false).await?;
            assert!(matches!(
                (mode, outcome),
                (Mode::RejectedStale, Err(ProgressReceiveError::Stale))
                    | (
                        Mode::RejectedUnknownTask,
                        Err(ProgressReceiveError::UnknownTask)
                    )
                    | (
                        Mode::RejectedInvalidScope,
                        Err(ProgressReceiveError::InvalidPlan)
                    )
            ));
            assert_eq!(evidence.requests, 1);
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_discovery_rejects_foreign_source_plan_and_mixed_second_page_without_partial_adoption(
) -> anyhow::Result<()> {
    timeout(Duration::from_secs(30), async {
        for (mode, refusal) in [
            (
                Mode::ForeignSource,
                ProgressSnapshotRefusal::DescriptorChanged,
            ),
            (
                Mode::ForeignPlan,
                ProgressSnapshotRefusal::DescriptorChanged,
            ),
            (
                Mode::MixedAttempt,
                ProgressSnapshotRefusal::DescriptorChanged,
            ),
            (
                Mode::MixedRevision,
                ProgressSnapshotRefusal::RevisionChanged,
            ),
        ] {
            let (outcome, _) = exchange(task(), mode, true).await?;
            ensure!(
                matches!(outcome,Err(ProgressReceiveError::Snapshot(reason)) if reason==refusal),
                "initial query must reject a scope outside the original plan"
            );
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}
