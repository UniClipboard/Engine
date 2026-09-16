//! 完整历史证据的拆分、重组与授权验证。

use super::super::{
    verify_signature, AdmissionActivationReceipt, AdmissionChangeFacts,
    BaseMembershipHistoryPosition, HistoricalMembershipSignatureVerifier, MembershipDecisionV2,
    MembershipEventId, MembershipEventV2, MembershipHistoryPageV2, MembershipHistoryV2Error,
    PersistedActivationBaselineV2, PersistedMembershipHistoryV2, VersionedMembershipHistory,
    MAX_MEMBERSHIP_HISTORY_FRAME_SIZE, MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE,
    MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2, PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
};
use sha2::{Digest, Sha256};

impl VersionedMembershipHistory {
    pub(super) fn validate_exchange_sender(
        &self,
        sender_admission: &AdmissionChangeFacts,
    ) -> Result<BaseMembershipHistoryPosition, MembershipHistoryV2Error> {
        let sender_member_instance_id = sender_admission.member_instance;
        let credential = self
            .credential_for(sender_member_instance_id)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        if credential.member_instance_id(&sender_admission.device_id) != sender_member_instance_id
            || (!self.active_members().contains(&sender_member_instance_id)
                && !self.has_removal_decision_by(sender_member_instance_id))
        {
            return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
        }
        self.current_position()
    }

    pub fn export_reconciliation_pages_v2(
        &self,
        sender_admission: AdmissionChangeFacts,
    ) -> Result<Vec<MembershipHistoryPageV2>, MembershipHistoryV2Error> {
        let sender_member = sender_admission.member_instance;
        let mut exchange_history = self.clone();
        exchange_history
            .peer_decisions
            .retain(|_, decision| decision.decided_by_member_instance_id == sender_member);
        exchange_history.export_pages_v2(sender_admission)
    }

    pub fn export_conflict_evidence_pages_v2(
        &self,
        sender_admission: AdmissionChangeFacts,
    ) -> Result<Vec<MembershipHistoryPageV2>, MembershipHistoryV2Error> {
        self.export_pages_v2(sender_admission)
    }

    pub(super) fn export_pages_v2(
        &self,
        sender_admission: AdmissionChangeFacts,
    ) -> Result<Vec<MembershipHistoryPageV2>, MembershipHistoryV2Error> {
        let position = self.validate_exchange_sender(&sender_admission)?;
        let persisted_bytes = self.encode_persisted_v2()?;
        let transfer_id = history_transfer_id(&persisted_bytes);
        let persisted: PersistedMembershipHistoryV2 = postcard::from_bytes(&persisted_bytes)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        let metadata = MembershipHistoryPageMetadata {
            transfer_id,
            lineage_id: &persisted.lineage_id,
            position: &position,
            sender_admission: &sender_admission,
            activation_baseline: persisted.activation_baseline.as_ref(),
            known_head: persisted.known_head,
        };
        let mut pages = vec![empty_history_page(&metadata, 0)?];
        for record in persisted
            .events
            .iter()
            .map(MembershipHistoryPageRecord::Event)
            .chain(
                persisted
                    .activation_receipts
                    .iter()
                    .map(MembershipHistoryPageRecord::ActivationReceipt),
            )
            .chain(
                persisted
                    .peer_decisions
                    .iter()
                    .map(MembershipHistoryPageRecord::Decision),
            )
        {
            append_history_page_record(&mut pages, &metadata, record)?;
        }
        let page_count = u32::try_from(pages.len())
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        for page in &mut pages {
            page.page_count = page_count;
            page.validate_envelope()?;
        }
        Ok(pages)
    }

    pub fn import_exchange_pages_v2(
        pages: &[MembershipHistoryPageV2],
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<Self, MembershipHistoryV2Error> {
        let first = pages
            .first()
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if first.page_count == 0 || pages.len() != first.page_count as usize {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let mut ordered = pages.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|page| page.page_index);
        for (expected_index, page) in ordered.iter().enumerate() {
            page.validate_envelope()?;
            if page.page_index as usize != expected_index
                || page.page_count != first.page_count
                || page.transfer_id != first.transfer_id
                || page.lineage_id != first.lineage_id
                || page.position != first.position
                || page.sender_admission != first.sender_admission
            {
                return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
            }
        }
        let mut persisted = PersistedMembershipHistoryV2 {
            format_version: PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
            lineage_id: first.lineage_id.clone(),
            events: Vec::new(),
            activation_receipts: Vec::new(),
            peer_decisions: Vec::new(),
            activation_baseline: first.activation_baseline.clone(),
            known_head: first.known_head,
        };
        for page in ordered {
            persisted.events.extend(page.events.iter().cloned());
            persisted
                .activation_receipts
                .extend(page.activation_receipts.iter().cloned());
            persisted
                .peer_decisions
                .extend(page.decisions.iter().cloned());
        }
        let encoded = postcard::to_stdvec(&persisted)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if history_transfer_id(&encoded) != first.transfer_id {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let history = Self::decode_persisted_v2(&encoded, verifier)?;
        let sender_member = first.sender_admission.member_instance;
        let sender_credential = history
            .credential_for(sender_member)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        if history.lineage_id != first.lineage_id
            || history.current_position()? != first.position
            || sender_credential.member_instance_id(&first.sender_admission.device_id)
                != sender_member
            || (!history.active_members().contains(&sender_member)
                && !history.has_removal_decision_by(sender_member))
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        verify_signature(
            verifier,
            sender_credential,
            &first.sender_admission.signing_payload(),
            &first.sender_admission.identity_signature,
        )?;
        Ok(history)
    }
}
pub(super) struct MembershipHistoryPageMetadata<'a> {
    transfer_id: [u8; 32],
    lineage_id: &'a str,
    position: &'a BaseMembershipHistoryPosition,
    sender_admission: &'a AdmissionChangeFacts,
    activation_baseline: Option<&'a PersistedActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}

#[derive(Clone, Copy)]
pub(super) enum MembershipHistoryPageRecord<'a> {
    Event(&'a MembershipEventV2),
    ActivationReceipt(&'a AdmissionActivationReceipt),
    Decision(&'a MembershipDecisionV2),
}

impl MembershipHistoryPageRecord<'_> {
    pub(super) fn count_in(self, page: &MembershipHistoryPageV2) -> usize {
        match self {
            Self::Event(_) => page.events.len(),
            Self::ActivationReceipt(_) => page.activation_receipts.len(),
            Self::Decision(_) => page.decisions.len(),
        }
    }

    pub(super) fn push_onto(self, page: &mut MembershipHistoryPageV2) {
        match self {
            Self::Event(event) => page.events.push(event.clone()),
            Self::ActivationReceipt(receipt) => page.activation_receipts.push(receipt.clone()),
            Self::Decision(decision) => page.decisions.push(decision.clone()),
        }
    }

    pub(super) fn pop_from(self, page: &mut MembershipHistoryPageV2) {
        match self {
            Self::Event(_) => {
                page.events.pop();
            }
            Self::ActivationReceipt(_) => {
                page.activation_receipts.pop();
            }
            Self::Decision(_) => {
                page.decisions.pop();
            }
        }
    }
}

pub(super) fn empty_history_page(
    metadata: &MembershipHistoryPageMetadata<'_>,
    page_index: usize,
) -> Result<MembershipHistoryPageV2, MembershipHistoryV2Error> {
    let page_index =
        u32::try_from(page_index).map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
    let page = MembershipHistoryPageV2 {
        exchange_format_version: MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2,
        transfer_id: metadata.transfer_id,
        page_index,
        page_count: u32::MAX,
        lineage_id: metadata.lineage_id.to_owned(),
        position: metadata.position.clone(),
        sender_admission: metadata.sender_admission.clone(),
        events: Vec::new(),
        activation_receipts: Vec::new(),
        decisions: Vec::new(),
        activation_baseline: (page_index == 0)
            .then(|| metadata.activation_baseline.cloned())
            .flatten(),
        known_head: (page_index == 0).then_some(metadata.known_head).flatten(),
    };
    if page.encoded_frame_size()? > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
    }
    Ok(page)
}

pub(super) fn append_history_page_record(
    pages: &mut Vec<MembershipHistoryPageV2>,
    metadata: &MembershipHistoryPageMetadata<'_>,
    record: MembershipHistoryPageRecord<'_>,
) -> Result<(), MembershipHistoryV2Error> {
    let current = pages
        .last_mut()
        .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
    if record.count_in(current) == MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE {
        let next_page_index = pages.len();
        pages.push(empty_history_page(metadata, next_page_index)?);
    }

    let current = pages
        .last_mut()
        .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
    record.push_onto(current);
    if current.encoded_frame_size()? <= MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Ok(());
    }
    record.pop_from(current);
    if current.record_counts().total() == 0 {
        return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
    }

    let next_page_index = pages.len();
    pages.push(empty_history_page(metadata, next_page_index)?);
    let current = pages
        .last_mut()
        .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
    record.push_onto(current);
    if current.encoded_frame_size()? > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        record.pop_from(current);
        return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
    }
    Ok(())
}

pub(super) fn history_transfer_id(encoded_history: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-history-transfer/v2\0");
    hasher.update((encoded_history.len() as u64).to_be_bytes());
    hasher.update(encoded_history);
    hasher.finalize().into()
}
