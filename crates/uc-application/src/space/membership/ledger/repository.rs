use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureVerifier, MembershipCredential,
    MembershipHistoryRelationship, VersionedMembershipHistory,
};

use super::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    LoadedMembershipLedger, MembershipEffectPhase, MembershipLedgerError, MembershipLedgerMutation,
    PausedSpaceMember, SpaceMemberPauseReason,
};

/// Loads the complete decrypted application membership record.
///
/// Implementations must encrypt every field at rest with the profile or
/// Space MasterKey. The application never permits a plaintext fallback.
#[async_trait]
pub trait LoadMembershipLedgerPort: Send + Sync {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError>;

    /// 返回同一进程内已知的最新账本版本；没有廉价来源时保持未知。
    fn current_revision(&self) -> Option<u64> {
        None
    }
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
    changes: tokio::sync::watch::Sender<()>,
    history_changes: tokio::sync::watch::Sender<()>,
    verified_snapshot_cache: Mutex<Option<Arc<VerifiedMembershipLedger>>>,
    operation: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
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
        let effect_pending = loaded.current_effects(history).iter().any(|(_, effect)| {
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
    pub(crate) fn new(
        loader: Arc<dyn LoadMembershipLedgerPort>,
        committer: Arc<dyn CommitMembershipLedgerPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        Self {
            loader,
            committer,
            verifier,
            changes: tokio::sync::watch::channel(()).0,
            history_changes: tokio::sync::watch::channel(()).0,
            verified_snapshot_cache: Mutex::new(None),
            operation: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn subscribe_history_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.history_changes.subscribe()
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
            record.effect_journal.clear();
            record.membership_conflicts.clear();
            record.membership_conflict_presentations.clear();
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
            record.effect_journal.clear();
            record.membership_conflicts.clear();
            record.membership_conflict_presentations.clear();
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
        cached_snapshot: Option<&VerifiedMembershipLedger>,
    ) -> Result<Option<VersionedMembershipHistory>, MembershipLedgerError> {
        if loaded
            .membership_conflict_presentations
            .iter()
            .any(|(id, data)| {
                loaded
                    .membership_conflicts
                    .get(id)
                    .is_none_or(|record| !data.matches_record(record))
            })
        {
            return Err(MembershipLedgerError::Corrupt);
        }
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
        let history = match cached_snapshot {
            Some(cached) if cached.record.membership_history.as_deref() == Some(history_bytes) => {
                cached
                    .history
                    .clone()
                    .ok_or(MembershipLedgerError::Corrupt)?
            }
            _ => VersionedMembershipHistory::decode_persisted_v2(
                history_bytes,
                self.verifier.as_ref(),
            )
            .map_err(|_| MembershipLedgerError::Corrupt)?,
        };
        if history.lineage_id() != lineage_id {
            return Err(MembershipLedgerError::Corrupt);
        }
        Ok(Some(history))
    }

    fn cached_verified(
        &self,
    ) -> Result<Option<Arc<VerifiedMembershipLedger>>, MembershipLedgerError> {
        let mut cached = self
            .verified_snapshot_cache
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)?;
        if cached.as_ref().is_some_and(|snapshot| {
            self.loader
                .current_revision()
                .is_some_and(|revision| revision != snapshot.record.revision)
        }) {
            *cached = None;
        }
        Ok(cached.clone())
    }

    pub(super) fn clear_cached_verified(&self) -> Result<(), MembershipLedgerError> {
        *self
            .verified_snapshot_cache
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)? = None;
        Ok(())
    }

    fn cache_verified(
        &self,
        snapshot: &VerifiedMembershipLedger,
    ) -> Result<(), MembershipLedgerError> {
        *self
            .verified_snapshot_cache
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)? = Some(Arc::new(snapshot.clone()));
        Ok(())
    }

    async fn load_verified_exclusive(
        &self,
    ) -> Result<Arc<VerifiedMembershipLedger>, MembershipLedgerError> {
        self.clear_cached_verified()?;
        let record = self.loader.load().await?;
        let history = self.validate_loaded(&record, None)?;
        let snapshot = VerifiedMembershipLedger { record, history };
        self.cache_verified(&snapshot)?;
        self.cached_verified()?
            .ok_or(MembershipLedgerError::Unavailable)
    }

    pub(crate) async fn load_verified(
        &self,
    ) -> Result<Arc<VerifiedMembershipLedger>, MembershipLedgerError> {
        if let Some(cached) = self.cached_verified()? {
            return Ok(cached);
        }
        let _operation = self.operation.lock().await;
        if let Some(cached) = self.cached_verified()? {
            return Ok(cached);
        }
        self.load_verified_exclusive().await
    }

    fn device_trust_changed(
        current: &LoadedMembershipLedger,
        replacement: &LoadedMembershipLedger,
    ) -> bool {
        current.lineage_id != replacement.lineage_id
            || current.membership_history != replacement.membership_history
            || current.local_device_id != replacement.local_device_id
            || current.local_member_instance != replacement.local_member_instance
            || current.local_join_active != replacement.local_join_active
            || current.effect_journal != replacement.effect_journal
            || current.membership_conflicts != replacement.membership_conflicts
            || current.membership_branch_transitions != replacement.membership_branch_transitions
            || current.membership_conflict_presentations
                != replacement.membership_conflict_presentations
            || current
                .peer_reconciliation
                .iter()
                .any(|(device_id, record)| {
                    replacement
                        .peer_reconciliation
                        .get(device_id)
                        .is_none_or(|candidate| {
                            record.relationship != candidate.relationship
                                || record.confirmed_position != candidate.confirmed_position
                        })
                })
            || replacement
                .peer_reconciliation
                .keys()
                .any(|device_id| !current.peer_reconciliation.contains_key(device_id))
    }

    pub(crate) async fn compare_and_commit(
        &self,
        update: impl FnOnce(&mut LoadedMembershipLedger) -> Result<(), MembershipLedgerError>,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let _operation = self.operation.lock().await;
        let snapshot = match self.cached_verified()? {
            Some(snapshot) => snapshot,
            None => self.load_verified_exclusive().await?,
        };
        let loaded = snapshot.record();
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
        let replacement_history = self.validate_loaded(&replacement, Some(&snapshot))?;
        let device_trust_changed = Self::device_trust_changed(loaded, &replacement);
        let committed = match self
            .committer
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: loaded.revision,
                expected_history_digest,
                device_trust_changed,
                replacement: replacement.clone(),
            })
            .await
        {
            Ok(committed) => committed,
            Err(error) => {
                self.clear_cached_verified()?;
                return Err(error);
            }
        };
        if committed != replacement {
            self.clear_cached_verified()?;
            return Err(MembershipLedgerError::Corrupt);
        }
        self.cache_verified(&VerifiedMembershipLedger {
            record: committed.clone(),
            history: replacement_history,
        })?;
        if committed.membership_history != loaded.membership_history
            || committed.local_join_active != loaded.local_join_active
        {
            self.history_changes.send_replace(());
        }
        self.changes.send_replace(());
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
        let _operation = self.operation.lock().await;
        let snapshot = match self.cached_verified()? {
            Some(snapshot) => snapshot,
            None => self.load_verified_exclusive().await?,
        };
        if snapshot.record.revision != expected_revision
            || snapshot.history_digest() != expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or(MembershipLedgerError::Corrupt)?;
        let was_active = snapshot.record.local_join_active;
        let mut replacement = snapshot.record.clone();
        let mut history = snapshot
            .history
            .clone()
            .ok_or(MembershipLedgerError::RecoveryRequired)?;
        let output = update(&mut replacement, &mut history, self.verifier.as_ref())?;
        replacement.membership_history = Some(
            history
                .encode_persisted_v2()
                .map_err(|_| MembershipLedgerError::Corrupt)?,
        );
        replacement.revision = next_revision;
        let replacement_history = self.validate_loaded(&replacement, None)?;
        let device_trust_changed = Self::device_trust_changed(&snapshot.record, &replacement);
        let committed = match self
            .committer
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision,
                expected_history_digest,
                device_trust_changed,
                replacement: replacement.clone(),
            })
            .await
        {
            Ok(committed) => committed,
            Err(error) => {
                self.clear_cached_verified()?;
                return Err(error);
            }
        };
        if committed != replacement {
            self.clear_cached_verified()?;
            return Err(MembershipLedgerError::Corrupt);
        }
        self.cache_verified(&VerifiedMembershipLedger {
            record: committed.clone(),
            history: replacement_history,
        })?;
        let history_digest = committed
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if history_digest != expected_history_digest || committed.local_join_active != was_active {
            self.history_changes.send_replace(());
        }
        self.changes.send_replace(());
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
    fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.changes.subscribe()
    }

    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        self.current_scope().await
    }
}
