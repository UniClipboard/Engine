//! Workspace convergence state SQLite encrypted repository (ADR-016).
//!
//! The whole persisted convergence state (membership history, peer branch
//! relationships, pending admissions, phase) is sealed with the
//! MasterKey AEAD before it is written; the only plaintext columns are the
//! space lookup token and the updated timestamp. Loading verifies that the
//! state belongs to the requested space, so a stale state from another
//! space can never be saved or read back after a space switch.

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Binary, Text};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use uc_core::crypto::EncryptionError;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    AdmissionChangeFacts, MemberInstanceId, MembershipHistoryRelationship, PendingAdmissionRecord,
    PendingAppliedMembershipEffect, PendingMembershipDecisionDelivery,
    WorkspaceConvergenceRepositoryError, WorkspaceConvergenceRepositoryPort,
    WorkspaceConvergenceState, WorkspaceFailureCategory, WorkspacePhase,
};
use uc_core::ports::pairing::PairingSessionId;

use uc_core::membership::RelayedSecurityUpdate;

#[derive(Deserialize, Serialize)]
struct UnversionedCurrentWorkspaceState {
    space_lineage: String,
    own_instance: Option<MemberInstanceId>,
    peer_history_relationships:
        std::collections::BTreeMap<uc_core::DeviceId, MembershipHistoryRelationship>,
    membership_reconciliation: Option<uc_core::membership::MembershipReconciliation>,
    pending_applied_membership_effects: Vec<PendingAppliedMembershipEffect>,
    pending_membership_decision_deliveries: Vec<PendingMembershipDecisionDelivery>,
    pending_admissions: std::collections::BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    phase: WorkspacePhase,
    failure_category: Option<WorkspaceFailureCategory>,
    revision: u64,
    removed: bool,
    updated_at_ms: i64,
}

/// Exact V2 payload layout written by Engine 1.1.0-rc.3.
///
/// Postcard encodes struct fields positionally, so this persisted layout must
/// not be replaced with the evolving domain type.
#[derive(Deserialize)]
struct WorkspaceConvergenceStateV2 {
    space_lineage: String,
    own_instance: Option<MemberInstanceId>,
    peer_history_relationships:
        std::collections::BTreeMap<uc_core::DeviceId, MembershipHistoryRelationship>,
    membership_reconciliation: Option<uc_core::membership::MembershipReconciliation>,
    pending_applied_membership_effects: Vec<PendingAppliedMembershipEffect>,
    pending_membership_decision_deliveries: Vec<PendingMembershipDecisionDelivery>,
    pending_admissions: std::collections::BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    phase: WorkspacePhase,
    failure_category: Option<WorkspaceFailureCategory>,
    revision: u64,
    removed: bool,
    updated_at_ms: i64,
    #[serde(default)]
    migrated_from_pre_adr_020: bool,
}

impl From<WorkspaceConvergenceStateV2> for WorkspaceConvergenceState {
    fn from(state: WorkspaceConvergenceStateV2) -> Self {
        let migrated_from_pre_adr_020 = state.migrated_from_pre_adr_020
            || state
                .membership_reconciliation
                .as_ref()
                .is_none_or(|history| history.applied_head().is_none());
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            membership_reconciliation: state.membership_reconciliation,
            pending_applied_membership_effects: state.pending_applied_membership_effects,
            pending_membership_decision_deliveries: state.pending_membership_decision_deliveries,
            pending_membership_history_transfers: std::collections::BTreeMap::new(),
            pending_admissions: state.pending_admissions,
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
            migrated_from_pre_adr_020,
        }
    }
}

#[derive(Deserialize)]
struct DeviceTrustInitialWorkspaceState {
    space_lineage: String,
    own_instance: Option<MemberInstanceId>,
    peer_history_relationships:
        std::collections::BTreeMap<uc_core::DeviceId, MembershipHistoryRelationship>,
    membership_reconciliation: Option<uc_core::membership::MembershipReconciliation>,
    pending_admissions: std::collections::BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    phase: WorkspacePhase,
    failure_category: Option<WorkspaceFailureCategory>,
    revision: u64,
    removed: bool,
    updated_at_ms: i64,
}

impl From<UnversionedCurrentWorkspaceState> for WorkspaceConvergenceState {
    fn from(state: UnversionedCurrentWorkspaceState) -> Self {
        let migrated_from_pre_adr_020 = state
            .membership_reconciliation
            .as_ref()
            .is_none_or(|history| history.applied_head().is_none());
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            membership_reconciliation: state.membership_reconciliation,
            pending_applied_membership_effects: state.pending_applied_membership_effects,
            pending_membership_decision_deliveries: state.pending_membership_decision_deliveries,
            pending_admissions: state.pending_admissions,
            pending_membership_history_transfers: std::collections::BTreeMap::new(),
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
            migrated_from_pre_adr_020,
        }
    }
}

impl From<DeviceTrustInitialWorkspaceState> for WorkspaceConvergenceState {
    fn from(state: DeviceTrustInitialWorkspaceState) -> Self {
        let migrated_from_pre_adr_020 = state
            .membership_reconciliation
            .as_ref()
            .is_none_or(|history| history.applied_head().is_none());
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            membership_reconciliation: state.membership_reconciliation,
            pending_applied_membership_effects: Vec::new(),
            pending_membership_decision_deliveries: Vec::new(),
            pending_admissions: state.pending_admissions,
            pending_membership_history_transfers: std::collections::BTreeMap::new(),
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
            migrated_from_pre_adr_020,
        }
    }
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
enum LegacyWorkspaceChangeKind {
    Admission,
    Removal,
}

#[derive(Serialize, Deserialize)]
struct LegacyRemovalChangeFacts {
    removed_instances: Vec<MemberInstanceId>,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyWorkspaceChange {
    space_lineage: String,
    kind: LegacyWorkspaceChangeKind,
    previous_epoch: u64,
    next_epoch: u64,
    previous_digest: [u8; 32],
    digest: [u8; 32],
    security_updates: Vec<RelayedSecurityUpdate>,
    admission: Option<AdmissionChangeFacts>,
    removal: Option<LegacyRemovalChangeFacts>,
    created_at_ms: i64,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyWorkspaceConfirmation {
    member_instance: MemberInstanceId,
    digest: [u8; 32],
    signature: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyPendingHandoff {
    recipient: MemberInstanceId,
    recipient_device: uc_core::DeviceId,
    confirmed_epoch: u64,
    target_digest: [u8; 32],
    has_more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
struct LegacyRemovalIntentId([u8; 32]);

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyRemovalIntentContent {
    space_lineage: String,
    view_epoch: u64,
    view_members: Vec<MemberInstanceId>,
    initiator: MemberInstanceId,
    target: MemberInstanceId,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyRemovalCausalProofMember {
    device_id: uc_core::DeviceId,
    instance: MemberInstanceId,
    signing_public_key: Vec<u8>,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyRemovalCausalProof {
    epoch: u64,
    members: Vec<LegacyRemovalCausalProofMember>,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacySignedRemovalIntent {
    content: LegacyRemovalIntentContent,
    intent_id: LegacyRemovalIntentId,
    signature: Vec<u8>,
    causal_proof: LegacyRemovalCausalProof,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyAdmissionCommittedFacts {
    change_digest: [u8; 32],
    change_count: u64,
    sponsor_facts: AdmissionChangeFacts,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
enum LegacyWorkspacePhase {
    LocallyApplied,
    Converging,
    WaitingForOfflineMember,
    Complete,
    RecoveryRequired,
}

#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
enum LegacyWorkspaceFailureCategory {
    SpaceMismatch,
    ContinuityGap,
    IdentityMismatch,
    DigestConflict,
    Unauthorized,
    VersionIncompatible,
    NoEffectiveMembers,
    Storage,
}

/// Last persisted layout used by Engine 6a7b644 before signed membership history.
#[allow(dead_code)]
#[derive(Serialize, Deserialize)]
struct LegacyWorkspaceConvergenceState {
    space_lineage: String,
    own_instance: Option<MemberInstanceId>,
    changes: Vec<LegacyWorkspaceChange>,
    confirmed: std::collections::BTreeMap<MemberInstanceId, LegacyWorkspaceConfirmation>,
    pending_handoffs: std::collections::BTreeMap<MemberInstanceId, LegacyPendingHandoff>,
    waiting_members: std::collections::BTreeSet<MemberInstanceId>,
    removal_intents: std::collections::BTreeSet<LegacyRemovalIntentId>,
    removal_intent_records: Vec<LegacySignedRemovalIntent>,
    peer_intent_acks: std::collections::BTreeMap<(uc_core::DeviceId, LegacyRemovalIntentId), i64>,
    notified_removals: std::collections::BTreeSet<LegacyRemovalIntentId>,
    accepted_removal_notices: std::collections::BTreeSet<LegacyRemovalIntentId>,
    member_devices: std::collections::BTreeMap<MemberInstanceId, uc_core::DeviceId>,
    pending_admissions: std::collections::BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    local_admission_committed: Option<LegacyAdmissionCommittedFacts>,
    phase: LegacyWorkspacePhase,
    failure_category: Option<LegacyWorkspaceFailureCategory>,
    revision: u64,
    removed: bool,
    updated_at_ms: i64,
}

impl LegacyWorkspaceConvergenceState {
    fn into_current(self) -> WorkspaceConvergenceState {
        let mut effective_members = std::collections::BTreeSet::new();
        for change in &self.changes {
            if let Some(admission) = &change.admission {
                effective_members.insert(admission.member_instance);
            }
            if let Some(removal) = &change.removal {
                for member in &removal.removed_instances {
                    effective_members.remove(member);
                }
            }
        }
        if self.changes.is_empty() {
            effective_members.extend(self.member_devices.keys().copied());
        }

        let mut peer_history_relationships = effective_members
            .into_iter()
            .filter(|member| Some(*member) != self.own_instance)
            .filter_map(|member| self.member_devices.get(&member).cloned())
            .map(|device| (device, MembershipHistoryRelationship::UpgradeRequired))
            .collect::<std::collections::BTreeMap<_, _>>();
        if self.removed {
            peer_history_relationships.clear();
        }

        WorkspaceConvergenceState {
            space_lineage: self.space_lineage,
            own_instance: self.own_instance,
            peer_history_relationships,
            membership_reconciliation: None,
            pending_applied_membership_effects: Vec::new(),
            pending_membership_decision_deliveries: Vec::new(),
            pending_admissions: self.pending_admissions,
            pending_membership_history_transfers: std::collections::BTreeMap::new(),
            phase: WorkspacePhase::Converging,
            failure_category: None,
            revision: self.revision,
            removed: self.removed,
            updated_at_ms: self.updated_at_ms,
            migrated_from_pre_adr_020: true,
        }
    }
}

use crate::db::ports::DbExecutor;
use crate::security::crypto_model::EncryptedBlob;
use crate::security::{v1_aead, InMemorySession, MasterKey};

use super::space_security_store::space_lookup_token;

const WORKSPACE_STATE_V2_PREFIX: &[u8] = b"uc-workspace-convergence-state-v2\0";
const WORKSPACE_STATE_V3_PREFIX: &[u8] = b"uc-workspace-convergence-state-v3\0";
const WORKSPACE_STATE_V3_GUARD_PREFIX: &[u8] = b"uc-workspace-convergence-v3-guard\0";
const WORKSPACE_STATE_V3_STORAGE_VERSION: u16 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceConvergenceStateV3 {
    storage_version: u16,
    state: WorkspaceConvergenceStateV3Payload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceConvergenceStateV3Payload {
    space_lineage: String,
    own_instance: Option<MemberInstanceId>,
    peer_history_relationships:
        std::collections::BTreeMap<uc_core::DeviceId, MembershipHistoryRelationship>,
    membership_reconciliation: Option<uc_core::membership::MembershipReconciliation>,
    pending_applied_membership_effects: Vec<PendingAppliedMembershipEffect>,
    pending_membership_decision_deliveries: Vec<PendingMembershipDecisionDelivery>,
    pending_membership_history_transfers: std::collections::BTreeMap<
        uc_core::DeviceId,
        uc_core::membership::PendingMembershipHistoryTransferV2,
    >,
    pending_admissions: std::collections::BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    phase: WorkspacePhase,
    failure_category: Option<WorkspaceFailureCategory>,
    revision: u64,
    removed: bool,
    updated_at_ms: i64,
    migrated_from_pre_adr_020: bool,
}

impl From<WorkspaceConvergenceState> for WorkspaceConvergenceStateV3Payload {
    fn from(state: WorkspaceConvergenceState) -> Self {
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            membership_reconciliation: state.membership_reconciliation,
            pending_applied_membership_effects: state.pending_applied_membership_effects,
            pending_membership_decision_deliveries: state.pending_membership_decision_deliveries,
            pending_membership_history_transfers: state.pending_membership_history_transfers,
            pending_admissions: state.pending_admissions,
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
            migrated_from_pre_adr_020: state.migrated_from_pre_adr_020,
        }
    }
}

impl From<WorkspaceConvergenceStateV3Payload> for WorkspaceConvergenceState {
    fn from(state: WorkspaceConvergenceStateV3Payload) -> Self {
        let migrated_from_pre_adr_020 = state.migrated_from_pre_adr_020
            || (state.phase == WorkspacePhase::Complete
                && state.own_instance.is_some()
                && !state.peer_history_relationships.is_empty()
                && state
                    .membership_reconciliation
                    .as_ref()
                    .is_none_or(|history| history.applied_head().is_none()));
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            membership_reconciliation: state.membership_reconciliation,
            pending_applied_membership_effects: state.pending_applied_membership_effects,
            pending_membership_decision_deliveries: state.pending_membership_decision_deliveries,
            pending_membership_history_transfers: state.pending_membership_history_transfers,
            pending_admissions: state.pending_admissions,
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
            migrated_from_pre_adr_020,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum WorkspaceConvergenceMigrationV3Phase {
    Staging,
    TargetVerified,
    Activated,
    CleanupPending,
}

#[derive(Serialize, Deserialize)]
struct WorkspaceConvergenceMigrationV3 {
    storage_version: u16,
    source_encrypted_payload: Vec<u8>,
    source_ciphertext_digest: [u8; 32],
    target_slot_id: String,
    phase: WorkspaceConvergenceMigrationV3Phase,
    target_plaintext_digest: Option<[u8; 32]>,
}

fn repository_error(error: impl std::fmt::Display) -> WorkspaceConvergenceRepositoryError {
    WorkspaceConvergenceRepositoryError::Repository(error.to_string())
}

fn session_error(error: EncryptionError) -> WorkspaceConvergenceRepositoryError {
    match error {
        EncryptionError::NotInitialized | EncryptionError::Locked => {
            WorkspaceConvergenceRepositoryError::Locked
        }
        error => repository_error(error),
    }
}

fn workspace_state_aad(space_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-state-v1|{space_id}").into_bytes()
}

fn seal_prefixed_payload<T: Serialize>(
    master_key: &MasterKey,
    prefix: &[u8],
    value: &T,
    aad: &[u8],
) -> Result<Vec<u8>, WorkspaceConvergenceRepositoryError> {
    let encoded = postcard::to_stdvec(value).map_err(repository_error)?;
    let mut plaintext = Vec::with_capacity(prefix.len() + encoded.len());
    plaintext.extend_from_slice(prefix);
    plaintext.extend_from_slice(&encoded);
    let encrypted =
        v1_aead::encrypt_blob_xchacha(master_key, &plaintext, aad).map_err(repository_error)?;
    serde_json::to_vec(&encrypted).map_err(repository_error)
}

fn open_workspace_plaintext(
    master_key: &MasterKey,
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, WorkspaceConvergenceRepositoryError> {
    let encrypted: EncryptedBlob = serde_json::from_slice(ciphertext)
        .map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)?;
    v1_aead::decrypt_blob_xchacha(master_key, &encrypted.nonce, &encrypted.ciphertext, aad)
        .map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)
}

fn decode_v2_workspace_payload(
    plaintext: &[u8],
) -> Result<WorkspaceConvergenceState, WorkspaceConvergenceRepositoryError> {
    let encoded = plaintext
        .strip_prefix(WORKSPACE_STATE_V2_PREFIX)
        .unwrap_or(plaintext);
    postcard::from_bytes::<WorkspaceConvergenceStateV2>(encoded)
        .map(WorkspaceConvergenceState::from)
        .map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)
}

#[derive(QueryableByName)]
struct WorkspaceStateRow {
    #[diesel(sql_type = Text, column_name = space_lookup_token)]
    _space_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(QueryableByName)]
struct WorkspaceV3ActiveRow {
    #[diesel(sql_type = Text)]
    slot_id: String,
    #[diesel(sql_type = BigInt)]
    generation: i64,
}

#[derive(QueryableByName)]
struct WorkspaceV3SlotRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(Debug, thiserror::Error)]
enum WorkspaceTransactionError {
    #[error(transparent)]
    Diesel(#[from] diesel::result::Error),
    #[error(transparent)]
    Workspace(WorkspaceConvergenceRepositoryError),
}

fn run_workspace_transaction<T>(
    conn: &mut SqliteConnection,
    operation: impl FnOnce(&mut SqliteConnection) -> Result<T, WorkspaceConvergenceRepositoryError>,
) -> Result<T, WorkspaceConvergenceRepositoryError> {
    conn.immediate_transaction::<_, WorkspaceTransactionError, _>(|conn| {
        operation(conn).map_err(WorkspaceTransactionError::Workspace)
    })
    .map_err(|error| match error {
        WorkspaceTransactionError::Diesel(error) => repository_error(error),
        WorkspaceTransactionError::Workspace(error) => error,
    })
}

pub struct DieselWorkspaceConvergenceStore<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselWorkspaceConvergenceStore<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

impl<E: DbExecutor> DieselWorkspaceConvergenceStore<E> {
    fn master_key(&self) -> Result<MasterKey, WorkspaceConvergenceRepositoryError> {
        self.session.get_master_key().map_err(session_error)
    }

    fn current_space_id(&self) -> Result<SpaceId, WorkspaceConvergenceRepositoryError> {
        self.session.current_space_id().map_err(session_error)
    }
}

fn validate_current_space(
    requested_space: &SpaceId,
    current_space: &SpaceId,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    if requested_space != current_space {
        return Err(WorkspaceConvergenceRepositoryError::Repository(
            "workspace convergence state belongs to a different space".to_owned(),
        ));
    }
    Ok(())
}

fn workspace_v3_slot_aad(space_id: &str, slot_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-v3-slot|{space_id}|{slot_id}").into_bytes()
}

fn workspace_v3_migration_aad(space_id: &str, migration_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-v3-migration|{space_id}|{migration_id}").into_bytes()
}

fn open_v3_slot_payload(
    master_key: &MasterKey,
    space_id: &SpaceId,
    slot_id: &str,
    row: &WorkspaceV3SlotRow,
) -> Result<(WorkspaceConvergenceState, [u8; 32]), WorkspaceConvergenceRepositoryError> {
    let plaintext = open_workspace_plaintext(
        master_key,
        &row.encrypted_payload,
        &workspace_v3_slot_aad(space_id.as_ref(), slot_id),
    )?;
    let encoded = plaintext
        .strip_prefix(WORKSPACE_STATE_V3_PREFIX)
        .ok_or(WorkspaceConvergenceRepositoryError::Corrupt)?;
    let stored: WorkspaceConvergenceStateV3 =
        postcard::from_bytes(encoded).map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)?;
    if stored.storage_version != WORKSPACE_STATE_V3_STORAGE_VERSION {
        return Err(WorkspaceConvergenceRepositoryError::Corrupt);
    }
    let state = WorkspaceConvergenceState::from(stored.state);
    validate_loaded_state(&state, space_id, row.updated_at_ms)?;
    Ok((state, Sha256::digest(&plaintext).into()))
}

fn load_v3_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
    lookup_token: &str,
) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
    let active = sql_query(
        "SELECT slot_id, generation FROM workspace_convergence_v3_active \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<WorkspaceV3ActiveRow>(conn)
    .optional()
    .map_err(repository_error)?;
    let Some(active) = active else {
        return Ok(None);
    };
    if active.generation <= 0 {
        return Err(WorkspaceConvergenceRepositoryError::Corrupt);
    }
    let slot = sql_query(
        "SELECT encrypted_payload, updated_at_ms FROM workspace_convergence_v3_slots \
         WHERE space_lookup_token = ? AND slot_id = ?",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<Text, _>(&active.slot_id)
    .get_result::<WorkspaceV3SlotRow>(conn)
    .optional()
    .map_err(repository_error)?
    .ok_or(WorkspaceConvergenceRepositoryError::Corrupt)?;
    open_v3_slot_payload(master_key, space_id, &active.slot_id, &slot).map(|(state, _)| Some(state))
}

fn recover_v3_storage_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
    lookup_token: &str,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    let active = sql_query(
        "SELECT slot_id, generation FROM workspace_convergence_v3_active \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<WorkspaceV3ActiveRow>(conn)
    .optional()
    .map_err(repository_error)?;
    if let Some(active) = active {
        if active.generation <= 0 {
            return Err(WorkspaceConvergenceRepositoryError::Corrupt);
        }
        let row = sql_query(
            "SELECT encrypted_payload, updated_at_ms FROM workspace_convergence_v3_slots \
             WHERE space_lookup_token = ? AND slot_id = ?",
        )
        .bind::<Text, _>(lookup_token)
        .bind::<Text, _>(&active.slot_id)
        .get_result::<WorkspaceV3SlotRow>(conn)
        .optional()
        .map_err(repository_error)?
        .ok_or(WorkspaceConvergenceRepositoryError::Corrupt)?;
        let _ = open_v3_slot_payload(master_key, space_id, &active.slot_id, &row)?;
        sql_query(
            "DELETE FROM workspace_convergence_v3_slots \
             WHERE space_lookup_token = ? AND slot_id <> ?",
        )
        .bind::<Text, _>(lookup_token)
        .bind::<Text, _>(&active.slot_id)
        .execute(conn)
        .map_err(repository_error)?;
    } else {
        sql_query("DELETE FROM workspace_convergence_v3_slots WHERE space_lookup_token = ?")
            .bind::<Text, _>(lookup_token)
            .execute(conn)
            .map_err(repository_error)?;
    }
    sql_query("DELETE FROM workspace_convergence_v3_migrations WHERE space_lookup_token = ?")
        .bind::<Text, _>(lookup_token)
        .execute(conn)
        .map_err(repository_error)?;
    Ok(())
}

fn write_v3_migration_phase(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
    lookup_token: &str,
    migration_id: &str,
    migration: &WorkspaceConvergenceMigrationV3,
    updated_at_ms: i64,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    let encrypted = seal_prefixed_payload(
        master_key,
        WORKSPACE_STATE_V3_PREFIX,
        migration,
        &workspace_v3_migration_aad(space_id.as_ref(), migration_id),
    )?;
    sql_query(
        "INSERT INTO workspace_convergence_v3_migrations \
         (space_lookup_token, migration_id, encrypted_payload, updated_at_ms) \
         VALUES (?, ?, ?, ?) ON CONFLICT(space_lookup_token, migration_id) DO UPDATE SET \
         encrypted_payload = excluded.encrypted_payload, updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<Text, _>(migration_id)
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;
    Ok(())
}

fn save_v3_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    state: &WorkspaceConvergenceState,
    source_encrypted_payload: Option<Vec<u8>>,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    let space_id = SpaceId::from_str(state.space_lineage.as_str());
    let lookup_token = space_lookup_token(master_key, &space_id).map_err(repository_error)?;
    let slot_id = Uuid::new_v4().to_string();
    let migration_id = source_encrypted_payload
        .as_ref()
        .map(|_| Uuid::new_v4().to_string());
    let mut migration = source_encrypted_payload.map(|source| WorkspaceConvergenceMigrationV3 {
        storage_version: WORKSPACE_STATE_V3_STORAGE_VERSION,
        source_ciphertext_digest: Sha256::digest(&source).into(),
        source_encrypted_payload: source,
        target_slot_id: slot_id.clone(),
        phase: WorkspaceConvergenceMigrationV3Phase::Staging,
        target_plaintext_digest: None,
    });
    if let (Some(migration_id), Some(migration)) = (&migration_id, &migration) {
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
    }

    let stored = WorkspaceConvergenceStateV3 {
        storage_version: WORKSPACE_STATE_V3_STORAGE_VERSION,
        state: state.clone().into(),
    };
    let encrypted_slot = seal_prefixed_payload(
        master_key,
        WORKSPACE_STATE_V3_PREFIX,
        &stored,
        &workspace_v3_slot_aad(space_id.as_ref(), &slot_id),
    )?;
    sql_query(
        "INSERT INTO workspace_convergence_v3_slots \
         (space_lookup_token, slot_id, encrypted_payload, updated_at_ms) VALUES (?, ?, ?, ?)",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Text, _>(&slot_id)
    .bind::<Binary, _>(&encrypted_slot)
    .bind::<BigInt, _>(state.updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;

    let inserted = sql_query(
        "SELECT encrypted_payload, updated_at_ms FROM workspace_convergence_v3_slots \
         WHERE space_lookup_token = ? AND slot_id = ?",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Text, _>(&slot_id)
    .get_result::<WorkspaceV3SlotRow>(conn)
    .map_err(repository_error)?;
    let (reopened, verified_digest) =
        open_v3_slot_payload(master_key, &space_id, &slot_id, &inserted)?;
    if reopened != *state {
        return Err(WorkspaceConvergenceRepositoryError::Corrupt);
    }
    if let (Some(migration_id), Some(migration)) = (&migration_id, &mut migration) {
        migration.phase = WorkspaceConvergenceMigrationV3Phase::TargetVerified;
        migration.target_plaintext_digest = Some(verified_digest);
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
    }

    let next_generation = sql_query(
        "SELECT slot_id, generation FROM workspace_convergence_v3_active \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(&lookup_token)
    .get_result::<WorkspaceV3ActiveRow>(conn)
    .optional()
    .map_err(repository_error)?
    .map_or(Ok(1_i64), |active| {
        active
            .generation
            .checked_add(1)
            .ok_or(WorkspaceConvergenceRepositoryError::Corrupt)
    })?;
    sql_query(
        "INSERT INTO workspace_convergence_v3_active (space_lookup_token, slot_id, generation) \
         VALUES (?, ?, ?) ON CONFLICT(space_lookup_token) DO UPDATE SET \
         slot_id = excluded.slot_id, generation = excluded.generation",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Text, _>(&slot_id)
    .bind::<BigInt, _>(next_generation)
    .execute(conn)
    .map_err(repository_error)?;

    let guard = seal_prefixed_payload(
        master_key,
        WORKSPACE_STATE_V3_GUARD_PREFIX,
        &WORKSPACE_STATE_V3_STORAGE_VERSION,
        &workspace_state_aad(space_id.as_ref()),
    )?;
    sql_query(
        "INSERT INTO workspace_convergence_state \
         (space_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?) \
         ON CONFLICT(space_lookup_token) DO UPDATE SET encrypted_payload = excluded.encrypted_payload, \
         updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Binary, _>(guard)
    .bind::<BigInt, _>(state.updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;

    if let (Some(migration_id), Some(migration)) = (&migration_id, &mut migration) {
        migration.phase = WorkspaceConvergenceMigrationV3Phase::Activated;
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
        migration.phase = WorkspaceConvergenceMigrationV3Phase::CleanupPending;
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
    }
    Ok(())
}

fn load_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
    let lookup_token = space_lookup_token(master_key, space_id).map_err(repository_error)?;
    run_workspace_transaction(conn, |conn| {
        recover_v3_storage_on(conn, master_key, space_id, &lookup_token)
    })?;
    if let Some(state) = load_v3_state_on(conn, master_key, space_id, &lookup_token)? {
        return Ok(Some(state));
    }
    let row = sql_query(
        "SELECT space_lookup_token, encrypted_payload, updated_at_ms \
         FROM workspace_convergence_state WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<WorkspaceStateRow>(conn)
    .optional()
    .map_err(repository_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let plaintext = open_workspace_plaintext(
        master_key,
        &row.encrypted_payload,
        &workspace_state_aad(space_id.as_ref()),
    )?;
    if plaintext.starts_with(WORKSPACE_STATE_V2_PREFIX) {
        let state = decode_v2_workspace_payload(&plaintext)?;
        validate_loaded_state(&state, space_id, row.updated_at_ms)?;
        let source = row.encrypted_payload;
        run_workspace_transaction(conn, |conn| {
            save_v3_state_on(conn, master_key, &state, Some(source))
        })?;
        return Ok(Some(state));
    }

    for state in [
        postcard::from_bytes::<UnversionedCurrentWorkspaceState>(&plaintext)
            .ok()
            .map(WorkspaceConvergenceState::from),
        postcard::from_bytes::<DeviceTrustInitialWorkspaceState>(&plaintext)
            .ok()
            .map(WorkspaceConvergenceState::from),
    ] {
        if let Some(state) = state {
            if validate_loaded_state(&state, space_id, row.updated_at_ms).is_ok() {
                let source = row.encrypted_payload.clone();
                run_workspace_transaction(conn, |conn| {
                    save_v3_state_on(conn, master_key, &state, Some(source))
                })?;
                return Ok(Some(state));
            }
        }
    }

    let legacy: LegacyWorkspaceConvergenceState = postcard::from_bytes(&plaintext)
        .map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)?;
    if legacy.space_lineage != space_id.as_ref() || legacy.updated_at_ms != row.updated_at_ms {
        return Err(WorkspaceConvergenceRepositoryError::Corrupt);
    }
    let state = legacy.into_current();
    validate_loaded_state(&state, space_id, row.updated_at_ms)?;
    run_workspace_transaction(conn, |conn| {
        save_v3_state_on(conn, master_key, &state, Some(row.encrypted_payload))
    })?;
    Ok(Some(state))
}

fn validate_loaded_state(
    state: &WorkspaceConvergenceState,
    space_id: &SpaceId,
    updated_at_ms: i64,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    if state.space_lineage != space_id.as_ref() || state.updated_at_ms != updated_at_ms {
        return Err(WorkspaceConvergenceRepositoryError::Corrupt);
    }
    Ok(())
}

fn save_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    state: &WorkspaceConvergenceState,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    save_v3_state_on(conn, master_key, state, None)
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> WorkspaceConvergenceRepositoryPort
    for DieselWorkspaceConvergenceStore<E>
{
    async fn save_state(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        validate_current_space(&SpaceId::from_str(state.space_lineage.as_str()), &space_id)?;
        let state = state.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    save_state_on(conn, &master_key, &state)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    Ok(())
                })
                .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(repository_error)
    }

    async fn load_state(
        &self,
    ) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        self.executor
            .run(move |conn| Ok(load_state_on(conn, &master_key, &space_id)))
            .map_err(repository_error)?
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use diesel::prelude::*;
    use diesel::sql_types::Binary;
    use diesel::QueryableByName;
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        AdmissionChangeFacts, MemberInstanceId, MembershipActivationBaselineV2,
        MembershipCredential, MembershipEvent, MembershipHistoryRelationship, MembershipOperation,
        MembershipReconciliation, PendingAdmissionRecord, PendingAppliedMembershipEffect,
        PendingMembershipDecisionDelivery, PendingMembershipHistoryTransferV2,
        VersionedMembershipHistory, WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState,
        WorkspacePhase, ED25519_SIGNATURE_ALGORITHM_V1,
    };

    use super::DieselWorkspaceConvergenceStore;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::{init_db_pool, DbPool};
    use crate::security::{InMemorySession, MasterKey};

    const SPACE: &str = "workspace-space";
    const SENSITIVE_MARKER: &[u8] = b"workspace-state-sensitive-marker";

    #[derive(QueryableByName)]
    struct EncryptedPayloadRow {
        #[diesel(sql_type = Binary)]
        encrypted_payload: Vec<u8>,
    }

    #[derive(QueryableByName)]
    struct CountRow {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        count: i64,
    }

    #[derive(serde::Serialize)]
    struct DeviceTrustInitialWorkspaceState {
        space_lineage: String,
        own_instance: Option<MemberInstanceId>,
        peer_history_relationships: BTreeMap<DeviceId, MembershipHistoryRelationship>,
        membership_reconciliation: Option<MembershipReconciliation>,
        pending_admissions:
            BTreeMap<uc_core::ports::pairing::PairingSessionId, PendingAdmissionRecord>,
        phase: WorkspacePhase,
        failure_category: Option<uc_core::membership::WorkspaceFailureCategory>,
        revision: u64,
        removed: bool,
        updated_at_ms: i64,
    }

    #[derive(serde::Serialize)]
    struct Rc3WorkspaceConvergenceState {
        space_lineage: String,
        own_instance: Option<MemberInstanceId>,
        peer_history_relationships: BTreeMap<DeviceId, MembershipHistoryRelationship>,
        membership_reconciliation: Option<MembershipReconciliation>,
        pending_applied_membership_effects: Vec<PendingAppliedMembershipEffect>,
        pending_membership_decision_deliveries: Vec<PendingMembershipDecisionDelivery>,
        pending_admissions:
            BTreeMap<uc_core::ports::pairing::PairingSessionId, PendingAdmissionRecord>,
        phase: WorkspacePhase,
        failure_category: Option<uc_core::membership::WorkspaceFailureCategory>,
        revision: u64,
        removed: bool,
        updated_at_ms: i64,
        migrated_from_pre_adr_020: bool,
    }

    #[derive(serde::Serialize)]
    struct Rc4WorkspaceConvergenceStateV3 {
        storage_version: u16,
        state: WorkspaceConvergenceState,
    }

    impl From<WorkspaceConvergenceState> for Rc3WorkspaceConvergenceState {
        fn from(state: WorkspaceConvergenceState) -> Self {
            Self {
                space_lineage: state.space_lineage,
                own_instance: state.own_instance,
                peer_history_relationships: state.peer_history_relationships,
                membership_reconciliation: state.membership_reconciliation,
                pending_applied_membership_effects: state.pending_applied_membership_effects,
                pending_membership_decision_deliveries: state
                    .pending_membership_decision_deliveries,
                pending_admissions: state.pending_admissions,
                phase: state.phase,
                failure_category: state.failure_category,
                revision: state.revision,
                removed: state.removed,
                updated_at_ms: state.updated_at_ms,
                migrated_from_pre_adr_020: state.migrated_from_pre_adr_020,
            }
        }
    }

    fn insert_unversioned_payload<T: serde::Serialize>(
        pool: &DbPool,
        value: &T,
        updated_at_ms: i64,
    ) {
        let plaintext = postcard::to_stdvec(value).unwrap();
        insert_encrypted_plaintext(pool, &plaintext, updated_at_ms);
    }

    fn insert_encrypted_plaintext(pool: &DbPool, plaintext: &[u8], updated_at_ms: i64) {
        let master_key = MasterKey::from_bytes(&[0x57; 32]).unwrap();
        let encrypted_blob = crate::security::v1_aead::encrypt_blob_xchacha(
            &master_key,
            plaintext,
            &super::workspace_state_aad(SPACE),
        )
        .unwrap();
        let encrypted = serde_json::to_vec(&encrypted_blob).unwrap();
        let lookup_token =
            super::space_lookup_token(&master_key, &SpaceId::from_str(SPACE)).unwrap();
        let mut connection = pool.get().unwrap();
        diesel::sql_query(
            "INSERT INTO workspace_convergence_state \
             (space_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?)",
        )
        .bind::<diesel::sql_types::Text, _>(lookup_token)
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .bind::<diesel::sql_types::BigInt, _>(updated_at_ms)
        .execute(&mut connection)
        .unwrap();
    }

    fn rc3_v2_plaintext(state: &WorkspaceConvergenceState) -> Vec<u8> {
        let rc3_state = Rc3WorkspaceConvergenceState::from(state.clone());
        let mut plaintext = super::WORKSPACE_STATE_V2_PREFIX.to_vec();
        plaintext.extend(postcard::to_stdvec(&rc3_state).unwrap());
        plaintext
    }

    fn expected_rc3_state(mut state: WorkspaceConvergenceState) -> WorkspaceConvergenceState {
        state.pending_membership_history_transfers.clear();
        state.migrated_from_pre_adr_020 = state.migrated_from_pre_adr_020
            || state
                .membership_reconciliation
                .as_ref()
                .is_none_or(|history| history.applied_head().is_none());
        state
    }

    fn session() -> InMemorySession {
        let session = InMemorySession::new();
        session.set_master_key_for_space(
            SpaceId::from_str(SPACE),
            MasterKey::from_bytes(&[0x57; 32]).unwrap(),
        );
        session
    }

    fn reopen_store(pool: DbPool) -> DieselWorkspaceConvergenceStore<DieselSqliteExecutor> {
        DieselWorkspaceConvergenceStore::new(DieselSqliteExecutor::new(pool), session())
    }

    fn persisted_state() -> WorkspaceConvergenceState {
        let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 123);
        state.peer_history_relationships = BTreeMap::from([(
            DeviceId::new("workspace-state-sensitive-marker"),
            MembershipHistoryRelationship::PendingRemovalDecision,
        )]);
        state.membership_reconciliation = Some(MembershipReconciliation::new(
            SPACE.to_owned(),
            MemberInstanceId::from_bytes([0x24; 32]),
        ));
        state.phase = WorkspacePhase::Converging;
        state.updated_at_ms = 123;
        let source_device = DeviceId::new("history-page-source");
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x61; 32]);
        let source_member = credential.member_instance_id(&source_device);
        let source_facts = AdmissionChangeFacts {
            member_instance: source_member,
            device_id: source_device.clone(),
            device_name: String::from_utf8(SENSITIVE_MARKER.to_vec()).unwrap(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![0x62; 32],
            transport_address_blob: vec![0x63; 16],
            identity_signature: vec![0x64; 64],
        };
        let history = VersionedMembershipHistory::from_activation_baseline(
            MembershipActivationBaselineV2::FullyVerifiedMigration {
                lineage_id: SPACE.to_owned(),
                head_event_id: uc_core::membership::MembershipEventId::from_hex(&"65".repeat(32))
                    .unwrap(),
                head_depth: 7,
                current_members: vec![(source_facts.clone(), credential)],
            },
        )
        .unwrap();
        let page = history
            .export_reconciliation_pages_v2(source_facts)
            .unwrap()
            .remove(0);
        state.pending_membership_history_transfers.insert(
            source_device,
            PendingMembershipHistoryTransferV2 {
                transfer_id: page.transfer_id(),
                page_count: page.page_count(),
                pages: vec![page],
            },
        );
        state
    }

    fn make_store() -> (
        DieselWorkspaceConvergenceStore<DieselSqliteExecutor>,
        DbPool,
        TempDir,
    ) {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("workspace.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        (reopen_store(pool.clone()), pool, directory)
    }

    #[tokio::test]
    async fn stale_space_state_cannot_be_saved_after_switching_spaces() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("workspace.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let session = session();
        let store = DieselWorkspaceConvergenceStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            session.clone(),
        );
        let state = persisted_state();

        session.set_master_key_for_space(
            SpaceId::from_str("another-space"),
            MasterKey::from_bytes(&[0x57; 32]).unwrap(),
        );

        assert!(store.save_state(&state).await.is_err());
        assert_eq!(store.load_state().await.unwrap(), None);
    }

    #[tokio::test]
    async fn state_survives_a_new_session_without_plaintext_on_disk() {
        let (store, pool, directory) = make_store();
        let state = persisted_state();

        store.save_state(&state).await.unwrap();

        let reopened = reopen_store(pool.clone());
        assert_eq!(reopened.load_state().await.unwrap(), Some(state.clone()));

        let mut connection = pool.get().unwrap();
        let rows = diesel::sql_query("SELECT encrypted_payload FROM workspace_convergence_state")
            .load::<EncryptedPayloadRow>(&mut connection)
            .unwrap();
        assert!(rows.iter().all(|row| {
            !row.encrypted_payload
                .windows(SENSITIVE_MARKER.len())
                .any(|window| window == SENSITIVE_MARKER)
        }));

        let markers: [&[u8]; 2] = [SENSITIVE_MARKER, b"workspace-space"];
        let files = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        assert!(files.iter().any(|path| path.ends_with("workspace.sqlite")));
        for path in files {
            let bytes = fs::read(&path).unwrap();
            for marker in markers {
                assert!(
                    !bytes.windows(marker.len()).any(|window| window == marker),
                    "sensitive marker persisted in database auxiliary file"
                );
            }
        }
    }

    #[tokio::test]
    async fn membership_history_state_and_revision_survive_a_round_trip() {
        let (store, pool, _directory) = make_store();
        let mut state = persisted_state();
        state.revision = 7;
        store.save_state(&state).await.unwrap();
        let reopened = reopen_store(pool.clone());
        let loaded = reopened.load_state().await.unwrap().unwrap();
        assert_eq!(
            loaded.membership_reconciliation,
            state.membership_reconciliation
        );
        assert_eq!(
            loaded.peer_history_relationships,
            state.peer_history_relationships
        );
        assert_eq!(loaded.revision, 7);
    }

    #[tokio::test]
    async fn affected_migrated_state_preserves_removed_history_across_an_encrypted_restart() {
        let (store, pool, _directory) = make_store();
        let local = MemberInstanceId::from_bytes([0x0a; 32]);
        let removed = MemberInstanceId::from_bytes([0x0b; 32]);
        let admission = |member_instance, device_id: &str, byte: u8| AdmissionChangeFacts {
            member_instance,
            device_id: DeviceId::new(device_id),
            device_name: device_id.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![byte; 32],
            transport_address_blob: vec![byte; 16],
            identity_signature: vec![byte; 64],
        };
        let genesis = MembershipEvent::new(
            SPACE.to_owned(),
            None,
            0,
            [1; 16],
            local,
            MembershipOperation::AddDevice {
                admission: admission(local, "device-a", 1),
            },
            [1; 32],
            [2; 32],
            Vec::new(),
            None,
            vec![1],
        );
        let addition = MembershipEvent::new(
            SPACE.to_owned(),
            Some(genesis.event_id()),
            1,
            [2; 16],
            local,
            MembershipOperation::AddDevice {
                admission: admission(removed, "removed-device-sensitive-marker", 2),
            },
            [2; 32],
            [3; 32],
            Vec::new(),
            None,
            vec![2],
        );
        let removal = MembershipEvent::new(
            SPACE.to_owned(),
            Some(addition.event_id()),
            2,
            [3; 16],
            local,
            MembershipOperation::RemoveDevice { member: removed },
            [3; 32],
            [4; 32],
            Vec::new(),
            None,
            vec![3],
        );
        let mut history = MembershipReconciliation::new(SPACE.to_owned(), local);
        for event in [genesis, addition, removal] {
            history.receive_verified(event).unwrap();
        }
        let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 123);
        state.own_instance = Some(local);
        state.membership_reconciliation = Some(history);
        state.migrated_from_pre_adr_020 = true;

        store.save_state(&state).await.unwrap();

        let loaded = reopen_store(pool.clone())
            .load_state()
            .await
            .unwrap()
            .unwrap();
        let loaded_history = loaded.membership_reconciliation.as_ref().unwrap();
        assert!(loaded.migrated_from_pre_adr_020);
        assert!(loaded_history.applied_head().is_some());
        assert!(
            !loaded_history.is_device_effective(&DeviceId::new("removed-device-sensitive-marker"))
        );

        let mut connection = pool.get().unwrap();
        let rows = diesel::sql_query("SELECT encrypted_payload FROM workspace_convergence_state")
            .load::<EncryptedPayloadRow>(&mut connection)
            .unwrap();
        let marker = b"removed-device-sensitive-marker";
        assert!(rows.iter().all(|row| {
            !row.encrypted_payload
                .windows(marker.len())
                .any(|window| window == marker)
        }));
    }

    #[tokio::test]
    async fn pre_adr_020_encrypted_state_is_read_without_discarding_the_workspace() {
        let (store, pool, _directory) = make_store();
        let local = MemberInstanceId::from_bytes([0x0a; 32]);
        let peer = MemberInstanceId::from_bytes([0x0b; 32]);
        let admission = |member_instance, device_id: &str, byte: u8| AdmissionChangeFacts {
            member_instance,
            device_id: DeviceId::new(device_id),
            device_name: device_id.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![byte; 32],
            transport_address_blob: vec![byte; 16],
            identity_signature: vec![byte; 64],
        };
        let change = |member_instance, device_id: &str, previous_epoch: u64, byte: u8| {
            super::LegacyWorkspaceChange {
                space_lineage: SPACE.to_owned(),
                kind: super::LegacyWorkspaceChangeKind::Admission,
                previous_epoch,
                next_epoch: previous_epoch + 1,
                previous_digest: [byte.saturating_sub(1); 32],
                digest: [byte; 32],
                security_updates: Vec::new(),
                admission: Some(admission(member_instance, device_id, byte)),
                removal: None,
                created_at_ms: i64::from(byte),
            }
        };
        let legacy = super::LegacyWorkspaceConvergenceState {
            space_lineage: SPACE.to_owned(),
            own_instance: Some(local),
            changes: vec![
                change(local, "device-a", 0, 1),
                change(peer, "device-b", 1, 2),
            ],
            confirmed: BTreeMap::new(),
            pending_handoffs: BTreeMap::new(),
            waiting_members: BTreeSet::new(),
            removal_intents: BTreeSet::new(),
            removal_intent_records: Vec::new(),
            peer_intent_acks: BTreeMap::new(),
            notified_removals: BTreeSet::new(),
            accepted_removal_notices: BTreeSet::new(),
            member_devices: BTreeMap::from([
                (local, DeviceId::new("device-a")),
                (peer, DeviceId::new("device-b")),
            ]),
            pending_admissions: BTreeMap::new(),
            local_admission_committed: None,
            phase: super::LegacyWorkspacePhase::Complete,
            failure_category: None,
            revision: 7,
            removed: false,
            updated_at_ms: 123,
        };
        insert_unversioned_payload(&pool, &legacy, legacy.updated_at_ms);

        let loaded = store.load_state().await.unwrap().unwrap();

        assert_eq!(loaded.space_lineage, SPACE);
        assert_eq!(loaded.own_instance, Some(local));
        assert_eq!(loaded.revision, 7);
        assert!(!loaded.removed);
        assert_eq!(
            loaded.peer_history_relationships,
            BTreeMap::from([(
                DeviceId::new("device-b"),
                MembershipHistoryRelationship::UpgradeRequired,
            )])
        );

        let reopened = reopen_store(pool);
        assert_eq!(reopened.load_state().await.unwrap(), Some(loaded));
    }

    #[tokio::test]
    async fn initial_device_trust_state_keeps_pre_adr_020_migration_provenance() {
        let (store, pool, _directory) = make_store();
        let local = MemberInstanceId::from_bytes([0x0a; 32]);
        let expected_relationships = BTreeMap::from([(
            DeviceId::new("device-b"),
            MembershipHistoryRelationship::Consistent,
        )]);
        let unversioned = DeviceTrustInitialWorkspaceState {
            space_lineage: SPACE.to_owned(),
            own_instance: Some(local),
            peer_history_relationships: expected_relationships.clone(),
            membership_reconciliation: None,
            pending_admissions: BTreeMap::new(),
            phase: WorkspacePhase::Complete,
            failure_category: None,
            revision: 9,
            removed: false,
            updated_at_ms: 456,
        };
        insert_unversioned_payload(&pool, &unversioned, unversioned.updated_at_ms);

        let loaded = store.load_state().await.unwrap().unwrap();

        assert_eq!(loaded.space_lineage, SPACE);
        assert_eq!(loaded.own_instance, Some(local));
        assert_eq!(loaded.peer_history_relationships, expected_relationships);
        assert_eq!(loaded.revision, 9);
        assert!(loaded.migrated_from_pre_adr_020);
        let reopened = reopen_store(pool);
        assert_eq!(reopened.load_state().await.unwrap(), Some(loaded));
    }

    #[tokio::test]
    async fn unversioned_state_without_current_history_keeps_legacy_migration_provenance() {
        let (store, pool, _directory) = make_store();
        let local = MemberInstanceId::from_bytes([0x0a; 32]);
        let unversioned = super::UnversionedCurrentWorkspaceState {
            space_lineage: SPACE.to_owned(),
            own_instance: Some(local),
            peer_history_relationships: BTreeMap::from([(
                DeviceId::new("device-b"),
                MembershipHistoryRelationship::UpgradeRequired,
            )]),
            membership_reconciliation: None,
            pending_applied_membership_effects: Vec::new(),
            pending_membership_decision_deliveries: Vec::new(),
            pending_admissions: BTreeMap::new(),
            phase: WorkspacePhase::Converging,
            failure_category: None,
            revision: 9,
            removed: false,
            updated_at_ms: 456,
        };
        insert_unversioned_payload(&pool, &unversioned, unversioned.updated_at_ms);

        let loaded = store.load_state().await.unwrap().unwrap();

        assert!(loaded.membership_reconciliation.is_none());
        assert!(loaded.migrated_from_pre_adr_020);
        let reopened = reopen_store(pool);
        assert_eq!(reopened.load_state().await.unwrap(), Some(loaded));
    }

    #[tokio::test]
    async fn unrecognized_encrypted_state_is_reported_as_corrupt() {
        let (store, pool, _directory) = make_store();
        insert_encrypted_plaintext(&pool, b"unsupported-workspace-state", 789);

        assert_eq!(
            store.load_state().await,
            Err(uc_core::membership::WorkspaceConvergenceRepositoryError::Corrupt)
        );
    }

    #[tokio::test]
    async fn versioned_state_with_mismatched_row_identity_is_reported_as_corrupt() {
        let (store, pool, _directory) = make_store();
        let mut state = persisted_state();
        state.space_lineage = "another-space".to_owned();
        let plaintext = rc3_v2_plaintext(&state);
        insert_encrypted_plaintext(&pool, &plaintext, state.updated_at_ms);

        assert_eq!(
            store.load_state().await,
            Err(uc_core::membership::WorkspaceConvergenceRepositoryError::Corrupt)
        );
    }

    #[tokio::test]
    async fn v2_state_is_side_written_verified_and_guarded_before_v3_activation() {
        let (store, pool, _directory) = make_store();
        let state = expected_rc3_state(persisted_state());
        let plaintext = rc3_v2_plaintext(&state);
        insert_encrypted_plaintext(&pool, &plaintext, state.updated_at_ms);

        assert_eq!(store.load_state().await.unwrap(), Some(state.clone()));

        let mut connection = pool.get().unwrap();
        for table in [
            "workspace_convergence_v3_slots",
            "workspace_convergence_v3_active",
            "workspace_convergence_v3_migrations",
        ] {
            let count = diesel::sql_query(format!("SELECT COUNT(*) AS count FROM {table}"))
                .get_result::<CountRow>(&mut connection)
                .unwrap()
                .count;
            assert_eq!(count, 1, "{table} must have one durable row");
        }

        let guarded =
            diesel::sql_query("SELECT encrypted_payload FROM workspace_convergence_state LIMIT 1")
                .get_result::<EncryptedPayloadRow>(&mut connection)
                .unwrap();
        let master_key = MasterKey::from_bytes(&[0x57; 32]).unwrap();
        let guard_plaintext = super::open_workspace_plaintext(
            &master_key,
            &guarded.encrypted_payload,
            &super::workspace_state_aad(SPACE),
        )
        .unwrap();
        assert!(guard_plaintext.starts_with(super::WORKSPACE_STATE_V3_GUARD_PREFIX));

        let reopened = reopen_store(pool);
        assert_eq!(reopened.load_state().await.unwrap(), Some(state));
    }

    #[tokio::test]
    async fn rc3_v2_state_is_recovered_and_migrated_to_v3() {
        let (store, pool, _directory) = make_store();
        let expected = expected_rc3_state(persisted_state());
        let plaintext = rc3_v2_plaintext(&expected);
        insert_encrypted_plaintext(&pool, &plaintext, expected.updated_at_ms);

        let encoded = plaintext
            .strip_prefix(super::WORKSPACE_STATE_V2_PREFIX)
            .unwrap();
        assert!(postcard::from_bytes::<WorkspaceConvergenceState>(encoded).is_err());

        assert_eq!(store.load_state().await.unwrap(), Some(expected.clone()));

        let reopened = reopen_store(pool);
        assert_eq!(reopened.load_state().await.unwrap(), Some(expected));
    }

    #[tokio::test]
    async fn rc3_v2_state_without_current_history_recovers_legacy_migration_provenance() {
        let (store, pool, _directory) = make_store();
        let mut expected = expected_rc3_state(persisted_state());
        expected.membership_reconciliation = None;
        expected.migrated_from_pre_adr_020 = false;
        let plaintext = rc3_v2_plaintext(&expected);
        insert_encrypted_plaintext(&pool, &plaintext, expected.updated_at_ms);

        let loaded = store.load_state().await.unwrap().unwrap();

        assert!(loaded.membership_reconciliation.is_none());
        assert!(loaded.migrated_from_pre_adr_020);
        let reopened = reopen_store(pool);
        assert_eq!(reopened.load_state().await.unwrap(), Some(loaded));
    }

    #[test]
    fn rc4_v3_payload_remains_readable_by_the_fixed_v3_layout() {
        let expected = persisted_state();
        let rc4 = Rc4WorkspaceConvergenceStateV3 {
            storage_version: super::WORKSPACE_STATE_V3_STORAGE_VERSION,
            state: expected.clone(),
        };

        let encoded = postcard::to_stdvec(&rc4).unwrap();
        let stored: super::WorkspaceConvergenceStateV3 = postcard::from_bytes(&encoded).unwrap();

        assert_eq!(stored.storage_version, rc4.storage_version);
        assert_eq!(WorkspaceConvergenceState::from(stored.state), expected);
    }

    #[test]
    fn v3_state_previously_migrated_without_provenance_recovers_legacy_origin() {
        let mut affected = persisted_state();
        affected.own_instance = Some(MemberInstanceId::from_bytes([0x24; 32]));
        affected.phase = WorkspacePhase::Complete;
        affected.membership_reconciliation = None;
        affected.migrated_from_pre_adr_020 = false;
        let stored = super::WorkspaceConvergenceStateV3Payload::from(affected);

        let recovered = WorkspaceConvergenceState::from(stored);

        assert!(recovered.migrated_from_pre_adr_020);
    }

    #[test]
    fn v3_state_without_complete_legacy_evidence_remains_fail_closed() {
        let mut incomplete = persisted_state();
        incomplete.own_instance = Some(MemberInstanceId::from_bytes([0x24; 32]));
        incomplete.membership_reconciliation = None;
        incomplete.migrated_from_pre_adr_020 = false;

        let mut missing_identity = incomplete.clone();
        missing_identity.phase = WorkspacePhase::Complete;
        missing_identity.own_instance = None;

        let mut missing_peer_relationship = incomplete.clone();
        missing_peer_relationship.phase = WorkspacePhase::Complete;
        missing_peer_relationship.peer_history_relationships.clear();

        for state in [incomplete, missing_identity, missing_peer_relationship] {
            let recovered = WorkspaceConvergenceState::from(
                super::WorkspaceConvergenceStateV3Payload::from(state),
            );
            assert!(!recovered.migrated_from_pre_adr_020);
        }
    }

    #[tokio::test]
    async fn failed_v3_reopen_keeps_original_v2_ciphertext_and_no_active_pointer() {
        let (store, pool, _directory) = make_store();
        let state = expected_rc3_state(persisted_state());
        let plaintext = rc3_v2_plaintext(&state);
        insert_encrypted_plaintext(&pool, &plaintext, state.updated_at_ms);
        let original = {
            let mut connection = pool.get().unwrap();
            let row = diesel::sql_query(
                "SELECT encrypted_payload FROM workspace_convergence_state LIMIT 1",
            )
            .get_result::<EncryptedPayloadRow>(&mut connection)
            .unwrap();
            diesel::sql_query(
                "CREATE TRIGGER corrupt_workspace_v3_slot AFTER INSERT ON \
                 workspace_convergence_v3_slots BEGIN UPDATE workspace_convergence_v3_slots \
                 SET encrypted_payload = X'00' WHERE slot_id = NEW.slot_id; END",
            )
            .execute(&mut connection)
            .unwrap();
            row.encrypted_payload
        };

        assert_eq!(
            store.load_state().await,
            Err(uc_core::membership::WorkspaceConvergenceRepositoryError::Corrupt)
        );

        let mut connection = pool.get().unwrap();
        let after =
            diesel::sql_query("SELECT encrypted_payload FROM workspace_convergence_state LIMIT 1")
                .get_result::<EncryptedPayloadRow>(&mut connection)
                .unwrap()
                .encrypted_payload;
        assert_eq!(after, original);
        let active =
            diesel::sql_query("SELECT COUNT(*) AS count FROM workspace_convergence_v3_active")
                .get_result::<CountRow>(&mut connection)
                .unwrap()
                .count;
        assert_eq!(active, 0);
    }

    #[tokio::test]
    async fn reopen_finishes_v3_cleanup_without_changing_the_active_state() {
        let (store, pool, _directory) = make_store();
        let state = expected_rc3_state(persisted_state());
        let plaintext = rc3_v2_plaintext(&state);
        insert_encrypted_plaintext(&pool, &plaintext, state.updated_at_ms);
        assert_eq!(store.load_state().await.unwrap(), Some(state.clone()));

        let reopened = reopen_store(pool.clone());
        assert_eq!(reopened.load_state().await.unwrap(), Some(state));
        let mut connection = pool.get().unwrap();
        let migrations =
            diesel::sql_query("SELECT COUNT(*) AS count FROM workspace_convergence_v3_migrations")
                .get_result::<CountRow>(&mut connection)
                .unwrap()
                .count;
        let slots =
            diesel::sql_query("SELECT COUNT(*) AS count FROM workspace_convergence_v3_slots")
                .get_result::<CountRow>(&mut connection)
                .unwrap()
                .count;
        assert_eq!(migrations, 0);
        assert_eq!(slots, 1);
    }
}
