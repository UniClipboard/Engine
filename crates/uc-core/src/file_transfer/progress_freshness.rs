//! 任务有效联系的纯时间规则；期限耗尽只改变确认程度，不推断文件结果。
use std::time::Duration;

pub const PROGRESS_CONTACT_GAP: Duration = Duration::from_secs(10);
pub const PROGRESS_UNKNOWN_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressFreshnessView {
    Waiting,
    Live,
    Unconfirmed,
    Settled,
}
pub enum ProgressFreshnessInput {
    /// 接受新尝试身份，但登记本身不刷新原发送意图的期限。
    NewAttempt,
    /// 仅重置新运行期producer的联系序号，不续期或重开已知终态。
    NewProducer,
    Notification(Duration),
    Contact {
        sequence: u64,
        now: Duration,
    },
    ContinuityLost,
    /// 当前任务无法核对；只改变确认程度，保留原联系时间与已知终态。
    VerificationLost,
    /// 完整当前事实重新通过核对；不续期原有效联系。
    VerificationRestored,
    Settled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressFreshnessEffect {
    /// 保存联系状态后才允许发布完整视图变化，不代表数据新增或文件成功。
    Advanced,
    /// 不改状态，不续期，不发布变化。
    Ignored,
}
pub struct ProgressFreshness {
    accepted: Option<Duration>,
    last: Option<Duration>,
    sequence: Option<u64>,
    received: bool,
    settled: bool,
    verification_lost: bool,
}
impl ProgressFreshness {
    /// 原完整发送负责人接受意图时建立首次期限；登记不得重建此状态。
    pub fn new(accepted: Option<Duration>) -> Self {
        Self {
            accepted,
            last: accepted,
            sequence: None,
            received: false,
            settled: false,
            verification_lost: false,
        }
    }
    pub fn view(&self, now: Option<Duration>) -> ProgressFreshnessView {
        if self.settled {
            return ProgressFreshnessView::Settled;
        }
        if self.verification_lost {
            return ProgressFreshnessView::Unconfirmed;
        }
        if self
            .last
            .zip(now)
            .and_then(|(last, now)| now.checked_sub(last))
            .is_none_or(|elapsed| elapsed >= PROGRESS_UNKNOWN_AFTER)
        {
            return ProgressFreshnessView::Unconfirmed;
        }
        if self.received {
            ProgressFreshnessView::Live
        } else {
            ProgressFreshnessView::Waiting
        }
    }
    pub fn apply(&mut self, input: ProgressFreshnessInput) -> ProgressFreshnessEffect {
        if matches!(input, ProgressFreshnessInput::NewAttempt) {
            self.sequence = None;
            // 新尝试不能借用前一尝试已确认的联系或时钟世代。
            if self.received {
                self.accepted = None;
            }
            self.last = self.accepted;
            self.received = false;
            self.settled = false;
            self.verification_lost = false;
            return ProgressFreshnessEffect::Advanced;
        }
        if matches!(input, ProgressFreshnessInput::NewProducer) {
            self.sequence = None;
            return ProgressFreshnessEffect::Advanced;
        }
        if self.settled {
            return ProgressFreshnessEffect::Ignored;
        }
        match input {
            ProgressFreshnessInput::NewAttempt | ProgressFreshnessInput::NewProducer => {
                return ProgressFreshnessEffect::Ignored
            }
            ProgressFreshnessInput::VerificationLost => {
                if self.verification_lost {
                    return ProgressFreshnessEffect::Ignored;
                }
                self.verification_lost = true;
            }
            ProgressFreshnessInput::VerificationRestored => {
                if !self.verification_lost {
                    return ProgressFreshnessEffect::Ignored;
                }
                self.verification_lost = false;
            }
            ProgressFreshnessInput::Settled => self.settled = true,
            ProgressFreshnessInput::ContinuityLost => {
                self.last = None;
                self.accepted = None;
            }
            ProgressFreshnessInput::Notification(now) => {
                self.verification_lost = false;
                self.last = Some(self.last.map_or(now, |old| old.max(now)));
                self.received = true;
            }
            ProgressFreshnessInput::Contact { sequence, now } => {
                if self.sequence.is_some_and(|old| sequence <= old) {
                    return ProgressFreshnessEffect::Ignored;
                }
                self.sequence = Some(sequence);
                self.verification_lost = false;
                self.last = Some(self.last.map_or(now, |old| old.max(now)));
                self.received = true;
            }
        }
        ProgressFreshnessEffect::Advanced
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sec(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    #[test]
    fn intent_starts_waiting_and_duplicate_or_old_contact_does_not_renew_sixty_seconds() {
        let mut state = ProgressFreshness::new(Some(sec(0)));
        assert_eq!(state.view(Some(sec(59))), ProgressFreshnessView::Waiting);
        assert_eq!(
            state.view(Some(sec(60))),
            ProgressFreshnessView::Unconfirmed
        );
        assert_eq!(
            state.apply(ProgressFreshnessInput::Contact {
                sequence: 2,
                now: sec(61)
            }),
            ProgressFreshnessEffect::Advanced
        );
        for sequence in [0, 1, 2] {
            assert_eq!(
                state.apply(ProgressFreshnessInput::Contact {
                    sequence,
                    now: sec(120)
                }),
                ProgressFreshnessEffect::Ignored
            );
        }
        assert_eq!(state.view(Some(sec(120))), ProgressFreshnessView::Live);
        assert_eq!(
            state.view(Some(sec(121))),
            ProgressFreshnessView::Unconfirmed
        );
    }
    #[test]
    fn continuity_loss_requires_a_new_valid_notification_not_just_a_healthy_clock() {
        let mut state = ProgressFreshness::new(Some(sec(100)));
        state.apply(ProgressFreshnessInput::Contact {
            sequence: 5,
            now: sec(101),
        });
        state.apply(ProgressFreshnessInput::ContinuityLost);
        assert_eq!(
            state.view(Some(sec(102))),
            ProgressFreshnessView::Unconfirmed
        );
        assert_eq!(
            state.apply(ProgressFreshnessInput::Contact {
                sequence: 5,
                now: sec(102)
            }),
            ProgressFreshnessEffect::Ignored
        );
        assert_eq!(
            state.view(Some(sec(102))),
            ProgressFreshnessView::Unconfirmed
        );
        state.apply(ProgressFreshnessInput::Notification(sec(102)));
        assert_eq!(state.view(Some(sec(102))), ProgressFreshnessView::Live);
    }
    #[test]
    fn settled_facts_ignore_all_later_contact_clock_loss_and_time() {
        let mut state = ProgressFreshness::new(Some(sec(0)));
        state.apply(ProgressFreshnessInput::Settled);
        for input in [
            ProgressFreshnessInput::ContinuityLost,
            ProgressFreshnessInput::Notification(sec(1)),
            ProgressFreshnessInput::Contact {
                sequence: 99,
                now: sec(1000),
            },
        ] {
            assert_eq!(state.apply(input), ProgressFreshnessEffect::Ignored);
        }
        assert_eq!(state.view(None), ProgressFreshnessView::Settled);
    }
    #[test]
    fn new_producer_rebases_only_contact_sequence_and_new_attempt_does_not_reset_intent_deadline() {
        let mut state = ProgressFreshness::new(Some(sec(0)));
        state.apply(ProgressFreshnessInput::Contact {
            sequence: 100,
            now: sec(5),
        });
        state.apply(ProgressFreshnessInput::NewProducer);
        assert_eq!(
            state.view(Some(sec(65))),
            ProgressFreshnessView::Unconfirmed
        );
        assert_eq!(
            state.apply(ProgressFreshnessInput::Contact {
                sequence: 1,
                now: sec(65)
            }),
            ProgressFreshnessEffect::Advanced
        );
        state.apply(ProgressFreshnessInput::Settled);
        state.apply(ProgressFreshnessInput::NewProducer);
        assert_eq!(state.view(None), ProgressFreshnessView::Settled);
        state.apply(ProgressFreshnessInput::NewAttempt);
        assert_eq!(
            state.view(Some(sec(66))),
            ProgressFreshnessView::Unconfirmed
        );
        state.apply(ProgressFreshnessInput::NewAttempt);
        assert_eq!(state.view(Some(sec(0))), ProgressFreshnessView::Unconfirmed);
    }
}
