use super::*;
use crate::file_transfer::{
    FileTransferCancellationReason, ProgressActive, ProgressFailureReason, ProgressFreshnessView,
    ProgressItem, ProgressItemKind, ProgressItemState, ProgressPhase, ProgressTerminal,
    ProgressTerminalOutcome, MAX_PROGRESS_ITEMS,
};

fn active(phase: ProgressPhase) -> Option<ProgressItemState> {
    Some(ProgressItemState::Active(ProgressActive {
        item: ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        },
        round: 0,
        sequence: 1,
        bytes: 8,
        total: Some(8),
        phase,
    }))
}
fn terminal(outcome: ProgressTerminalOutcome) -> Option<ProgressItemState> {
    Some(ProgressItemState::Terminal(ProgressTerminal {
        item: ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        },
        round: 0,
        sequence: 2,
        result_revision: 1,
        bytes: 8,
        total: Some(8),
        outcome,
    }))
}

#[test]
fn empty_retired_scope_is_not_a_successful_batch() {
    for freshness in [
        ProgressFreshnessView::Waiting,
        ProgressFreshnessView::Settled,
        ProgressFreshnessView::Unconfirmed,
    ] {
        let summary = summarize_progress([], freshness);
        assert_eq!(summary.state, ProgressBatchState::NoItems);
        assert_eq!(summary.counts, ProgressBatchCounts::default());
    }
}

#[test]
fn waiting_and_real_work_remain_counted_until_business_settlement() {
    let summary = summarize_progress(
        [
            None,
            active(ProgressPhase::Waiting),
            active(ProgressPhase::Transferring),
            active(ProgressPhase::Processing),
            active(ProgressPhase::Retrying),
            terminal(ProgressTerminalOutcome::Completed),
        ],
        ProgressFreshnessView::Live,
    );
    assert_eq!(summary.state, ProgressBatchState::InProgress);
    assert_eq!(
        summary.counts,
        ProgressBatchCounts {
            total: 6,
            waiting: 2,
            running: 3,
            completed: 1,
            ..ProgressBatchCounts::default()
        }
    );
    assert_eq!(
        summarize_progress([None], ProgressFreshnessView::Waiting).state,
        ProgressBatchState::Waiting
    );
}

#[test]
fn unknown_results_do_not_become_failures_or_erase_known_results() {
    let summary = summarize_progress(
        [
            None,
            active(ProgressPhase::Processing),
            terminal(ProgressTerminalOutcome::Completed),
            terminal(ProgressTerminalOutcome::Failed(
                ProgressFailureReason::StorageUnavailable,
            )),
            terminal(ProgressTerminalOutcome::Cancelled(
                FileTransferCancellationReason::LocalUser,
            )),
        ],
        ProgressFreshnessView::Unconfirmed,
    );
    assert_eq!(summary.state, ProgressBatchState::PartiallyUnconfirmed);
    assert_eq!(
        summary.counts,
        ProgressBatchCounts {
            total: 5,
            unconfirmed: 2,
            completed: 1,
            failed: 1,
            cancelled: 1,
            ..ProgressBatchCounts::default()
        }
    );
    let all_unknown = summarize_progress(
        [None, active(ProgressPhase::Retrying)],
        ProgressFreshnessView::Unconfirmed,
    );
    assert_eq!(all_unknown.state, ProgressBatchState::Unconfirmed);
    assert_eq!(all_unknown.counts.failed, 0);
    assert_eq!(all_unknown.counts.unconfirmed, 2);
}

#[test]
fn final_result_requires_every_item_and_preserves_each_failure_and_cancellation() {
    let completed = terminal(ProgressTerminalOutcome::Completed);
    let failed = terminal(ProgressTerminalOutcome::Failed(
        ProgressFailureReason::StorageUnavailable,
    ));
    let cancelled = terminal(ProgressTerminalOutcome::Cancelled(
        FileTransferCancellationReason::LocalUser,
    ));
    for (items, expected) in [
        (vec![completed, completed], ProgressBatchState::Completed),
        (vec![failed, failed], ProgressBatchState::Failed),
        (vec![cancelled, cancelled], ProgressBatchState::Cancelled),
        (vec![completed, failed], ProgressBatchState::PartiallyFailed),
        (vec![cancelled, failed], ProgressBatchState::PartiallyFailed),
        (
            vec![completed, cancelled],
            ProgressBatchState::PartiallyCancelled,
        ),
        (
            vec![completed, failed, cancelled],
            ProgressBatchState::PartiallyFailed,
        ),
    ] {
        for freshness in [
            ProgressFreshnessView::Live,
            ProgressFreshnessView::Unconfirmed,
            ProgressFreshnessView::Settled,
        ] {
            let summary = summarize_progress(items.clone(), freshness);
            assert_eq!(summary.state, expected);
            assert_eq!(summary.counts.total, items.len());
            assert_eq!(
                summary.counts.completed + summary.counts.failed + summary.counts.cancelled,
                items.len()
            );
        }
    }
    assert_eq!(
        summarize_progress(
            [failed, active(ProgressPhase::Processing)],
            ProgressFreshnessView::Live
        )
        .state,
        ProgressBatchState::InProgress
    );
    assert_eq!(
        summarize_progress([failed, None], ProgressFreshnessView::Waiting).state,
        ProgressBatchState::Waiting
    );
}

#[test]
fn directory_with_all_network_bytes_is_still_one_processing_unit() {
    let directory = ProgressItem {
        kind: ProgressItemKind::DirectoryUnit,
        index: 0,
    };
    let Some(ProgressItemState::Active(mut processing)) = active(ProgressPhase::Processing) else {
        panic!("active fixture")
    };
    processing.item = directory;
    let pending = summarize_progress(
        [Some(ProgressItemState::Active(processing))],
        ProgressFreshnessView::Live,
    );
    assert_eq!(pending.state, ProgressBatchState::InProgress);
    assert_eq!(pending.counts.running, 1);
    assert_eq!(pending.counts.completed, 0);
    let Some(ProgressItemState::Terminal(mut result)) =
        terminal(ProgressTerminalOutcome::Completed)
    else {
        panic!("terminal fixture")
    };
    result.item = directory;
    let complete = summarize_progress(
        [Some(ProgressItemState::Terminal(result))],
        ProgressFreshnessView::Settled,
    );
    assert_eq!(complete.state, ProgressBatchState::Completed);
    assert_eq!(complete.counts.total, 1);
}

#[test]
fn largest_original_plan_keeps_exact_partition_counts() {
    let summary = summarize_progress(
        (0..MAX_PROGRESS_ITEMS).map(|_| None),
        ProgressFreshnessView::Waiting,
    );
    assert_eq!(summary.counts.total, MAX_PROGRESS_ITEMS);
    assert_eq!(summary.counts.waiting, MAX_PROGRESS_ITEMS);
    assert_eq!(summary.state, ProgressBatchState::Waiting);
}
