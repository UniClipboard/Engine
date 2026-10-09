//! 历史规则的稳定结果与错误。

use super::MembershipEventId;
use crate::error_class::ErrorClass;
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// 纯结构校验失败时 `source` 为空；postcard 编解码失败时保留为来源。
    InvalidPersistedHistory {
        source: Option<postcard::Error>,
    },
    IncompleteHistoryProof,
    HistoryPositionChanged,
}

impl MembershipHistoryV2Error {
    pub fn invalid_persisted_history() -> Self {
        Self::InvalidPersistedHistory { source: None }
    }

    pub fn invalid_persisted_history_from(source: postcard::Error) -> Self {
        Self::InvalidPersistedHistory {
            source: Some(source),
        }
    }
}

impl ErrorClass for MembershipHistoryV2Error {
    fn class(&self) -> &'static str {
        match self {
            Self::UpgradeRequired => "upgrade_required",
            Self::InvalidLineage => "invalid_lineage",
            Self::InvalidGenesis => "invalid_genesis",
            Self::UnknownParent => "unknown_parent",
            Self::InvalidParentDepth => "invalid_parent_depth",
            Self::OperationReplay => "operation_replay",
            Self::UnauthorizedAuthor => "unauthorized_author",
            Self::AwaitingActivationReceipt => "awaiting_activation_receipt",
            Self::InvalidCredential => "invalid_credential",
            Self::CredentialConflict => "credential_conflict",
            Self::InvalidSignature => "invalid_signature",
            Self::UnsupportedSignatureAlgorithm => "unsupported_signature_algorithm",
            Self::InvalidSecurityCommitment => "invalid_security_commitment",
            Self::InvalidActivationBaseline => "invalid_activation_baseline",
            Self::InvalidOperation => "invalid_operation",
            Self::ResultingMembersDigestMismatch => "resulting_members_digest_mismatch",
            Self::InvalidActivationReceipt => "invalid_activation_receipt",
            Self::ActivationReceiptConflict => "activation_receipt_conflict",
            Self::UnknownRemoval => "unknown_removal",
            Self::InvalidDecision => "invalid_decision",
            Self::DecisionConflict => "decision_conflict",
            Self::InvalidPersistedHistory { .. } => "invalid_persisted_history",
            Self::IncompleteHistoryProof => "incomplete_history_proof",
            Self::HistoryPositionChanged => "history_position_changed",
            Self::MissingMembershipEvent(_) => "missing_membership_event",
        }
    }
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
            Self::InvalidPersistedHistory { .. } => "persisted membership history is invalid",
            Self::IncompleteHistoryProof => "complete membership history evidence is required",
            Self::HistoryPositionChanged => "membership history changed during transfer",
        })
    }
}

impl std::error::Error for MembershipHistoryV2Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidPersistedHistory {
                source: Some(source),
            } => Some(source),
            _ => None,
        }
    }
}
