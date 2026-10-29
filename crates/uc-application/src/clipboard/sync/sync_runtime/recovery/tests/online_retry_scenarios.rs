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

/// 较旧条目 A 的结果迟到：A 仍在后台收尾时，较新的条目 B 已发出并用尽 3 次计时重试；仓库对 A 的
/// 写入覆盖其“已取代”状态并发布可恢复结果。A 不是该目标当前最新的未送达内容，
/// 它的结果既不重新开始计时，也不让 B 再获得任何计时重试；之后只有上线事件再补发 B 一次。
#[tokio::test(start_paused = true)]
async fn a_late_result_for_an_older_entry_does_not_restart_the_newer_entrys_retries() {
    let harness = RetryHarness::start_with(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
        RetryScenario {
            older_entry: Some(EntryDeliveryStatus::Superseded),
            ..RetryScenario::default()
        },
    );
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len());

    harness
        .record_results([(
            harness.older.entry_id.clone(),
            harness.target,
            EntryDeliveryStatus::Unreachable,
        )])
        .await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        RETRY_BACKOFF.len(),
        "a late result for older content gives the exhausted newer entry no retry"
    );

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len() + 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 多于结果通知缓冲上限（64）的一批结果，在恢复负责人处理之前一次写入。
const OVERFLOWING_RESULTS: usize = 80;

/// 与恢复无关的目标的一批已结算结果，足以让此前的结果通知积压丢失。
fn unrelated_results() -> impl Iterator<Item = (EntryId, DeviceId, EntryDeliveryStatus)> {
    (0..OVERFLOWING_RESULTS).map(|index| {
        (
            EntryId::from(format!("unrelated-entry-{index}").as_str()),
            DeviceId::new(&format!("unrelated-target-{index}")),
            EntryDeliveryStatus::Delivered,
        )
    })
}

/// 其他目标的大量结果让通知积压丢失：恢复负责人按持久记录核对丢失的通知，只把仍有未送达
/// 内容的目标排入原有退避，不立即补发，也不清除已用尽标记，因此已用尽 3 次的条目不再补发。
#[tokio::test(start_paused = true)]
async fn lagged_results_do_not_resend_to_a_target_that_exhausted_its_retries() {
    let harness = RetryHarness::start_with(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
        RetryScenario {
            known_target: true,
            ..RetryScenario::default()
        },
    );
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len());

    harness.record_results(unrelated_results()).await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        RETRY_BACKOFF.len(),
        "lost notifications give an exhausted entry no new send"
    );

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len() + 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 大量在线目标的可恢复结果同时积压丢失：核对只排期、不在核对时补发；之后的重试一次只处理
/// 一个到期目标，其结果在下一个到期前就被处理，不会再次超出缓冲上限，每个目标仍只有 3 次计时重试。
#[tokio::test(start_paused = true)]
async fn reconciling_lagged_results_does_not_sustain_another_overflow() {
    let harness = RetryHarness::start_with(
        None,
        &[Attempt::Interrupted; OVERFLOWING_RESULTS * RETRY_BACKOFF.len()],
        RetryScenario {
            other_targets: OVERFLOWING_RESULTS,
            ..RetryScenario::default()
        },
    );
    // 暂停时钟下 sleep 只在其余任务都空闲后推进，启动扫描在此之前结束。
    tokio::time::sleep(JUST_BEFORE).await;

    harness
        .record_results((0..OVERFLOWING_RESULTS).map(|index| {
            let (target, entry) = other_target(index);
            (entry.entry_id, target, EntryDeliveryStatus::Unreachable)
        }))
        .await;
    tokio::time::sleep(JUST_BEFORE).await;
    assert_eq!(
        harness.delivery.calls(),
        0,
        "reconciling lost notifications sends nothing by itself"
    );

    tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
    settle().await;
    assert_eq!(
        harness.delivery.calls(),
        OVERFLOWING_RESULTS * RETRY_BACKOFF.len(),
        "every target keeps exactly three timed retries"
    );
    harness.recovery.shutdown().await.unwrap();
}

/// 通知积压丢失后核对时配对设备列表读取失败：核对按退避再做一次，丢失通知的目标随后仍被补发。
#[tokio::test(start_paused = true)]
async fn lagged_reconciliation_retries_after_the_peer_list_read_fails() {
    let harness = RetryHarness::start_with(
        None,
        &[Attempt::Accepted],
        RetryScenario {
            known_target: true,
            ..RetryScenario::default()
        },
    );
    settle().await;
    harness.reads.failing_peer_lists.store(1, Ordering::SeqCst);
    let lost = (
        harness.entry.entry_id.clone(),
        harness.target,
        EntryDeliveryStatus::Unreachable,
    );
    harness
        .record_results(std::iter::once(lost).chain(unrelated_results()))
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 1);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 核对时第一个目标的条目列表读取失败：不放弃后面的目标，失败的目标按退避再核对后同样被补发。
#[tokio::test(start_paused = true)]
async fn lagged_reconciliation_continues_past_a_failed_target_and_rechecks_it() {
    let harness = RetryHarness::start_with(
        None,
        &[Attempt::Accepted, Attempt::Accepted],
        RetryScenario {
            other_targets: 1,
            ..RetryScenario::default()
        },
    );
    settle().await;
    harness.reads.failing_listings.store(1, Ordering::SeqCst);
    let (other, other_entry) = other_target(0);
    let lost = [
        (
            harness.entry.entry_id.clone(),
            harness.target,
            EntryDeliveryStatus::Unreachable,
        ),
        (
            other_entry.entry_id,
            other,
            EntryDeliveryStatus::Unreachable,
        ),
    ];
    harness
        .record_results(lost.into_iter().chain(unrelated_results()))
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2, "both targets are recovered");
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 判断迟到结果的先后时较新条目 B 的来源读取失败：不能据此把较旧条目 A 当作最新内容，
/// B 已用尽的次数保持不变，再核对时 B 仍是最新内容，因此不会再补发。
#[tokio::test(start_paused = true)]
async fn an_unreadable_newer_entry_does_not_let_a_late_older_result_restart_retries() {
    let harness = RetryHarness::start_with(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
        RetryScenario {
            older_entry: Some(EntryDeliveryStatus::Superseded),
            known_target: true,
            ..RetryScenario::default()
        },
    );
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len());

    harness.reads.failing_sources.store(1, Ordering::SeqCst);
    harness
        .record_results([(
            harness.older.entry_id.clone(),
            harness.target,
            EntryDeliveryStatus::Unreachable,
        )])
        .await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len());
    harness.recovery.shutdown().await.unwrap();
}

/// 较旧条目 A 已用尽 3 次后，真正较新的条目 B 留下可恢复结果，但判断先后时条目列表读取失败：
/// B 的通知不会丢失，再核对时确认 B 是最新内容并按完整的 3 次重试补发。
#[tokio::test(start_paused = true)]
async fn a_newer_result_with_unknown_ordering_is_rechecked_later() {
    let harness = RetryHarness::start_with(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
        RetryScenario {
            older_entry: Some(EntryDeliveryStatus::Unreachable),
            known_target: true,
            ..RetryScenario::default()
        },
    );
    // 启动扫描补发 A 一次，随后 3 次计时重试全部中断。
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len() + 1);

    harness.reads.failing_listings.store(1, Ordering::SeqCst);
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), RETRY_BACKOFF.len() + 2);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}

/// 较新条目 B 已用 2 次计时重试，较旧条目 A 的结果迟到后，B 第 3 次重试时读取 B 失败：
/// 恢复轮次不能跳过 B 去补发较旧的 A（那会重发历史并让 B 的次数重新开始），这一轮按失败计入，
/// B 的次数保持有界；之后只有上线事件再补发 B 一次。
#[tokio::test(start_paused = true)]
async fn an_unreadable_newer_entry_never_makes_a_retry_resend_older_content() {
    let harness = RetryHarness::start_with(
        None,
        &[
            Attempt::Interrupted,
            Attempt::Interrupted,
            Attempt::Accepted,
        ],
        RetryScenario {
            older_entry: Some(EntryDeliveryStatus::Superseded),
            ..RetryScenario::default()
        },
    );
    settle().await;
    harness
        .record_dispatch(EntryDeliveryStatus::Unreachable)
        .await;
    tokio::time::sleep(RETRY_BACKOFF[0] + RETRY_BACKOFF[1] + JUST_BEFORE).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2);

    harness
        .record_results([(
            harness.older.entry_id.clone(),
            harness.target,
            EntryDeliveryStatus::Unreachable,
        )])
        .await;
    settle().await;
    harness.reads.failing_sources.store(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(600)).await;
    settle().await;
    assert_eq!(harness.delivery.calls(), 2, "older content is never resent");

    harness
        .reachability
        .announce(harness.target, ReachabilityState::Online);
    settle().await;
    assert_eq!(harness.delivery.calls(), 3);
    assert_eq!(harness.status(), Some(EntryDeliveryStatus::Delivered));
    harness.recovery.shutdown().await.unwrap();
}
