//! 删除不能丢失尚在接收或尚需回滚的完整责任。
use crate::ports::AttemptState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReceiveDeletionRefusal {
    #[error("entry has a nonterminal remote receive")]
    NonterminalReceive,
    #[error("entry has unsettled receive artifacts")]
    UnsettledArtifacts,
    #[error("entry has unsettled directory publication")]
    UnsettledPublication,
}

/// 已发布目录可保留；未发布目录必须逐attempt匹配实际回滚的结算证明。
pub fn check_receive_deletion(
    attempt: Option<AttemptState>,
    unsettled_artifacts: bool,
    unpublished_attempts: &[String],
    rolled_back_attempts: &[String],
) -> Result<(), ReceiveDeletionRefusal> {
    if attempt.is_some_and(|state| !state.is_terminal()) {
        return Err(ReceiveDeletionRefusal::NonterminalReceive);
    }
    if unsettled_artifacts {
        return Err(ReceiveDeletionRefusal::UnsettledArtifacts);
    }
    if unpublished_attempts
        .iter()
        .any(|attempt| !rolled_back_attempts.contains(attempt))
    {
        return Err(ReceiveDeletionRefusal::UnsettledPublication);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{check_receive_deletion, ReceiveDeletionRefusal};
    use crate::ports::AttemptState;

    #[test]
    fn active_or_unsettled_receives_keep_their_deletion_barrier() {
        for state in [
            AttemptState::Receiving,
            AttemptState::Committing,
            AttemptState::Cancelling,
            AttemptState::Failing,
        ] {
            assert_eq!(
                check_receive_deletion(Some(state), false, &[], &[]),
                Err(ReceiveDeletionRefusal::NonterminalReceive)
            );
        }
        assert_eq!(
            check_receive_deletion(None, true, &[], &[]),
            Err(ReceiveDeletionRefusal::UnsettledArtifacts)
        );
    }

    #[test]
    fn each_unpublished_attempt_requires_its_own_completed_rollback() {
        let publications = vec!["old".into(), "current".into()];
        for proof in [
            vec![],
            vec!["old".into()],
            vec!["other".into(), "current".into()],
        ] {
            assert_eq!(
                check_receive_deletion(Some(AttemptState::Failed), false, &publications, &proof),
                Err(ReceiveDeletionRefusal::UnsettledPublication)
            );
        }
        assert!(check_receive_deletion(None, false, &publications, &publications).is_ok());
        for state in [
            AttemptState::Completed,
            AttemptState::Failed,
            AttemptState::Cancelled,
        ] {
            assert!(check_receive_deletion(Some(state), false, &[], &[]).is_ok());
        }
    }
}
