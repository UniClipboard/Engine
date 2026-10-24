use std::time::Duration;

use super::*;

const fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn started(poll: RetransmitPoll) -> Duration {
    match poll {
        RetransmitPoll::Start { deadline } => deadline,
        other => panic!("expected start, got {other:?}"),
    }
}

#[test]
fn retransmit_follows_default_slots_and_never_overlaps_a_round() {
    let mut state = TerminalRetransmit::new(Some(secs(100)));
    assert_eq!(started(state.poll(Some(secs(100)))), secs(105));
    // 前次 5 秒内仍在途：第 2 秒时点到期也不新开一轮。
    assert_eq!(state.poll(Some(secs(102))), RetransmitPoll::Busy);
    state.finish_round();
    // 本轮结束于 104.9 秒，已到期的 2 秒时点合并为一次资格。
    assert_eq!(started(state.poll(Some(secs(105)))), secs(110));
    state.finish_round();
    assert_eq!(
        state.poll(Some(secs(106))),
        RetransmitPoll::WaitUntil(secs(110))
    );
}

#[test]
fn suspend_merges_missed_slots_into_one_round_and_never_extends_retention() {
    let mut state = TerminalRetransmit::new(Some(secs(0)));
    started(state.poll(Some(secs(0))));
    state.finish_round();
    // 挂起到第 130 秒：2／10／30／60／120 时点合并为一轮，不追补五轮。
    assert_eq!(started(state.poll(Some(secs(130)))), secs(135));
    state.finish_round();
    assert_eq!(
        state.poll(Some(secs(131))),
        RetransmitPoll::WaitUntil(secs(300))
    );
    // 恢复时已越过 10 分钟：直接停止，不再发送。
    assert_eq!(
        state.poll(Some(secs(10_000))),
        RetransmitPoll::Stopped(RetransmitStop::Expired)
    );
}

#[test]
fn deadline_is_clamped_and_exact_expiry_stops() {
    let mut state = TerminalRetransmit::new(Some(secs(0)));
    started(state.poll(Some(secs(0))));
    state.finish_round();
    // 540 秒时点启动的一轮截止为 min(545, 600)；595 秒启动则被保留期截断。
    assert_eq!(started(state.poll(Some(secs(595)))), secs(600));
    state.finish_round();
    assert_eq!(
        state.poll(Some(secs(600))),
        RetransmitPoll::Stopped(RetransmitStop::Expired)
    );
}

#[test]
fn eight_starts_exhaust_the_budget() {
    let mut state = TerminalRetransmit::new(Some(secs(0)));
    let mut starts = 0;
    for at in [0, 2, 10, 30, 60, 120, 300, 540] {
        started(state.poll(Some(secs(at))));
        state.finish_round();
        starts += 1;
    }
    assert_eq!(starts, 8);
    assert_eq!(
        state.poll(Some(secs(560))),
        RetransmitPoll::Stopped(RetransmitStop::Exhausted)
    );
}

#[test]
fn network_recovery_consumes_one_future_slot_after_the_minimum_gap() {
    let mut state = TerminalRetransmit::new(Some(secs(0)));
    started(state.poll(Some(secs(0))));
    state.finish_round();
    // 第 1 秒前次刚结束：恢复机会不能立即启动。
    state.network_recovered();
    state.network_recovered();
    assert_eq!(
        state.poll(Some(secs(1))),
        RetransmitPoll::WaitUntil(secs(2))
    );
    // 距前次启动 2 秒后提前消费最近一个时点（此处恰与 2 秒时点重合）。
    started(state.poll(Some(secs(2))));
    state.finish_round();
    // 重复恢复机会合并成一次，提前消费 10 秒时点；其后 30 秒时点仍在。
    state.network_recovered();
    state.network_recovered();
    started(state.poll(Some(secs(4))));
    state.finish_round();
    assert_eq!(
        state.poll(Some(secs(5))),
        RetransmitPoll::WaitUntil(secs(30))
    );
}

#[test]
fn ack_and_unreadable_or_regressing_clock_stop_retransmission() {
    let mut acked = TerminalRetransmit::new(Some(secs(0)));
    started(acked.poll(Some(secs(0))));
    acked.acknowledge();
    assert_eq!(
        acked.poll(Some(secs(1))),
        RetransmitPoll::Stopped(RetransmitStop::Acknowledged)
    );
    for mut state in [
        TerminalRetransmit::new(None),
        TerminalRetransmit::new(Some(secs(0))),
    ] {
        assert_eq!(
            state.poll(None),
            RetransmitPoll::Stopped(RetransmitStop::Expired)
        );
    }
    let mut regressed = TerminalRetransmit::new(Some(secs(10)));
    started(regressed.poll(Some(secs(50))));
    regressed.finish_round();
    assert_eq!(
        regressed.poll(Some(secs(40))),
        RetransmitPoll::Stopped(RetransmitStop::Expired)
    );
}

#[test]
fn largest_manifest_keeps_only_the_peer_capacity_and_leaves_other_peers_alone() {
    let mut ledger = TerminalLedger::<u8, u16>::default();
    ledger.admit(2, 0, secs(1));
    let mut evicted = Vec::new();
    for item in 0..1024u16 {
        evicted.extend(ledger.admit(1, item, secs(10 + u64::from(item))));
    }
    // 1024 项只保留该端最新 64 项，淘汰的恰是最早生成的项。
    assert_eq!(evicted.len(), 1024 - TERMINAL_PEER_CAPACITY);
    assert!(evicted.iter().all(|(peer, item)| *peer == 1 && *item < 960));
    assert!(ledger.contains(&2, &0));
    assert!(ledger.contains(&1, &1023) && !ledger.contains(&1, &959));
    assert_eq!(ledger.len(), TERMINAL_PEER_CAPACITY + 1);
}

#[test]
fn global_overflow_evicts_the_earliest_and_duplicates_take_no_capacity() {
    let mut ledger = TerminalLedger::<u8, u16>::default();
    for peer in 0..5u8 {
        for item in 0..64u16 {
            let generated = secs(u64::from(peer) * 100 + u64::from(item));
            assert!(ledger.admit(peer, item, generated).is_empty() || peer == 4);
        }
    }
    // 5 × 64 = 320 超过 256：全局最早的 64 项（对端 0）被淘汰。
    assert_eq!(ledger.len(), TERMINAL_GLOBAL_CAPACITY);
    assert!(!ledger.contains(&0, &0) && ledger.contains(&1, &0));
    assert!(ledger.admit(4, 63, secs(999)).is_empty());
    assert_eq!(ledger.len(), TERMINAL_GLOBAL_CAPACITY);
    assert!(ledger.release(&4, &63) && !ledger.release(&4, &63));
    assert_eq!(ledger.expire_through(secs(163)).len(), 64);
}

#[test]
fn stuck_peer_only_holds_its_own_slot_and_queue() {
    let mut queue = ExchangeQueue::<u8, u16>::default();
    // 对端 0 的第一轮交换永远不结束，并持续排入工作。
    for work in 0..16 {
        assert!(queue.enqueue(0, work).is_ok());
    }
    assert_eq!(
        queue.enqueue(0, 99),
        Err(ExchangeRefusal::PeerQueueFull(99))
    );
    for peer in 1..=3u8 {
        for work in 0..4 {
            assert!(queue.enqueue(peer, work).is_ok());
        }
    }
    let mut served = vec![0usize; 4];
    let mut stuck_taken = false;
    // 其他对端每个交换立即结束；对端 0 的首个交换从不 complete。
    while let Some((peer, _)) = queue.next_ready() {
        served[usize::from(peer)] += 1;
        if peer == 0 {
            assert!(!stuck_taken, "stuck peer must not be dispatched twice");
            stuck_taken = true;
        } else {
            queue.complete(&peer);
        }
    }
    assert_eq!(served, vec![1, 4, 4, 4]);
    assert_eq!(queue.active(), 1);
    assert_eq!(queue.queued(), 15);
}

#[test]
fn global_active_and_queue_limits_hold_and_peers_rotate() {
    let mut queue = ExchangeQueue::<u8, u16>::default();
    for peer in 0..4u8 {
        for work in 0..16 {
            assert!(queue.enqueue(peer, work).is_ok());
        }
    }
    assert_eq!(queue.queued(), 64);
    assert_eq!(
        queue.enqueue(9, 0),
        Err(ExchangeRefusal::GlobalQueueFull(0))
    );
    let first: Vec<u8> = (0..4)
        .filter_map(|_| queue.next_ready().map(|(peer, _)| peer))
        .collect();
    assert_eq!(first, vec![0, 1, 2, 3]);
    assert_eq!(queue.next_ready(), None);
    queue.complete(&2);
    // 额度释放后继续轮转，不让先完成的对端连续占用。
    assert_eq!(queue.next_ready().map(|(peer, _)| peer), Some(2));
}
