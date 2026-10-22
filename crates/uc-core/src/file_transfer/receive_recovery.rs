//! 原尝试的失败修复只能完成自身结算，不得改动新尝试或已成功结果。
use crate::error_class::ErrorClass;
use crate::ports::{AttemptState, EntryReceiveAttempt, PartialReceiveTerminal};

pub enum ReceiveFailureRecoveryAction {
    ClaimFailure,
    Settle(PartialReceiveTerminal),
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveFailureRecoveryRefusal {
    #[error("receive failure recovery has no matching attempt")]
    Missing,
    #[error("receive failure recovery was superseded")]
    Superseded,
    #[error("receive failure recovery cannot change completed delivery")]
    Completed,
}

pub fn receive_failure_recovery_action(
    entry: &str,
    attempt: &str,
    current: Option<&EntryReceiveAttempt>,
) -> Result<ReceiveFailureRecoveryAction, ReceiveFailureRecoveryRefusal> {
    let current = current.ok_or(ReceiveFailureRecoveryRefusal::Missing)?;
    if current.entry_id != entry || current.current_attempt_id != attempt {
        return Err(ReceiveFailureRecoveryRefusal::Superseded);
    }
    match current.state {
        AttemptState::Receiving | AttemptState::Committing => {
            Ok(ReceiveFailureRecoveryAction::ClaimFailure)
        }
        AttemptState::Failing | AttemptState::Failed => Ok(ReceiveFailureRecoveryAction::Settle(
            PartialReceiveTerminal::Failed,
        )),
        AttemptState::Cancelling | AttemptState::Cancelled => Ok(
            ReceiveFailureRecoveryAction::Settle(PartialReceiveTerminal::Cancelled),
        ),
        AttemptState::Completed => Err(ReceiveFailureRecoveryRefusal::Completed),
    }
}

impl ErrorClass for ReceiveFailureRecoveryRefusal {
    fn class(&self) -> &'static str {
        match self {
            Self::Missing => "receive_recovery_missing",
            Self::Superseded => "receive_recovery_superseded",
            Self::Completed => "receive_recovery_completed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(state: AttemptState) -> EntryReceiveAttempt {
        EntryReceiveAttempt {
            entry_id: "entry".into(),
            current_attempt_id: "attempt".into(),
            state,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn original_failure_recovery_obeys_authority_and_cancel_winner() {
        for state in [AttemptState::Receiving, AttemptState::Committing] {
            assert!(matches!(
                receive_failure_recovery_action("entry", "attempt", Some(&attempt(state))),
                Ok(ReceiveFailureRecoveryAction::ClaimFailure)
            ));
        }
        for state in [AttemptState::Failing, AttemptState::Failed] {
            assert!(matches!(
                receive_failure_recovery_action("entry", "attempt", Some(&attempt(state))),
                Ok(ReceiveFailureRecoveryAction::Settle(
                    PartialReceiveTerminal::Failed
                ))
            ));
        }
        for state in [AttemptState::Cancelling, AttemptState::Cancelled] {
            assert!(matches!(
                receive_failure_recovery_action("entry", "attempt", Some(&attempt(state))),
                Ok(ReceiveFailureRecoveryAction::Settle(
                    PartialReceiveTerminal::Cancelled
                ))
            ));
        }
        assert!(matches!(
            receive_failure_recovery_action(
                "entry",
                "attempt",
                Some(&attempt(AttemptState::Completed))
            ),
            Err(ReceiveFailureRecoveryRefusal::Completed)
        ));
        assert!(matches!(
            receive_failure_recovery_action("entry", "attempt", None),
            Err(ReceiveFailureRecoveryRefusal::Missing)
        ));
        for (entry, id) in [("other", "attempt"), ("entry", "other")] {
            assert!(matches!(
                receive_failure_recovery_action(entry, id, Some(&attempt(AttemptState::Receiving))),
                Err(ReceiveFailureRecoveryRefusal::Superseded)
            ));
        }
    }
}
