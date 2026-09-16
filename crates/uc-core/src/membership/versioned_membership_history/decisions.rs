//! 本机移除与接受、拒绝决定的纯规则。

use super::{
    verify_signature, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipCredential, MembershipDecisionStoreOutcome, MembershipDecisionV2, MembershipEventId,
    MembershipEventV2, MembershipHistoryV2Error, MembershipOperationV2, RemovalDecision,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_DECISION_FORMAT_V2,
    MEMBERSHIP_EVENT_FORMAT_V2,
};

impl VersionedMembershipHistory {
    /// Builds a rule-complete removal event for the current local branch.
    /// The caller must sign its payload before applying it to the history.
    pub fn create_unsigned_local_removal_event(
        &self,
        author: MemberInstanceId,
        author_credential: &MembershipCredential,
        target: MemberInstanceId,
        operation_id: [u8; 16],
        security_state_digest: [u8; 32],
    ) -> Result<MembershipEventV2, MembershipHistoryV2Error> {
        if self.credentials.get(&author) != Some(author_credential) {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        if !self.active_members().contains(&author) {
            return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
        }
        if author == target || !self.effective_members().contains(&target) {
            return Err(MembershipHistoryV2Error::InvalidOperation);
        }

        let position = self.current_position()?;
        let operation = MembershipOperationV2::RemoveDevice { member: target };
        let resulting_members_digest =
            self.expected_resulting_members_digest(position.event_id, &operation)?;

        Ok(MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            self.lineage_id.clone(),
            position.event_id,
            position.depth.saturating_add(1),
            operation_id,
            author,
            author_credential.credential_id,
            author_credential.signature_algorithm_version,
            operation,
            resulting_members_digest,
            security_state_digest,
            Vec::new(),
            None,
            Vec::new(),
        ))
    }

    /// Builds a rule-complete local decision for the current pending removal.
    /// The caller must sign its payload before applying it to the history.
    pub fn create_unsigned_local_removal_decision(
        &self,
        removal_event_id: MembershipEventId,
        local_member: MemberInstanceId,
        local_credential: &MembershipCredential,
        decision: RemovalDecision,
        decision_nonce: [u8; 16],
    ) -> Result<MembershipDecisionV2, MembershipHistoryV2Error> {
        if self.pending_removal_decision(local_member) != Some(removal_event_id) {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        if self.credentials.get(&local_member) != Some(local_credential) {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        let removal = self
            .events
            .get(&removal_event_id)
            .ok_or(MembershipHistoryV2Error::UnknownRemoval)?;
        let parent_id = removal
            .parent_event_id
            .ok_or(MembershipHistoryV2Error::InvalidDecision)?;
        let resulting_members_digest = match decision {
            RemovalDecision::Accept => removal.resulting_members_digest,
            RemovalDecision::Reject => self
                .members_digest_at(parent_id)
                .ok_or(MembershipHistoryV2Error::UnknownParent)?,
        };

        Ok(MembershipDecisionV2::new(
            MEMBERSHIP_DECISION_FORMAT_V2,
            self.lineage_id.clone(),
            removal_event_id,
            local_member,
            local_credential.credential_id,
            local_credential.signature_algorithm_version,
            decision,
            Some(parent_id),
            resulting_members_digest,
            decision_nonce,
            Vec::new(),
        ))
    }

    /// Verifies and applies a signed local decision to this membership branch.
    pub fn apply_signed_local_removal_decision(
        &mut self,
        decision: MembershipDecisionV2,
        local_member: MemberInstanceId,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipDecisionStoreOutcome, MembershipHistoryV2Error> {
        if decision.decided_by_member_instance_id != local_member {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        let removal = self
            .events
            .get(&decision.removal_event_id)
            .ok_or(MembershipHistoryV2Error::UnknownRemoval)?;
        if self
            .peer_decisions
            .get(&(decision.removal_event_id, local_member))
            == Some(&decision)
        {
            return Ok(MembershipDecisionStoreOutcome::AlreadyKnown);
        }
        if removal.parent_event_id != self.known_head {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        let removal_event_id = decision.removal_event_id;
        let choice = decision.decision;
        let outcome = self.verify_and_record_peer_decision(decision, verifier)?;
        if choice == RemovalDecision::Accept {
            self.known_head = Some(removal_event_id);
        }
        self.rebuild_snapshots()?;
        Ok(outcome)
    }

    pub fn verify_and_record_peer_decision(
        &mut self,
        decision: MembershipDecisionV2,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipDecisionStoreOutcome, MembershipHistoryV2Error> {
        if decision.decision_format_version != MEMBERSHIP_DECISION_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if decision.signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if decision.lineage_id != self.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        let removal = self
            .events
            .get(&decision.removal_event_id)
            .ok_or(MembershipHistoryV2Error::UnknownRemoval)?;
        let MembershipOperationV2::RemoveDevice { .. } = removal.operation else {
            return Err(MembershipHistoryV2Error::UnknownRemoval);
        };
        let parent_id = removal
            .parent_event_id
            .ok_or(MembershipHistoryV2Error::InvalidDecision)?;
        let parent_snapshot = self
            .snapshots
            .get(&parent_id)
            .ok_or(MembershipHistoryV2Error::UnknownParent)?;
        if !parent_snapshot
            .members
            .contains(&decision.decided_by_member_instance_id)
            || decision.decided_by_member_instance_id == removal.author_member_instance_id
            || decision.observed_applied_head != Some(parent_id)
        {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        let credential = self
            .credentials
            .get(&decision.decided_by_member_instance_id)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        if decision.decider_credential_id != credential.credential_id
            || decision.signature_algorithm_version != credential.signature_algorithm_version
        {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        let expected_digest = match decision.decision {
            RemovalDecision::Accept => removal.resulting_members_digest,
            RemovalDecision::Reject => self
                .members_digest_at(parent_id)
                .ok_or(MembershipHistoryV2Error::UnknownParent)?,
        };
        if decision.resulting_members_digest != expected_digest {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        verify_signature(
            verifier,
            credential,
            &decision.signing_payload(),
            &decision.signature,
        )?;
        let key = (
            decision.removal_event_id,
            decision.decided_by_member_instance_id,
        );
        if let Some(existing) = self.peer_decisions.get(&key) {
            return if existing == &decision {
                Ok(MembershipDecisionStoreOutcome::AlreadyKnown)
            } else {
                Err(MembershipHistoryV2Error::DecisionConflict)
            };
        }
        self.peer_decisions.insert(key, decision);
        Ok(MembershipDecisionStoreOutcome::Stored)
    }
}
