use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureVerifier, MembershipConflictEvidenceV3,
    MembershipConflictPolicy, MembershipCredential, MembershipHistoryPageV2,
    MembershipHistoryRelationship, VersionedMembershipHistory,
};

use super::model::MembershipConflictRecord;
use super::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    LoadedMembershipLedger, MembershipConflictStatus, MembershipEffectPhase, MembershipLedgerError,
    MembershipLedgerMutation, PausedSpaceMember, SpaceMemberPauseReason,
};

/// Loads the complete decrypted application membership record.
///
/// Implementations must encrypt every field at rest with the profile or
/// Space MasterKey. The application never permits a plaintext fallback.
#[async_trait]
pub trait LoadMembershipLedgerPort: Send + Sync {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError>;
}

/// Atomically commits the complete sensitive membership record.
///
/// Implementations must compare both revision and history digest in the same
/// encrypted transaction. Partial writes and plaintext mirrors are invalid.
#[async_trait]
pub trait CommitMembershipLedgerPort: Send + Sync {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError>;
}

pub(crate) struct MembershipLedger {
    loader: Arc<dyn LoadMembershipLedgerPort>,
    committer: Arc<dyn CommitMembershipLedgerPort>,
    pub(super) verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

pub(crate) struct VerifiedMembershipLedger {
    record: LoadedMembershipLedger,
    history: Option<VersionedMembershipHistory>,
}

impl VerifiedMembershipLedger {
    pub(crate) fn record(&self) -> &LoadedMembershipLedger {
        &self.record
    }

    pub(crate) fn history(&self) -> Option<&VersionedMembershipHistory> {
        self.history.as_ref()
    }

    pub(crate) fn current_scope(
        &self,
    ) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        let history = self
            .history
            .as_ref()
            .ok_or(CurrentSpaceMemberScopeError::NoCurrentSpace)?;
        derive_current_scope(&self.record, history)
    }

    pub(crate) fn history_digest(&self) -> Option<[u8; 32]> {
        self.record
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)))
    }
}

fn derive_current_scope(
    loaded: &LoadedMembershipLedger,
    history: &VersionedMembershipHistory,
) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
    let local_device_id = loaded
        .local_device_id
        .as_ref()
        .ok_or(CurrentSpaceMemberScopeError::RecoveryRequired)?;
    let local_member_instance = loaded
        .local_member_instance
        .ok_or(CurrentSpaceMemberScopeError::RecoveryRequired)?;
    let local_member_active = loaded.local_join_active
        && history.active_members().contains(&local_member_instance)
        && history
            .admission_facts_for(local_member_instance)
            .is_some_and(|facts| &facts.device_id == local_device_id);

    let mut usable_peer_device_ids = Vec::new();
    let mut paused_peer_devices = Vec::new();
    for member in history.active_members() {
        if member == local_member_instance {
            continue;
        }
        let facts = history
            .admission_facts_for(member)
            .ok_or(CurrentSpaceMemberScopeError::RecoveryRequired)?;
        let peer_device_id = facts.device_id.clone();
        let effect_pending = loaded.pending_effects.values().any(|effect| {
            effect.phase < MembershipEffectPhase::Activated
                && effect.affected_device_ids.contains(&peer_device_id)
        });
        let pause_reason = if !local_member_active {
            Some(SpaceMemberPauseReason::LocalMemberInactive)
        } else if effect_pending {
            Some(SpaceMemberPauseReason::EffectPending)
        } else {
            match loaded.peer_reconciliation.get(&peer_device_id) {
                Some(record) if record.peer_device_id != peer_device_id => {
                    return Err(CurrentSpaceMemberScopeError::RecoveryRequired);
                }
                Some(record) => match record.relationship {
                    MembershipHistoryRelationship::Consistent => None,
                    MembershipHistoryRelationship::PendingRemovalDecision => {
                        Some(SpaceMemberPauseReason::PendingLocalDecision)
                    }
                    MembershipHistoryRelationship::Diverged => {
                        Some(SpaceMemberPauseReason::Diverged)
                    }
                    MembershipHistoryRelationship::Invalid => Some(SpaceMemberPauseReason::Invalid),
                    MembershipHistoryRelationship::UpgradeRequired => {
                        Some(SpaceMemberPauseReason::UpgradeRequired)
                    }
                    MembershipHistoryRelationship::Unknown => {
                        Some(SpaceMemberPauseReason::RelationshipUnconfirmed)
                    }
                },
                None => Some(SpaceMemberPauseReason::RelationshipUnconfirmed),
            }
        };
        if let Some(reason) = pause_reason {
            paused_peer_devices.push(PausedSpaceMember {
                device_id: peer_device_id,
                reason,
            });
        } else {
            usable_peer_device_ids.push(peer_device_id);
        }
    }
    usable_peer_device_ids.sort();
    paused_peer_devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));

    Ok(CurrentSpaceMemberScope {
        revision: loaded.revision,
        local_member_active,
        usable_peer_device_ids,
        paused_peer_devices,
    })
}

impl MembershipLedger {
    pub(crate) async fn exchange_conflict_evidence(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        evidence: &MembershipConflictEvidenceV3,
    ) -> Result<Option<Vec<MembershipHistoryPageV2>>, MembershipLedgerError> {
        let snapshot = self.load_verified().await?;
        let local = snapshot
            .history()
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let Ok(remote) = self.verify_exchange_pages(&evidence.pages) else {
            return Ok(None);
        };
        let Ok(remote_position) = remote.current_position() else {
            return Ok(None);
        };
        if evidence.transfer_id != remote_position.history_digest
            || remote
                .effective_member_for_device(source_device_id)
                .and_then(|member| remote.admission_facts_for(member))
                .is_none_or(|facts| &facts.device_id != source_device_id)
        {
            return Ok(None);
        }
        let local_member = snapshot
            .record()
            .local_member_instance
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let local_sender = local
            .admission_facts_for(local_member)
            .cloned()
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let Ok(response_pages) = local.export_conflict_evidence_pages_v2(local_sender) else {
            return Err(MembershipLedgerError::Corrupt);
        };
        let Ok(conflict) = MembershipConflictPolicy::describe(local, &remote, local_member) else {
            return Ok(None);
        };
        let local_choice = conflict
            .choice_for(conflict.local_branch_id)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let remote_choice = conflict
            .choice_for(conflict.remote_branch_id)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let evidence_already_recorded = snapshot
            .record()
            .membership_conflicts
            .get(&conflict.conflict_id)
            .is_some_and(|current| current.evidence_peer_device_ids.contains(source_device_id))
            && snapshot
                .record()
                .peer_reconciliation
                .get(source_device_id)
                .is_some_and(|peer| {
                    peer.relationship == MembershipHistoryRelationship::Diverged
                        && peer.confirmed_position.is_none()
                });
        if evidence_already_recorded {
            return Ok(Some(response_pages));
        }
        let source_device_id = source_device_id.clone();
        self.compare_and_commit(|record| {
            let peer = record
                .peer_reconciliation
                .get_mut(&source_device_id)
                .ok_or(MembershipLedgerError::RecoveryRequired)?;
            peer.relationship = MembershipHistoryRelationship::Diverged;
            peer.confirmed_position = None;
            record
                .membership_conflicts
                .entry(conflict.conflict_id)
                .and_modify(|current| {
                    current
                        .evidence_peer_device_ids
                        .insert(source_device_id.clone());
                })
                .or_insert_with(|| MembershipConflictRecord {
                    conflict_id: conflict.conflict_id,
                    local_branch_id: conflict.local_branch_id,
                    remote_branch_id: conflict.remote_branch_id,
                    local_choice,
                    remote_choice,
                    evidence_peer_device_ids: [source_device_id.clone()].into(),
                    detected_at_revision: record.revision,
                    status: MembershipConflictStatus::Unresolved,
                    selected_branch_id: None,
                    transition_id: None,
                });
            Ok(())
        })
        .await?;
        Ok(Some(response_pages))
    }

    pub(crate) fn new(
        loader: Arc<dyn LoadMembershipLedgerPort>,
        committer: Arc<dyn CommitMembershipLedgerPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        Self {
            loader,
            committer,
            verifier,
        }
    }

    pub(crate) fn verify_exchange_pages(
        &self,
        pages: &[uc_core::membership::MembershipHistoryPageV2],
    ) -> Result<VersionedMembershipHistory, uc_core::membership::MembershipHistoryV2Error> {
        VersionedMembershipHistory::import_exchange_pages_v2(pages, self.verifier.as_ref())
    }

    pub(crate) async fn initialize_current_space(
        &self,
        lineage_id: String,
        local_facts: AdmissionChangeFacts,
        local_credential: MembershipCredential,
    ) -> Result<(), MembershipLedgerError> {
        let local_device_id = local_facts.device_id.clone();
        let local_member_instance = local_facts.member_instance;
        let history = VersionedMembershipHistory::new_single_member_root(
            lineage_id.clone(),
            local_facts,
            local_credential,
        )
        .map_err(|_| MembershipLedgerError::Corrupt)?
        .encode_persisted_v2()
        .map_err(|_| MembershipLedgerError::Corrupt)?;
        self.compare_and_commit(move |record| {
            if record.lineage_id.is_some() || record.membership_history.is_some() {
                return Err(MembershipLedgerError::Conflict);
            }
            record.lineage_id = Some(lineage_id);
            record.membership_history = Some(history);
            record.local_device_id = Some(local_device_id);
            record.local_member_instance = Some(local_member_instance);
            record.local_join_active = true;
            record.peer_reconciliation.clear();
            record.inbound_transfers.clear();
            record.completed_inbound_transfers.clear();
            record.pending_effects.clear();
            record.membership_conflicts.clear();
            record.membership_branch_transitions.clear();
            record.consumed_membership_recovery_nonces.clear();
            record.membership_branch_recovery_sessions.clear();
            Ok(())
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn reset_for_space_rebuild(&self) -> Result<(), MembershipLedgerError> {
        self.compare_and_commit(|record| {
            record.lineage_id = None;
            record.membership_history = None;
            record.local_device_id = None;
            record.local_member_instance = None;
            record.local_join_active = false;
            record.peer_reconciliation.clear();
            record.inbound_transfers.clear();
            record.completed_inbound_transfers.clear();
            record.pending_effects.clear();
            record.membership_conflicts.clear();
            record.membership_branch_transitions.clear();
            record.consumed_membership_recovery_nonces.clear();
            record.membership_branch_recovery_sessions.clear();
            Ok(())
        })
        .await?;
        Ok(())
    }

    fn validate_loaded(
        &self,
        loaded: &LoadedMembershipLedger,
    ) -> Result<Option<VersionedMembershipHistory>, MembershipLedgerError> {
        if loaded
            .membership_branch_recovery_sessions
            .iter()
            .any(|(key, session)| key != session.transition_id() || !session.validate())
        {
            return Err(MembershipLedgerError::Corrupt);
        }
        let Some(lineage_id) = loaded.lineage_id.as_deref() else {
            if loaded.membership_history.is_some()
                || loaded.local_device_id.is_some()
                || loaded.local_member_instance.is_some()
                || loaded.local_join_active
                || !loaded.membership_branch_recovery_sessions.is_empty()
            {
                return Err(MembershipLedgerError::Corrupt);
            }
            return Ok(None);
        };
        let history_bytes = loaded
            .membership_history
            .as_deref()
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let history =
            VersionedMembershipHistory::decode_persisted_v2(history_bytes, self.verifier.as_ref())
                .map_err(|_| MembershipLedgerError::Corrupt)?;
        if history.lineage_id() != lineage_id {
            return Err(MembershipLedgerError::Corrupt);
        }
        Ok(Some(history))
    }

    pub(crate) async fn load_verified(
        &self,
    ) -> Result<VerifiedMembershipLedger, MembershipLedgerError> {
        let record = self.loader.load().await?;
        let history = self.validate_loaded(&record)?;
        Ok(VerifiedMembershipLedger { record, history })
    }

    pub(crate) async fn compare_and_commit(
        &self,
        update: impl FnOnce(&mut LoadedMembershipLedger) -> Result<(), MembershipLedgerError>,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let loaded = self.loader.load().await?;
        self.validate_loaded(&loaded)?;
        let expected_history_digest = loaded
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        let next_revision = loaded
            .revision
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let mut replacement = loaded.clone();
        update(&mut replacement)?;
        replacement.revision = next_revision;
        self.validate_loaded(&replacement)?;
        let committed = self
            .committer
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: loaded.revision,
                expected_history_digest,
                replacement: replacement.clone(),
            })
            .await?;
        if committed != replacement {
            return Err(MembershipLedgerError::Corrupt);
        }
        Ok(committed)
    }

    pub(crate) async fn compare_and_commit_history<T>(
        &self,
        expected_revision: u64,
        expected_history_digest: Option<[u8; 32]>,
        update: impl FnOnce(
            &mut LoadedMembershipLedger,
            &mut VersionedMembershipHistory,
            &dyn HistoricalMembershipSignatureVerifier,
        ) -> Result<T, MembershipLedgerError>,
    ) -> Result<(LoadedMembershipLedger, T), MembershipLedgerError> {
        let snapshot = self.load_verified().await?;
        if snapshot.record.revision != expected_revision
            || snapshot.history_digest() != expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let mut replacement = snapshot.record;
        let mut history = snapshot
            .history
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let output = update(&mut replacement, &mut history, self.verifier.as_ref())?;
        replacement.membership_history = Some(
            history
                .encode_persisted_v2()
                .map_err(|_| MembershipLedgerError::Corrupt)?,
        );
        replacement.revision = next_revision;
        self.validate_loaded(&replacement)?;
        let committed = self
            .committer
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision,
                expected_history_digest,
                replacement: replacement.clone(),
            })
            .await?;
        if committed != replacement {
            return Err(MembershipLedgerError::Corrupt);
        }
        Ok((committed, output))
    }

    pub(crate) async fn current_scope(
        &self,
    ) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        self.load_verified().await?.current_scope()
    }
}

#[async_trait]
impl crate::space::lifecycle::SpaceMembershipResetPort for MembershipLedger {
    async fn reset(&self) -> Result<(), crate::space::lifecycle::SpaceMembershipRebuildError> {
        self.reset_for_space_rebuild()
            .await
            .map_err(|error| match error {
                MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
                    crate::space::lifecycle::SpaceMembershipRebuildError::Inconsistent
                }
                MembershipLedgerError::Locked
                | MembershipLedgerError::Conflict
                | MembershipLedgerError::Unavailable => {
                    crate::space::lifecycle::SpaceMembershipRebuildError::Unavailable
                }
            })
    }
}

#[async_trait]
impl CurrentSpaceMemberScopePort for MembershipLedger {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        self.current_scope().await
    }
}
