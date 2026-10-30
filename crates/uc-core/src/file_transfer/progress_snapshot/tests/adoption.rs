use super::plan;
use crate::file_transfer::{
    merge_progress_snapshot, ProgressActive, ProgressItemState, ProgressPhase,
    ProgressSnapshotMerge, ProgressSnapshotRefusal, ProgressTerminal, ProgressTerminalOutcome,
};
fn active(index: u16, sequence: u64, bytes: u64) -> ProgressItemState {
    ProgressItemState::Active(ProgressActive {
        item: plan(3)[usize::from(index)],
        round: 0,
        sequence,
        bytes,
        total: Some(10),
        phase: ProgressPhase::Transferring,
    })
}
fn terminal(index: u16, bytes: u64) -> ProgressItemState {
    ProgressItemState::Terminal(ProgressTerminal {
        item: plan(3)[usize::from(index)],
        round: 0,
        sequence: 3,
        result_revision: 1,
        bytes,
        total: Some(10),
        outcome: ProgressTerminalOutcome::Completed,
    })
}
#[test]
fn complete_snapshot_recovers_missing_items_without_rolling_back_newer_updates_or_terminal_facts() {
    let current = [None, Some(active(1, 8, 8)), Some(terminal(2, 10))];
    let incoming = [terminal(0, 10), active(1, 1, 1), active(2, 2, 2)];
    let ProgressSnapshotMerge::Adopt(merged) =
        merge_progress_snapshot(&plan(3), &current, &incoming).unwrap()
    else {
        panic!("missing item must be recovered");
    };
    assert_eq!(
        merged.iter().map(|item| item.state()).collect::<Vec<_>>(),
        vec![Some(incoming[0]), current[1], current[2]]
    );
}
#[test]
fn late_conflict_refuses_whole_snapshot_and_incomplete_or_invalid_frames_never_authorize_adoption()
{
    let current = [None, Some(terminal(1, 10))];
    assert!(matches!(
        merge_progress_snapshot(&plan(2), &current, &[terminal(0, 10), terminal(1, 9)]),
        Err(ProgressSnapshotRefusal::StateConflict)
    ));
    assert!(matches!(
        merge_progress_snapshot(&plan(2), &current, &[terminal(0, 10)]),
        Err(ProgressSnapshotRefusal::WrongTotal)
    ));
    assert!(matches!(
        merge_progress_snapshot(&plan(2), &current, &[terminal(1, 10), terminal(0, 10)]),
        Err(ProgressSnapshotRefusal::WrongItem)
    ));
    assert!(matches!(
        merge_progress_snapshot(&plan(2), &current, &[terminal(0, 11), terminal(1, 10)]),
        Err(ProgressSnapshotRefusal::InvalidState)
    ));
}
#[test]
fn duplicate_or_stale_full_snapshot_has_no_save_or_publish_obligation() {
    let current = [Some(active(0, 8, 8)), Some(terminal(1, 10))];
    assert!(matches!(
        merge_progress_snapshot(&plan(2), &current, &[active(0, 7, 7), active(1, 1, 1)]).unwrap(),
        ProgressSnapshotMerge::Unchanged
    ));
}
