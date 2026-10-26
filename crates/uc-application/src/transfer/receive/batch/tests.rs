//! 子下载完成与持久结算之间的围栏；最后成功不能覆盖已有失败。
use super::ReceiveBatch;
use std::io;
use uc_core::file_transfer::OutboundProgressStatus;
use uc_core::ports::AttemptState;

#[test]
fn terminal_handoff_waits_for_persistence_and_last_success_never_erases_failure() {
    for child_failed in [false, true] {
        let batch = ReceiveBatch::default();
        batch.plan(2);
        batch.item_result(
            "first",
            if child_failed { 0 } else { 10 },
            if child_failed {
                OutboundProgressStatus::Failed
            } else {
                OutboundProgressStatus::Completed
            },
            child_failed.then(|| io::Error::from(io::ErrorKind::PermissionDenied).into()),
        );
        batch.item_result("last", 10, OutboundProgressStatus::Completed, None);
        let facts = batch.facts();
        assert!(!facts.finished);
        assert_eq!(facts.settled, None);
        if child_failed {
            let state = batch.state.lock().unwrap();
            assert_eq!(
                state.items["first"]
                    .source
                    .as_ref()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            drop(state);
            batch.settled(AttemptState::Failed);
        }
        // 持久化失败或原子项失败都必须保留失败；重复结束不能改写。
        batch.finish(child_failed);
        batch.finish(true);
        let facts = batch.facts();
        assert!(facts.finished);
        assert_eq!(facts.settled, Some(AttemptState::Failed));
    }
}

#[test]
fn only_persisted_completion_survives_repeated_finish() {
    let batch = ReceiveBatch::default();
    batch.plan(2);
    batch.item_result("first", 10, OutboundProgressStatus::Completed, None);
    batch.item_result("second", 10, OutboundProgressStatus::Completed, None);
    assert_eq!(batch.facts().settled, None);
    batch.settled(AttemptState::Completed);
    batch.finish(true);
    batch.finish(false);
    let facts = batch.facts();
    assert!(facts.finished);
    assert_eq!(facts.settled, Some(AttemptState::Completed));
}

#[path = "round_tests.rs"]
mod round_tests;
