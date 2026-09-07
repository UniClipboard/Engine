//! 已验证成员历史：唯一状态与公开记录出口，处理流程由外层完整动作负责。

use super::{
    AdmissionChangeFacts, MemberInstanceId, MembershipDecisionId, MembershipEventId,
    RemovalDecision,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

mod admission;
mod archive;
mod branches;
mod credentials;
mod decisions;
mod exchange;
mod history;
mod outcome;
mod receiving;
mod records;
mod signing;
mod validation;

pub use admission::{
    AdmissionActivationReceipt, AdmissionCompletionV1, MembershipActivationReceiptRecord,
    PreparedAdmissionProofV1,
};
pub use credentials::{
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier,
    MembershipCredential, MembershipCredentialId,
};
pub use exchange::{
    MembershipHistoryPageRecordCountsV2, MembershipHistoryPageV2, MembershipHistorySuffixPageV3,
    MembershipHistoryV2Ack, MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES,
};
pub use outcome::{
    MembershipActivationReceiptStoreOutcome, MembershipDecisionStoreOutcome,
    MembershipHistoryV2Error, MembershipHistoryV2ReceiveOutcome,
};
pub use records::{
    AdmissionSecurityCommitmentV1, BaseMembershipHistoryPosition, MembershipActivationBaselineV2,
    MembershipAdmissionV2, MembershipDecisionV2, MembershipEventV2, MembershipOperationV2,
};

use archive::{
    PersistedActivationBaselineV2, PersistedMembershipHistoryV2,
    PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
};
use exchange::MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3;
use signing::{append_field, append_operation, append_optional_digest, append_optional_event_id};
use validation::{members_digest, verify_signature};

pub const MEMBERSHIP_CREDENTIAL_FORMAT_V1: u16 = 1;
pub const ED25519_SIGNATURE_ALGORITHM_V1: u16 = 1;
pub const MEMBERSHIP_EVENT_FORMAT_V2: u16 = 2;
pub const MEMBERSHIP_DECISION_FORMAT_V2: u16 = 2;
pub const ADMISSION_SECURITY_COMMITMENT_FORMAT_V1: u16 = 1;
pub const PREPARED_ADMISSION_PROOF_FORMAT_V1: u16 = 1;
pub const ADMISSION_COMPLETION_FORMAT_V1: u16 = 1;
pub const MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2: u16 = 2;
pub const MAX_MEMBERSHIP_HISTORY_FRAME_SIZE: usize = 4 * 1024 * 1024;
pub const MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE: usize = 256;
const MEMBERSHIP_HISTORY_PAGE_FRAME_OVERHEAD: usize = 2;
const ACTIVATION_RECEIPT_FORMAT_V1: u16 = 1;
const ACTIVATION_RECEIPT_RECORD_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MembershipHistorySnapshot {
    members: BTreeSet<MemberInstanceId>,
    active_members: BTreeSet<MemberInstanceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedMembershipHistory {
    lineage_id: String,
    events: BTreeMap<MembershipEventId, MembershipEventV2>,
    snapshots: BTreeMap<MembershipEventId, MembershipHistorySnapshot>,
    credentials: BTreeMap<MemberInstanceId, MembershipCredential>,
    operation_ids: BTreeSet<[u8; 16]>,
    activation_receipts: BTreeMap<MembershipEventId, MembershipActivationReceiptRecord>,
    peer_decisions: BTreeMap<(MembershipEventId, MemberInstanceId), MembershipDecisionV2>,
    activation_baseline: Option<MembershipActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}
