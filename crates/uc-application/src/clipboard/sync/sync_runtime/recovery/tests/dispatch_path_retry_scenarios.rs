//! 经真实发送用例与记录负责人的端到端重试：写入失败、终态结果与后台收尾。

use super::super::schedule::RETRY_BACKOFF;
use super::support::*;
use super::*;

/// 在线目标的重试轮次里 `Pending` 意图写入失败一次：恢复负责人仍按退避再试，
/// 存储恢复后送达，对端只保存一份副本。
#[tokio::test(start_paused = true)]
async fn a_failed_retry_intent_write_keeps_the_delivery_in_recovery() {
    let harness = RealPathHarness::start(&[PeerStep::Interrupted, PeerStep::Receive]);
    drain().await;
    harness.copy().await;
    drain().await;
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    harness.deliveries.fail_next(BrokenWrite::Intent);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        1,
        "a failed intent write sends nothing"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    tokio::time::sleep(RETRY_BACKOFF[1]).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);

    tokio::time::sleep(Duration::from_secs(600)).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2, "a delivered target is not retried");
    harness.shutdown().await;
}

/// 在线目标的重试轮次再次中断，且最终 `Unreachable` 写入失败一次（持久状态停在 `Pending`）：
/// 恢复负责人仍按退避再试，存储恢复后送达，对端只保存一份副本。
#[tokio::test(start_paused = true)]
async fn a_failed_retry_result_write_keeps_the_delivery_in_recovery() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::Interrupted,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;

    harness.deliveries.fail_next(BrokenWrite::Final);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Pending));

    tokio::time::sleep(RETRY_BACKOFF[1]).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 3);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}

/// 首次自动发送的最终写入失败（持久状态停在 `Pending`）：结果仍交给恢复负责人，
/// 无需新的上线事件或重新复制即可送达。
#[tokio::test(start_paused = true)]
async fn an_initial_send_whose_result_write_failed_still_reaches_recovery() {
    let harness = RealPathHarness::start(&[PeerStep::Interrupted, PeerStep::Receive]);
    drain().await;
    harness.deliveries.fail_next(BrokenWrite::Final);
    harness.copy().await;
    drain().await;
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Pending));

    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}

/// 重试以准入拒绝终结：落为终态失败、不再重试，且该目标的退避状态被清除。
/// 清除与否由之后同一内容的新可恢复结果的首个间隔区分：残留状态会沿用已增长的间隔。
#[tokio::test(start_paused = true)]
async fn a_retry_that_ends_terminally_clears_the_target_backoff() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::Denied,
        PeerStep::Interrupted,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(
        harness.status(),
        Some(EntryDeliveryStatus::Failed {
            reason: DeliveryFailureReason::Io
        })
    );
    tokio::time::sleep(Duration::from_secs(600)).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2, "a denied delivery is not retried");

    harness.resend().await;
    drain().await;
    assert_eq!(harness.peer.dials(), 3);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        4,
        "the next recoverable result starts from the first interval"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.shutdown().await;
}

/// 重试越过前台期限、随后在后台收尾中送达：该目标的退避状态被清除。
#[tokio::test(start_paused = true)]
async fn a_retry_completed_by_the_deferred_drain_clears_the_target_backoff() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::ReceiveAfter(Duration::from_secs(6)),
        PeerStep::Interrupted,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;
    tokio::time::sleep(RETRY_BACKOFF[0] + Duration::from_secs(6) + JUST_BEFORE).await;
    drain().await;
    assert_eq!(harness.peer.dials(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    assert_eq!(harness.peer.copies(), 1);

    harness.resend().await;
    drain().await;
    assert_eq!(harness.peer.dials(), 3);
    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        4,
        "the next recoverable result starts from the first interval"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Duplicate));
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}

/// 对端每次都确认收下，但最终结果写入一直失败（持久状态停在 `Pending`）：已确认不等于已结算，
/// 恢复负责人只按记录负责人的结算通知清除退避，因此同一条目仍只有 3 次计时重试，
/// 不会因每次确认后又收到“未能落库”而无限重新开始计时。
#[tokio::test(start_paused = true)]
async fn acknowledged_retries_whose_result_write_keeps_failing_stop_after_three() {
    let harness = RealPathHarness::start(&[
        PeerStep::Interrupted,
        PeerStep::Receive,
        PeerStep::Receive,
        PeerStep::Receive,
    ]);
    drain().await;
    harness.copy().await;
    drain().await;
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    harness.deliveries.fail_every(BrokenWrite::Final);
    let mut due = Instant::now();
    for (retry, delay) in RETRY_BACKOFF.iter().enumerate() {
        due += *delay;
        tokio::time::sleep_until(due - JUST_BEFORE).await;
        drain().await;
        assert_eq!(
            harness.peer.dials(),
            retry + 1,
            "retry {retry} waits for its interval"
        );
        tokio::time::sleep_until(due + JUST_BEFORE).await;
        drain().await;
        assert_eq!(harness.peer.dials(), retry + 2);
        assert_eq!(harness.status(), Some(EntryDeliveryStatus::Pending));
    }

    tokio::time::sleep(Duration::from_secs(600)).await;
    drain().await;
    assert_eq!(
        harness.peer.dials(),
        RETRY_BACKOFF.len() + 1,
        "no fourth timed retry while the result write keeps failing"
    );
    assert_eq!(harness.peer.copies(), 1);
    harness.shutdown().await;
}
