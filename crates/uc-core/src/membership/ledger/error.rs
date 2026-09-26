use crate::membership::MembershipHistoryV2Error;

/// 输入在当前状态下不合法，或记录已无法安全推进。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerTransitionError {
    #[error("the membership input belongs to another space lineage")]
    LineageMismatch,
    #[error("the membership input does not match the current membership state")]
    InputMismatch,
    #[error("the membership snapshot violates a membership invariant")]
    InvalidSnapshot,
    #[error("the membership history cannot be evaluated")]
    History(#[source] MembershipHistoryV2Error),
    #[error("the membership revision overflowed")]
    RevisionOverflow,
    #[error("the history sync retry counter overflowed")]
    RetryOverflow,
}

/// 流程负责人据此选择拒绝输入或进入恢复。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerTransitionErrorCategory {
    /// 输入不适用于当前状态，记录本身仍可继续推进。
    Rejected,
    /// 记录已无法按规则推进，需要恢复处理。
    RecoveryRequired,
}

impl LedgerTransitionError {
    pub fn category(&self) -> LedgerTransitionErrorCategory {
        match self {
            Self::LineageMismatch | Self::InputMismatch => LedgerTransitionErrorCategory::Rejected,
            Self::InvalidSnapshot
            | Self::History(_)
            | Self::RevisionOverflow
            | Self::RetryOverflow => LedgerTransitionErrorCategory::RecoveryRequired,
        }
    }
}

impl From<MembershipHistoryV2Error> for LedgerTransitionError {
    fn from(source: MembershipHistoryV2Error) -> Self {
        Self::History(source)
    }
}
