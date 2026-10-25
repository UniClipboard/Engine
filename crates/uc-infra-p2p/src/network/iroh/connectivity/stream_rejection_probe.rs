//! 真实双端拒绝必须保留业务分类，不能被流结束伪装为暂时网络失败。
use super::progress_probe::{progress_budget, Fixture};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use anyhow::{bail, ensure};
use std::collections::HashSet;
use std::error::Error;
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressBound, ProgressClaimRefusal, ProgressReceiveError, ProgressSourceTask,
    ProgressTaskAuthorityPort, ProgressTaskDescriptor, ProgressTerminalNotice, ProgressWirePort,
    TransferProgressReceiver,
};
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressItemState, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, Envelope, Message, RejectionReason, WireId, TRANSFER_PROGRESS_ALPN,
};

fn id(n: u8) -> ProgressIdentity {
    ProgressIdentity::new([n; 16])
}
fn item(index: u16) -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index,
    }
}
struct Authority;
impl ProgressTaskAuthorityPort for Authority {
    fn current_task(&self, _: &DeviceId, source: ProgressIdentity) -> Option<ProgressSourceTask> {
        Some(ProgressSourceTask {
            descriptor: ProgressTaskDescriptor {
                source,
                producer: id(2),
                attempt: id(3),
                revision: 8,
                item_count: 1,
            },
            items: Arc::from([item(0)]),
            notification_closed: Arc::new(HashSet::new()),
        })
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressIdentity,
        _: ProgressIdentity,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        Err(ProgressClaimRefusal::AlreadyObserved)
    }
}
async fn register(
    fixture: &Fixture,
    owner: &TransferProgressReceiver,
) -> (
    Result<ProgressBound, ProgressReceiveError>,
    Result<(), ProgressReceiveError>,
) {
    let cancel = CancellationToken::new();
    let client_deadline = progress_budget();
    let client = async {
        let mut stream = fixture
            .sending
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?;
        TransferProgressWire
            .register(
                &mut stream,
                &fixture.sending,
                id(1),
                &Authority,
                &client_deadline,
                &cancel,
            )
            .await
    };
    let server = async {
        let stream = fixture.receiving.accept_bi().await?;
        TransferProgressInbound::serve(
            stream,
            &fixture.receiving,
            1,
            progress_budget(),
            &cancel,
            owner,
        )
        .await
    };
    tokio::join!(client, server)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_rejection_before_challenge_reaches_client_as_unknown_task(
) -> anyhow::Result<()> {
    let fixture = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    let owner = TransferProgressReceiver::new(
        fixture.receiving.peer(),
        id(9),
        vec![item(0)],
        Arc::new(SystemElapsedClock),
    )?;
    let (client, server) = register(&fixture, &owner).await;
    assert!(matches!(server, Err(ProgressReceiveError::UnknownTask)));
    assert_eq!(
        client.err().unwrap().class(),
        "progress_task_not_authorized"
    );
    fixture.close().await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_rejection_after_challenge_reaches_client_as_invalid_plan(
) -> anyhow::Result<()> {
    let fixture = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    let owner = TransferProgressReceiver::new(
        fixture.receiving.peer(),
        id(1),
        vec![item(0), item(1)],
        Arc::new(SystemElapsedClock),
    )?;
    let (client, server) = register(&fixture, &owner).await;
    assert!(matches!(server, Err(ProgressReceiveError::InvalidPlan)));
    assert_eq!(client.err().unwrap().class(), "progress_plan_invalid");
    fixture.close().await?;
    Ok(())
}

fn notice(binding: ProgressIdentity) -> ProgressTerminalNotice {
    ProgressTerminalNotice {
        binding,
        source: id(1),
        attempt: id(3),
        revision: 8,
        result: ProgressTerminal {
            item: item(0),
            round: 0,
            sequence: 1,
            result_revision: 1,
            bytes: 8,
            total: Some(8),
            outcome: ProgressTerminalOutcome::Completed,
        },
    }
}
async fn settle(
    fixture: &Fixture,
    owner: &TransferProgressReceiver,
    notice: ProgressTerminalNotice,
) -> (
    Result<ProgressTerminalNotice, ProgressReceiveError>,
    Result<(), ProgressReceiveError>,
) {
    let cancel = CancellationToken::new();
    let deadline = progress_budget();
    let client = async {
        let mut stream = fixture
            .sending
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?;
        TransferProgressWire
            .terminal(&mut stream, &fixture.sending, notice, &deadline, &cancel)
            .await
    };
    let server = async {
        let stream = fixture.receiving.accept_bi().await?;
        TransferProgressInbound::serve(
            stream,
            &fixture.receiving,
            1,
            progress_budget(),
            &cancel,
            owner,
        )
        .await
    };
    tokio::join!(client, server)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_rejection_never_acknowledges_or_overwrites_the_adopted_result(
) -> anyhow::Result<()> {
    let fixture = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    let owner = TransferProgressReceiver::new(
        fixture.receiving.peer(),
        id(1),
        vec![item(0)],
        Arc::new(SystemElapsedClock),
    )?;
    let (bound, served) = register(&fixture, &owner).await;
    served?;
    let accepted = notice(bound?.binding);
    let mut wrong = accepted;
    wrong.source = id(9);
    let (client, server) = settle(&fixture, &owner, wrong).await;
    assert!(matches!(client, Err(ProgressReceiveError::UnknownTask)));
    assert!(matches!(server, Err(ProgressReceiveError::UnknownTask)));
    assert_eq!(owner.snapshot().summary.counts.completed, 0);
    let (client, server) = settle(&fixture, &owner, accepted).await;
    server?;
    assert_eq!(client?, accepted);
    let mut conflict = accepted;
    conflict.result.bytes = 7;
    let (client, server) = settle(&fixture, &owner, conflict).await;
    assert!(matches!(client, Err(ProgressReceiveError::InvalidPlan)));
    assert!(matches!(server, Err(ProgressReceiveError::Conflict)));
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Terminal(accepted.result))
    );
    fixture.close().await?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplyStage {
    EarlyRegister,
    ChallengedRegister,
    Terminal,
}
async fn remote_rejection(
    fixture: &Fixture,
    stage: ReplyStage,
    reason: RejectionReason,
    wrong: bool,
    trailing: bool,
) -> anyhow::Result<ProgressReceiveError> {
    let cancel = CancellationToken::new();
    let deadline = progress_budget();
    let client = async {
        let mut stream = fixture
            .sending
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?;
        if stage == ReplyStage::Terminal {
            TransferProgressWire
                .terminal(
                    &mut stream,
                    &fixture.sending,
                    notice(id(7)),
                    &deadline,
                    &cancel,
                )
                .await
                .map(|_| ())
        } else {
            TransferProgressWire
                .register(
                    &mut stream,
                    &fixture.sending,
                    id(1),
                    &Authority,
                    &deadline,
                    &cancel,
                )
                .await
                .map(|_| ())
        }
    };
    let server = async {
        let mut stream = fixture.receiving.accept_bi().await?;
        let request = read_frame(&mut stream.receive).await?.message;
        let correlation = match (stage, request) {
            (ReplyStage::EarlyRegister, Message::Register { source_entry }) => source_entry,
            (ReplyStage::ChallengedRegister, Message::Register { .. }) => {
                let nonce = WireId(id(8).bytes());
                write_frame(
                    &mut stream.send,
                    &Envelope {
                        message: Message::Challenge { nonce },
                        trace_context: None,
                    },
                )
                .await?;
                ensure!(
                    matches!(read_frame(&mut stream.receive).await?.message, Message::CurrentDescriptor { challenge, .. } if challenge == nonce),
                    "current descriptor must answer the challenge"
                );
                nonce
            }
            (ReplyStage::Terminal, Message::Terminal { binding, .. }) => binding,
            _ => bail!("unexpected fixture request"),
        };
        if stage != ReplyStage::EarlyRegister {
            let eof = stream.receive.read_u8().await;
            ensure!(
                eof.is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof),
                "request must end before response"
            );
        }
        let response = Message::Rejected {
            challenge: if wrong {
                WireId(id(9).bytes())
            } else {
                correlation
            },
            reason,
        };
        write_frame(
            &mut stream.send,
            &Envelope {
                message: response,
                trace_context: None,
            },
        )
        .await?;
        if stage == ReplyStage::EarlyRegister {
            let eof = stream.receive.read_u8().await;
            if !wrong {
                ensure!(
                    eof.is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof),
                    "rejected client must end its request"
                );
            }
        }
        if trailing {
            stream.send.write_all(&[0]).await?;
        }
        stream.send.finish()?;
        anyhow::Ok(())
    };
    let (client, server) = timeout(Duration::from_secs(10), async {
        tokio::join!(client, server)
    })
    .await?;
    server?;
    client
        .err()
        .ok_or_else(|| anyhow::anyhow!("rejection cannot acknowledge success"))
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_five_rejection_codes_keep_their_class_in_each_response_position() -> anyhow::Result<()>
{
    let fixture = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    for stage in [
        ReplyStage::EarlyRegister,
        ReplyStage::ChallengedRegister,
        ReplyStage::Terminal,
    ] {
        for (reason, class) in [
            (RejectionReason::AccessDenied, "progress_access_denied"),
            (RejectionReason::Busy, "progress_registration_busy"),
            (RejectionReason::Stale, "progress_binding_stale"),
            (RejectionReason::UnknownTask, "progress_task_not_authorized"),
            (RejectionReason::InvalidScope, "progress_plan_invalid"),
        ] {
            let rejected = remote_rejection(&fixture, stage, reason, false, false).await?;
            assert_eq!(rejected.class(), class);
            if reason == RejectionReason::AccessDenied {
                assert!(rejected.source().is_some());
            }
        }
    }
    fixture.close().await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_correlation_and_trailing_bytes_cannot_be_adopted_as_a_valid_rejection(
) -> anyhow::Result<()> {
    let fixture = Fixture::new(2, 2, TRANSFER_PROGRESS_ALPN).await?;
    for stage in [
        ReplyStage::EarlyRegister,
        ReplyStage::ChallengedRegister,
        ReplyStage::Terminal,
    ] {
        let stale = remote_rejection(&fixture, stage, RejectionReason::Busy, true, false).await?;
        assert!(matches!(stale, ProgressReceiveError::Stale));
        let malformed =
            remote_rejection(&fixture, stage, RejectionReason::Busy, false, true).await?;
        assert!(matches!(malformed, ProgressReceiveError::Malformed(_)));
        assert!(malformed.source().is_some());
    }
    fixture.close().await?;
    Ok(())
}
