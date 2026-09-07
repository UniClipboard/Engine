//! 已签名历史的规范存档表示；其字节参与历史身份计算。

use super::{
    AdmissionActivationReceipt, AdmissionChangeFacts, HistoricalMembershipSignatureVerifier,
    MembershipActivationBaselineV2, MembershipCredential, MembershipDecisionV2, MembershipEventId,
    MembershipEventV2, MembershipHistoryV2Error, VersionedMembershipHistory,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(super) const PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum PersistedActivationBaselineV2 {
    Established {
        lineage_id: String,
        head_event_id: MembershipEventId,
        head_depth: u64,
        current_members: Vec<(AdmissionChangeFacts, MembershipCredential)>,
    },
}

impl From<MembershipActivationBaselineV2> for PersistedActivationBaselineV2 {
    fn from(value: MembershipActivationBaselineV2) -> Self {
        match value {
            MembershipActivationBaselineV2::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            } => Self::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            },
        }
    }
}

impl From<PersistedActivationBaselineV2> for MembershipActivationBaselineV2 {
    fn from(value: PersistedActivationBaselineV2) -> Self {
        match value {
            PersistedActivationBaselineV2::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            } => Self::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PersistedMembershipHistoryV2 {
    pub(super) format_version: u16,
    pub(super) lineage_id: String,
    pub(super) events: Vec<MembershipEventV2>,
    pub(super) activation_receipts: Vec<AdmissionActivationReceipt>,
    pub(super) peer_decisions: Vec<MembershipDecisionV2>,
    pub(super) activation_baseline: Option<PersistedActivationBaselineV2>,
    pub(super) known_head: Option<MembershipEventId>,
}

impl VersionedMembershipHistory {
    pub(crate) fn activation_baseline_identity(&self) -> Result<Vec<u8>, MembershipHistoryV2Error> {
        postcard::to_stdvec(
            &self
                .activation_baseline
                .clone()
                .map(PersistedActivationBaselineV2::from),
        )
        .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)
    }

    pub fn encode_persisted_v2(&self) -> Result<Vec<u8>, MembershipHistoryV2Error> {
        let persisted = PersistedMembershipHistoryV2 {
            format_version: PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
            lineage_id: self.lineage_id.clone(),
            events: self.events.values().cloned().collect(),
            activation_receipts: self
                .activation_receipts
                .values()
                .map(|record| record.activation_receipt.clone())
                .collect(),
            peer_decisions: self.peer_decisions.values().cloned().collect(),
            activation_baseline: self.activation_baseline.clone().map(Into::into),
            known_head: self.known_head,
        };
        postcard::to_stdvec(&persisted)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)
    }

    pub fn decode_persisted_v2(
        bytes: &[u8],
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<Self, MembershipHistoryV2Error> {
        let mut persisted: PersistedMembershipHistoryV2 = postcard::from_bytes(bytes)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if persisted.format_version != PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        let mut history = match persisted.activation_baseline.take() {
            Some(baseline) => Self::from_activation_baseline(baseline.into())?,
            None => Self::new(persisted.lineage_id.clone()),
        };
        history.verify_activation_baseline_signatures(verifier)?;
        if history.lineage_id != persisted.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        persisted
            .events
            .sort_by_key(|event| (event.parent_depth, event.event_id()));
        let mut receipts_by_event = BTreeMap::<_, Vec<_>>::new();
        for receipt in persisted.activation_receipts {
            receipts_by_event
                .entry(receipt.event_id)
                .or_default()
                .push(receipt);
        }
        for event in persisted.events {
            let event_id = event.event_id();
            history.verify_and_receive_event(event, verifier)?;
            if let Some(receipts) = receipts_by_event.remove(&event_id) {
                for receipt in receipts {
                    history.verify_and_record_activation_receipt(receipt, verifier)?;
                }
            }
        }
        for receipts in receipts_by_event.into_values() {
            for receipt in receipts {
                history.verify_and_record_activation_receipt(receipt, verifier)?;
            }
        }
        persisted
            .peer_decisions
            .sort_by_key(|decision| decision.decision_id());
        for decision in persisted.peer_decisions {
            history.verify_and_record_peer_decision(decision, verifier)?;
        }
        if persisted.known_head.is_some()
            && !persisted.known_head.is_some_and(|head| {
                history.snapshots.contains_key(&head)
                    || history
                        .activation_baseline
                        .as_ref()
                        .is_some_and(|baseline| baseline.head_and_depth().0 == head)
            })
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        history.known_head = persisted.known_head;
        history.rebuild_snapshots()?;
        Ok(history)
    }
}
