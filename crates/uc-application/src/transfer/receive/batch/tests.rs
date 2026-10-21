use super::ReceiveBatch;
use crate::transfer::blob::notifications::OutboundNotifications;
use crate::transfer::blob::test_support::RejectUnexpectedNetwork;
use crate::transfer::blob::transport::{BlobShutdownWait, BlobTransport};
use async_trait::async_trait;
use std::io;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;
use tokio::time::{advance, Duration};
use uc_core::file_transfer::{OutboundProgressReporterPort, OutboundProgressStatus};
use uc_core::ids::DeviceId;
use uc_core::ports::AttemptState;

#[derive(Default)]
struct Reporter {
    records: Mutex<Vec<(u64, Option<u64>, OutboundProgressStatus)>>,
    terminal: Notify,
}
#[async_trait]
impl OutboundProgressReporterPort for Reporter {
    async fn report(
        &self,
        _: &DeviceId,
        _: &str,
        bytes: u64,
        total: Option<u64>,
        status: OutboundProgressStatus,
    ) {
        let terminal = !matches!(status, OutboundProgressStatus::InProgress);
        self.records.lock().unwrap().push((bytes, total, status));
        if terminal {
            self.terminal.notify_one();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_handoff_waits_for_persistence_and_last_success_never_erases_failure() {
    for child_failed in [false, true] {
        let transport = BlobTransport::new(
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
        );
        let reporter = Arc::new(Reporter::default());
        let notifications = OutboundNotifications::new(reporter.clone(), &transport);
        let batch = ReceiveBatch::default();
        let peer = DeviceId::new("source");
        batch.plan(2, 20);
        batch.record(
            notifications.clone(),
            peer,
            "remote",
            "first",
            10,
            OutboundProgressStatus::Completed,
        );
        batch.item_result(
            "first",
            if child_failed { 0 } else { 10 },
            if child_failed {
                OutboundProgressStatus::Failed
            } else {
                OutboundProgressStatus::Completed
            },
            if child_failed {
                Some(io::Error::from(io::ErrorKind::PermissionDenied).into())
            } else {
                None
            },
        );
        advance(Duration::from_millis(200)).await;
        batch.record(
            notifications,
            peer,
            "remote",
            "last",
            10,
            OutboundProgressStatus::Completed,
        );
        batch.item_result("last", 10, OutboundProgressStatus::Completed, None);
        tokio::task::yield_now().await;
        assert!(reporter
            .records
            .lock()
            .unwrap()
            .iter()
            .all(|(_, _, status)| matches!(status, OutboundProgressStatus::InProgress)));
        if child_failed {
            let state = batch.0.lock().unwrap();
            let source = state.items["first"].source.as_ref().unwrap();
            assert_eq!(
                source.downcast_ref::<io::Error>().unwrap().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        // 没有子项失败时模拟持久化失败；两种情况都不能报告成功。
        if child_failed {
            batch.settled(AttemptState::Failed);
        }
        batch.finish(child_failed);
        reporter.terminal.notified().await;
        let records = reporter.records.lock().unwrap().clone();
        assert!(matches!(
            records.last().unwrap().2,
            OutboundProgressStatus::Failed
        ));
        assert!(!records
            .iter()
            .any(|(_, _, status)| matches!(status, OutboundProgressStatus::Completed)));
        transport
            .shutdown(BlobShutdownWait::Complete)
            .await
            .unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn combined_progress_is_bounded_by_five_hz_and_terminal_is_exactly_once() {
    let transport = BlobTransport::new(
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
        Arc::new(RejectUnexpectedNetwork),
    );
    let reporter = Arc::new(Reporter::default());
    let notifications = OutboundNotifications::new(reporter.clone(), &transport);
    let batch = ReceiveBatch::default();
    let peer = DeviceId::new("source");
    batch.plan(2, 20);
    for index in 0..100 {
        batch.record(
            notifications.clone(),
            peer,
            "remote",
            "first",
            index.min(10),
            OutboundProgressStatus::InProgress,
        );
        tokio::task::yield_now().await;
    }
    assert_eq!(reporter.records.lock().unwrap().len(), 1);
    advance(Duration::from_millis(200)).await;
    batch.record(
        notifications,
        peer,
        "remote",
        "second",
        10,
        OutboundProgressStatus::Completed,
    );
    batch.item_result("first", 10, OutboundProgressStatus::Completed, None);
    batch.item_result("second", 10, OutboundProgressStatus::Completed, None);
    batch.settled(AttemptState::Completed);
    batch.finish(true);
    batch.finish(true);
    reporter.terminal.notified().await;
    let records = reporter.records.lock().unwrap().clone();
    assert_eq!(
        records
            .iter()
            .filter(|(_, _, status)| matches!(status, OutboundProgressStatus::Completed))
            .count(),
        1
    );
    assert_eq!(records.last().unwrap().0, 20);
    assert_eq!(records.last().unwrap().1, Some(20));
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
}
