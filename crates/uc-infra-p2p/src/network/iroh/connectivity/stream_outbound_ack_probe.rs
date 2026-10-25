//! 实际专属profile中先采用原结果，再逐字段破坏回执；正确重试仍须取得精确ACK。
use super::super::profile_probe::ProfilePair;
use super::{item, progress_budget, terminal, Authority, UnusedSnapshotProvider, BUDGET, SOURCE};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use anyhow::{bail, ensure};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressReceiveError, ProgressReceiverPort, ProgressTaskAuthorityPort, ProgressTerminalNotice,
    ProgressTerminalRoundPort, TransferProgressProducer, TransferProgressReceiver,
};
use uc_core::file_transfer::{
    FileTransferCancellationReason, ProgressFailureReason, ProgressIdentity, ProgressItemState,
    ProgressTerminal, ProgressTerminalOutcome,
};
use uc_infra_local::SystemElapsedClock;
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, CancelReason, Envelope, FailureReason, ItemKind, Message,
    TerminalOutcome, WireId,
};

#[derive(Clone, Copy, Debug)]
enum AckChange {
    Exact,
    Binding,
    Source,
    Attempt,
    OwnerRevision,
    ItemKind,
    ItemIndex,
    Round,
    Sequence,
    ResultRevision,
    Bytes,
    TotalValue,
    TotalAbsent,
    TotalZeroAbsent,
    TotalZeroPresent,
    FailedOutcome,
    CancelledOutcome,
    FailureReason,
    CancellationReason,
    WrongMessage,
    Trailing,
}
impl AckChange {
    fn original(self) -> (ProgressTerminal, TerminalOutcome) {
        match self {
            Self::Bytes | Self::TotalValue | Self::FailureReason => (
                ProgressTerminal {
                    outcome: ProgressTerminalOutcome::Failed(
                        ProgressFailureReason::NetworkUnavailable,
                    ),
                    ..terminal(10)
                },
                TerminalOutcome::Failed(FailureReason::NetworkUnavailable),
            ),
            Self::CancellationReason => (
                ProgressTerminal {
                    outcome: ProgressTerminalOutcome::Cancelled(
                        FileTransferCancellationReason::LocalUser,
                    ),
                    ..terminal(10)
                },
                TerminalOutcome::Cancelled(CancelReason::LocalUser),
            ),
            Self::TotalZeroAbsent => (terminal(0), TerminalOutcome::Completed),
            Self::TotalZeroPresent => (
                ProgressTerminal {
                    total: None,
                    ..terminal(0)
                },
                TerminalOutcome::Completed,
            ),
            _ => (terminal(10), TerminalOutcome::Completed),
        }
    }
    fn change(self, message: &mut Message) {
        let Message::TerminalAck {
            binding,
            scope,
            owner_revision,
            result,
        } = message
        else {
            panic!("只能改变已采用原结果的回执");
        };
        match self {
            Self::Exact | Self::Trailing => {}
            Self::Binding => *binding = WireId([9; 16]),
            Self::Source => scope.source_entry = WireId([9; 16]),
            Self::Attempt => scope.attempt = WireId([9; 16]),
            Self::OwnerRevision => *owner_revision += 1,
            Self::ItemKind => result.item.kind = ItemKind::Representation,
            Self::ItemIndex => result.item.index += 1,
            Self::Round => result.round += 1,
            Self::Sequence => result.sequence += 1,
            Self::ResultRevision => result.result_revision += 1,
            Self::Bytes => result.bytes -= 1,
            Self::TotalValue => result.total = Some(11),
            Self::TotalAbsent | Self::TotalZeroAbsent => result.total = None,
            Self::TotalZeroPresent => result.total = Some(0),
            Self::FailedOutcome | Self::FailureReason => {
                result.outcome = TerminalOutcome::Failed(FailureReason::StorageUnavailable)
            }
            Self::CancelledOutcome => {
                result.outcome = TerminalOutcome::Cancelled(CancelReason::LocalUser)
            }
            Self::CancellationReason => {
                result.outcome = TerminalOutcome::Cancelled(CancelReason::RemotePeer)
            }
            Self::WrongMessage => {
                *message = Message::Terminal {
                    binding: *binding,
                    scope: *scope,
                    owner_revision: *owner_revision,
                    result: result.clone(),
                };
            }
        }
    }
}

async fn prove_ack(
    change: AckChange,
    original: ProgressTerminal,
    wire_outcome: TerminalOutcome,
) -> anyhow::Result<()> {
    let pair = ProfilePair::new().await?;
    let owner = TransferProgressReceiver::new(
        pair.receiving.peer(),
        SOURCE,
        vec![item()],
        Arc::new(SystemElapsedClock),
    )?;
    let authority = Arc::new(Authority::default());
    let expected = authority
        .current_task(&pair.peer, SOURCE)
        .unwrap()
        .descriptor;
    let producer = TransferProgressProducer::new(
        pair.outbound.clone(),
        Arc::new(TransferProgressWire),
        authority,
        Arc::new(UnusedSnapshotProvider),
        Arc::new(SystemElapsedClock),
    );
    let stop = CancellationToken::new();
    let serve = async {
        TransferProgressInbound::serve(
            pair.receiving.accept_bi().await?,
            &pair.receiving,
            1,
            progress_budget(),
            &stop,
            &owner,
            &UnusedSnapshotProvider,
        )
        .await?;
        let mut stream = pair.receiving.accept_bi().await?;
        let Message::Terminal {
            binding,
            scope,
            owner_revision,
            result,
        } = read_frame(&mut stream.receive).await?.message
        else {
            bail!("原完整动作必须发送终态");
        };
        let mut end = [0];
        ensure!(
            timeout(BUDGET, stream.receive.read(&mut end)).await?? == 0,
            "原请求必须完整结束"
        );
        ensure!(
            scope.source_entry.0 == expected.source.bytes()
                && scope.attempt.0 == expected.attempt.bytes()
                && owner_revision == expected.revision,
            "原登记关联必须完整保留"
        );
        ensure!(
            result.item.kind == ItemKind::File
                && result.item.index == original.item.index
                && result.round == original.round
                && result.sequence == original.sequence
                && result.result_revision == original.result_revision
                && result.bytes == original.bytes
                && result.total == original.total
                && result.outcome == wire_outcome,
            "原结果必须按线上合同逐字段保留"
        );
        let notice = ProgressTerminalNotice {
            binding: ProgressIdentity::new(binding.0),
            source: expected.source,
            attempt: expected.attempt,
            revision: owner_revision,
            result: original,
        };
        ensure!(
            owner
                .adopt_terminal(&pair.receiving.peer(), 1, notice)
                .await?
                == notice,
            "实际采用后才能形成回执"
        );
        let mut reply = Message::TerminalAck {
            binding,
            scope,
            owner_revision,
            result,
        };
        change.change(&mut reply);
        write_frame(
            &mut stream.send,
            &Envelope {
                message: reply,
                trace_context: None,
            },
        )
        .await?;
        if matches!(change, AckChange::Trailing) {
            stream.send.write_all(&[0]).await?;
        }
        stream.send.finish()?;
        anyhow::Ok(())
    };
    let budget = progress_budget();
    let (reported, served) = timeout(Duration::from_secs(15), async {
        tokio::join!(
            producer.report_settled(&pair.peer, expected, original, &budget, &stop),
            serve
        )
    })
    .await?;
    served?;
    match change {
        AckChange::Exact => ensure!(reported.is_ok(), "精确回执必须成功"),
        AckChange::Trailing | AckChange::WrongMessage => ensure!(
            matches!(reported, Err(ProgressReceiveError::Malformed(_))),
            "错误边界必须保留解析失败"
        ),
        _ => ensure!(
            matches!(reported, Err(ProgressReceiveError::Conflict)),
            "改变回执字段不得成功：{change:?}"
        ),
    }
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Terminal(original))
    );
    // 用原入站负责人处理正确重试，证明坏回执既不回滚已采用结果，也不损坏同连接的后续动作。
    let serve = async {
        for _ in 0..2 {
            TransferProgressInbound::serve(
                pair.receiving.accept_bi().await?,
                &pair.receiving,
                1,
                progress_budget(),
                &stop,
                &owner,
                &UnusedSnapshotProvider,
            )
            .await?;
        }
        anyhow::Ok(())
    };
    let budget = progress_budget();
    let (reported, served) = timeout(Duration::from_secs(15), async {
        tokio::join!(
            producer.report_settled(&pair.peer, expected, original, &budget, &stop),
            serve
        )
    })
    .await?;
    served?;
    reported?;
    assert_eq!(
        owner.snapshot().items[0].1,
        Some(ProgressItemState::Terminal(original))
    );
    pair.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_ack_identity_result_field_and_frame_boundary_is_checked_over_actual_profile(
) -> anyhow::Result<()> {
    for change in [
        AckChange::Exact,
        AckChange::Binding,
        AckChange::Source,
        AckChange::Attempt,
        AckChange::OwnerRevision,
        AckChange::ItemKind,
        AckChange::ItemIndex,
        AckChange::Round,
        AckChange::Sequence,
        AckChange::ResultRevision,
        AckChange::Bytes,
        AckChange::TotalValue,
        AckChange::TotalAbsent,
        AckChange::TotalZeroAbsent,
        AckChange::TotalZeroPresent,
        AckChange::FailedOutcome,
        AckChange::CancelledOutcome,
        AckChange::FailureReason,
        AckChange::CancellationReason,
        AckChange::WrongMessage,
        AckChange::Trailing,
    ] {
        let (original, wire_outcome) = change.original();
        prove_ack(change, original, wire_outcome).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_outcome_reason_codes_keep_the_exact_business_result_over_actual_profile(
) -> anyhow::Result<()> {
    for (outcome, wire_outcome) in [
        (
            ProgressTerminalOutcome::Completed,
            TerminalOutcome::Completed,
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::NetworkUnavailable),
            TerminalOutcome::Failed(FailureReason::NetworkUnavailable),
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::TimedOut),
            TerminalOutcome::Failed(FailureReason::TimedOut),
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::AccessDenied),
            TerminalOutcome::Failed(FailureReason::AccessDenied),
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::StorageUnavailable),
            TerminalOutcome::Failed(FailureReason::StorageUnavailable),
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::IntegrityCheckFailed),
            TerminalOutcome::Failed(FailureReason::IntegrityCheckFailed),
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::Interrupted),
            TerminalOutcome::Failed(FailureReason::Interrupted),
        ),
        (
            ProgressTerminalOutcome::Failed(ProgressFailureReason::Unknown),
            TerminalOutcome::Failed(FailureReason::Unknown),
        ),
        (
            ProgressTerminalOutcome::Cancelled(FileTransferCancellationReason::LocalUser),
            TerminalOutcome::Cancelled(CancelReason::LocalUser),
        ),
        (
            ProgressTerminalOutcome::Cancelled(FileTransferCancellationReason::RemotePeer),
            TerminalOutcome::Cancelled(CancelReason::RemotePeer),
        ),
        (
            ProgressTerminalOutcome::Cancelled(FileTransferCancellationReason::Replaced),
            TerminalOutcome::Cancelled(CancelReason::Replaced),
        ),
        (
            ProgressTerminalOutcome::Cancelled(FileTransferCancellationReason::Timeout),
            TerminalOutcome::Cancelled(CancelReason::Timeout),
        ),
        (
            ProgressTerminalOutcome::Cancelled(
                FileTransferCancellationReason::ConnectivityRecovery,
            ),
            TerminalOutcome::Cancelled(CancelReason::ConnectivityRecovery),
        ),
        (
            ProgressTerminalOutcome::Cancelled(FileTransferCancellationReason::Unknown),
            TerminalOutcome::Cancelled(CancelReason::Unknown),
        ),
    ] {
        prove_ack(
            AckChange::Exact,
            ProgressTerminal {
                outcome,
                ..terminal(10)
            },
            wire_outcome,
        )
        .await?;
    }
    Ok(())
}
