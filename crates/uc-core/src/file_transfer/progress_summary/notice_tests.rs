use super::summarize_outbound_progress;
use crate::file_transfer::{
    FileTransferCancellationReason, OutboundProgressStatus, ProgressActive, ProgressFailureReason,
    ProgressItem, ProgressItemKind, ProgressItemState, ProgressPhase, ProgressSnapshotRefusal,
    ProgressTerminal, ProgressTerminalOutcome, MAX_PROGRESS_ITEMS,
};
fn item(index: u16) -> ProgressItem {
    ProgressItem {
        kind: ProgressItemKind::File,
        index,
    }
}
fn done(index: u16, outcome: ProgressTerminalOutcome) -> ProgressItemState {
    ProgressItemState::Terminal(ProgressTerminal {
        item: item(index),
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 8,
        total: Some(8),
        outcome,
    })
}
#[test]
fn last_completed_item_cannot_hide_an_earlier_real_failure() {
    let summary = summarize_outbound_progress(&[
        done(
            0,
            ProgressTerminalOutcome::Failed(ProgressFailureReason::StorageUnavailable),
        ),
        done(1, ProgressTerminalOutcome::Completed),
    ])
    .unwrap();
    assert_eq!(summary.status, OutboundProgressStatus::Failed);
    assert_eq!((summary.bytes, summary.total), (16, Some(16)));
}
#[test]
fn waiting_processing_and_retrying_block_batch_completion_even_at_full_bytes() {
    for phase in [
        ProgressPhase::Waiting,
        ProgressPhase::Processing,
        ProgressPhase::Retrying,
        ProgressPhase::Transferring,
    ] {
        let active = ProgressItemState::Active(ProgressActive {
            item: item(0),
            round: 2,
            sequence: 1,
            bytes: 8,
            total: Some(8),
            phase,
        });
        assert_eq!(
            summarize_outbound_progress(&[active, done(1, ProgressTerminalOutcome::Completed)])
                .unwrap()
                .status,
            OutboundProgressStatus::InProgress
        );
    }
}
#[test]
fn cancellation_keeps_an_agreed_reason_and_mixed_causes_are_unknown() {
    let cancelled = |index, reason| done(index, ProgressTerminalOutcome::Cancelled(reason));
    let first = cancelled(0, FileTransferCancellationReason::LocalUser);
    assert_eq!(
        summarize_outbound_progress(&[first, done(1, ProgressTerminalOutcome::Completed)])
            .unwrap()
            .status,
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::LocalUser
        }
    );
    assert_eq!(
        summarize_outbound_progress(&[
            first,
            cancelled(1, FileTransferCancellationReason::Timeout)
        ])
        .unwrap()
        .status,
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::Unknown
        }
    );
}
#[test]
fn maximum_plan_and_completed_empty_directory_use_actual_terminals() {
    let items = (0..MAX_PROGRESS_ITEMS as u16)
        .map(|index| done(index, ProgressTerminalOutcome::Completed))
        .collect::<Vec<_>>();
    let summary = summarize_outbound_progress(&items).unwrap();
    assert_eq!(summary.status, OutboundProgressStatus::Completed);
    assert_eq!(
        (summary.bytes, summary.total),
        (
            MAX_PROGRESS_ITEMS as u64 * 8,
            Some(MAX_PROGRESS_ITEMS as u64 * 8)
        )
    );
    let empty = ProgressItemState::Terminal(ProgressTerminal {
        item: ProgressItem {
            kind: ProgressItemKind::DirectoryUnit,
            index: 0,
        },
        round: 0,
        sequence: 1,
        result_revision: 1,
        bytes: 0,
        total: Some(0),
        outcome: ProgressTerminalOutcome::Completed,
    });
    assert_eq!(
        summarize_outbound_progress(&[empty]).unwrap().status,
        OutboundProgressStatus::Completed
    );
}
#[test]
fn unknown_or_unrepresentable_total_stays_unknown_but_byte_overflow_is_refused() {
    let active = |index, bytes, total| {
        ProgressItemState::Active(ProgressActive {
            item: item(index),
            round: 0,
            sequence: 1,
            bytes,
            total,
            phase: ProgressPhase::Transferring,
        })
    };
    assert_eq!(
        summarize_outbound_progress(&[active(0, 1, None), active(1, 1, Some(8))])
            .unwrap()
            .total,
        None
    );
    assert_eq!(
        summarize_outbound_progress(&[active(0, 1, Some(u64::MAX)), active(1, 1, Some(u64::MAX))])
            .unwrap()
            .total,
        None
    );
    assert_eq!(
        summarize_outbound_progress(&[active(0, u64::MAX, None), active(1, 1, None)]).unwrap_err(),
        ProgressSnapshotRefusal::InvalidState
    );
}
#[test]
fn empty_duplicate_and_invalid_states_cannot_become_a_summary() {
    assert_eq!(
        summarize_outbound_progress(&[]).unwrap_err(),
        ProgressSnapshotRefusal::InvalidPlan
    );
    let one = done(0, ProgressTerminalOutcome::Completed);
    assert_eq!(
        summarize_outbound_progress(&[one, one]).unwrap_err(),
        ProgressSnapshotRefusal::InvalidPlan
    );
    let invalid = ProgressItemState::Active(ProgressActive {
        item: item(0),
        round: 0,
        sequence: 1,
        bytes: 9,
        total: Some(8),
        phase: ProgressPhase::Transferring,
    });
    assert_eq!(
        summarize_outbound_progress(&[invalid]).unwrap_err(),
        ProgressSnapshotRefusal::InvalidState
    );
}
