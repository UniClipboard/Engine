//! 已验证记录的接纳、回执应用与历史合并规则。

use super::{
    verify_signature, AdmissionActivationReceipt, HistoricalMembershipSignatureVerifier,
    MemberInstanceId, MembershipActivationReceiptRecord, MembershipActivationReceiptStoreOutcome,
    MembershipDecisionStoreOutcome, MembershipDecisionV2, MembershipEventV2,
    MembershipHistoryV2Error, MembershipHistoryV2ReceiveOutcome, MembershipOperationV2,
    VersionedMembershipHistory, ACTIVATION_RECEIPT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
    MEMBERSHIP_EVENT_FORMAT_V2,
};

impl VersionedMembershipHistory {
    pub fn merge_remote_history(
        &mut self,
        incoming: &Self,
        local_member: MemberInstanceId,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<bool, MembershipHistoryV2Error> {
        if self.lineage_id != incoming.lineage_id
            || self.activation_baseline != incoming.activation_baseline
        {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        let mut changed = false;
        let mut events = incoming.events.values().cloned().collect::<Vec<_>>();
        events.sort_by_key(|event| (event.parent_depth, event.event_id()));
        for event in events {
            let event_id = event.event_id();
            if let Some(existing) = self.events.get(&event_id) {
                if existing != &event {
                    return Err(MembershipHistoryV2Error::InvalidSignature);
                }
            } else {
                self.verify_and_receive_remote_event_for_local_member(
                    event,
                    local_member,
                    verifier,
                )?;
                changed = true;
            }
            if let Some(record) = incoming.activation_receipts.get(&event_id) {
                match self.verify_and_record_activation_receipt(
                    record.activation_receipt.clone(),
                    verifier,
                )? {
                    MembershipActivationReceiptStoreOutcome::Stored => changed = true,
                    MembershipActivationReceiptStoreOutcome::AlreadyKnown => {}
                }
            }
        }
        let mut decisions = incoming
            .peer_decisions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        decisions.sort_by_key(MembershipDecisionV2::decision_id);
        for decision in decisions {
            match self.verify_and_record_peer_decision(decision, verifier)? {
                MembershipDecisionStoreOutcome::Stored => changed = true,
                MembershipDecisionStoreOutcome::AlreadyKnown => {}
            }
        }
        Ok(changed)
    }

    pub fn verify_and_receive_remote_event_for_local_member(
        &mut self,
        event: MembershipEventV2,
        local_member: MemberInstanceId,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipHistoryV2ReceiveOutcome, MembershipHistoryV2Error> {
        let previous_head = self.known_head;
        let waits_for_local_decision = event.parent_event_id == previous_head
            && event.author_member_instance_id != local_member
            && matches!(event.operation, MembershipOperationV2::RemoveDevice { .. })
            && previous_head
                .and_then(|head| self.snapshots.get(&head))
                .is_some_and(|snapshot| snapshot.members.contains(&local_member));
        let outcome = self.verify_and_receive_event(event, verifier)?;
        if outcome == MembershipHistoryV2ReceiveOutcome::Applied && waits_for_local_decision {
            self.known_head = previous_head;
            self.rebuild_snapshots()?;
        }
        Ok(outcome)
    }

    pub fn verify_and_receive_event(
        &mut self,
        event: MembershipEventV2,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipHistoryV2ReceiveOutcome, MembershipHistoryV2Error> {
        if event.event_format_version != MEMBERSHIP_EVENT_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if event.author_signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if event.lineage_id != self.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        let event_id = event.event_id();
        if let Some(existing) = self.events.get(&event_id) {
            return if existing == &event {
                Ok(MembershipHistoryV2ReceiveOutcome::AlreadyKnown)
            } else {
                Err(MembershipHistoryV2Error::InvalidSignature)
            };
        }
        if self.operation_ids.contains(&event.operation_id) {
            return Err(MembershipHistoryV2Error::OperationReplay);
        }

        let (parent_snapshot, author_credential) = self.validate_parent_and_author(&event)?;
        self.verify_signature(&event, &author_credential, verifier)?;
        self.validate_operation(&event, &parent_snapshot)?;

        let expected_digest =
            self.expected_resulting_members_digest(event.parent_event_id, &event.operation)?;
        if event.resulting_members_digest != expected_digest {
            return Err(MembershipHistoryV2Error::ResultingMembersDigestMismatch);
        }

        if let MembershipOperationV2::AddDevice { admission } = &event.operation {
            if let Some(existing) = self.credentials.get(&admission.facts.member_instance) {
                if existing != &admission.membership_credential {
                    return Err(MembershipHistoryV2Error::CredentialConflict);
                }
                return Err(MembershipHistoryV2Error::InvalidOperation);
            }
            self.credentials.insert(
                admission.facts.member_instance,
                admission.membership_credential.clone(),
            );
        }

        let extends_known_head = event.parent_event_id == self.known_head;
        self.operation_ids.insert(event.operation_id);
        self.events.insert(event_id, event);
        self.rebuild_snapshots()?;
        if self.known_head.is_none() || extends_known_head {
            self.known_head = Some(event_id);
            Ok(MembershipHistoryV2ReceiveOutcome::Applied)
        } else {
            Ok(MembershipHistoryV2ReceiveOutcome::Diverged)
        }
    }

    pub fn verify_and_record_activation_receipt(
        &mut self,
        receipt: AdmissionActivationReceipt,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipActivationReceiptStoreOutcome, MembershipHistoryV2Error> {
        if receipt.receipt_format_version != ACTIVATION_RECEIPT_FORMAT_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        let event = self.events.get(&receipt.event_id).ok_or(
            MembershipHistoryV2Error::MissingMembershipEvent(receipt.event_id),
        )?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(MembershipHistoryV2Error::InvalidActivationReceipt);
        };
        if receipt.joiner_member_instance_id != admission.facts.member_instance
            || receipt.applied_history_digest != event.resulting_members_digest
            || receipt.installed_security_commitment_id != admission.security_commitment_id
        {
            return Err(MembershipHistoryV2Error::InvalidActivationReceipt);
        }
        verify_signature(
            verifier,
            &admission.membership_credential,
            &receipt.signing_payload(),
            &receipt.signature,
        )?;
        let record = MembershipActivationReceiptRecord::new(receipt);
        if let Some(existing) = self.activation_receipts.get(&record.event_id) {
            return if existing == &record {
                Ok(MembershipActivationReceiptStoreOutcome::AlreadyKnown)
            } else {
                Err(MembershipHistoryV2Error::ActivationReceiptConflict)
            };
        }
        self.activation_receipts.insert(record.event_id, record);
        self.rebuild_snapshots()?;
        Ok(MembershipActivationReceiptStoreOutcome::Stored)
    }
}
