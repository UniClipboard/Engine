//! Persists Space join progress inside the membership ledger.
//!
//! Admission use cases decide protocol steps, invitation handling, replies,
//! delivery, and recovery scheduling. This module saves the resulting
//! `SpaceJoinRecord` and any coupled membership history, peer relationship,
//! pending effect, or invitation claim in one conditional ledger commit.

use uc_core::membership::{
    AdmissionInboxRecord, AdmissionProfileMetadata, MembershipHistoryRelationship, SpaceJoinRecord,
    SpaceJoinRecordId, VersionedMembershipHistory,
};

use super::{MembershipLedger, MembershipLedgerError, PeerReconciliationRecord};

impl MembershipLedger {
    pub(crate) async fn start_join_record(
        &self,
        record: SpaceJoinRecord,
        consumed_invitation_digest: Option<[u8; 32]>,
        expected_membership_history_v2: Option<Vec<u8>>,
    ) -> Result<AdmissionProfileMetadata, MembershipLedgerError> {
        let record_id = record.record_id;
        let committed = self
            .compare_and_commit(move |ledger| {
                let key = *record_id.as_bytes();
                if ledger.admission_records.contains_key(&key) {
                    return Err(MembershipLedgerError::Conflict);
                }
                if let Some(expected_history) = expected_membership_history_v2.as_deref() {
                    if ledger.membership_history.as_deref() != Some(expected_history) {
                        return Err(MembershipLedgerError::Conflict);
                    }
                }
                let next_revision = ledger
                    .revision
                    .checked_add(1)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                let metadata = ledger
                    .admission_profile
                    .as_mut()
                    .ok_or(MembershipLedgerError::RecoveryRequired)?;
                if let Some(digest) = consumed_invitation_digest {
                    if metadata
                        .consumed_invitation_attempts
                        .insert(digest, record_id)
                        .is_some()
                    {
                        return Err(MembershipLedgerError::Conflict);
                    }
                }
                metadata.device_trust_revision = next_revision;
                ledger.admission_records.insert(key, record);
                Ok(())
            })
            .await?;
        committed
            .admission_profile
            .ok_or(MembershipLedgerError::Corrupt)
    }

    pub(crate) async fn load_join_record(
        &self,
        record_id: SpaceJoinRecordId,
    ) -> Result<Option<SpaceJoinRecord>, MembershipLedgerError> {
        Ok(self
            .load_verified()
            .await?
            .record()
            .admission_records
            .get(record_id.as_bytes())
            .cloned())
    }

    pub(crate) async fn activate_joined_space(
        &self,
        mut next: SpaceJoinRecord,
        expected_membership_history_v2: Vec<u8>,
        membership_history_v2: Vec<u8>,
    ) -> Result<AdmissionProfileMetadata, MembershipLedgerError> {
        let record_id = next.record_id;
        let expected_record_version = next.record_version;
        next.record_version = expected_record_version
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let target_history = VersionedMembershipHistory::decode_persisted_v2(
            &membership_history_v2,
            self.verifier.as_ref(),
        )
        .map_err(|_| MembershipLedgerError::Corrupt)?;
        let target_lineage_id = target_history.lineage_id().to_owned();
        let local_member_instance = next
            .joiner_member_instance
            .ok_or(MembershipLedgerError::Corrupt)?;
        let local_device_id = target_history
            .admission_facts_for(local_member_instance)
            .map(|facts| facts.device_id.clone())
            .ok_or(MembershipLedgerError::Corrupt)?;
        let peer_device_ids = target_history
            .active_members()
            .iter()
            .filter(|member| **member != local_member_instance)
            .map(|member| {
                target_history
                    .admission_facts_for(*member)
                    .map(|facts| facts.device_id.clone())
                    .ok_or(MembershipLedgerError::Corrupt)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let committed = self
            .compare_and_commit(move |ledger| {
                if ledger.membership_history.as_deref()
                    != Some(expected_membership_history_v2.as_slice())
                {
                    return Err(MembershipLedgerError::Conflict);
                }
                let key = *record_id.as_bytes();
                let current = ledger
                    .admission_records
                    .get(&key)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if current.record_version != expected_record_version {
                    return Err(MembershipLedgerError::Conflict);
                }
                let next_revision = ledger
                    .revision
                    .checked_add(1)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                ledger.lineage_id = Some(target_lineage_id);
                ledger.membership_history = Some(membership_history_v2);
                ledger.local_device_id = Some(local_device_id);
                ledger.local_member_instance = Some(local_member_instance);
                ledger.local_join_active = true;
                ledger.peer_reconciliation.clear();
                for peer_device_id in peer_device_ids {
                    ledger.peer_reconciliation.insert(
                        peer_device_id.clone(),
                        PeerReconciliationRecord {
                            peer_device_id,
                            relationship: MembershipHistoryRelationship::Unknown,
                            confirmed_position: None,
                            restricted_delivery: Vec::new(),
                            updated_at_ms: 0,
                        },
                    );
                }
                ledger.inbound_transfers.clear();
                ledger.completed_inbound_transfers.clear();
                ledger.pending_effects.clear();
                ledger.admission_records.insert(key, next);
                let metadata = ledger
                    .admission_profile
                    .as_mut()
                    .ok_or(MembershipLedgerError::RecoveryRequired)?;
                metadata.device_trust_revision = next_revision;
                Ok(())
            })
            .await?;
        committed
            .admission_profile
            .ok_or(MembershipLedgerError::Corrupt)
    }

    pub(crate) async fn recoverable_join_records(
        &self,
    ) -> Result<Vec<SpaceJoinRecord>, MembershipLedgerError> {
        let mut records = self
            .load_verified()
            .await?
            .record()
            .admission_records
            .values()
            .filter(|record| record.has_recovery_work())
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by_key(|record| *record.record_id.as_bytes());
        Ok(records)
    }

    pub(crate) async fn settle_join_message(
        &self,
        record_id: SpaceJoinRecordId,
        expected_record_version: u64,
        message_id: [u8; 32],
        acknowledgment: AdmissionInboxRecord,
    ) -> Result<AdmissionProfileMetadata, MembershipLedgerError> {
        let committed = self
            .compare_and_commit(move |ledger| {
                let key = *record_id.as_bytes();
                let record = ledger
                    .admission_records
                    .get_mut(&key)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if record.record_version != expected_record_version {
                    return Err(MembershipLedgerError::Conflict);
                }
                let message = record
                    .outboxes
                    .iter_mut()
                    .find(|message| !message.superseded && message.message_id == message_id)
                    .ok_or(MembershipLedgerError::Conflict)?;
                message.superseded = true;
                if !record.inbox_dedup.contains(&acknowledgment) {
                    record.inbox_dedup.push(acknowledgment);
                }
                record.record_version = expected_record_version
                    .checked_add(1)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                let next_revision = ledger
                    .revision
                    .checked_add(1)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                let metadata = ledger
                    .admission_profile
                    .as_mut()
                    .ok_or(MembershipLedgerError::RecoveryRequired)?;
                metadata.device_trust_revision = next_revision;
                Ok(())
            })
            .await?;
        committed
            .admission_profile
            .ok_or(MembershipLedgerError::Corrupt)
    }

    pub(crate) async fn current_local_join_record(
        &self,
    ) -> Result<Option<SpaceJoinRecord>, MembershipLedgerError> {
        let snapshot = self.load_verified().await?;
        let floor = snapshot
            .record()
            .admission_profile
            .as_ref()
            .map(|metadata| metadata.join_projection_floor_ordinal)
            .unwrap_or(0);
        Ok(snapshot
            .record()
            .admission_records
            .values()
            .filter(|record| record.is_joiner())
            .filter_map(|record| {
                record
                    .local_join_ordinal
                    .filter(|ordinal| *ordinal >= floor)
                    .map(|ordinal| (ordinal, record))
            })
            .max_by_key(|(ordinal, _)| *ordinal)
            .map(|(_, record)| record.clone()))
    }

    pub(crate) async fn save_join_record_progress(
        &self,
        mut next: SpaceJoinRecord,
    ) -> Result<AdmissionProfileMetadata, MembershipLedgerError> {
        let record_id = next.record_id;
        let expected_record_version = next.record_version;
        next.record_version = expected_record_version
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let committed = self
            .compare_and_commit(move |ledger| {
                let key = *record_id.as_bytes();
                let current = ledger
                    .admission_records
                    .get(&key)
                    .ok_or(MembershipLedgerError::Conflict)?;
                if current.record_version != expected_record_version {
                    return Err(MembershipLedgerError::Conflict);
                }
                ledger.admission_records.insert(key, next);
                let next_revision = ledger
                    .revision
                    .checked_add(1)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                let metadata = ledger
                    .admission_profile
                    .as_mut()
                    .ok_or(MembershipLedgerError::RecoveryRequired)?;
                metadata.device_trust_revision = next_revision;
                Ok(())
            })
            .await?;
        committed
            .admission_profile
            .ok_or(MembershipLedgerError::Corrupt)
    }
}
