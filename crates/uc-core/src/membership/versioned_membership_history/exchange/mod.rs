//! 有界成员历史证据的记录模型与完整性约束。

use super::{
    AdmissionActivationReceipt, AdmissionChangeFacts, BaseMembershipHistoryPosition,
    MembershipDecisionV2, MembershipEventId, MembershipEventV2, MembershipHistoryV2Error,
    PersistedActivationBaselineV2, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
    MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE, MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2,
    MEMBERSHIP_HISTORY_PAGE_FRAME_OVERHEAD,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MembershipHistoryPageRecordCountsV2 {
    pub events: usize,
    pub activation_receipts: usize,
    pub decisions: usize,
}

impl MembershipHistoryPageRecordCountsV2 {
    pub(super) fn total(self) -> usize {
        self.events + self.activation_receipts + self.decisions
    }
}

/// One deterministic, bounded fragment of a complete V2 history image.
/// Pages remain untrusted until the complete transfer is reassembled and
/// verified through [`super::VersionedMembershipHistory::import_exchange_pages_v2`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistoryPageV2 {
    exchange_format_version: u16,
    transfer_id: [u8; 32],
    page_index: u32,
    page_count: u32,
    lineage_id: String,
    position: BaseMembershipHistoryPosition,
    sender_admission: AdmissionChangeFacts,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    decisions: Vec<MembershipDecisionV2>,
    activation_baseline: Option<PersistedActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}

pub(super) const MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3: u16 = 3;
pub const MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES: usize = 64;

/// V3 只携带 `base_position` 之后的连续记录；接收方必须从完全匹配的 base 原子应用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistorySuffixPageV3 {
    format_version: u16,
    transfer_id: [u8; 32],
    page_index: u32,
    page_count: u32,
    lineage_id: String,
    base_position: BaseMembershipHistoryPosition,
    target_position: BaseMembershipHistoryPosition,
    sender_admission: AdmissionChangeFacts,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    decisions: Vec<MembershipDecisionV2>,
}

impl MembershipHistorySuffixPageV3 {
    pub fn transfer_id(&self) -> [u8; 32] {
        self.transfer_id
    }

    pub fn page_index(&self) -> u32 {
        self.page_index
    }

    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    pub fn base_position(&self) -> &BaseMembershipHistoryPosition {
        &self.base_position
    }

    pub fn target_position(&self) -> &BaseMembershipHistoryPosition {
        &self.target_position
    }

    pub fn sender_admission(&self) -> &AdmissionChangeFacts {
        &self.sender_admission
    }

    pub fn validate_envelope(&self) -> Result<(), MembershipHistoryV2Error> {
        let record_count =
            self.events.len() + self.activation_receipts.len() + self.decisions.len();
        if self.format_version != MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3
            || self.page_count == 0
            || self.page_index >= self.page_count
            || record_count != 1
            || postcard::to_stdvec(self)
                .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?
                .len()
                > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        Ok(())
    }
}

impl MembershipHistoryPageV2 {
    pub fn validate_envelope(&self) -> Result<(), MembershipHistoryV2Error> {
        let counts = self.record_counts();
        if self.exchange_format_version != MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.page_count == 0
            || self.page_index >= self.page_count
            || counts.events > MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE
            || counts.activation_receipts > MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE
            || counts.decisions > MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE
            || self.encoded_frame_size()? > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
            || (self.page_index != 0
                && (self.activation_baseline.is_some() || self.known_head.is_some()))
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        Ok(())
    }

    pub(super) fn encoded_frame_size(&self) -> Result<usize, MembershipHistoryV2Error> {
        postcard::to_stdvec(self)
            .map(|page| page.len() + MEMBERSHIP_HISTORY_PAGE_FRAME_OVERHEAD)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)
    }

    pub fn transfer_id(&self) -> [u8; 32] {
        self.transfer_id
    }

    pub fn page_index(&self) -> u32 {
        self.page_index
    }

    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    pub fn sender_admission(&self) -> &AdmissionChangeFacts {
        &self.sender_admission
    }

    pub fn record_counts(&self) -> MembershipHistoryPageRecordCountsV2 {
        MembershipHistoryPageRecordCountsV2 {
            events: self.events.len(),
            activation_receipts: self.activation_receipts.len(),
            decisions: self.decisions.len(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipHistoryV2Ack {
    Continue {
        transfer_id: [u8; 32],
        next_page_index: u32,
    },
    Consistent,
    UpdatesApplied,
    Diverged,
    Invalid,
}

mod pages;
mod suffix;
