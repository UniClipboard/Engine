//! 离线投递恢复的按目标退避：只决定某个目标何时再试，不决定恢复哪条内容。
//!
//! 是否恢复、恢复哪条内容仍由恢复轮次按“只发最新内容”的规则判断；这里只保存
//! 每个目标的下一次到期时间和连续未送达的轮数，因此状态规模以目标数为上限。

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use uc_core::ids::{DeviceId, EntryId};

/// 同一目标连续未送达时的重试间隔。超过末项后保持末项：目标仍在线就继续按
/// 上限间隔重试，不放弃；目标离线后改由上线事件负责。
pub(super) const RETRY_BACKOFF: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(60),
];

#[derive(Default)]
pub(super) struct RetrySchedule {
    targets: HashMap<DeviceId, TargetRetry>,
}

struct TargetRetry {
    /// 已经因到期而开始的重试轮数，用于选择下一次间隔。
    rounds: usize,
    due: Option<Instant>,
    /// 最近一次可恢复结果所属的条目；新内容让退避从头开始。
    entry: Option<EntryId>,
}

impl RetrySchedule {
    /// 记录目标留下了可恢复结果。已排期时不推迟；新内容把退避重置为首项，
    /// 且到期时间只会提前。`entry` 为空表示恢复轮次自身失败，没有新的投递结果。
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
                let first = now + backoff(0);
                retry.due = Some(retry.due.map_or(first, |due| due.min(first)));
                return;
            }
        }
        if retry.due.is_none() {
            retry.due = Some(now + backoff(retry.rounds));
        }
    }

    /// 最早的到期时间；没有排期时返回 `None`。
    pub(super) fn next_due(&self) -> Option<Instant> {
        self.targets.values().filter_map(|retry| retry.due).min()
    }

    /// 取出一个已到期的目标。取出即开始一轮重试，下一次间隔随之增长。
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

    /// 恢复轮次已向目标补发 `entry`、结果尚未通知：记下所等待的条目，保留已累计的轮数，
    /// 以便随后的可恢复结果沿用当前间隔、已结算结果能清除该目标。目标没有退避状态时
    /// （例如由上线事件触发的轮次）不新建状态，随后的可恢复结果会从首项开始排期。
    pub(super) fn await_result(&mut self, target: &DeviceId, entry: EntryId) {
        if let Some(retry) = self.targets.get_mut(target) {
            retry.entry = Some(entry);
        }
    }

    /// 目标最近等待或记录的条目已结算（送达、重复确认或终态失败）时清除其退避。
    /// 其他条目的结算（例如较旧内容的后台收尾）不影响仍在等待的最新内容。
    pub(super) fn settle(&mut self, target: &DeviceId, entry: &EntryId) {
        if self
            .targets
            .get(target)
            .is_some_and(|retry| retry.entry.as_ref() == Some(entry))
        {
            self.targets.remove(target);
        }
    }

    /// 目标已送达、进入终态，或改由上线事件负责时清除退避状态。
    pub(super) fn clear(&mut self, target: &DeviceId) {
        self.targets.remove(target);
    }

    #[cfg(test)]
    fn is_tracking(&self, target: &DeviceId) -> bool {
        self.targets.contains_key(target)
    }
}

fn backoff(rounds: usize) -> Duration {
    RETRY_BACKOFF[rounds.min(RETRY_BACKOFF.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> DeviceId {
        DeviceId::new("target")
    }

    #[test]
    fn repeated_failures_back_off_up_to_the_last_interval_and_stay_there() {
        let start = Instant::now();
        let mut schedule = RetrySchedule::default();
        let entry = EntryId::from("entry");
        let mut now = start;
        let mut observed = Vec::new();
        for _ in 0..RETRY_BACKOFF.len() + 2 {
            schedule.record_recoverable(target(), Some(entry.clone()), now);
            let due = schedule.next_due().expect("scheduled");
            observed.push(due - now);
            assert_eq!(schedule.take_due(due - Duration::from_millis(1)), None);
            now = due;
            assert_eq!(schedule.take_due(now), Some(target()));
        }
        let mut expected = RETRY_BACKOFF.to_vec();
        expected.extend([RETRY_BACKOFF[RETRY_BACKOFF.len() - 1]; 2]);
        assert_eq!(observed, expected);
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
        for _ in 0..4 {
            schedule.record_recoverable(target(), Some(EntryId::from("old")), now);
            now = schedule.next_due().expect("scheduled");
            schedule.take_due(now);
        }
        schedule.record_recoverable(target(), Some(EntryId::from("old")), now);
        assert_eq!(schedule.next_due(), Some(now + RETRY_BACKOFF[4]));

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
