//! 增量历史证据的连续性、身份和完整性验证。

use super::super::{
    verify_signature, AdmissionActivationReceipt, AdmissionChangeFacts,
    BaseMembershipHistoryPosition, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipDecisionV2, MembershipEventV2, MembershipHistorySuffixPageV4,
    MembershipHistoryV2Error, VersionedMembershipHistory, MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES,
    MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V4,
};
use super::MembershipHistoryProof;
use serde::Serialize;
use sha2::{Digest, Sha256};

impl VersionedMembershipHistory {
    pub fn export_suffix_pages_v4(
        &self,
        sender_admission: AdmissionChangeFacts,
        base_position: BaseMembershipHistoryPosition,
    ) -> Result<Vec<MembershipHistorySuffixPageV4>, MembershipHistoryV2Error> {
        let target_position = self.validate_exchange_sender(&sender_admission)?;
        let proof = MembershipHistoryProof::for_history(self)?;
        if base_position == target_position {
            return Ok(Vec::new());
        }
        let base_event = base_position
            .event_id
            .ok_or(MembershipHistoryV2Error::UnknownParent)?;
        if self.depth(base_event) != Some(base_position.depth) {
            return Err(MembershipHistoryV2Error::UnknownParent);
        }
        let mut suffix_events = Vec::new();
        let mut cursor = self.known_head;
        while cursor != Some(base_event) {
            let event_id = cursor.ok_or(MembershipHistoryV2Error::UnknownParent)?;
            let event = self
                .events
                .get(&event_id)
                .cloned()
                .ok_or(MembershipHistoryV2Error::UnknownParent)?;
            cursor = event.parent_event_id;
            suffix_events.push(event);
        }
        suffix_events.reverse();

        let mut records = Vec::new();
        for event in suffix_events {
            let event_id = event.event_id();
            records.push(MembershipHistorySuffixRecordV4::Event(event));
            if let Some(receipt) = self.activation_receipts.get(&event_id) {
                records.push(MembershipHistorySuffixRecordV4::ActivationReceipt(
                    receipt.activation_receipt.clone(),
                ));
            }
        }
        records.extend(
            self.peer_decisions
                .values()
                .filter(|decision| {
                    decision.decided_by_member_instance_id == sender_admission.member_instance
                })
                .cloned()
                .map(MembershipHistorySuffixRecordV4::Decision),
        );
        if records.is_empty() {
            records.push(MembershipHistorySuffixRecordV4::ProofOnly);
        }
        if records.len() > MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let transfer_id = suffix_transfer_id_v4(
            &self.lineage_id,
            &base_position,
            &target_position,
            &sender_admission,
            &records,
            &proof,
        )?;
        let page_count = u32::try_from(records.len())
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        records
            .into_iter()
            .enumerate()
            .map(|(index, record)| {
                let (events, activation_receipts, decisions) = match record {
                    MembershipHistorySuffixRecordV4::Event(event) => {
                        (vec![event], Vec::new(), Vec::new())
                    }
                    MembershipHistorySuffixRecordV4::ActivationReceipt(receipt) => {
                        (Vec::new(), vec![receipt], Vec::new())
                    }
                    MembershipHistorySuffixRecordV4::Decision(decision) => {
                        (Vec::new(), Vec::new(), vec![decision])
                    }
                    MembershipHistorySuffixRecordV4::ProofOnly => {
                        (Vec::new(), Vec::new(), Vec::new())
                    }
                };
                let page = MembershipHistorySuffixPageV4 {
                    format_version: MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V4,
                    transfer_id,
                    page_index: u32::try_from(index)
                        .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?,
                    page_count,
                    lineage_id: self.lineage_id.clone(),
                    base_position: base_position.clone(),
                    target_position: target_position.clone(),
                    sender_admission: sender_admission.clone(),
                    events,
                    activation_receipts,
                    decisions,
                    sender_proof: (index == 0).then(|| proof.clone()),
                };
                page.validate_envelope()?;
                Ok(page)
            })
            .collect()
    }

    pub fn apply_suffix_pages_v4(
        &mut self,
        pages: &[MembershipHistorySuffixPageV4],
        local_member: MemberInstanceId,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<VersionedMembershipHistory, MembershipHistoryV2Error> {
        let first = pages
            .first()
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if pages.len() != first.page_count as usize
            || pages.len() > MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES
            || self.lineage_id != first.lineage_id
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        if self.current_position()? != first.base_position {
            return Err(MembershipHistoryV2Error::HistoryPositionChanged);
        }
        let proof = first
            .sender_proof
            .as_ref()
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        let mut ordered = pages.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|page| page.page_index);
        let mut records = Vec::with_capacity(ordered.len());
        let mut sender_projection = self.clone();
        let mut candidate = self.clone();
        for (index, page) in ordered.iter().enumerate() {
            page.validate_envelope()?;
            if page.page_index as usize != index
                || page.page_count != first.page_count
                || page.transfer_id != first.transfer_id
                || page.lineage_id != first.lineage_id
                || page.base_position != first.base_position
                || page.target_position != first.target_position
                || page.sender_admission != first.sender_admission
            {
                return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
            }
            if let Some(event) = page.events.first() {
                records.push(MembershipHistorySuffixRecordV4::Event(event.clone()));
                sender_projection.verify_and_receive_event(event.clone(), verifier)?;
                candidate.verify_and_receive_remote_event_for_local_member(
                    event.clone(),
                    local_member,
                    verifier,
                )?;
            } else if let Some(receipt) = page.activation_receipts.first() {
                records.push(MembershipHistorySuffixRecordV4::ActivationReceipt(
                    receipt.clone(),
                ));
                sender_projection
                    .verify_and_record_activation_receipt(receipt.clone(), verifier)?;
                candidate.verify_and_record_activation_receipt(receipt.clone(), verifier)?;
            } else if let Some(decision) = page.decisions.first() {
                records.push(MembershipHistorySuffixRecordV4::Decision(decision.clone()));
                sender_projection.verify_and_record_peer_decision(decision.clone(), verifier)?;
                if decision.decided_by_member_instance_id != first.sender_admission.member_instance
                {
                    return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
                }
                candidate.verify_and_record_peer_decision(decision.clone(), verifier)?;
            } else {
                records.push(MembershipHistorySuffixRecordV4::ProofOnly);
            }
        }
        if suffix_transfer_id_v4(
            &first.lineage_id,
            &first.base_position,
            &first.target_position,
            &first.sender_admission,
            &records,
            proof,
        )? != first.transfer_id
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        if records.iter().any(|record| !proof.covers(record)) {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let sender_projection =
            proof.verify(&sender_projection, &first.target_position, verifier)?;
        // 发送者可以由本批后缀首次引入；必须在完整历史验证后再确认其当前资格和身份签名。
        sender_projection.validate_exchange_sender(&first.sender_admission)?;
        let sender_credential = sender_projection
            .credential_for(first.sender_admission.member_instance)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        verify_signature(
            verifier,
            sender_credential,
            &first.sender_admission.signing_payload(),
            &first.sender_admission.identity_signature,
        )?;
        *self = candidate;
        Ok(sender_projection)
    }
}
#[derive(Serialize)]
pub(super) enum MembershipHistorySuffixRecordV4 {
    Event(MembershipEventV2),
    ActivationReceipt(AdmissionActivationReceipt),
    Decision(MembershipDecisionV2),
    ProofOnly,
}

pub(super) fn suffix_transfer_id_v4(
    lineage_id: &str,
    base_position: &BaseMembershipHistoryPosition,
    target_position: &BaseMembershipHistoryPosition,
    sender_admission: &AdmissionChangeFacts,
    records: &[MembershipHistorySuffixRecordV4],
    proof: &MembershipHistoryProof,
) -> Result<[u8; 32], MembershipHistoryV2Error> {
    let encoded = postcard::to_stdvec(&(
        MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V4,
        lineage_id,
        base_position,
        target_position,
        sender_admission,
        records,
        proof,
    ))
    .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-history-suffix/v4\0");
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(hasher.finalize().into())
}
