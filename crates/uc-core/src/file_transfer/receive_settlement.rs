//! 独立文件的提交与取消竞争；状态先保存，再履行返回的本地效果。
use self::{ReceiveFileSettlementEffect as Effect, ReceiveFileSettlementInput as Input};
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReceiveFileSettlement {
    phase: Phase,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Pending,
    Committing,
    Cancelling,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Copy)]
pub enum ReceiveFileSettlementInput {
    BeginCommit,
    Cancel,
    Committed,
    Failed,
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveFileSettlementEffect {
    /// 保存Committing状态后才可开始不可遗弃的本地提交。
    Commit,
    RefuseCommit,
    /// 先封口；等待已经开始的实际工作和清理，不宣称取消已经完成。
    Wait,
    AlreadyCompleted,
    Settled,
    Unchanged,
}
impl ReceiveFileSettlement {
    pub fn apply(&mut self, input: ReceiveFileSettlementInput) -> ReceiveFileSettlementEffect {
        if self.is_settled() {
            return match input {
                Input::Cancel if self.phase == Phase::Completed => Effect::AlreadyCompleted,
                Input::BeginCommit => Effect::RefuseCommit,
                _ => Effect::Unchanged,
            };
        }
        match input {
            Input::BeginCommit if self.phase == Phase::Pending => {
                self.phase = Phase::Committing;
                Effect::Commit
            }
            Input::BeginCommit => Effect::RefuseCommit,
            Input::Cancel => {
                if self.phase == Phase::Pending {
                    self.phase = Phase::Cancelling;
                }
                Effect::Wait
            }
            Input::Committed if self.phase == Phase::Committing => {
                self.phase = Phase::Completed;
                Effect::Settled
            }
            Input::Committed => Effect::Unchanged,
            Input::Failed => {
                self.phase = Phase::Failed;
                Effect::Settled
            }
            Input::Cancelled => {
                self.phase = Phase::Cancelled;
                Effect::Settled
            }
        }
    }
    pub fn is_settled(&self) -> bool {
        matches!(
            self.phase,
            Phase::Completed | Phase::Failed | Phase::Cancelled
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_before_commit_seals_the_file_until_actual_cleanup() {
        let mut file = ReceiveFileSettlement::default();
        assert_eq!(
            file.apply(ReceiveFileSettlementInput::Cancel),
            ReceiveFileSettlementEffect::Wait
        );
        assert!(!file.is_settled());
        assert_eq!(
            file.apply(ReceiveFileSettlementInput::BeginCommit),
            ReceiveFileSettlementEffect::RefuseCommit
        );
        assert_eq!(
            file.apply(ReceiveFileSettlementInput::Cancelled),
            ReceiveFileSettlementEffect::Settled
        );
        assert_eq!(
            file.apply(ReceiveFileSettlementInput::Committed),
            ReceiveFileSettlementEffect::Unchanged
        );
    }
    #[test]
    fn started_commit_waits_for_real_result_and_cannot_revert_after_success() {
        for succeeded in [false, true] {
            let mut file = ReceiveFileSettlement::default();
            assert_eq!(
                file.apply(ReceiveFileSettlementInput::BeginCommit),
                ReceiveFileSettlementEffect::Commit
            );
            assert_eq!(
                file.apply(ReceiveFileSettlementInput::Cancel),
                ReceiveFileSettlementEffect::Wait
            );
            assert!(!file.is_settled());
            let input = if succeeded {
                ReceiveFileSettlementInput::Committed
            } else {
                ReceiveFileSettlementInput::Failed
            };
            assert_eq!(file.apply(input), ReceiveFileSettlementEffect::Settled);
            assert!(file.is_settled());
            assert_eq!(
                file.apply(ReceiveFileSettlementInput::Cancelled),
                ReceiveFileSettlementEffect::Unchanged
            );
            assert_eq!(
                file.apply(ReceiveFileSettlementInput::Cancel),
                if succeeded {
                    ReceiveFileSettlementEffect::AlreadyCompleted
                } else {
                    ReceiveFileSettlementEffect::Unchanged
                }
            );
        }
    }
}
