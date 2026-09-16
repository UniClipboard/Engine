use super::super::{
    BaseMembershipHistoryPosition, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipEventId, MembershipHistoryV2Error, PersistedMembershipHistoryV2,
    VersionedMembershipHistory, PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 发送方归档的精确证明范围；不要求接收方删除自己的旁支或决定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MembershipHistoryProof {
    baseline_digest: [u8; 32],
    events: Vec<MembershipEventId>,
    activation_receipts: Vec<MembershipEventId>,
    decisions: Vec<(MembershipEventId, MemberInstanceId)>,
}

impl MembershipHistoryProof {
    pub(super) fn covers(&self, record: &super::suffix::MembershipHistorySuffixRecordV4) -> bool {
        use super::suffix::MembershipHistorySuffixRecordV4;
        match record {
            MembershipHistorySuffixRecordV4::Event(event) => {
                self.events.binary_search(&event.event_id()).is_ok()
            }
            MembershipHistorySuffixRecordV4::ActivationReceipt(receipt) => self
                .activation_receipts
                .binary_search(&receipt.event_id)
                .is_ok(),
            MembershipHistorySuffixRecordV4::Decision(decision) => self
                .decisions
                .binary_search(&(
                    decision.removal_event_id,
                    decision.decided_by_member_instance_id,
                ))
                .is_ok(),
            MembershipHistorySuffixRecordV4::ProofOnly => true,
        }
    }

    pub(super) fn for_history(
        history: &VersionedMembershipHistory,
    ) -> Result<Self, MembershipHistoryV2Error> {
        Ok(Self {
            baseline_digest: Sha256::digest(history.activation_baseline_identity()?).into(),
            events: history.events.keys().copied().collect(),
            activation_receipts: history.activation_receipts.keys().copied().collect(),
            decisions: history.peer_decisions.keys().copied().collect(),
        })
    }

    pub(super) fn verify(
        &self,
        available: &VersionedMembershipHistory,
        target: &BaseMembershipHistoryPosition,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<VersionedMembershipHistory, MembershipHistoryV2Error> {
        if !strictly_ordered(&self.events)
            || !strictly_ordered(&self.activation_receipts)
            || !strictly_ordered(&self.decisions)
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        if self.baseline_digest
            != <[u8; 32]>::from(Sha256::digest(available.activation_baseline_identity()?))
        {
            return Err(MembershipHistoryV2Error::IncompleteHistoryProof);
        }
        let missing = || MembershipHistoryV2Error::IncompleteHistoryProof;
        let archive = PersistedMembershipHistoryV2 {
            format_version: PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
            lineage_id: available.lineage_id.clone(),
            events: self
                .events
                .iter()
                .map(|id| available.events.get(id).cloned().ok_or_else(missing))
                .collect::<Result<_, _>>()?,
            activation_receipts: self
                .activation_receipts
                .iter()
                .map(|id| {
                    available
                        .activation_receipts
                        .get(id)
                        .map(|r| r.activation_receipt.clone())
                        .ok_or_else(missing)
                })
                .collect::<Result<_, _>>()?,
            peer_decisions: self
                .decisions
                .iter()
                .map(|id| {
                    available
                        .peer_decisions
                        .get(id)
                        .cloned()
                        .ok_or_else(missing)
                })
                .collect::<Result<_, _>>()?,
            activation_baseline: available.activation_baseline.clone().map(Into::into),
            known_head: target.event_id,
        };
        let bytes = postcard::to_stdvec(&archive)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        let verified = VersionedMembershipHistory::decode_persisted_v2(&bytes, verifier)?;
        if verified.current_position()? != *target {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        Ok(verified)
    }
}

fn strictly_ordered<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}
