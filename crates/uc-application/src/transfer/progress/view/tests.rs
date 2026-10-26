//! 宿主视图保留已确认成果，计时失败只影响未结算项；共享错误仍能追溯原始来源。
use super::{error::SharedQueryFailure, mapping::present, TransferProgressQueryError};
use crate::connectivity::{ConnectivityError, ElapsedClockError};
use crate::transfer::progress::{
    ProgressProjectionKey, ProgressProjectionSnapshot, ProgressReceiveError, ProgressTaskDescriptor,
};
use std::error::Error;
use std::io;
use std::sync::Arc;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{
    summarize_progress, FileTransferCancellationReason, ProgressActive, ProgressFreshnessView,
    ProgressIdentity, ProgressItem, ProgressItemKind, ProgressItemState, ProgressPhase,
    ProgressTerminal, ProgressTerminalOutcome, TransferProgressItemStatus,
};
use uc_core::ids::DeviceId;

#[test]
fn clock_failure_keeps_confirmed_results_and_only_marks_unsettled_items_unconfirmed() {
    for (cancelled, displayed) in [
        (
            FileTransferCancellationReason::LocalUser,
            FileTransferCancellationReason::RemotePeer,
        ),
        (
            FileTransferCancellationReason::RemotePeer,
            FileTransferCancellationReason::LocalUser,
        ),
        (
            FileTransferCancellationReason::Timeout,
            FileTransferCancellationReason::Timeout,
        ),
    ] {
        let source = ProgressIdentity::new([1; 16]);
        let attempt = ProgressIdentity::new([2; 16]);
        let items = (0..3)
            .map(|index| ProgressItem {
                kind: ProgressItemKind::File,
                index,
            })
            .collect::<Vec<_>>();
        let values = vec![
            Some(ProgressItemState::Active(ProgressActive {
                item: items[0],
                round: 2,
                sequence: 9,
                bytes: 20,
                total: Some(100),
                phase: ProgressPhase::Retrying,
            })),
            Some(ProgressItemState::Terminal(ProgressTerminal {
                item: items[1],
                round: 0,
                sequence: 5,
                result_revision: 1,
                bytes: 100,
                total: Some(100),
                outcome: ProgressTerminalOutcome::Completed,
            })),
            Some(ProgressItemState::Terminal(ProgressTerminal {
                item: items[2],
                round: 1,
                sequence: 4,
                result_revision: 1,
                bytes: 40,
                total: Some(100),
                outcome: ProgressTerminalOutcome::Cancelled(cancelled),
            })),
        ];
        let summary =
            summarize_progress(values.iter().copied(), ProgressFreshnessView::Unconfirmed);
        let view = present(
            ProgressProjectionKey {
                peer: DeviceId::new("peer"),
                source,
            },
            ProgressProjectionSnapshot {
                freshness: Err(Arc::new(ElapsedClockError {
                    source: Box::new(io::Error::other("clock unavailable")),
                })),
                summary,
                version: 11,
                bound: None,
                task: Some(ProgressTaskDescriptor {
                    producer: ProgressIdentity::new([3; 16]),
                    revision: 4,
                    source,
                    attempt,
                    item_count: 3,
                }),
                items: items.into_iter().zip(values).collect(),
            },
        );
        assert!(!view.freshness_available);
        assert_eq!(view.freshness, ProgressFreshnessView::Unconfirmed);
        assert_eq!(view.revision, 11);
        assert_eq!(view.summary, summary);
        assert_eq!(view.summary.counts.completed, 1);
        assert_eq!(view.summary.counts.cancelled, 1);
        assert_eq!(view.summary.counts.unconfirmed, 1);
        assert_eq!(
            view.items[0].status,
            TransferProgressItemStatus::Unconfirmed
        );
        assert_eq!(
            view.items[0].last_known_phase,
            Some(ProgressPhase::Retrying)
        );
        assert_eq!(view.items[0].round, Some(2));
        assert_eq!(view.items[0].bytes, Some(20));
        assert_eq!(view.items[1].status, TransferProgressItemStatus::Completed);
        assert_eq!(view.items[2].status, TransferProgressItemStatus::Cancelled);
        assert_eq!(view.items[2].cancellation, Some(displayed));
        assert_eq!(
            view.attempt.as_deref(),
            Some("02020202-0202-0202-0202-020202020202")
        );
        assert_eq!(
            view.source_entry.as_ref(),
            "01010101-0101-0101-0101-010101010101"
        );
    }
}

#[test]
fn shared_query_failure_keeps_original_io_source_at_the_display_boundary() {
    let result = Arc::new(Err(ProgressReceiveError::Malformed(Box::new(
        io::Error::from(io::ErrorKind::PermissionDenied),
    ))));
    let error = TransferProgressQueryError::Unavailable(Box::new(SharedQueryFailure(result)));
    assert_eq!(error.class(), "progress_query_unavailable");
    assert_eq!(error.to_string(), "transfer progress is unavailable");
    let mut source: &dyn Error = &error;
    loop {
        if let Some(source) = source.downcast_ref::<io::Error>() {
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            break;
        }
        source = source.source().expect("original source chain");
    }
}

#[test]
fn remote_authorization_denial_is_not_reported_as_local_unavailability() {
    let outcome = Arc::new(Err(ProgressReceiveError::AccessDenied(Box::new(
        io::Error::from(io::ErrorKind::PermissionDenied),
    ))));
    let error = super::owner::classify(outcome).unwrap_err();
    assert_eq!(error.class(), "progress_query_access_denied");
    let mut source: &dyn Error = &error;
    loop {
        if let Some(source) = source.downcast_ref::<io::Error>() {
            assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
            break;
        }
        source = source.source().expect("original authorization source");
    }
}

#[test]
fn corrupt_reply_and_local_clock_failure_have_different_stable_categories() {
    for (source, expected) in [
        (
            ProgressReceiveError::Malformed(Box::new(io::Error::from(
                io::ErrorKind::UnexpectedEof,
            ))),
            "progress_query_invalid_data",
        ),
        (
            ProgressReceiveError::Clock(ElapsedClockError {
                source: Box::new(io::Error::other("clock unavailable")),
            }),
            "progress_query_unavailable",
        ),
        (
            ProgressReceiveError::Connection(ConnectivityError::Denied),
            "progress_query_access_denied",
        ),
    ] {
        let error = super::owner::classify(Arc::new(Err(source))).unwrap_err();
        assert_eq!(error.class(), expected);
        assert!(error.source().is_some());
    }
}
