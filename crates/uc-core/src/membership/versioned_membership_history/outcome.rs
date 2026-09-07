//! 历史规则的稳定结果与错误。

use super::MembershipEventId;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryV2ReceiveOutcome {
    Applied,
    AlreadyKnown,
    Diverged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipActivationReceiptStoreOutcome {
    Stored,
    AlreadyKnown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipDecisionStoreOutcome {
    Stored,
    AlreadyKnown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryV2Error {
    UpgradeRequired,
    InvalidLineage,
    InvalidGenesis,
    UnknownParent,
    InvalidParentDepth,
    OperationReplay,
    UnauthorizedAuthor,
    AwaitingActivationReceipt,
    InvalidCredential,
    CredentialConflict,
    InvalidSignature,
    UnsupportedSignatureAlgorithm,
    InvalidSecurityCommitment,
    InvalidActivationBaseline,
    InvalidOperation,
    ResultingMembersDigestMismatch,
    MissingMembershipEvent(MembershipEventId),
    InvalidActivationReceipt,
    ActivationReceiptConflict,
    UnknownRemoval,
    InvalidDecision,
    DecisionConflict,
    InvalidPersistedHistory,
}

impl fmt::Display for MembershipHistoryV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UpgradeRequired => "membership history version requires an upgrade",
            Self::InvalidLineage => "membership history lineage is invalid",
            Self::InvalidGenesis => "membership history genesis is invalid",
            Self::UnknownParent => "membership history parent is unknown",
            Self::InvalidParentDepth => "membership history parent depth is invalid",
            Self::OperationReplay => "membership operation identifier was already used",
            Self::UnauthorizedAuthor => "membership event author was not authorized at the parent",
            Self::AwaitingActivationReceipt => {
                "membership event author is awaiting activation proof"
            }
            Self::InvalidCredential => "membership credential is invalid",
            Self::CredentialConflict => "membership credential conflicts with retained history",
            Self::InvalidSignature => "membership history signature is invalid",
            Self::UnsupportedSignatureAlgorithm => {
                "membership history signature algorithm is not supported"
            }
            Self::InvalidSecurityCommitment => "admission security commitment is invalid",
            Self::InvalidActivationBaseline => "membership activation baseline is invalid",
            Self::InvalidOperation => "membership history operation is invalid at the parent",
            Self::ResultingMembersDigestMismatch => {
                "membership event resulting members digest does not match"
            }
            Self::MissingMembershipEvent(_) => {
                "membership activation receipt references an unknown event"
            }
            Self::InvalidActivationReceipt => "membership activation receipt is invalid",
            Self::ActivationReceiptConflict => {
                "membership activation receipt conflicts with retained history"
            }
            Self::UnknownRemoval => "membership decision references an unknown removal",
            Self::InvalidDecision => "membership decision is invalid at the removal parent",
            Self::DecisionConflict => "membership decision conflicts with retained history",
            Self::InvalidPersistedHistory => "persisted membership history is invalid",
        })
    }
}

impl std::error::Error for MembershipHistoryV2Error {}
