//! 离线投递恢复的按目标退避：只决定某个目标何时再试，不决定恢复哪条内容。
//!
//! 是否恢复、恢复哪条内容仍由恢复轮次按“只发最新内容”的规则判断；这里只保存
//! 每个目标的下一次到期时间、当前条目已开始的计时重试次数，以及次数用尽后的标记，
//! 因此状态规模以目标数为上限。
//!
//! 计时重试有界：同一目标上的同一条目最多计时重试 [`RETRY_BACKOFF`] 的项数次。这遵循
//! 产品规则“禁止自动重发历史/最终一致性”（离线是预期状态，不建立队列，也不让同步变成
//! 最终一致）：次数用尽后，该条目只能由设备的下一次上线事件再补发一次，或被较新的内容取代。

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use uc_core::ids::{DeviceId, EntryId};

/// 同一目标上同一条目的计时重试间隔，项数即计时重试的上限：最多 3 次，分别约在 2、5、10 秒后。
/// 第三次重试仍未送达后不再排期，只保留“已用尽”标记，避免该条目随后的可恢复结果重新开始
/// 计时；较新的条目重新获得完整的 3 次。设备离线后再上线时，上线事件清除该目标的状态并补发
/// 最新一条未送达内容一次；若这次补发仍未送达，它的可恢复结果开启新的 3 次窗口。因此每次
/// 上线最多带来一次补发加 3 次计时重试，仍然有界。
pub(super) const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
];

#[derive(Default)]
pub(super) struct RetrySchedule {
    targets: HashMap<DeviceId, TargetRetry>,
}

struct TargetRetry {
    /// 当前条目已经因到期而开始的计时重试次数，用于选择下一次间隔；达到
    /// [`RETRY_BACKOFF`] 的项数后不再排期，状态保留为该条目的“已用尽”标记。
    rounds: usize,
    due: Option<Instant>,
    /// 最近一次可恢复结果所属的条目；新内容让退避从头开始。
    entry: Option<EntryId>,
}

impl RetrySchedule {
    /// 记录目标留下了可恢复结果。已排期时不推迟；新内容把退避重置为首项，
    /// 且到期时间只会提前。`entry` 为空表示恢复轮次自身失败，没有新的投递结果。
    /// 与正在跟踪的条目不同的 `entry` 一律视为较新内容：调用方只能传入目标当前最新的
    /// 未结束内容（见恢复负责人对结果通知的先后判断），不能传入较旧内容的迟到结果。
    /// 当前条目的计时重试已用尽时不再排期：每次失败的重试都会发布一条可恢复结果，
    /// 若据此重新开始计时就会变成无限重试。
    pub(super) fn record_recoverable(
        &mut self,
        target: DeviceId,
        entry: Option<EntryId>,
        now: Instant,
    ) {
        let retry = self.targets.entry(target).or_insert(TargetRetry {
            rounds: 0,
            due: None,
            entry: None,
        });
        if let Some(entry) = entry {
            if retry.entry.as_ref() != Some(&entry) {
                retry.rounds = 0;
                retry.entry = Some(entry);
                if let Some(first) = retry_at(0, now) {
                    retry.due = Some(retry.due.map_or(first, |due| due.min(first)));
                }
                return;
            }
        }
        if retry.due.is_none() {
            retry.due = retry_at(retry.rounds, now);
        }
    }

    /// 目标正在跟踪的条目：最近一次被记录为可恢复或等待结果的条目；没有退避状态或只有
    /// 未产生结果的失败轮次时返回 `None`。
    pub(super) fn tracked_entry(&self, target: &DeviceId) -> Option<&EntryId> {
        self.targets
            .get(target)
            .and_then(|retry| retry.entry.as_ref())
    }

    /// 最早的到期时间；没有排期时返回 `None`。已用尽的目标没有排期。
    pub(super) fn next_due(&self) -> Option<Instant> {
        self.targets.values().filter_map(|retry| retry.due).min()
    }

    /// 取出一个已到期的目标。取出即开始一轮计时重试，计入当前条目的重试次数。
    pub(super) fn take_due(&mut self, now: Instant) -> Option<DeviceId> {
        let target = self
            .targets
            .iter()
            .filter_map(|(target, retry)| {
                retry
                    .due
                    .filter(|due| *due <= now)
                    .map(|due| (due, *target))
            })
            .min_by_key(|(due, _)| *due)
            .map(|(_, target)| target)?;
        if let Some(retry) = self.targets.get_mut(&target) {
            retry.due = None;
            retry.rounds = retry.rounds.saturating_add(1);
        }
        Some(target)
    }

    /// 恢复轮次已向目标补发 `entry`、结果尚未通知：记下所等待的条目，以便随后的可恢复结果
    /// 沿用当前间隔、已结算结果能清除该目标。只有等待的仍是同一条目（或此前尚无条目、次数
    /// 只来自未产生结果的失败轮次）时才保留已累计的次数；改为等待另一条较新内容时退避从首项
    /// 开始，不继承旧内容已增长的间隔或已用尽的次数。目标没有退避状态时（例如由上线事件触发
    /// 的轮次）不新建状态，随后的可恢复结果会从首项开始排期。
    pub(super) fn await_result(&mut self, target: &DeviceId, entry: EntryId) {
        if let Some(retry) = self.targets.get_mut(target) {
            if retry
                .entry
                .as_ref()
                .is_some_and(|tracked| *tracked != entry)
            {
                retry.rounds = 0;
            }
            retry.entry = Some(entry);
        }
    }

    /// 目标最近等待或记录的条目已结算（送达、重复确认或终态失败）时清除其退避，包括
    /// 已用尽标记。其他条目的结算（例如较旧内容的后台收尾）不影响仍在等待的最新内容。
    pub(super) fn settle(&mut self, target: &DeviceId, entry: &EntryId) {
        if self
            .targets
            .get(target)
            .is_some_and(|retry| retry.entry.as_ref() == Some(entry))
        {
            self.targets.remove(target);
        }
    }

    /// 目标已送达、进入终态，或改由上线事件负责时清除退避状态，包括已用尽标记。
    pub(super) fn clear(&mut self, target: &DeviceId) {
        self.targets.remove(target);
    }

    #[cfg(test)]
    fn is_tracking(&self, target: &DeviceId) -> bool {
        self.targets.contains_key(target)
    }

    #[cfg(test)]
    fn is_exhausted(&self, target: &DeviceId) -> bool {
        self.targets
            .get(target)
            .is_some_and(|retry| retry.due.is_none() && retry.rounds >= RETRY_BACKOFF.len())
    }
}

/// 第 `rounds` 次计时重试的到期时间；当前条目的计时重试已用尽时返回 `None`。
fn retry_at(rounds: usize, now: Instant) -> Option<Instant> {
    RETRY_BACKOFF.get(rounds).map(|delay| now + *delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> DeviceId {
        DeviceId::new("target")
    }

    /// 让 `entry` 的可恢复结果连续触发全部计时重试，返回最后一次重试开始的时刻。
    fn exhaust(schedule: &mut RetrySchedule, entry: &EntryId, start: Instant) -> Instant {
        let mut now = start;
        for _ in 0..RETRY_BACKOFF.len() {
            schedule.record_recoverable(target(), Some(entry.clone()), now);
            now = schedule.next_due().expect("scheduled");
            assert_eq!(schedule.take_due(now), Some(target()));
        }
        now
    }

    #[test]
    fn the_retry_schedule_is_three_retries_after_two_five_and_ten_seconds() {
        assert_eq!(
            RETRY_BACKOFF,
            [
                Duration::from_secs(2),
                Duration::from_secs(5),
                Duration::from_secs(10),
            ]
        );
    }

    #[test]
    fn repeated_failures_back_off_through_each_interval_once() {
        let start = Instant::now();
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        let mut now = start;
        let mut observed = Vec::new();
        for _ in 0..RETRY_BACKOFF.len() {
            schedule.record_recoverable(target(), Some(entry.clone()), now);
            let due = schedule.next_due().expect("scheduled");
            observed.push(due - now);
            assert_eq!(schedule.take_due(due - Duration::from_millis(1)), None);
            now = due;
            assert_eq!(schedule.take_due(now), Some(target()));
        }
        assert_eq!(observed, RETRY_BACKOFF.to_vec());
    }

    #[test]
    fn an_exhausted_entry_is_never_rescheduled_by_its_later_recoverable_results() {
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        let now = exhaust(&mut schedule, &entry, Instant::now());

        // 第三次重试失败后发布的可恢复结果，以及之后同一条目的任何可恢复结果或失败轮次。
        for later in 0..5 {
            let at = now + Duration::from_secs(later * 60);
            schedule.record_recoverable(target(), Some(entry.clone()), at);
            schedule.record_recoverable(target(), None, at);
            assert_eq!(schedule.next_due(), None);
            assert_eq!(schedule.take_due(at + Duration::from_secs(3600)), None);
        }
        assert!(schedule.is_exhausted(&target()));
    }

    #[test]
    fn a_newer_entry_after_exhaustion_gets_a_fresh_three_retries() {
        let mut schedule = RetrySchedule::default();
        let old = EntryId::from("old");
        let now = exhaust(&mut schedule, &old, Instant::now());
        schedule.record_recoverable(target(), Some(old), now);
        assert!(schedule.is_exhausted(&target()));

        let new = EntryId::from("new");
        let mut at = now;
        let mut observed = Vec::new();
        for _ in 0..RETRY_BACKOFF.len() {
            schedule.record_recoverable(target(), Some(new.clone()), at);
            let due = schedule.next_due().expect("scheduled");
            observed.push(due - at);
            at = due;
            assert_eq!(schedule.take_due(at), Some(target()));
        }
        assert_eq!(observed, RETRY_BACKOFF.to_vec());
        schedule.record_recoverable(target(), Some(new), at);
        assert_eq!(schedule.next_due(), None);
        assert!(schedule.is_exhausted(&target()));
    }

    #[test]
    fn settling_the_exhausted_entry_drops_its_marker_but_other_entries_do_not() {
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        let now = exhaust(&mut schedule, &entry, Instant::now());
        schedule.record_recoverable(target(), Some(entry.clone()), now);

        schedule.settle(&target(), &EntryId::from("older"));
        assert!(schedule.is_exhausted(&target()));

        schedule.settle(&target(), &entry);
        assert!(!schedule.is_tracking(&target()));
        schedule.record_recoverable(target(), Some(entry), now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[0]));
    }

    #[test]
    fn clearing_an_exhausted_target_drops_its_marker() {
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        let now = exhaust(&mut schedule, &entry, Instant::now());
        schedule.record_recoverable(target(), Some(entry.clone()), now);
        assert!(schedule.is_exhausted(&target()));

        // 例如上线事件：清除后，同一条目这次补发的可恢复结果开启新的计时窗口。
        schedule.clear(&target());
        assert!(!schedule.is_tracking(&target()));
        schedule.record_recoverable(target(), Some(entry), now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[0]));
    }

    #[test]
    fn a_pending_retry_is_never_postponed_by_another_result_for_the_same_entry() {
        let now = Instant::now();
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        schedule.record_recoverable(target(), Some(entry.clone()), now);
        let first = schedule.next_due();
        schedule.record_recoverable(target(), Some(entry), now + Duration::from_secs(1));
        schedule.record_recoverable(target(), None, now + Duration::from_secs(1));
        assert_eq!(schedule.next_due(), first);
    }

    #[test]
    fn newer_content_restarts_the_backoff_and_only_moves_the_retry_earlier() {
        let start = Instant::now();
        let mut schedule = RetrySchedule::default();
        let mut now = start;
        for _ in 0..RETRY_BACKOFF.len() - 1 {
            schedule.record_recoverable(target(), Some(EntryId::from("old")), now);
            now = schedule.next_due().expect("scheduled");
            schedule.take_due(now);
        }
        schedule.record_recoverable(target(), Some(EntryId::from("old")), now);
        assert_eq!(
            schedule.next_due(),
            Some(now + RETRY_BACKOFF[RETRY_BACKOFF.len() - 1])
        );

        schedule.record_recoverable(target(), Some(EntryId::from("new")), now);

        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[0]));
    }

    #[test]
    fn a_failed_round_without_a_result_still_schedules_the_next_interval() {
        let now = Instant::now();
        let mut schedule = RetrySchedule::default();
        schedule.record_recoverable(target(), None, now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[0]));
        assert_eq!(schedule.take_due(now + RETRY_BACKOFF[0]), Some(target()));
        schedule.record_recoverable(target(), None, now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[1]));
    }

    #[test]
    fn settling_the_awaited_entry_drops_the_target_but_other_entries_do_not() {
        let now = Instant::now();
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        schedule.record_recoverable(target(), None, now);
        assert_eq!(schedule.take_due(now + RETRY_BACKOFF[0]), Some(target()));
        schedule.await_result(&target(), entry.clone());

        schedule.settle(&target(), &EntryId::from("older"));
        assert!(schedule.is_tracking(&target()));

        schedule.settle(&target(), &entry);
        assert!(!schedule.is_tracking(&target()));
    }

    #[test]
    fn awaiting_a_result_keeps_the_grown_interval_for_the_next_recoverable_result() {
        let now = Instant::now();
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        schedule.record_recoverable(target(), None, now);
        assert_eq!(schedule.take_due(now + RETRY_BACKOFF[0]), Some(target()));
        schedule.await_result(&target(), entry.clone());
        schedule.record_recoverable(target(), Some(entry), now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[1]));
    }

    #[test]
    fn awaiting_a_different_entry_restarts_the_backoff_from_the_first_interval() {
        let start = Instant::now();
        let mut schedule = RetrySchedule::default();
        let old = EntryId::from("old");
        let mut now = start;
        for _ in 0..RETRY_BACKOFF.len() - 1 {
            schedule.record_recoverable(target(), Some(old.clone()), now);
            now = schedule.next_due().expect("scheduled");
            assert_eq!(schedule.take_due(now), Some(target()));
        }
        schedule.record_recoverable(target(), Some(old.clone()), now);
        assert_eq!(
            schedule.next_due(),
            Some(now + RETRY_BACKOFF[RETRY_BACKOFF.len() - 1])
        );
        now = schedule.next_due().expect("scheduled");
        assert_eq!(schedule.take_due(now), Some(target()));

        // 旧内容的最后一次计时重试已开始，恢复轮次随后改为等待较新的内容：
        // 较新内容不继承旧内容已用尽的次数。
        let new = EntryId::from("new");
        schedule.await_result(&target(), new.clone());
        schedule.record_recoverable(target(), Some(new), now);

        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[0]));
    }

    #[test]
    fn awaiting_the_same_entry_keeps_its_grown_interval() {
        let mut now = Instant::now();
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        for _ in 0..2 {
            schedule.record_recoverable(target(), Some(entry.clone()), now);
            now = schedule.next_due().expect("scheduled");
            assert_eq!(schedule.take_due(now), Some(target()));
        }
        schedule.await_result(&target(), entry.clone());
        schedule.record_recoverable(target(), Some(entry), now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[2]));
    }

    #[test]
    fn awaiting_without_retry_state_creates_none() {
        let mut schedule = RetrySchedule::default();
        schedule.await_result(&target(), EntryId::from("entry"));
        assert!(!schedule.is_tracking(&target()));
        assert_eq!(schedule.next_due(), None);
    }

    #[test]
    fn clearing_a_target_drops_its_retry_and_backoff() {
        let now = Instant::now();
        let mut schedule = RetrySchedule::default();
        let other = DeviceId::new("other");
        schedule.record_recoverable(target(), None, now);
        schedule.record_recoverable(other, None, now + Duration::from_secs(1));
        schedule.clear(&target());
        assert_eq!(
            schedule.next_due(),
            Some(now + Duration::from_secs(1) + RETRY_BACKOFF[0])
        );
        assert_eq!(
            schedule.take_due(now + Duration::from_secs(60)),
            Some(other)
        );
        assert_eq!(schedule.take_due(now + Duration::from_secs(60)), None);
    }
}
