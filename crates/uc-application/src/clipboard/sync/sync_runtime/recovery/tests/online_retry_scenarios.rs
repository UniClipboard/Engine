//! 设备保持在线时的有界退避重试：同一条目最多 3 次计时重试，结果经脚本化补发产生。

use super::super::schedule::RETRY_BACKOFF;
use super::support::*;
use super::*;

/// presence 一直在线、从未产生新的上线事件：内容拨号失败后按退避自动重试，
/// 第二次失败后间隔增长，第三次成功即停止，用户无需重新复制。
#[tokio::test(start_paused = true)]
async fn online_target_retries_a_failed_content_dial_without_a_new_online_event() {
    let harness = RetryHarness::start(None, &[Attempt::Interrupted, Attempt::Accepted]);
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;

    tokio::time::sleep(RETRY_BACKOFF[0] - JUST_BEFORE).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        0,
        "no retry before the first backoff"
    );
    tokio::time::sleep(JUST_BEFORE * 2).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    tokio::time::sleep(RETRY_BACKOFF[1] - JUST_BEFORE * 3).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        1,
        "second retry waits for a longer backoff"
    );
    tokio::time::sleep(JUST_BEFORE * 3).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));

    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        2,
        "a delivered target is not retried"
    );
    harness.recovery.shutdown().await.unwrap();
}

/// 流在确认前被重置（落为可恢复结果）后，重试得到重复确认即结束；
/// 接收端的去重保证确认丢失时不会重复写入。
#[tokio::test(start_paused = true)]
async fn lost_ack_retry_settles_as_duplicate_without_a_new_copy() {
    let harness = RetryHarness::start(None, &[Attempt::Duplicate]);
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;

    tokio::time::sleep(RETRY_BACKOFF[0] + JUST_BEFORE).await;
    settle().await;

    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Duplicate));
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    harness.recovery.shutdown().await.unwrap();
}

/// 终态失败（策略拒绝、协议不兼容、不可重试错误）不会触发任何自动重试。
#[tokio::test(start_paused = true)]
async fn terminal_results_are_never_retried() {
    for reason in [
        DeliveryFailureReason::PeerRejected,
        DeliveryFailureReason::PeerIncompatible,
        DeliveryFailureReason::LocalPolicy,
        DeliveryFailureReason::Io,
        DeliveryFailureReason::Internal,
    ] {
        let harness = RetryHarness::start(None, &[]);
        settle().await;
        harness
            .record_dispatch(EntryDeliveryStatus::Failed { reason })
            .await;
        tokio::time::sleep(Duration::from_secs(600)).await;
        settle().await;
        assert_eq!(harness.delivery.calls(), 0);
        harness.recovery.shutdown().await.unwrap();
    }
}

/// 到期时目标已离线：不空转重试，改由下一次上线事件恢复。
#[tokio::test(start_paused = true)]
async fn due_retry_for_an_offline_target_waits_for_the_next_online_event() {
    let harness = RetryHarness::start(None, &[Attempt::Accepted]);
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;
    harness.reachability.set(ReachabilityState::Offline);

    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 0);

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 恢复轮次自身失败（补发前存储失败，没有新的投递结果）也按退避再试。
#[tokio::test(start_paused = true)]
async fn a_failed_recovery_round_is_retried_with_backoff() {
    let harness = RetryHarness::start(
        Some(EntryDeliveryStatus::Unreachable),
        &[Attempt::StorageFailure, Attempt::Accepted],
    );
    settle().await;
    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);

    tokio::time::sleep(RETRY_BACKOFF[0] - JUST_BEFORE).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    tokio::time::sleep(JUST_BEFORE * 2).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// presence 一直在线、每次补发都中断：同一条目只计时重试 3 次（约 2、5、10 秒后），
/// 第三次失败发布的可恢复结果不再排期，长时间内不再补发；之后只有新的上线事件才再补发一次。
#[tokio::test(start_paused = true)]
async fn an_online_target_gets_exactly_three_timed_retries_for_the_same_entry() {
    let harness = RetryHarness::start(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
    );
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    settle().await;

    // 每次重试在上一结果到达后按下一个间隔到期；暂停时钟下结果在到期时刻即被处理。
    let mut due = Instant::now();
    for (retry, delay) in RETRY_BACKOFF.iter().enumerate() {
        due += *delay;
        tokio::time::sleep_until(due - JUST_BEFORE).await;
        settle().await;
        assert_eq!(
            harness.delivery.calls(),
            retry,
            "retry {retry} waits for its interval"
        );
        tokio::time::sleep_until(due + JUST_BEFORE).await;
        settle().await;
        assert_eq!(harness.delivery.calls(), retry + 1);
        assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));
    }

    tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        RETRY_BACKOFF.len(),
        "no timed retry after the third failed retry while the target stays online"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Unreachable));

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        RETRY_BACKOFF.len() + 1,
        "the next online event resends the newest undelivered entry once"
    );
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}
