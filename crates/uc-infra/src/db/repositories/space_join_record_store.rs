use std::collections::BTreeMap;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use serde::{Deserialize, Serialize};
use uc_application::deps::{
    CurrentLocalJoinProjection, LocalJoinStartMutation, MembershipHistoryRepositoryError,
    MembershipHistoryRepositoryPort, SpaceJoinRecordStoreError, SpaceJoinRecordStorePort,
};
use uc_core::membership::{
    AdmissionOutboxPurpose, AdmissionProfileMetadata, AdmissionSpaceTransitionResultV2,
    AdmissionSpaceTransitionV2, AdmissionTerminalResult, CompletedSpaceJoinRecord,
    CompletionHelperAdmissionStage, JoinerAdmissionStage, SpaceJoinRecord, SpaceJoinRecordId,
    SpaceJoinRoleState, SponsorAdmissionStage, COMPLETED_SPACE_JOIN_RECORD_FORMAT_V1,
};

use crate::db::ports::DbExecutor;
use crate::security::{AdmissionKeyManager, WrappedSpaceJoinRecordDataKey};

const REPOSITORY_FORMAT_V1: u16 = 1;
const REPOSITORY_PAYLOAD_PURPOSE: &[u8] = b"admission-repository-state";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredSpaceJoinRecordV1 {
    wrapped_data_key: WrappedSpaceJoinRecordDataKey,
    encrypted_payload: Vec<u8>,
    consumed_invitation_digest: Option<[u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SpaceJoinRecordStoreStateV1 {
    format_version: u16,
    metadata: AdmissionProfileMetadata,
    attempts: BTreeMap<SpaceJoinRecordId, StoredSpaceJoinRecordV1>,
    terminals: BTreeMap<SpaceJoinRecordId, CompletedSpaceJoinRecord>,
    #[serde(default)]
    membership_history_v2: Option<Vec<u8>>,
}

impl SpaceJoinRecordStoreStateV1 {
    fn fresh(profile_generation: [u8; 16]) -> Self {
        Self {
            format_version: REPOSITORY_FORMAT_V1,
            metadata: AdmissionProfileMetadata::fresh(profile_generation),
            attempts: BTreeMap::new(),
            terminals: BTreeMap::new(),
            membership_history_v2: None,
        }
    }
}

#[derive(QueryableByName)]
struct EncryptedRepositoryRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

pub struct DieselSpaceJoinRecordStore<E> {
    executor: E,
    keys: AdmissionKeyManager,
}

impl<E> DieselSpaceJoinRecordStore<E> {
    pub fn new(executor: E, keys: AdmissionKeyManager) -> Self {
        Self { executor, keys }
    }
}

fn map_key_error(error: crate::security::AdmissionKeyError) -> SpaceJoinRecordStoreError {
    match error {
        crate::security::AdmissionKeyError::SecureStorage => SpaceJoinRecordStoreError::Locked,
        crate::security::AdmissionKeyError::Corrupt
        | crate::security::AdmissionKeyError::OpenFailed => SpaceJoinRecordStoreError::Corrupt,
    }
}

fn repository_error(error: impl std::fmt::Display) -> SpaceJoinRecordStoreError {
    SpaceJoinRecordStoreError::Repository(error.to_string())
}

fn executor_error(error: anyhow::Error) -> SpaceJoinRecordStoreError {
    error
        .downcast_ref::<SpaceJoinRecordStoreError>()
        .cloned()
        .unwrap_or_else(|| repository_error(error))
}

fn membership_history_repository_error(
    error: SpaceJoinRecordStoreError,
) -> MembershipHistoryRepositoryError {
    match error {
        SpaceJoinRecordStoreError::Locked => MembershipHistoryRepositoryError::Locked,
        SpaceJoinRecordStoreError::Corrupt => MembershipHistoryRepositoryError::Corrupt,
        SpaceJoinRecordStoreError::VersionConflict => MembershipHistoryRepositoryError::Conflict,
        SpaceJoinRecordStoreError::AlreadyExists
        | SpaceJoinRecordStoreError::NotFound
        | SpaceJoinRecordStoreError::PreviousJoinCannotBeSuperseded
        | SpaceJoinRecordStoreError::Repository(_) => MembershipHistoryRepositoryError::Unavailable,
    }
}

fn decode_space_transition(
    encoded: Option<&[u8]>,
) -> Result<Option<AdmissionSpaceTransitionV2>, SpaceJoinRecordStoreError> {
    encoded
        .map(|bytes| {
            AdmissionSpaceTransitionV2::decode(bytes).ok_or(SpaceJoinRecordStoreError::Corrupt)
        })
        .transpose()
}

fn decode_space_transition_result(
    encoded: Option<&[u8]>,
) -> Result<Option<AdmissionSpaceTransitionResultV2>, SpaceJoinRecordStoreError> {
    encoded
        .map(|bytes| {
            AdmissionSpaceTransitionResultV2::decode(bytes)
                .ok_or(SpaceJoinRecordStoreError::Corrupt)
        })
        .transpose()
}

impl<E: DbExecutor> DieselSpaceJoinRecordStore<E> {
    fn load_state_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<SpaceJoinRecordStoreStateV1, SpaceJoinRecordStoreError> {
        let row = sql_query(
            "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
        )
        .get_result::<EncryptedRepositoryRow>(conn)
        .optional()
        .map_err(repository_error)?;
        let Some(row) = row else {
            return Ok(SpaceJoinRecordStoreStateV1::fresh(
                self.keys.profile_generation(),
            ));
        };
        let plaintext = self
            .keys
            .open_profile_payload(REPOSITORY_PAYLOAD_PURPOSE, &row.encrypted_payload)
            .map_err(map_key_error)?;
        let state: SpaceJoinRecordStoreStateV1 =
            postcard::from_bytes(&plaintext).map_err(|_| SpaceJoinRecordStoreError::Corrupt)?;
        if state.format_version != REPOSITORY_FORMAT_V1
            || state.metadata.format_version
                != uc_core::membership::ADMISSION_PROFILE_METADATA_FORMAT_V1
            || state.metadata.profile_generation != self.keys.profile_generation()
            || state.metadata.join_projection_floor_ordinal > state.metadata.next_local_join_ordinal
            || state.metadata.device_trust_revision < state.metadata.next_local_join_ordinal
        {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        Ok(state)
    }

    fn save_state_on(
        &self,
        conn: &mut SqliteConnection,
        state: &SpaceJoinRecordStoreStateV1,
    ) -> Result<(), SpaceJoinRecordStoreError> {
        let plaintext = postcard::to_stdvec(state).map_err(repository_error)?;
        let encrypted = self
            .keys
            .seal_profile_payload(REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(encrypted)
        .execute(conn)
        .map_err(repository_error)?;
        let reopened = self.load_state_on(conn)?;
        if reopened != *state {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        Ok(())
    }

    fn open_attempt(
        &self,
        attempt_id: SpaceJoinRecordId,
        stored: &StoredSpaceJoinRecordV1,
    ) -> Result<SpaceJoinRecord, SpaceJoinRecordStoreError> {
        let plaintext = self
            .keys
            .open_attempt_payload(
                *attempt_id.as_bytes(),
                &stored.wrapped_data_key,
                &stored.encrypted_payload,
            )
            .map_err(map_key_error)?;
        let attempt = SpaceJoinRecord::decode_persisted(&plaintext)
            .map_err(|_| SpaceJoinRecordStoreError::Corrupt)?;
        validate_attempt(&attempt)?;
        if attempt.record_id != attempt_id {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        Ok(attempt)
    }

    fn seal_attempt(
        &self,
        attempt: &SpaceJoinRecord,
        wrapped: WrappedSpaceJoinRecordDataKey,
        consumed_invitation_digest: Option<[u8; 32]>,
    ) -> Result<StoredSpaceJoinRecordV1, SpaceJoinRecordStoreError> {
        validate_attempt(attempt)?;
        let plaintext = postcard::to_stdvec(attempt).map_err(repository_error)?;
        let encrypted_payload = self
            .keys
            .seal_attempt_payload(*attempt.record_id.as_bytes(), &wrapped, &plaintext)
            .map_err(map_key_error)?;
        Ok(StoredSpaceJoinRecordV1 {
            wrapped_data_key: wrapped,
            encrypted_payload,
            consumed_invitation_digest,
        })
    }
}

fn validate_attempt(attempt: &SpaceJoinRecord) -> Result<(), SpaceJoinRecordStoreError> {
    if attempt.format_version != uc_core::membership::SPACE_JOIN_RECORD_FORMAT_V1
        || attempt.stage_rank().is_none()
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    let joiner = attempt.is_joiner();
    let helper = matches!(attempt.role_state, SpaceJoinRoleState::CompletionHelper(_));
    if joiner != attempt.join_id.is_some() || joiner != attempt.local_join_ordinal.is_some() {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    let rejected = matches!(
        attempt.role_state,
        SpaceJoinRoleState::Sponsor(uc_core::membership::SponsorAdmissionState {
            stage: SponsorAdmissionStage::Rejected,
        }) | SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
            stage: JoinerAdmissionStage::Rejected,
        })
    );
    let completed = matches!(
        attempt.role_state,
        SpaceJoinRoleState::Sponsor(uc_core::membership::SponsorAdmissionState {
            stage: SponsorAdmissionStage::Completed,
        }) | SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
            stage: JoinerAdmissionStage::Completed,
        }) | SpaceJoinRoleState::CompletionHelper(
            uc_core::membership::CompletionHelperAdmissionState {
                stage: CompletionHelperAdmissionStage::Completed,
            }
        )
    );
    let superseded = matches!(
        attempt.role_state,
        SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
            stage: JoinerAdmissionStage::Superseded,
        })
    );
    if rejected {
        if attempt.terminal_result != Some(AdmissionTerminalResult::Rejected)
            || attempt.rejection_reason.is_none()
        {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
    } else if superseded {
        if attempt.terminal_result != Some(AdmissionTerminalResult::SupersededByNewJoin)
            || attempt.rejection_reason.is_some()
            || attempt.cancel_request.is_none()
            || attempt.outboxes.iter().all(|message| {
                message.purpose != uc_core::membership::AdmissionOutboxPurpose::CancelRequested
            })
        {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
    } else if completed {
        if attempt.terminal_result.is_none() || attempt.completion.is_none() {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
    } else if attempt.terminal_result.is_some() || attempt.rejection_reason.is_some() {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    let rank = attempt
        .stage_rank()
        .ok_or(SpaceJoinRecordStoreError::Corrupt)?;
    let transition = decode_space_transition(attempt.space_transition.as_deref())?;
    let transition_result =
        decode_space_transition_result(attempt.space_transition_result.as_deref())?;
    if transition.as_ref().is_some_and(|transition| {
        !joiner
            || rank < 3
            || transition.attempt_id() != attempt.record_id
            || attempt.target_access_state.is_none()
    }) {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if let Some(result) = &transition_result {
        let valid_terminal = joiner
            && attempt.terminal_result == Some(AdmissionTerminalResult::Active)
            && transition
                .as_ref()
                .is_some_and(|transition| result.matches_cleanup_pending(transition));
        if !valid_terminal {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
    } else if attempt.terminal_result == Some(AdmissionTerminalResult::Active)
        && transition.is_some()
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if helper {
        let helper_material_is_complete = attempt.lineage_id.is_some()
            && attempt.base_history_position.is_some()
            && attempt.candidate_event.is_some()
            && attempt.candidate_event_id.is_some()
            && attempt.candidate_key_package.is_some()
            && attempt.target_members_digest.is_some()
            && attempt.security_commitment.is_some()
            && attempt.security_commit.is_some()
            && attempt.security_welcome.is_some()
            && attempt.target_protection_group_id.is_some()
            && attempt.target_key_catalog.is_some()
            && attempt.existing_member_security_deliveries.is_some()
            && attempt.activation_receipt.is_some()
            && attempt.resume_public_key.is_some()
            && !attempt.resume_peers.is_empty()
            && !attempt.completion_recovery_deliveries.is_empty();
        let helper_has_forbidden_material = attempt.invitation_claim.is_some()
            || attempt.space_transition.is_some()
            || attempt.space_transition_result.is_some()
            || attempt.prepared_proof.is_some()
            || attempt.cancel_request.is_some()
            || attempt.cancel_outcome.is_some()
            || attempt.resume_private_key.is_some()
            || attempt.joiner_pending_security_state.is_some()
            || attempt.staged_security_state.is_some()
            || attempt.target_access_state.is_some();
        if !helper_material_is_complete || helper_has_forbidden_material {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
    }
    if rank >= 2
        && !rejected
        && !superseded
        && !helper
        && (attempt.lineage_id.is_none()
            || attempt.base_history_position.is_none()
            || attempt.candidate_event.is_none()
            || attempt.candidate_event_id.is_none()
            || attempt.candidate_key_package.is_none()
            || attempt.target_members_digest.is_none()
            || attempt.security_commitment.is_none()
            || attempt.security_commit.is_none()
            || attempt.security_welcome.is_none()
            || attempt.target_protection_group_id.is_none()
            || attempt.target_key_catalog.is_none()
            || attempt.target_relationships.is_none()
            || attempt.existing_member_security_deliveries.is_none()
            || attempt.staged_security_state.is_none())
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if rank >= 2 && !rejected && !superseded && !helper && attempt.base_membership_history.is_none()
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if rank >= 3 && !rejected && !superseded && !helper && attempt.prepared_proof.is_none() {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if joiner
        && rank >= 3
        && !rejected
        && !superseded
        && attempt.verified_membership_history.is_none()
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if rank >= 5 && !rejected && !superseded && attempt.activation_receipt.is_none() {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if matches!(
        attempt.role_state,
        SpaceJoinRoleState::Sponsor(uc_core::membership::SponsorAdmissionState {
            stage: SponsorAdmissionStage::Accepted,
        })
    ) && attempt.invitation_claim.is_none()
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    let mut outbox_keys = std::collections::BTreeSet::new();
    if attempt.outboxes.iter().any(|message| {
        !outbox_keys.insert((
            message.purpose,
            message.recipient.clone(),
            message.message_id,
        ))
    }) {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    Ok(())
}

fn validate_local_join_replacement(
    attempt: &SpaceJoinRecord,
    expected_ordinal: u64,
) -> Result<(), SpaceJoinRecordStoreError> {
    validate_attempt(attempt)?;
    if attempt.record_version != 0
        || attempt.local_join_ordinal != Some(expected_ordinal)
        || !matches!(
            attempt.role_state,
            SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
                stage: JoinerAdmissionStage::Initiated,
            })
        )
        || attempt.terminal_result.is_some()
        || attempt.rejection_reason.is_some()
        || attempt.prepared_proof.is_some()
        || attempt.write_ahead_recovery.is_some()
        || attempt.space_transition.is_some()
        || attempt.space_transition_result.is_some()
        || attempt.cancel_request.is_some()
        || attempt.cancel_outcome.is_some()
        || attempt.cleanup_pending
        || attempt.joiner_pending_security_state.is_none()
        || attempt.candidate_key_package.is_none()
        || attempt.joiner_member_instance.is_none()
        || attempt
            .resume_public_key
            .as_ref()
            .is_none_or(|key| key.len() != 32)
        || attempt
            .resume_private_key
            .as_ref()
            .is_none_or(|key| key.len() != 32)
        || attempt.outboxes.len() != 1
        || !attempt.outboxes.iter().all(|message| {
            message.purpose == uc_core::membership::AdmissionOutboxPurpose::JoinRequest
                && message.predecessor_message_id.is_none()
                && !message.recipient.is_empty()
                && !message.payload.is_empty()
                && !message.superseded
        })
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    Ok(())
}

fn validate_attempt_update(
    current: &SpaceJoinRecord,
    next: &SpaceJoinRecord,
) -> Result<(), SpaceJoinRecordStoreError> {
    validate_attempt(current)?;
    validate_attempt(next)?;
    let current_transition = decode_space_transition(current.space_transition.as_deref())?;
    let next_transition = decode_space_transition(next.space_transition.as_deref())?;
    let transition_is_valid = match (&current_transition, &next_transition) {
        (None, None) => true,
        (None, Some(next)) => next.is_initial(),
        (Some(current), Some(next)) => current == next || current.can_advance_to(next),
        (Some(current), None) => {
            next.terminal_result == Some(AdmissionTerminalResult::Rejected)
                && current.phase_rank() < current.activation_started_rank()
                && next.space_transition_result.is_none()
        }
    };
    if !transition_is_valid {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if current.space_transition_result.is_some()
        && current.space_transition_result != next.space_transition_result
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    let access_is_filled_during_joiner_preparation = current.target_access_state.is_none()
        && next.target_access_state.is_some()
        && matches!(
            current.role_state,
            SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
                stage: JoinerAdmissionStage::Initiated,
            })
        )
        && matches!(
            next.role_state,
            SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
                stage: JoinerAdmissionStage::Prepared,
            })
        );
    let access_is_valid = current.target_access_state == next.target_access_state
        || access_is_filled_during_joiner_preparation
        || (current_transition.is_some()
            && next_transition.is_none()
            && next.terminal_result == Some(AdmissionTerminalResult::Rejected)
            && next.target_access_state.is_none());
    if !access_is_valid {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    if current.identity_binding.is_some() && current.identity_binding != next.identity_binding {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    Ok(())
}

fn validate_terminal_delivery_update(
    current: &SpaceJoinRecord,
    next: &SpaceJoinRecord,
) -> Result<(), SpaceJoinRecordStoreError> {
    if next.cleanup_pending != current.cleanup_pending
        || !next.inbox_dedup.starts_with(&current.inbox_dedup)
        || next.outboxes.len() < current.outboxes.len()
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    for (offset, record) in next.inbox_dedup[current.inbox_dedup.len()..]
        .iter()
        .enumerate()
    {
        if next.inbox_dedup[..current.inbox_dedup.len() + offset].contains(record) {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
    }
    if current
        .outboxes
        .iter()
        .zip(&next.outboxes)
        .any(|(current, next)| {
            current.purpose != next.purpose
                || current.recipient != next.recipient
                || current.message_id != next.message_id
                || current.predecessor_message_id != next.predecessor_message_id
                || current.payload != next.payload
                || (current.superseded && !next.superseded)
        })
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    let complete_id = current
        .outboxes
        .iter()
        .find(|message| message.purpose == uc_core::membership::AdmissionOutboxPurpose::Complete)
        .map(|message| message.message_id);
    let appended = &next.outboxes[current.outboxes.len()..];
    if (!appended.is_empty() && complete_id.is_none())
        || appended.iter().any(|message| {
            !matches!(
                message.purpose,
                uc_core::membership::AdmissionOutboxPurpose::ExistingMemberSecurityUpdate
                    | uc_core::membership::AdmissionOutboxPurpose::HistoryOrReceiptBatch
            ) || message.predecessor_message_id != complete_id
                || message.superseded
        })
    {
        return Err(SpaceJoinRecordStoreError::Corrupt);
    }
    Ok(())
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> SpaceJoinRecordStorePort for DieselSpaceJoinRecordStore<E> {
    async fn reset_admission_profile(
        &self,
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state.attempts.clear();
                    state.terminals.clear();
                    state.membership_history_v2 = None;
                    state.metadata.join_projection_floor_ordinal =
                        state.metadata.next_local_join_ordinal;
                    state.metadata.consumed_invitation_attempts.clear();
                    state.metadata.completion_recovery_challenges.clear();
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn commit_local_join_start(
        &self,
        mutation: LocalJoinStartMutation,
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        let (
            expected_previous_attempt_id,
            expected_previous_record_version,
            previous_terminal,
            replacement,
        ) = match mutation {
            LocalJoinStartMutation::Create { replacement } => {
                let metadata = self.profile_metadata().await?;
                validate_local_join_replacement(&replacement, metadata.next_local_join_ordinal)?;
                return self.create(&replacement, None, None).await;
            }
            LocalJoinStartMutation::Supersede {
                expected_previous_attempt_id,
                expected_previous_record_version,
                previous_terminal,
                replacement,
            } => (
                expected_previous_attempt_id,
                expected_previous_record_version,
                previous_terminal,
                replacement,
            ),
        };

        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    validate_local_join_replacement(
                        &replacement,
                        state.metadata.next_local_join_ordinal,
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                    if replacement.record_id == expected_previous_attempt_id
                        || state.attempts.contains_key(&replacement.record_id)
                        || state.terminals.contains_key(&replacement.record_id)
                        || state
                            .terminals
                            .values()
                            .any(|terminal| terminal.join_id == replacement.join_id)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                    }

                    let previous_stored = state
                        .attempts
                        .get(&expected_previous_attempt_id)
                        .cloned()
                        .ok_or_else(|| {
                            anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict)
                        })?;
                    let previous = self
                        .open_attempt(expected_previous_attempt_id, &previous_stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if previous.record_version != expected_previous_record_version {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    if previous.stage_rank().is_some_and(|rank| rank >= 3) {
                        return Err(anyhow::anyhow!(
                            SpaceJoinRecordStoreError::PreviousJoinCannotBeSuperseded
                        ));
                    }
                    if state.attempts.iter().any(|(attempt_id, stored)| {
                        if *attempt_id == expected_previous_attempt_id {
                            return false;
                        }
                        self.open_attempt(*attempt_id, stored)
                            .map_or(true, |attempt| {
                                !attempt.is_terminal()
                                    || attempt.write_ahead_recovery.is_some()
                                    || (attempt.space_transition.is_some()
                                        && attempt.space_transition_result.is_none())
                                    || attempt.cleanup_pending
                            })
                    }) {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    if previous_terminal.record_id != expected_previous_attempt_id
                        || previous_terminal.record_version
                            != expected_previous_record_version
                                .checked_add(1)
                                .ok_or_else(|| {
                                    anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt)
                                })?
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    let cleanup = previous_terminal
                        .outboxes
                        .last()
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    let mut expected_terminal = previous
                        .superseded_by_new_join(cleanup)
                        .map_err(|_| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    expected_terminal.record_version = previous_terminal.record_version;
                    if expected_terminal != previous_terminal {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt));
                    }
                    validate_attempt_update(&previous, &previous_terminal)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.attempts.iter().any(|(attempt_id, stored)| {
                        *attempt_id != expected_previous_attempt_id
                            && self
                                .open_attempt(*attempt_id, stored)
                                .map_or(true, |attempt| attempt.join_id == replacement.join_id)
                    }) {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                    }

                    let previous_resealed = self
                        .seal_attempt(
                            &previous_terminal,
                            previous_stored.wrapped_data_key,
                            previous_stored.consumed_invitation_digest,
                        )
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let replacement_key = self
                        .keys
                        .create_wrapped_attempt_key(*replacement.record_id.as_bytes())
                        .map_err(|error| anyhow::anyhow!(map_key_error(error)))?;
                    let replacement_stored = self
                        .seal_attempt(&replacement, replacement_key, None)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state
                        .attempts
                        .insert(expected_previous_attempt_id, previous_resealed);
                    state
                        .attempts
                        .insert(replacement.record_id, replacement_stored);
                    state.metadata.next_local_join_ordinal = state
                        .metadata
                        .next_local_join_ordinal
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn create(
        &self,
        attempt: &SpaceJoinRecord,
        consumed_invitation_digest: Option<[u8; 32]>,
        initial_membership_history_v2: Option<&[u8]>,
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        if initial_membership_history_v2.is_some_and(|history| history.is_empty()) {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        let attempt = attempt.clone();
        let initial_membership_history_v2 = initial_membership_history_v2.map(ToOwned::to_owned);
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.attempts.contains_key(&attempt.record_id)
                        || state.terminals.contains_key(&attempt.record_id)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                    }
                    if attempt.join_id.is_some()
                        && state
                            .terminals
                            .values()
                            .any(|terminal| terminal.join_id == attempt.join_id)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                    }
                    for (persisted_id, persisted) in &state.attempts {
                        let persisted = self
                            .open_attempt(*persisted_id, persisted)
                            .map_err(|error| anyhow::anyhow!(error))?;
                        if attempt.join_id.is_some() && persisted.join_id == attempt.join_id {
                            return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                        }
                        if !persisted.is_terminal()
                            || persisted.write_ahead_recovery.is_some()
                            || persisted.cleanup_pending
                        {
                            return Err(anyhow::anyhow!(
                                SpaceJoinRecordStoreError::VersionConflict
                            ));
                        }
                    }
                    if attempt.is_joiner() {
                        if attempt.local_join_ordinal
                            != Some(state.metadata.next_local_join_ordinal)
                        {
                            return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt));
                        }
                        state.metadata.next_local_join_ordinal = state
                            .metadata
                            .next_local_join_ordinal
                            .checked_add(1)
                            .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    }
                    if let Some(digest) = consumed_invitation_digest {
                        if state
                            .metadata
                            .consumed_invitation_attempts
                            .contains_key(&digest)
                        {
                            return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                        }
                        state
                            .metadata
                            .consumed_invitation_attempts
                            .insert(digest, attempt.record_id);
                    }
                    if let Some(initial_history) = &initial_membership_history_v2 {
                        match &state.membership_history_v2 {
                            Some(current) if current != initial_history => {
                                return Err(anyhow::anyhow!(
                                    SpaceJoinRecordStoreError::VersionConflict
                                ));
                            }
                            None => {
                                state.membership_history_v2 = Some(initial_history.clone());
                            }
                            Some(_) => {}
                        }
                    }
                    let wrapped = self
                        .keys
                        .create_wrapped_attempt_key(*attempt.record_id.as_bytes())
                        .map_err(|error| anyhow::anyhow!(map_key_error(error)))?;
                    let stored = self
                        .seal_attempt(&attempt, wrapped, consumed_invitation_digest)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state.attempts.insert(attempt.record_id, stored);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn load(
        &self,
        attempt_id: SpaceJoinRecordId,
    ) -> Result<Option<SpaceJoinRecord>, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                let state = self
                    .load_state_on(conn)
                    .map_err(|error| anyhow::anyhow!(error))?;
                state
                    .attempts
                    .get(&attempt_id)
                    .map(|stored| self.open_attempt(attempt_id, stored))
                    .transpose()
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
    }

    async fn save_completion_recovery_challenge(
        &self,
        attempt_id: SpaceJoinRecordId,
        challenge: &[u8],
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        if challenge.is_empty() {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        let challenge = challenge.to_vec();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.attempts.contains_key(&attempt_id)
                        || state.terminals.contains_key(&attempt_id)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                    }
                    if let Some(existing) = state
                        .metadata
                        .completion_recovery_challenges
                        .get(&attempt_id)
                    {
                        if existing == &challenge {
                            return Ok(state.metadata);
                        }
                    }
                    state
                        .metadata
                        .completion_recovery_challenges
                        .insert(attempt_id, challenge);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn load_completion_recovery_challenge(
        &self,
        attempt_id: SpaceJoinRecordId,
    ) -> Result<Option<Vec<u8>>, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                self.load_state_on(conn)
                    .map(|state| {
                        state
                            .metadata
                            .completion_recovery_challenges
                            .get(&attempt_id)
                            .cloned()
                    })
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
    }

    async fn create_completion_helper(
        &self,
        attempt: &SpaceJoinRecord,
        expected_challenge: &[u8],
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        if expected_challenge.is_empty()
            || !matches!(attempt.role_state, SpaceJoinRoleState::CompletionHelper(_))
        {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        let attempt = attempt.clone();
        let expected_challenge = expected_challenge.to_vec();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.attempts.contains_key(&attempt.record_id)
                        || state.terminals.contains_key(&attempt.record_id)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::AlreadyExists));
                    }
                    if state
                        .metadata
                        .completion_recovery_challenges
                        .get(&attempt.record_id)
                        != Some(&expected_challenge)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    for (persisted_id, persisted) in &state.attempts {
                        let persisted = self
                            .open_attempt(*persisted_id, persisted)
                            .map_err(|error| anyhow::anyhow!(error))?;
                        if !persisted.is_terminal()
                            || persisted.write_ahead_recovery.is_some()
                            || persisted.cleanup_pending
                        {
                            return Err(anyhow::anyhow!(
                                SpaceJoinRecordStoreError::VersionConflict
                            ));
                        }
                    }
                    let wrapped = self
                        .keys
                        .create_wrapped_attempt_key(*attempt.record_id.as_bytes())
                        .map_err(|error| anyhow::anyhow!(map_key_error(error)))?;
                    let stored = self
                        .seal_attempt(&attempt, wrapped, None)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state.attempts.insert(attempt.record_id, stored);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn compare_and_advance(
        &self,
        attempt_id: SpaceJoinRecordId,
        expected_record_version: u64,
        next: &SpaceJoinRecord,
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        let next = next.clone();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let stored = state
                        .attempts
                        .get(&attempt_id)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::NotFound))?;
                    let current = self
                        .open_attempt(attempt_id, &stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if current.record_version != expected_record_version
                        || next.record_version != expected_record_version.saturating_add(1)
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    if next.record_id != attempt_id
                        || !current.same_role_as(&next)
                        || next.stage_rank() < current.stage_rank()
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt));
                    }
                    validate_attempt_update(&current, &next)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if current.is_terminal() {
                        validate_terminal_delivery_update(&current, &next)
                            .map_err(|error| anyhow::anyhow!(error))?;
                        let mut allowed = current.clone();
                        allowed.record_version = next.record_version;
                        allowed.inbox_dedup = next.inbox_dedup.clone();
                        allowed.outboxes = next.outboxes.clone();
                        allowed.cleanup_pending = next.cleanup_pending;
                        if allowed != next {
                            return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt));
                        }
                    }
                    let replacement = self
                        .seal_attempt(
                            &next,
                            stored.wrapped_data_key,
                            stored.consumed_invitation_digest,
                        )
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state.attempts.insert(attempt_id, replacement);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn compare_and_advance_with_membership_history_v2(
        &self,
        attempt_id: SpaceJoinRecordId,
        expected_record_version: u64,
        next: &SpaceJoinRecord,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        if membership_history_v2.is_empty() {
            return Err(SpaceJoinRecordStoreError::Corrupt);
        }
        let next = next.clone();
        let expected_membership_history_v2 = expected_membership_history_v2.map(ToOwned::to_owned);
        let membership_history_v2 = membership_history_v2.to_vec();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let stored = state
                        .attempts
                        .get(&attempt_id)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::NotFound))?;
                    let current = self
                        .open_attempt(attempt_id, &stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.membership_history_v2.as_deref()
                        != expected_membership_history_v2.as_deref()
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    if current.record_version != expected_record_version
                        || next.record_version != expected_record_version.saturating_add(1)
                        || next.record_id != attempt_id
                        || !current.same_role_as(&next)
                        || next.stage_rank() < current.stage_rank()
                        || current.is_terminal()
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    validate_attempt_update(&current, &next)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let replacement = self
                        .seal_attempt(
                            &next,
                            stored.wrapped_data_key,
                            stored.consumed_invitation_digest,
                        )
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state.attempts.insert(attempt_id, replacement);
                    state.membership_history_v2 = Some(membership_history_v2);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn scan_recoverable(&self) -> Result<Vec<SpaceJoinRecord>, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                let state = self
                    .load_state_on(conn)
                    .map_err(|error| anyhow::anyhow!(error))?;
                state
                    .attempts
                    .iter()
                    .map(|(attempt_id, stored)| self.open_attempt(*attempt_id, stored))
                    .filter_map(|result| match result {
                        Ok(attempt) if attempt.has_recovery_work() => Some(Ok(attempt)),
                        Ok(_) => None,
                        Err(error) => Some(Err(error)),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
    }

    async fn compact_terminal(
        &self,
        attempt_id: SpaceJoinRecordId,
        expected_record_version: u64,
    ) -> Result<CompletedSpaceJoinRecord, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let stored = state
                        .attempts
                        .get(&attempt_id)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::NotFound))?;
                    let attempt = self
                        .open_attempt(attempt_id, &stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if attempt.record_version != expected_record_version {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    let terminal_result = attempt
                        .terminal_result
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    let identity_binding = attempt.identity_binding.clone();
                    if !matches!(
                        terminal_result,
                        AdmissionTerminalResult::Rejected
                            | AdmissionTerminalResult::SupersededByNewJoin
                    ) && identity_binding.is_none()
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt));
                    }
                    if attempt.outboxes.iter().any(|message| !message.superseded)
                        || attempt.write_ahead_recovery.is_some()
                        || attempt.cleanup_pending
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    let replay_result = if terminal_result == AdmissionTerminalResult::Rejected {
                        attempt
                            .outboxes
                            .iter()
                            .find(|message| message.purpose == AdmissionOutboxPurpose::Rejected)
                            .map(|message| {
                                let mut replay = message.clone();
                                replay.superseded = false;
                                postcard::to_stdvec(&replay)
                            })
                            .transpose()
                            .map_err(|error| anyhow::anyhow!(error))?
                            .unwrap_or_default()
                    } else {
                        attempt.completion.unwrap_or_default()
                    };
                    let terminal = CompletedSpaceJoinRecord {
                        format_version: COMPLETED_SPACE_JOIN_RECORD_FORMAT_V1,
                        attempt_id,
                        join_id: attempt.join_id,
                        local_join_ordinal: attempt.local_join_ordinal,
                        invitation_digest: stored.consumed_invitation_digest,
                        identity_binding,
                        terminal_result,
                        rejection_reason: attempt.rejection_reason,
                        candidate_event_id: attempt.candidate_event_id,
                        cancel_outcome: attempt.cancel_outcome,
                        replay_result,
                        space_transition_result: attempt.space_transition_result,
                        acknowledgment_rebuild: attempt.inbox_dedup,
                    };
                    state.terminals.insert(attempt_id, terminal.clone());
                    state.attempts.remove(&attempt_id);
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(terminal)
                })
            })
            .map_err(executor_error)
    }

    async fn load_terminal(
        &self,
        attempt_id: SpaceJoinRecordId,
    ) -> Result<Option<CompletedSpaceJoinRecord>, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                self.load_state_on(conn)
                    .map(|state| state.terminals.get(&attempt_id).cloned())
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
    }

    async fn profile_metadata(
        &self,
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                self.load_state_on(conn)
                    .map(|state| state.metadata)
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
    }

    async fn project_current_local_join(
        &self,
    ) -> Result<Option<CurrentLocalJoinProjection>, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                let state = self
                    .load_state_on(conn)
                    .map_err(|error| anyhow::anyhow!(error))?;
                let mut pending = state
                    .attempts
                    .iter()
                    .map(|(attempt_id, stored)| self.open_attempt(*attempt_id, stored))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .filter(|attempt| attempt.is_joiner() && !attempt.is_terminal())
                    .filter_map(|attempt| {
                        Some(CurrentLocalJoinProjection {
                            device_trust_revision: state.metadata.device_trust_revision,
                            attempt_id: attempt.record_id,
                            join_id: attempt.join_id?,
                            local_join_ordinal: attempt.local_join_ordinal?,
                            terminal_result: None,
                            rejection_reason: None,
                        })
                    })
                    .max_by_key(|projection| projection.local_join_ordinal);
                if pending.is_some() {
                    return Ok(pending.take());
                }
                Ok(state
                    .terminals
                    .values()
                    .filter_map(|terminal| {
                        if terminal.terminal_result == AdmissionTerminalResult::SupersededByNewJoin
                        {
                            return None;
                        }
                        let ordinal = terminal.local_join_ordinal?;
                        if ordinal < state.metadata.join_projection_floor_ordinal {
                            return None;
                        }
                        Some(CurrentLocalJoinProjection {
                            device_trust_revision: state.metadata.device_trust_revision,
                            attempt_id: terminal.attempt_id,
                            join_id: terminal.join_id?,
                            local_join_ordinal: ordinal,
                            terminal_result: Some(terminal.terminal_result),
                            rejection_reason: terminal.rejection_reason,
                        })
                    })
                    .max_by_key(|projection| projection.local_join_ordinal))
            })
            .map_err(executor_error)
    }

    async fn advance_projection_floor(
        &self,
        expected_device_trust_revision: u64,
    ) -> Result<AdmissionProfileMetadata, SpaceJoinRecordStoreError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.metadata.device_trust_revision != expected_device_trust_revision {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    for (attempt_id, stored) in &state.attempts {
                        let attempt = self
                            .open_attempt(*attempt_id, stored)
                            .map_err(|error| anyhow::anyhow!(error))?;
                        if attempt.has_recovery_work() {
                            return Err(anyhow::anyhow!(
                                SpaceJoinRecordStoreError::VersionConflict
                            ));
                        }
                    }
                    state.metadata.join_projection_floor_ordinal =
                        state.metadata.next_local_join_ordinal;
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> MembershipHistoryRepositoryPort
    for DieselSpaceJoinRecordStore<E>
{
    async fn load_membership_history(
        &self,
    ) -> Result<Option<Vec<u8>>, MembershipHistoryRepositoryError> {
        self.executor
            .run(|conn| {
                self.load_state_on(conn)
                    .map(|state| state.membership_history_v2)
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
            .map_err(membership_history_repository_error)
    }

    async fn compare_and_replace_membership_history(
        &self,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<u64, MembershipHistoryRepositoryError> {
        if membership_history_v2.is_empty() {
            return Err(MembershipHistoryRepositoryError::Corrupt);
        }
        let expected_membership_history_v2 = expected_membership_history_v2.map(ToOwned::to_owned);
        let membership_history_v2 = membership_history_v2.to_vec();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.membership_history_v2.as_deref()
                        != expected_membership_history_v2.as_deref()
                    {
                        return Err(anyhow::anyhow!(SpaceJoinRecordStoreError::VersionConflict));
                    }
                    state.membership_history_v2 = Some(membership_history_v2);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(SpaceJoinRecordStoreError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata.device_trust_revision)
                })
            })
            .map_err(executor_error)
            .map_err(membership_history_repository_error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::{Arc, Mutex};

    use diesel::RunQueryDsl;
    use tempfile::tempdir;
    use uc_application::deps::LocalJoinStartMutation;
    use uc_application::deps::{
        MembershipHistoryRepositoryError, MembershipHistoryRepositoryPort, SpaceJoinRecordStorePort,
    };
    use uc_core::membership::{
        AdmissionInboxRecord, AdmissionOutboxMessage, AdmissionOutboxPurpose,
        AdmissionRejectionReason, AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2,
        AdmissionTerminalResult, CrossSpaceTransitionPhaseV2, CrossSpaceTransitionResultV2,
        CrossSpaceTransitionV2, JoinerAdmissionStage, MemberInstanceId, SpaceJoinRecord,
        SpaceJoinRecordId, SpaceJoinRoleState, SponsorAdmissionStage, SponsorAdmissionState,
        CROSS_SPACE_TRANSITION_FORMAT_V2,
    };
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::security::AdmissionKeyManager;

    #[derive(Default)]
    struct MemorySecureStorage {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    #[derive(Default)]
    struct FaultInjectingSecureStorage {
        values: Mutex<HashMap<String, Vec<u8>>>,
        successful_gets_before_failure: Mutex<Option<usize>>,
    }

    impl FaultInjectingSecureStorage {
        fn fail_after_successful_gets(&self, count: usize) {
            *self.successful_gets_before_failure.lock().unwrap() = Some(count);
        }
    }

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    impl SecureStoragePort for FaultInjectingSecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            let mut failure = self.successful_gets_before_failure.lock().unwrap();
            if let Some(remaining) = failure.as_mut() {
                if *remaining == 0 {
                    *failure = None;
                    return Err(SecureStorageError::Unavailable(
                        "injected admission key read failure".to_owned(),
                    ));
                }
                *remaining -= 1;
            }
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn cross_space_transition(
        attempt_id: SpaceJoinRecordId,
        phase: CrossSpaceTransitionPhaseV2,
    ) -> CrossSpaceTransitionV2 {
        let finalized = phase.rank() >= CrossSpaceTransitionPhaseV2::SourceFinalized.rank();
        CrossSpaceTransitionV2 {
            transition_format_version: CROSS_SPACE_TRANSITION_FORMAT_V2,
            attempt_id,
            source_space_id: "source-space".to_owned(),
            source_generation: [0xb1; 16],
            source_backup_ref: b"encrypted-source-backup".to_vec(),
            source_backup_digest: [0xb2; 32],
            source_revision_at_backup: 7,
            target_space_id: "target-space".to_owned(),
            target_generation: [0xb3; 16],
            target_keyslot_ref: b"target-keyslot".to_vec(),
            target_workspace_ref: b"target-workspace".to_vec(),
            phase,
            final_source_revision: finalized.then_some(9),
            final_manifest_digest: finalized.then_some([0xb4; 32]),
            migrated_records: 3,
            preserved_unreadable_records: 1,
            preserve_unreadable_history: true,
        }
    }

    fn prepared_joiner(attempt_id: SpaceJoinRecordId) -> SpaceJoinRecord {
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [0xb5; 16], JoinerAdmissionStage::Prepared);
        attempt.local_join_ordinal = Some(0);
        attempt.lineage_id = Some("target-space".to_owned());
        attempt.base_history_position = Some(b"base-position".to_vec());
        attempt.candidate_event = Some(b"candidate-event".to_vec());
        attempt.candidate_event_id = Some([0xb6; 32]);
        attempt.candidate_key_package = Some(b"key-package".to_vec());
        attempt.target_members_digest = Some([0xb7; 32]);
        attempt.security_commitment = Some(b"security-commitment".to_vec());
        attempt.security_commit = Some(b"security-commit".to_vec());
        attempt.security_welcome = Some(b"security-welcome".to_vec());
        attempt.target_protection_group_id = Some("target-protection-group".to_owned());
        attempt.target_key_catalog = Some(b"target-key-catalog".to_vec());
        attempt.target_relationships = Some(Vec::new());
        attempt.existing_member_security_deliveries = Some(Vec::new());
        attempt.staged_security_state = Some(b"staged-security".to_vec());
        attempt.base_membership_history = Some(b"base-history".to_vec());
        attempt.verified_membership_history = Some(b"verified-history".to_vec());
        attempt.prepared_proof = Some(b"prepared-proof".to_vec());
        attempt.target_access_state = Some(b"target-access".to_vec());
        attempt
    }

    fn supersedable_joiner(
        attempt_id: SpaceJoinRecordId,
        join_id: [u8; 16],
        ordinal: u64,
    ) -> SpaceJoinRecord {
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, join_id, JoinerAdmissionStage::Initiated);
        attempt.local_join_ordinal = Some(ordinal);
        attempt.joiner_pending_security_state = Some(vec![1]);
        attempt.candidate_key_package = Some(vec![2]);
        attempt.joiner_member_instance = Some(MemberInstanceId::from_bytes([3; 32]));
        attempt.resume_public_key = Some(vec![4; 32]);
        attempt.resume_private_key = Some(vec![5; 32]);
        attempt.outboxes.push(AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::JoinRequest,
            recipient: vec![6],
            message_id: [7; 32],
            predecessor_message_id: None,
            payload: vec![8],
            superseded: false,
        });
        attempt
    }

    fn rejected_sponsor(attempt_id: SpaceJoinRecordId) -> SpaceJoinRecord {
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [0; 16], JoinerAdmissionStage::Initiated);
        attempt.join_id = None;
        attempt.role_state = SpaceJoinRoleState::Sponsor(SponsorAdmissionState {
            stage: SponsorAdmissionStage::Rejected,
        });
        attempt.invitation_claim = Some(b"saved-invitation-claim".to_vec());
        attempt.terminal_result = Some(AdmissionTerminalResult::Rejected);
        attempt.rejection_reason = Some(AdmissionRejectionReason::InvitationUnavailable);
        attempt
    }

    #[tokio::test]
    async fn admission_profile_reset_clears_state_and_preserves_monotonic_metadata() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("device-management-reset.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xd1; 16]),
        );
        let attempt_id = SpaceJoinRecordId::from_bytes([0xd2; 32]);
        let attempt = supersedable_joiner(attempt_id, [0xd3; 16], 0);
        let before = store
            .create(&attempt, Some([0xd4; 32]), Some(b"old-membership-history"))
            .await
            .unwrap();

        let reset = store.reset_admission_profile().await.unwrap();

        assert_eq!(reset.profile_generation, before.profile_generation);
        assert_eq!(
            reset.next_local_join_ordinal,
            before.next_local_join_ordinal
        );
        assert_eq!(
            reset.join_projection_floor_ordinal,
            reset.next_local_join_ordinal
        );
        assert!(reset.device_trust_revision > before.device_trust_revision);
        assert!(reset.consumed_invitation_attempts.is_empty());
        assert!(reset.completion_recovery_challenges.is_empty());
        assert!(store.load(attempt_id).await.unwrap().is_none());
        assert!(store.load_membership_history().await.unwrap().is_none());
        assert!(store.scan_recoverable().await.unwrap().is_empty());
        assert!(store.project_current_local_join().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn commit_local_join_start_supersedes_and_creates_atomically() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-supersession.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xe1; 16]),
        );
        let previous_id = SpaceJoinRecordId::from_bytes([0xe2; 32]);
        let previous = supersedable_joiner(previous_id, [0xe3; 16], 0);
        let created = store
            .commit_local_join_start(LocalJoinStartMutation::Create {
                replacement: previous.clone(),
            })
            .await
            .unwrap();
        assert_eq!(created.next_local_join_ordinal, 1);
        assert_eq!(created.device_trust_revision, 1);

        let cleanup = AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::CancelRequested,
            recipient: vec![6],
            message_id: [0xe4; 32],
            predecessor_message_id: Some([7; 32]),
            payload: vec![9],
            superseded: false,
        };
        let mut previous_terminal = previous.superseded_by_new_join(cleanup).unwrap();
        previous_terminal.record_version = 1;
        let replacement_id = SpaceJoinRecordId::from_bytes([0xe5; 32]);
        let replacement = supersedable_joiner(replacement_id, [0xe6; 16], 1);
        let committed = store
            .commit_local_join_start(LocalJoinStartMutation::Supersede {
                expected_previous_attempt_id: previous_id,
                expected_previous_record_version: 0,
                previous_terminal: previous_terminal.clone(),
                replacement: replacement.clone(),
            })
            .await
            .unwrap();

        assert_eq!(committed.next_local_join_ordinal, 2);
        assert_eq!(committed.device_trust_revision, 2);
        assert_eq!(
            store.load(previous_id).await.unwrap(),
            Some(previous_terminal)
        );
        assert_eq!(store.load(replacement_id).await.unwrap(), Some(replacement));
        assert_eq!(
            store
                .project_current_local_join()
                .await
                .unwrap()
                .unwrap()
                .attempt_id,
            replacement_id
        );

        let mut settled_previous = store.load(previous_id).await.unwrap().unwrap();
        let settled_version = settled_previous.record_version;
        settled_previous.record_version += 1;
        settled_previous
            .outboxes
            .iter_mut()
            .find(|message| message.purpose == AdmissionOutboxPurpose::CancelRequested)
            .unwrap()
            .superseded = true;
        store
            .compare_and_advance(previous_id, settled_version, &settled_previous)
            .await
            .unwrap();
        let compacted = store
            .compact_terminal(previous_id, settled_previous.record_version)
            .await
            .unwrap();
        assert_eq!(
            compacted.terminal_result,
            AdmissionTerminalResult::SupersededByNewJoin
        );
        assert_eq!(
            store
                .project_current_local_join()
                .await
                .unwrap()
                .unwrap()
                .attempt_id,
            replacement_id
        );
    }

    #[tokio::test]
    async fn supersession_failure_recovers_whole_old_or_new_state() {
        let directory = tempdir().unwrap();
        let database_path = directory
            .path()
            .join("admission-supersession-rollback.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xe7; 16]),
        );
        let previous_id = SpaceJoinRecordId::from_bytes([0xe8; 32]);
        let previous = supersedable_joiner(previous_id, [0xe9; 16], 0);
        store
            .commit_local_join_start(LocalJoinStartMutation::Create {
                replacement: previous.clone(),
            })
            .await
            .unwrap();
        let before = store.profile_metadata().await.unwrap();
        let cleanup = AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::CancelRequested,
            recipient: vec![6],
            message_id: [0xea; 32],
            predecessor_message_id: Some([7; 32]),
            payload: vec![9],
            superseded: false,
        };
        let mut previous_terminal = previous.superseded_by_new_join(cleanup).unwrap();
        previous_terminal.record_version = 1;
        let replacement_id = SpaceJoinRecordId::from_bytes([0xeb; 32]);
        let replacement = supersedable_joiner(replacement_id, [0xec; 16], 1);

        assert_eq!(
            store
                .commit_local_join_start(LocalJoinStartMutation::Supersede {
                    expected_previous_attempt_id: previous_id,
                    expected_previous_record_version: 1,
                    previous_terminal: previous_terminal.clone(),
                    replacement: replacement.clone(),
                })
                .await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::VersionConflict)
        );
        assert_eq!(store.profile_metadata().await.unwrap(), before);
        assert_eq!(
            store.load(previous_id).await.unwrap(),
            Some(previous.clone())
        );
        assert_eq!(store.load(replacement_id).await.unwrap(), None);
        assert_eq!(
            store
                .project_current_local_join()
                .await
                .unwrap()
                .unwrap()
                .attempt_id,
            previous_id
        );

        store
            .commit_local_join_start(LocalJoinStartMutation::Supersede {
                expected_previous_attempt_id: previous_id,
                expected_previous_record_version: 0,
                previous_terminal,
                replacement,
            })
            .await
            .unwrap();
        assert_eq!(
            store
                .project_current_local_join()
                .await
                .unwrap()
                .unwrap()
                .attempt_id,
            replacement_id
        );
    }

    #[tokio::test]
    async fn supersession_crypto_failures_roll_back_atomically() {
        // After opening repository state and the previous attempt, these fail at
        // previous resealing, replacement-key creation, and replacement sealing.
        for (case, successful_gets_before_failure) in [2, 3, 4].into_iter().enumerate() {
            let directory = tempdir().unwrap();
            let database_path = directory
                .path()
                .join(format!("admission-supersession-crypto-{case}.sqlite"));
            let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
            let secure_storage = Arc::new(FaultInjectingSecureStorage::default());
            let store = super::DieselSpaceJoinRecordStore::new(
                DieselSqliteExecutor::new(pool),
                AdmissionKeyManager::new(secure_storage.clone(), [0xed; 16]),
            );
            let previous_id = SpaceJoinRecordId::from_bytes([0xee; 32]);
            let previous = supersedable_joiner(previous_id, [0xef; 16], 0);
            store
                .commit_local_join_start(LocalJoinStartMutation::Create {
                    replacement: previous.clone(),
                })
                .await
                .unwrap();
            let before = store.profile_metadata().await.unwrap();
            let cleanup = AdmissionOutboxMessage {
                purpose: AdmissionOutboxPurpose::CancelRequested,
                recipient: vec![6],
                message_id: [0xf0; 32],
                predecessor_message_id: Some([7; 32]),
                payload: vec![9],
                superseded: false,
            };
            let mut previous_terminal = previous.superseded_by_new_join(cleanup).unwrap();
            previous_terminal.record_version = 1;
            let replacement_id = SpaceJoinRecordId::from_bytes([0xf1; 32]);
            let replacement = supersedable_joiner(replacement_id, [0xf2; 16], 1);

            secure_storage.fail_after_successful_gets(successful_gets_before_failure);
            assert_eq!(
                store
                    .commit_local_join_start(LocalJoinStartMutation::Supersede {
                        expected_previous_attempt_id: previous_id,
                        expected_previous_record_version: 0,
                        previous_terminal,
                        replacement,
                    })
                    .await,
                Err(uc_application::deps::SpaceJoinRecordStoreError::Locked)
            );
            assert_eq!(store.profile_metadata().await.unwrap(), before);
            assert_eq!(store.load(previous_id).await.unwrap(), Some(previous));
            assert_eq!(store.load(replacement_id).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn supersession_counter_overflows_roll_back_atomically() {
        let directory = tempdir().unwrap();
        let database_path = directory
            .path()
            .join("admission-supersession-overflow.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xd0; 16]),
        );
        let previous_id = SpaceJoinRecordId::from_bytes([0xd1; 32]);
        let previous = supersedable_joiner(previous_id, [0xd2; 16], 0);
        store
            .commit_local_join_start(LocalJoinStartMutation::Create {
                replacement: previous.clone(),
            })
            .await
            .unwrap();
        let cleanup = AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::CancelRequested,
            recipient: vec![6],
            message_id: [0xd3; 32],
            predecessor_message_id: Some([7; 32]),
            payload: vec![9],
            superseded: false,
        };
        let mut previous_terminal = previous.superseded_by_new_join(cleanup.clone()).unwrap();
        previous_terminal.record_version = 1;
        let replacement_id = SpaceJoinRecordId::from_bytes([0xd4; 32]);

        let mut connection = pool.get().unwrap();
        let mut state = store.load_state_on(&mut connection).unwrap();
        state.metadata.next_local_join_ordinal = u64::MAX;
        state.metadata.device_trust_revision = u64::MAX;
        store.save_state_on(&mut connection, &state).unwrap();
        drop(connection);
        let replacement = supersedable_joiner(replacement_id, [0xd5; 16], u64::MAX);
        assert_eq!(
            store
                .commit_local_join_start(LocalJoinStartMutation::Supersede {
                    expected_previous_attempt_id: previous_id,
                    expected_previous_record_version: 0,
                    previous_terminal: previous_terminal.clone(),
                    replacement,
                })
                .await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );
        assert_eq!(
            store.load(previous_id).await.unwrap(),
            Some(previous.clone())
        );
        assert_eq!(store.load(replacement_id).await.unwrap(), None);
        assert_eq!(
            store
                .profile_metadata()
                .await
                .unwrap()
                .next_local_join_ordinal,
            u64::MAX
        );

        let mut connection = pool.get().unwrap();
        let mut state = store.load_state_on(&mut connection).unwrap();
        state.metadata.next_local_join_ordinal = 1;
        state.metadata.device_trust_revision = u64::MAX;
        store.save_state_on(&mut connection, &state).unwrap();
        drop(connection);
        let replacement = supersedable_joiner(replacement_id, [0xd5; 16], 1);
        assert_eq!(
            store
                .commit_local_join_start(LocalJoinStartMutation::Supersede {
                    expected_previous_attempt_id: previous_id,
                    expected_previous_record_version: 0,
                    previous_terminal: previous_terminal.clone(),
                    replacement,
                })
                .await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );
        assert_eq!(
            store.load(previous_id).await.unwrap(),
            Some(previous.clone())
        );
        assert_eq!(store.load(replacement_id).await.unwrap(), None);
        let metadata = store.profile_metadata().await.unwrap();
        assert_eq!(metadata.next_local_join_ordinal, 1);
        assert_eq!(metadata.device_trust_revision, u64::MAX);

        let mut connection = pool.get().unwrap();
        let mut state = store.load_state_on(&mut connection).unwrap();
        state.metadata.device_trust_revision = 1;
        let stored = state.attempts.get(&previous_id).cloned().unwrap();
        let mut max_version_previous = store.open_attempt(previous_id, &stored).unwrap();
        max_version_previous.record_version = u64::MAX;
        let resealed = store
            .seal_attempt(
                &max_version_previous,
                stored.wrapped_data_key,
                stored.consumed_invitation_digest,
            )
            .unwrap();
        state.attempts.insert(previous_id, resealed);
        store.save_state_on(&mut connection, &state).unwrap();
        drop(connection);
        let mut max_version_terminal = max_version_previous
            .superseded_by_new_join(cleanup)
            .unwrap();
        max_version_terminal.record_version = u64::MAX;
        let replacement = supersedable_joiner(replacement_id, [0xd5; 16], 1);
        assert_eq!(
            store
                .commit_local_join_start(LocalJoinStartMutation::Supersede {
                    expected_previous_attempt_id: previous_id,
                    expected_previous_record_version: u64::MAX,
                    previous_terminal: max_version_terminal,
                    replacement,
                })
                .await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );
        assert_eq!(
            store.load(previous_id).await.unwrap(),
            Some(max_version_previous)
        );
        assert_eq!(store.load(replacement_id).await.unwrap(), None);
        let metadata = store.profile_metadata().await.unwrap();
        assert_eq!(metadata.next_local_join_ordinal, 1);
        assert_eq!(metadata.device_trust_revision, 1);
    }

    #[tokio::test]
    async fn consumed_invitation_stays_bound_to_its_original_attempt() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-invitation-binding.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xf3; 16]),
        );
        let invitation_digest = [0xf4; 32];
        let original_id = SpaceJoinRecordId::from_bytes([0xf5; 32]);
        let original = rejected_sponsor(original_id);
        store
            .create(&original, Some(invitation_digest), None)
            .await
            .unwrap();
        store.compact_terminal(original_id, 0).await.unwrap();

        let replacement_id = SpaceJoinRecordId::from_bytes([0xf6; 32]);
        let replacement = rejected_sponsor(replacement_id);
        assert_eq!(
            store
                .create(&replacement, Some(invitation_digest), None)
                .await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::AlreadyExists)
        );
        assert_eq!(store.load(replacement_id).await.unwrap(), None);
        assert_eq!(
            store
                .load_terminal(original_id)
                .await
                .unwrap()
                .unwrap()
                .invitation_digest,
            Some(invitation_digest)
        );
    }

    fn applied_completion_helper(attempt_id: SpaceJoinRecordId) -> SpaceJoinRecord {
        let mut attempt = SpaceJoinRecord::new_completion_helper(attempt_id);
        attempt.lineage_id = Some("target-space".to_owned());
        attempt.base_history_position = Some(b"helper-position".to_vec());
        attempt.candidate_event = Some(b"candidate-event".to_vec());
        attempt.candidate_event_id = Some([0xd3; 32]);
        attempt.candidate_key_package = Some(b"candidate-key-package".to_vec());
        attempt.target_members_digest = Some([0xd4; 32]);
        attempt.security_commitment = Some(b"security-commitment".to_vec());
        attempt.security_commit = Some(b"security-commit".to_vec());
        attempt.security_welcome = Some(b"security-welcome".to_vec());
        attempt.target_protection_group_id = Some("target-protection-group".to_owned());
        attempt.target_key_catalog = Some(b"target-key-catalog".to_vec());
        attempt.existing_member_security_deliveries = Some(Vec::new());
        attempt.activation_receipt = Some(b"activation-receipt".to_vec());
        attempt.resume_public_key = Some(vec![0xd5; 32]);
        attempt
            .resume_peers
            .push(b"saved-signed-challenge".to_vec());
        attempt
            .completion_recovery_deliveries
            .push(b"authenticated-response".to_vec());
        attempt
    }

    #[tokio::test]
    async fn unknown_profile_metadata_version_fails_closed() {
        let directory = tempdir().unwrap();
        let database_path = directory
            .path()
            .join("admission-metadata-validation.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0xaf; 16];
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage, generation),
        );

        let mut connection = pool.get().unwrap();
        let mut state = store.load_state_on(&mut connection).unwrap();
        state.metadata.format_version += 1;
        let plaintext = postcard::to_stdvec(&state).unwrap();
        let encrypted = store
            .keys
            .seal_profile_payload(super::REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .unwrap();
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        assert_eq!(
            store.profile_metadata().await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );
    }

    #[tokio::test]
    async fn profile_counter_corruption_fails_closed() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-counter-corruption.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0xb0; 16];
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage, generation),
        );

        let mut connection = pool.get().unwrap();
        let mut state = store.load_state_on(&mut connection).unwrap();
        state.metadata.next_local_join_ordinal = 1;
        state.metadata.device_trust_revision = 0;
        let plaintext = postcard::to_stdvec(&state).unwrap();
        let encrypted = store
            .keys
            .seal_profile_payload(super::REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .unwrap();
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        assert_eq!(
            store.profile_metadata().await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );

        let mut state = super::SpaceJoinRecordStoreStateV1::fresh(generation);
        state.metadata.join_projection_floor_ordinal = 1;
        state.metadata.device_trust_revision = 1;
        let plaintext = postcard::to_stdvec(&state).unwrap();
        let encrypted = store
            .keys
            .seal_profile_payload(super::REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .unwrap();
        let mut connection = pool.get().unwrap();
        diesel::sql_query(
            "UPDATE admission_repository_state SET encrypted_payload = ? WHERE singleton_id = 1",
        )
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        assert_eq!(
            store.profile_metadata().await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );
    }

    #[tokio::test]
    async fn stale_revision_and_counter_overflow_leave_profile_state_unchanged() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-counter-rollback.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0xb1; 16];
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage, generation),
        );

        let initial = store.profile_metadata().await.unwrap();
        assert_eq!(
            store.advance_projection_floor(1).await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::VersionConflict)
        );
        assert_eq!(store.profile_metadata().await.unwrap(), initial);

        let mut connection = pool.get().unwrap();
        let mut state = store.load_state_on(&mut connection).unwrap();
        state.metadata.device_trust_revision = u64::MAX;
        let plaintext = postcard::to_stdvec(&state).unwrap();
        let encrypted = store
            .keys
            .seal_profile_payload(super::REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .unwrap();
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        assert_eq!(
            store
                .compare_and_replace_membership_history(None, b"target-space-history")
                .await,
            Err(MembershipHistoryRepositoryError::Corrupt)
        );
        assert_eq!(
            store
                .profile_metadata()
                .await
                .unwrap()
                .device_trust_revision,
            u64::MAX
        );
        assert_eq!(store.load_membership_history().await.unwrap(), None);

        let mut state = super::SpaceJoinRecordStoreStateV1::fresh(generation);
        state.metadata.next_local_join_ordinal = u64::MAX;
        state.metadata.device_trust_revision = u64::MAX;
        let plaintext = postcard::to_stdvec(&state).unwrap();
        let encrypted = store
            .keys
            .seal_profile_payload(super::REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .unwrap();
        let mut connection = pool.get().unwrap();
        diesel::sql_query(
            "UPDATE admission_repository_state SET encrypted_payload = ? WHERE singleton_id = 1",
        )
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .execute(&mut connection)
        .unwrap();
        drop(connection);

        let attempt_id = SpaceJoinRecordId::from_bytes([0xb3; 32]);
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [0xb4; 16], JoinerAdmissionStage::Initiated);
        attempt.local_join_ordinal = Some(u64::MAX);
        assert_eq!(
            store.create(&attempt, None, None).await,
            Err(uc_application::deps::SpaceJoinRecordStoreError::Corrupt)
        );
        assert_eq!(store.load(attempt_id).await.unwrap(), None);
        let metadata = store.profile_metadata().await.unwrap();
        assert_eq!(metadata.next_local_join_ordinal, u64::MAX);
        assert_eq!(metadata.device_trust_revision, u64::MAX);
    }

    #[tokio::test]
    async fn profile_revision_remains_monotonic_during_cross_space_transition() {
        let directory = tempdir().unwrap();
        let database_path = directory
            .path()
            .join("admission-cross-space-revision.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xb2; 16]),
        );

        let attempt_id = SpaceJoinRecordId::from_bytes([0xb5; 32]);
        let mut initiated =
            SpaceJoinRecord::new_joiner(attempt_id, [0xb6; 16], JoinerAdmissionStage::Initiated);
        initiated.local_join_ordinal = Some(0);
        initiated.candidate_key_package = Some(b"key-package".to_vec());
        let source = store
            .create(&initiated, None, Some(b"source-space-history"))
            .await
            .unwrap();

        let mut target_staged = prepared_joiner(attempt_id);
        target_staged.record_version = 1;
        target_staged.join_id = initiated.join_id;
        target_staged.local_join_ordinal = initiated.local_join_ordinal;
        target_staged.candidate_key_package = initiated.candidate_key_package.clone();
        target_staged.space_transition = AdmissionSpaceTransitionV2::CrossSpace(
            cross_space_transition(attempt_id, CrossSpaceTransitionPhaseV2::TargetStaged),
        )
        .encode();
        let target = store
            .compare_and_advance(attempt_id, 0, &target_staged)
            .await
            .unwrap();

        let mut activation_started = target_staged;
        activation_started.record_version = 2;
        activation_started.space_transition = AdmissionSpaceTransitionV2::CrossSpace(
            cross_space_transition(attempt_id, CrossSpaceTransitionPhaseV2::ActivationStarted),
        )
        .encode();
        let staged = store
            .compare_and_advance(attempt_id, 1, &activation_started)
            .await
            .unwrap();

        assert_eq!(source.device_trust_revision, 1);
        assert_eq!(target.device_trust_revision, 2);
        assert_eq!(staged.device_trust_revision, 3);
        assert_eq!(target.profile_generation, source.profile_generation);
        assert_eq!(staged.profile_generation, source.profile_generation);
    }

    #[test]
    fn cross_space_transition_is_joiner_bound_and_forward_only() {
        let attempt_id = SpaceJoinRecordId::from_bytes([0xb8; 32]);
        let mut current = prepared_joiner(attempt_id);
        current.space_transition = AdmissionSpaceTransitionV2::CrossSpace(cross_space_transition(
            attempt_id,
            CrossSpaceTransitionPhaseV2::TargetStaged,
        ))
        .encode();
        assert!(super::validate_attempt(&current).is_ok());

        let mut wrong_attempt = current.clone();
        wrong_attempt.space_transition =
            AdmissionSpaceTransitionV2::CrossSpace(cross_space_transition(
                SpaceJoinRecordId::from_bytes([0xb9; 32]),
                CrossSpaceTransitionPhaseV2::TargetStaged,
            ))
            .encode();
        assert!(super::validate_attempt(&wrong_attempt).is_err());

        let mut sponsor = current.clone();
        sponsor.join_id = None;
        sponsor.local_join_ordinal = None;
        sponsor.role_state = SpaceJoinRoleState::Sponsor(SponsorAdmissionState {
            stage: SponsorAdmissionStage::Prepared,
        });
        assert!(super::validate_attempt(&sponsor).is_err());

        let mut skipped = current.clone();
        skipped.record_version += 1;
        skipped.space_transition = AdmissionSpaceTransitionV2::CrossSpace(cross_space_transition(
            attempt_id,
            CrossSpaceTransitionPhaseV2::SourceFinalized,
        ))
        .encode();
        assert!(super::validate_attempt_update(&current, &skipped).is_err());

        let mut replaced_access = current.clone();
        replaced_access.record_version += 1;
        replaced_access.target_access_state = Some(b"other-target-access".to_vec());
        assert!(super::validate_attempt_update(&current, &replaced_access).is_err());
    }

    #[test]
    fn joiner_target_access_is_filled_once_and_then_immutable() {
        let attempt_id = SpaceJoinRecordId::from_bytes([0xc0; 32]);
        let mut initiated =
            SpaceJoinRecord::new_joiner(attempt_id, [0xc1; 16], JoinerAdmissionStage::Initiated);
        initiated.local_join_ordinal = Some(0);
        initiated.candidate_key_package = Some(b"key-package".to_vec());

        let mut prepared = prepared_joiner(attempt_id);
        prepared.record_version = 1;
        prepared.join_id = initiated.join_id;
        prepared.local_join_ordinal = initiated.local_join_ordinal;
        prepared.candidate_key_package = initiated.candidate_key_package.clone();

        assert!(super::validate_attempt_update(&initiated, &prepared).is_ok());

        let mut replay = prepared.clone();
        replay.record_version += 1;
        assert!(super::validate_attempt_update(&prepared, &replay).is_ok());

        let mut replaced = replay.clone();
        replaced.record_version += 1;
        replaced.target_access_state = Some(b"different-target-access".to_vec());
        assert!(super::validate_attempt_update(&replay, &replaced).is_err());
    }

    #[test]
    fn cross_space_terminal_result_cannot_be_missing_or_replaced() {
        let attempt_id = SpaceJoinRecordId::from_bytes([0xba; 32]);
        let transition =
            cross_space_transition(attempt_id, CrossSpaceTransitionPhaseV2::CleanupPending);
        let mut active = prepared_joiner(attempt_id);
        active.role_state = SpaceJoinRoleState::Joiner(uc_core::membership::JoinerAdmissionState {
            stage: JoinerAdmissionStage::Completed,
        });
        active.activation_receipt = Some(b"activation-receipt".to_vec());
        active.completion = Some(b"completion".to_vec());
        active.terminal_result = Some(AdmissionTerminalResult::Active);
        active.space_transition =
            AdmissionSpaceTransitionV2::CrossSpace(transition.clone()).encode();
        assert!(super::validate_attempt(&active).is_err());

        let result = CrossSpaceTransitionResultV2::from_cleanup_pending(&transition).unwrap();
        active.space_transition_result =
            AdmissionSpaceTransitionResultV2::CrossSpace(result.clone()).encode();
        assert!(super::validate_attempt(&active).is_ok());

        let mut replaced = active.clone();
        replaced.record_version += 1;
        let mut changed_result = result;
        changed_result.migrated_records += 1;
        replaced.space_transition_result =
            AdmissionSpaceTransitionResultV2::CrossSpace(changed_result).encode();
        assert!(super::validate_attempt_update(&active, &replaced).is_err());
    }

    #[tokio::test]
    async fn attempt_and_outbox_advance_atomically_and_survive_restart() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0x11; 16];
        let attempt_id = SpaceJoinRecordId::from_bytes([0x22; 32]);
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );

        let mut initiated =
            SpaceJoinRecord::new_joiner(attempt_id, [0x33; 16], JoinerAdmissionStage::Initiated);
        initiated.local_join_ordinal = Some(0);
        store.create(&initiated, None, None).await.unwrap();

        let mut prepared = initiated.clone();
        prepared.record_version = 1;
        assert!(prepared.set_joiner_stage(JoinerAdmissionStage::Prepared));
        prepared.lineage_id = Some("target-space-private".to_owned());
        prepared.base_history_position = Some(b"base-history-private".to_vec());
        prepared.candidate_event = Some(b"candidate-event-private".to_vec());
        prepared.candidate_event_id = Some([0x46; 32]);
        prepared.candidate_key_package = Some(b"candidate-key-package-private".to_vec());
        prepared.target_members_digest = Some([0x45; 32]);
        prepared.security_commitment = Some(b"public-security-commitment".to_vec());
        prepared.security_commit = Some(b"security-commit".to_vec());
        prepared.security_welcome = Some(b"security-welcome-private".to_vec());
        prepared.target_protection_group_id = Some("target-protection-group-private".to_owned());
        prepared.target_key_catalog = Some(b"target-key-catalog-private".to_vec());
        prepared.target_relationships = Some(Vec::new());
        prepared.existing_member_security_deliveries = Some(Vec::new());
        prepared.staged_security_state = Some(b"staged-mls-private-state".to_vec());
        prepared.base_membership_history = Some(b"base-history-private".to_vec());
        prepared.verified_membership_history = Some(b"verified-history-private".to_vec());
        prepared.prepared_proof = Some(b"prepared-proof-private".to_vec());
        prepared.outboxes.push(AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::Prepared,
            recipient: b"sponsor-private-identity".to_vec(),
            message_id: [0x44; 32],
            predecessor_message_id: None,
            payload: b"prepared-private-payload".to_vec(),
            superseded: false,
        });
        store
            .compare_and_advance(attempt_id, 0, &prepared)
            .await
            .unwrap();

        let reopened = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(secure_storage, generation),
        );
        assert_eq!(reopened.load(attempt_id).await.unwrap(), Some(prepared));
    }

    #[tokio::test]
    async fn completion_recovery_challenge_is_durable_before_helper_creation() {
        let directory = tempdir().unwrap();
        let database_path = directory
            .path()
            .join("completion-recovery-challenge.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0xd1; 16];
        let attempt_id = SpaceJoinRecordId::from_bytes([0xd2; 32]);
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );
        let challenge = b"signed-completion-recovery-challenge";

        store
            .save_completion_recovery_challenge(attempt_id, challenge)
            .await
            .unwrap();

        let reopened = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(secure_storage, generation),
        );
        assert_eq!(
            reopened
                .load_completion_recovery_challenge(attempt_id)
                .await
                .unwrap()
                .as_deref(),
            Some(challenge.as_slice())
        );
        assert_eq!(reopened.load(attempt_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn completion_helper_creation_requires_the_exact_saved_challenge() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("completion-helper.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xd6; 16]),
        );
        let attempt_id = SpaceJoinRecordId::from_bytes([0xd7; 32]);
        let challenge = b"saved-challenge";
        store
            .save_completion_recovery_challenge(attempt_id, challenge)
            .await
            .unwrap();
        let helper = applied_completion_helper(attempt_id);

        assert!(store
            .create_completion_helper(&helper, b"different-challenge")
            .await
            .is_err());
        store
            .create_completion_helper(&helper, challenge)
            .await
            .unwrap();

        assert_eq!(store.load(attempt_id).await.unwrap(), Some(helper));
        assert_eq!(
            store
                .load_completion_recovery_challenge(attempt_id)
                .await
                .unwrap()
                .as_deref(),
            Some(challenge.as_slice())
        );
    }

    #[tokio::test]
    async fn attempt_and_membership_history_roll_back_together_on_write_failure() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-history-atomic.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0x23; 16];
        let attempt_id = SpaceJoinRecordId::from_bytes([0x24; 32]);
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );
        let mut initiated =
            SpaceJoinRecord::new_joiner(attempt_id, [0x25; 16], JoinerAdmissionStage::Initiated);
        initiated.local_join_ordinal = Some(0);
        store
            .create(&initiated, None, Some(b"base-membership-history"))
            .await
            .unwrap();

        {
            let mut connection = pool.get().unwrap();
            diesel::sql_query(
                "CREATE TRIGGER fail_admission_history_advance \
                 BEFORE UPDATE ON admission_repository_state \
                 BEGIN SELECT RAISE(FAIL, 'forced admission history failure'); END",
            )
            .execute(&mut connection)
            .unwrap();
        }
        let mut next = initiated.clone();
        next.record_version = 1;
        next.outboxes.push(AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::JoinRequest,
            recipient: b"sponsor".to_vec(),
            message_id: [0x26; 32],
            predecessor_message_id: None,
            payload: b"request".to_vec(),
            superseded: false,
        });
        assert!(store
            .compare_and_advance_with_membership_history_v2(
                attempt_id,
                0,
                &next,
                Some(b"base-membership-history"),
                b"advanced-membership-history",
            )
            .await
            .is_err());
        {
            let mut connection = pool.get().unwrap();
            diesel::sql_query("DROP TRIGGER fail_admission_history_advance")
                .execute(&mut connection)
                .unwrap();
        }

        let reopened = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(secure_storage, generation),
        );
        assert_eq!(reopened.load(attempt_id).await.unwrap(), Some(initiated));
        assert_eq!(
            reopened.load_membership_history().await.unwrap(),
            Some(b"base-membership-history".to_vec())
        );
    }

    #[tokio::test]
    async fn a_second_non_terminal_attempt_cannot_take_the_profile_slot() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-slot.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x51; 16]),
        );
        let mut first = SpaceJoinRecord::new_joiner(
            SpaceJoinRecordId::from_bytes([0x52; 32]),
            [0x53; 16],
            JoinerAdmissionStage::Initiated,
        );
        first.local_join_ordinal = Some(0);
        store.create(&first, None, None).await.unwrap();

        let mut second = SpaceJoinRecord::new_joiner(
            SpaceJoinRecordId::from_bytes([0x54; 32]),
            [0x55; 16],
            JoinerAdmissionStage::Initiated,
        );
        second.local_join_ordinal = Some(1);
        assert!(store.create(&second, None, None).await.is_err());
        let metadata = store.profile_metadata().await.unwrap();
        assert_eq!(metadata.next_local_join_ordinal, 1);
        assert_eq!(metadata.device_trust_revision, 1);
    }

    #[tokio::test]
    async fn prepared_stage_is_rejected_when_its_recovery_material_is_incomplete() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-incomplete.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x61; 16]),
        );
        let attempt_id = SpaceJoinRecordId::from_bytes([0x62; 32]);
        let mut initiated =
            SpaceJoinRecord::new_joiner(attempt_id, [0x63; 16], JoinerAdmissionStage::Initiated);
        initiated.local_join_ordinal = Some(0);
        store.create(&initiated, None, None).await.unwrap();

        let mut incomplete = initiated;
        incomplete.record_version = 1;
        assert!(incomplete.set_joiner_stage(JoinerAdmissionStage::Prepared));
        assert!(store
            .compare_and_advance(attempt_id, 0, &incomplete)
            .await
            .is_err());
        assert_eq!(
            store
                .load(attempt_id)
                .await
                .unwrap()
                .unwrap()
                .record_version,
            0
        );
    }

    #[tokio::test]
    async fn terminal_compaction_preserves_replay_result_and_reset_only_advances_the_floor() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-terminal.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0x71; 16];
        let attempt_id = SpaceJoinRecordId::from_bytes([0x72; 32]);
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );
        let mut initiated =
            SpaceJoinRecord::new_joiner(attempt_id, [0x73; 16], JoinerAdmissionStage::Initiated);
        initiated.local_join_ordinal = Some(0);
        store.create(&initiated, None, None).await.unwrap();

        let mut rejected = initiated;
        rejected.record_version = 1;
        assert!(rejected.set_joiner_stage(JoinerAdmissionStage::Rejected));
        rejected.terminal_result = Some(uc_core::membership::AdmissionTerminalResult::Rejected);
        rejected.rejection_reason = Some(uc_core::membership::AdmissionRejectionReason::Cancelled);
        rejected.cancel_outcome = Some(b"cancelled-before-commit".to_vec());
        store
            .compare_and_advance(attempt_id, 0, &rejected)
            .await
            .unwrap();
        let terminal = store.compact_terminal(attempt_id, 1).await.unwrap();
        assert!(terminal.identity_binding.is_none());
        assert!(store.load(attempt_id).await.unwrap().is_none());
        assert_eq!(
            store.load_terminal(attempt_id).await.unwrap(),
            Some(terminal.clone())
        );

        let metadata = store.advance_projection_floor(2).await.unwrap();
        assert_eq!(metadata.next_local_join_ordinal, 1);
        assert_eq!(metadata.join_projection_floor_ordinal, 1);
        assert_eq!(metadata.device_trust_revision, 3);

        let reopened = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(secure_storage, generation),
        );
        assert_eq!(
            reopened.load_terminal(attempt_id).await.unwrap(),
            Some(terminal)
        );
    }

    #[tokio::test]
    async fn terminal_updates_are_monotonic_and_preserve_delivery_records() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-terminal-update.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xa1; 16]),
        );
        let attempt_id = SpaceJoinRecordId::from_bytes([0xa2; 32]);
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [0xa3; 16], JoinerAdmissionStage::Initiated);
        attempt.local_join_ordinal = Some(0);
        store.create(&attempt, None, None).await.unwrap();

        let first_inbox = AdmissionInboxRecord {
            message_id: [0xa4; 32],
            payload_digest: [0xa5; 32],
            acknowledgment_payload: b"first-ack".to_vec(),
        };
        let second_inbox = AdmissionInboxRecord {
            message_id: [0xa6; 32],
            payload_digest: [0xa7; 32],
            acknowledgment_payload: b"second-ack".to_vec(),
        };
        let mut terminal = attempt;
        terminal.record_version = 1;
        assert!(terminal.set_joiner_stage(JoinerAdmissionStage::Rejected));
        terminal.terminal_result = Some(AdmissionTerminalResult::Rejected);
        terminal.rejection_reason = Some(uc_core::membership::AdmissionRejectionReason::Cancelled);
        terminal.identity_binding = Some(b"terminal-binding".to_vec());
        terminal.inbox_dedup.push(first_inbox.clone());
        terminal.outboxes.extend([
            AdmissionOutboxMessage {
                purpose: AdmissionOutboxPurpose::JoinRequest,
                recipient: b"sponsor".to_vec(),
                message_id: [0xa8; 32],
                predecessor_message_id: None,
                payload: b"join-request".to_vec(),
                superseded: true,
            },
            AdmissionOutboxMessage {
                purpose: AdmissionOutboxPurpose::Rejected,
                recipient: b"joiner".to_vec(),
                message_id: [0xa9; 32],
                predecessor_message_id: Some([0xa8; 32]),
                payload: b"rejected".to_vec(),
                superseded: false,
            },
        ]);
        store
            .compare_and_advance(attempt_id, 0, &terminal)
            .await
            .unwrap();

        let mut reactivated = terminal.clone();
        reactivated.record_version = 2;
        reactivated.outboxes[0].superseded = false;
        assert!(store
            .compare_and_advance(attempt_id, 1, &reactivated)
            .await
            .is_err());

        let mut altered = terminal.clone();
        altered.record_version = 2;
        altered.outboxes[1].recipient = b"other-device".to_vec();
        assert!(store
            .compare_and_advance(attempt_id, 1, &altered)
            .await
            .is_err());

        let mut removed_inbox = terminal.clone();
        removed_inbox.record_version = 2;
        removed_inbox.inbox_dedup.clear();
        assert!(store
            .compare_and_advance(attempt_id, 1, &removed_inbox)
            .await
            .is_err());

        let mut acknowledged = terminal.clone();
        acknowledged.record_version = 2;
        acknowledged.outboxes[1].superseded = true;
        acknowledged.inbox_dedup.push(second_inbox);
        store
            .compare_and_advance(attempt_id, 1, &acknowledged)
            .await
            .unwrap();
        assert_eq!(store.load(attempt_id).await.unwrap(), Some(acknowledged));
    }

    #[tokio::test]
    async fn current_local_join_projection_uses_pending_then_latest_visible_ordinal() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-projection.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x74; 16]),
        );
        assert!(store.project_current_local_join().await.unwrap().is_none());

        let first_id = SpaceJoinRecordId::from_bytes([0x75; 32]);
        let mut first =
            SpaceJoinRecord::new_joiner(first_id, [0x76; 16], JoinerAdmissionStage::Initiated);
        first.local_join_ordinal = Some(0);
        store.create(&first, None, None).await.unwrap();
        let pending = store.project_current_local_join().await.unwrap().unwrap();
        assert_eq!(pending.attempt_id, first_id);
        assert_eq!(pending.local_join_ordinal, 0);
        assert_eq!(pending.terminal_result, None);

        let mut rejected = first;
        rejected.record_version = 1;
        assert!(rejected.set_joiner_stage(JoinerAdmissionStage::Rejected));
        rejected.terminal_result = Some(AdmissionTerminalResult::Rejected);
        rejected.rejection_reason = Some(uc_core::membership::AdmissionRejectionReason::Cancelled);
        rejected.identity_binding = Some(b"first-binding".to_vec());
        store
            .compare_and_advance(first_id, 0, &rejected)
            .await
            .unwrap();
        store.compact_terminal(first_id, 1).await.unwrap();
        let first_terminal = store.project_current_local_join().await.unwrap().unwrap();
        assert_eq!(first_terminal.attempt_id, first_id);
        assert_eq!(
            first_terminal.terminal_result,
            Some(AdmissionTerminalResult::Rejected)
        );

        let second_id = SpaceJoinRecordId::from_bytes([0x77; 32]);
        let mut second =
            SpaceJoinRecord::new_joiner(second_id, [0x78; 16], JoinerAdmissionStage::Initiated);
        second.local_join_ordinal = Some(1);
        store.create(&second, None, None).await.unwrap();
        assert_eq!(
            store
                .project_current_local_join()
                .await
                .unwrap()
                .unwrap()
                .attempt_id,
            second_id
        );

        let mut second_rejected = second;
        second_rejected.record_version = 1;
        assert!(second_rejected.set_joiner_stage(JoinerAdmissionStage::Rejected));
        second_rejected.terminal_result = Some(AdmissionTerminalResult::Rejected);
        second_rejected.rejection_reason =
            Some(uc_core::membership::AdmissionRejectionReason::IdentityConflict);
        second_rejected.identity_binding = Some(b"second-binding".to_vec());
        store
            .compare_and_advance(second_id, 0, &second_rejected)
            .await
            .unwrap();
        store.compact_terminal(second_id, 1).await.unwrap();
        assert_eq!(
            store
                .project_current_local_join()
                .await
                .unwrap()
                .unwrap()
                .attempt_id,
            second_id
        );
        let revision = store
            .profile_metadata()
            .await
            .unwrap()
            .device_trust_revision;
        store.advance_projection_floor(revision).await.unwrap();
        assert!(store.project_current_local_join().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn compacted_join_id_cannot_be_reused_by_another_attempt() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-join-id.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x81; 16]),
        );
        let join_id = [0x82; 16];
        let first_id = SpaceJoinRecordId::from_bytes([0x83; 32]);
        let mut first =
            SpaceJoinRecord::new_joiner(first_id, join_id, JoinerAdmissionStage::Initiated);
        first.local_join_ordinal = Some(0);
        store.create(&first, None, None).await.unwrap();
        let mut rejected = first;
        rejected.record_version = 1;
        assert!(rejected.set_joiner_stage(JoinerAdmissionStage::Rejected));
        rejected.terminal_result = Some(uc_core::membership::AdmissionTerminalResult::Rejected);
        rejected.rejection_reason = Some(uc_core::membership::AdmissionRejectionReason::Cancelled);
        rejected.identity_binding = Some(b"joiner-and-sponsor-binding".to_vec());
        store
            .compare_and_advance(first_id, 0, &rejected)
            .await
            .unwrap();
        store.compact_terminal(first_id, 1).await.unwrap();

        let mut replay = SpaceJoinRecord::new_joiner(
            SpaceJoinRecordId::from_bytes([0x84; 32]),
            join_id,
            JoinerAdmissionStage::Initiated,
        );
        replay.local_join_ordinal = Some(1);
        assert!(store.create(&replay, None, None).await.is_err());
        assert_eq!(
            store
                .profile_metadata()
                .await
                .unwrap()
                .next_local_join_ordinal,
            1
        );
    }

    #[tokio::test]
    async fn admission_identity_security_state_and_messages_never_reach_sqlite_in_plaintext() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-encrypted.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x91; 16]),
        );
        let mut attempt = SpaceJoinRecord::new_joiner(
            SpaceJoinRecordId::from_bytes([0x92; 32]),
            [0x93; 16],
            JoinerAdmissionStage::Initiated,
        );
        attempt.local_join_ordinal = Some(0);
        attempt.resume_private_key = Some(b"private-resume-key-marker".to_vec());
        attempt.invitation_claim = Some(b"private-invitation-marker".to_vec());
        attempt.target_access_state = Some(b"private-target-access-marker".to_vec());
        attempt.outboxes.push(AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::JoinRequest,
            recipient: b"private-recipient-marker".to_vec(),
            message_id: [0x94; 32],
            predecessor_message_id: None,
            payload: b"private-message-marker".to_vec(),
            superseded: false,
        });
        store.create(&attempt, None, None).await.unwrap();

        let markers: [&[u8]; 5] = [
            b"private-resume-key-marker",
            b"private-invitation-marker",
            b"private-recipient-marker",
            b"private-message-marker",
            b"private-target-access-marker",
        ];
        let files = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        assert!(files
            .iter()
            .any(|path| path.ends_with("admission-encrypted.sqlite")));
        for path in files {
            let bytes = fs::read(path).unwrap();
            for marker in markers {
                assert!(!bytes.windows(marker.len()).any(|window| window == marker));
            }
        }
    }

    #[tokio::test]
    async fn superseded_terminal_and_cleanup_never_reach_sqlite_files_in_plaintext() {
        let directory = tempdir().unwrap();
        let database_path = directory
            .path()
            .join("admission-superseded-encrypted.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselSpaceJoinRecordStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xf7; 16]),
        );
        let previous_id = SpaceJoinRecordId::from_bytes([0xf8; 32]);
        let mut previous = supersedable_joiner(previous_id, [0xf9; 16], 0);
        previous.joiner_pending_security_state = Some(b"old-private-state-marker".to_vec());
        previous.candidate_key_package = Some(b"old-key-package-marker".to_vec());
        previous.outboxes[0].recipient = b"old-private-recipient-marker".to_vec();
        previous.outboxes[0].payload = b"old-private-request-marker".to_vec();
        store
            .commit_local_join_start(LocalJoinStartMutation::Create {
                replacement: previous.clone(),
            })
            .await
            .unwrap();

        let cleanup = AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::CancelRequested,
            recipient: b"old-private-recipient-marker".to_vec(),
            message_id: [0xfa; 32],
            predecessor_message_id: Some([7; 32]),
            payload: b"private-cleanup-payload-marker".to_vec(),
            superseded: false,
        };
        let mut previous_terminal = previous.superseded_by_new_join(cleanup).unwrap();
        previous_terminal.record_version = 1;
        let replacement_id = SpaceJoinRecordId::from_bytes([0xfb; 32]);
        let mut replacement = supersedable_joiner(replacement_id, [0xfc; 16], 1);
        replacement.joiner_pending_security_state = Some(b"new-private-state-marker".to_vec());
        replacement.candidate_key_package = Some(b"new-key-package-marker".to_vec());
        replacement.outboxes[0].recipient = b"new-private-recipient-marker".to_vec();
        replacement.outboxes[0].payload = b"new-private-request-marker".to_vec();
        store
            .commit_local_join_start(LocalJoinStartMutation::Supersede {
                expected_previous_attempt_id: previous_id,
                expected_previous_record_version: 0,
                previous_terminal,
                replacement,
            })
            .await
            .unwrap();

        let _open_connection = pool.get().unwrap();
        let markers: [&[u8]; 9] = [
            b"old-private-state-marker",
            b"old-key-package-marker",
            b"old-private-recipient-marker",
            b"old-private-request-marker",
            b"private-cleanup-payload-marker",
            b"new-private-state-marker",
            b"new-key-package-marker",
            b"new-private-recipient-marker",
            b"new-private-request-marker",
        ];
        let files = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        for expected in [
            "admission-superseded-encrypted.sqlite",
            "admission-superseded-encrypted.sqlite-wal",
            "admission-superseded-encrypted.sqlite-shm",
        ] {
            assert!(files.iter().any(|path| path.ends_with(expected)));
        }
        for path in files {
            let bytes = fs::read(path).unwrap();
            for marker in markers {
                assert!(!bytes.windows(marker.len()).any(|window| window == marker));
            }
        }
    }
}
