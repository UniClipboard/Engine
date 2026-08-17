use std::collections::BTreeMap;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use serde::{Deserialize, Serialize};
use uc_core::membership::{
    AdmissionAttemptId, AdmissionAttemptRepositoryError, AdmissionAttemptRepositoryPort,
    AdmissionAttemptRoleStateV1, AdmissionAttemptV1, AdmissionProfileMetadataV1,
    AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2, AdmissionTerminalResultV1,
    CompletionHelperAdmissionStageV1, CurrentLocalJoinProjectionV1, JoinerAdmissionStageV1,
    SponsorAdmissionStageV1, TerminalAdmissionAttemptV1, TERMINAL_ADMISSION_ATTEMPT_FORMAT_V1,
};

use crate::db::ports::DbExecutor;
use crate::security::{AdmissionKeyManager, WrappedAdmissionAttemptDataKey};

const REPOSITORY_FORMAT_V1: u16 = 1;
const REPOSITORY_PAYLOAD_PURPOSE: &[u8] = b"admission-repository-state";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredAdmissionAttemptV1 {
    wrapped_data_key: WrappedAdmissionAttemptDataKey,
    encrypted_payload: Vec<u8>,
    consumed_invitation_digest: Option<[u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AdmissionRepositoryStateV1 {
    format_version: u16,
    metadata: AdmissionProfileMetadataV1,
    attempts: BTreeMap<AdmissionAttemptId, StoredAdmissionAttemptV1>,
    terminals: BTreeMap<AdmissionAttemptId, TerminalAdmissionAttemptV1>,
    #[serde(default)]
    membership_history_v2: Option<Vec<u8>>,
}

impl AdmissionRepositoryStateV1 {
    fn fresh(profile_generation: [u8; 16]) -> Self {
        Self {
            format_version: REPOSITORY_FORMAT_V1,
            metadata: AdmissionProfileMetadataV1::fresh(profile_generation),
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

pub struct DieselAdmissionAttemptStore<E> {
    executor: E,
    keys: AdmissionKeyManager,
}

impl<E> DieselAdmissionAttemptStore<E> {
    pub fn new(executor: E, keys: AdmissionKeyManager) -> Self {
        Self { executor, keys }
    }
}

fn map_key_error(error: crate::security::AdmissionKeyError) -> AdmissionAttemptRepositoryError {
    match error {
        crate::security::AdmissionKeyError::SecureStorage => {
            AdmissionAttemptRepositoryError::Locked
        }
        crate::security::AdmissionKeyError::Corrupt
        | crate::security::AdmissionKeyError::OpenFailed => {
            AdmissionAttemptRepositoryError::Corrupt
        }
    }
}

fn repository_error(error: impl std::fmt::Display) -> AdmissionAttemptRepositoryError {
    AdmissionAttemptRepositoryError::Repository(error.to_string())
}

fn executor_error(error: anyhow::Error) -> AdmissionAttemptRepositoryError {
    error
        .downcast_ref::<AdmissionAttemptRepositoryError>()
        .cloned()
        .unwrap_or_else(|| repository_error(error))
}

fn decode_space_transition(
    encoded: Option<&[u8]>,
) -> Result<Option<AdmissionSpaceTransitionV2>, AdmissionAttemptRepositoryError> {
    encoded
        .map(|bytes| {
            AdmissionSpaceTransitionV2::decode(bytes)
                .ok_or(AdmissionAttemptRepositoryError::Corrupt)
        })
        .transpose()
}

fn decode_space_transition_result(
    encoded: Option<&[u8]>,
) -> Result<Option<AdmissionSpaceTransitionResultV2>, AdmissionAttemptRepositoryError> {
    encoded
        .map(|bytes| {
            AdmissionSpaceTransitionResultV2::decode(bytes)
                .ok_or(AdmissionAttemptRepositoryError::Corrupt)
        })
        .transpose()
}

impl<E: DbExecutor> DieselAdmissionAttemptStore<E> {
    fn load_state_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<AdmissionRepositoryStateV1, AdmissionAttemptRepositoryError> {
        let row = sql_query(
            "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
        )
        .get_result::<EncryptedRepositoryRow>(conn)
        .optional()
        .map_err(repository_error)?;
        let Some(row) = row else {
            return Ok(AdmissionRepositoryStateV1::fresh(
                self.keys.profile_generation(),
            ));
        };
        let plaintext = self
            .keys
            .open_profile_payload(REPOSITORY_PAYLOAD_PURPOSE, &row.encrypted_payload)
            .map_err(map_key_error)?;
        let state: AdmissionRepositoryStateV1 = postcard::from_bytes(&plaintext)
            .map_err(|_| AdmissionAttemptRepositoryError::Corrupt)?;
        if state.format_version != REPOSITORY_FORMAT_V1
            || state.metadata.profile_generation != self.keys.profile_generation()
            || state.metadata.join_projection_floor_ordinal > state.metadata.next_local_join_ordinal
        {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
        Ok(state)
    }

    fn save_state_on(
        &self,
        conn: &mut SqliteConnection,
        state: &AdmissionRepositoryStateV1,
    ) -> Result<(), AdmissionAttemptRepositoryError> {
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
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
        Ok(())
    }

    fn open_attempt(
        &self,
        attempt_id: AdmissionAttemptId,
        stored: &StoredAdmissionAttemptV1,
    ) -> Result<AdmissionAttemptV1, AdmissionAttemptRepositoryError> {
        let plaintext = self
            .keys
            .open_attempt_payload(
                *attempt_id.as_bytes(),
                &stored.wrapped_data_key,
                &stored.encrypted_payload,
            )
            .map_err(map_key_error)?;
        let attempt: AdmissionAttemptV1 = postcard::from_bytes(&plaintext)
            .map_err(|_| AdmissionAttemptRepositoryError::Corrupt)?;
        validate_attempt(&attempt)?;
        if attempt.attempt_id != attempt_id {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
        Ok(attempt)
    }

    fn seal_attempt(
        &self,
        attempt: &AdmissionAttemptV1,
        wrapped: WrappedAdmissionAttemptDataKey,
        consumed_invitation_digest: Option<[u8; 32]>,
    ) -> Result<StoredAdmissionAttemptV1, AdmissionAttemptRepositoryError> {
        validate_attempt(attempt)?;
        let plaintext = postcard::to_stdvec(attempt).map_err(repository_error)?;
        let encrypted_payload = self
            .keys
            .seal_attempt_payload(*attempt.attempt_id.as_bytes(), &wrapped, &plaintext)
            .map_err(map_key_error)?;
        Ok(StoredAdmissionAttemptV1 {
            wrapped_data_key: wrapped,
            encrypted_payload,
            consumed_invitation_digest,
        })
    }
}

fn validate_attempt(attempt: &AdmissionAttemptV1) -> Result<(), AdmissionAttemptRepositoryError> {
    if attempt.format_version != uc_core::membership::ADMISSION_ATTEMPT_FORMAT_V1
        || attempt.stage_rank().is_none()
    {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    let joiner = attempt.is_joiner();
    let helper = matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::CompletionHelper(_)
    );
    if joiner != attempt.join_id.is_some() || joiner != attempt.local_join_ordinal.is_some() {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    let rejected = matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(uc_core::membership::SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        }) | AdmissionAttemptRoleStateV1::Joiner(uc_core::membership::JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Rejected,
        })
    );
    let completed = matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(uc_core::membership::SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Completed,
        }) | AdmissionAttemptRoleStateV1::Joiner(uc_core::membership::JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Completed,
        }) | AdmissionAttemptRoleStateV1::CompletionHelper(
            uc_core::membership::CompletionHelperAdmissionStateV1 {
                stage: CompletionHelperAdmissionStageV1::Completed,
            }
        )
    );
    if rejected {
        if attempt.terminal_result != Some(AdmissionTerminalResultV1::Rejected)
            || attempt.rejection_reason.is_none()
        {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
    } else if completed {
        if attempt.terminal_result.is_none() || attempt.completion.is_none() {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
    } else if attempt.terminal_result.is_some() || attempt.rejection_reason.is_some() {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    let rank = attempt
        .stage_rank()
        .ok_or(AdmissionAttemptRepositoryError::Corrupt)?;
    let transition = decode_space_transition(attempt.space_transition.as_deref())?;
    let transition_result =
        decode_space_transition_result(attempt.space_transition_result.as_deref())?;
    if transition.as_ref().is_some_and(|transition| {
        !joiner
            || rank < 3
            || transition.attempt_id() != attempt.attempt_id
            || attempt.target_access_state.is_none()
    }) {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if let Some(result) = &transition_result {
        let valid_terminal = joiner
            && attempt.terminal_result == Some(AdmissionTerminalResultV1::Active)
            && transition
                .as_ref()
                .is_some_and(|transition| result.matches_cleanup_pending(transition));
        if !valid_terminal {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
    } else if attempt.terminal_result == Some(AdmissionTerminalResultV1::Active)
        && transition.is_some()
    {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if rank >= 2
        && !rejected
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
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if rank >= 2 && !rejected && attempt.base_membership_history.is_none() {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if rank >= 3 && !rejected && !helper && attempt.prepared_proof.is_none() {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if joiner && rank >= 3 && !rejected && attempt.verified_membership_history.is_none() {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if rank >= 5 && !rejected && attempt.activation_receipt.is_none() {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(uc_core::membership::SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Accepted,
        })
    ) && attempt.invitation_claim.is_none()
    {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    let mut outbox_keys = std::collections::BTreeSet::new();
    if attempt.outboxes.iter().any(|message| {
        !outbox_keys.insert((
            message.purpose,
            message.recipient.clone(),
            message.message_id,
        ))
    }) {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    Ok(())
}

fn validate_attempt_update(
    current: &AdmissionAttemptV1,
    next: &AdmissionAttemptV1,
) -> Result<(), AdmissionAttemptRepositoryError> {
    validate_attempt(current)?;
    validate_attempt(next)?;
    let current_transition = decode_space_transition(current.space_transition.as_deref())?;
    let next_transition = decode_space_transition(next.space_transition.as_deref())?;
    let transition_is_valid = match (&current_transition, &next_transition) {
        (None, None) => true,
        (None, Some(next)) => next.is_initial(),
        (Some(current), Some(next)) => current == next || current.can_advance_to(next),
        (Some(current), None) => {
            next.terminal_result == Some(AdmissionTerminalResultV1::Rejected)
                && current.phase_rank() < current.activation_started_rank()
                && next.space_transition_result.is_none()
        }
    };
    if !transition_is_valid {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if current.space_transition_result.is_some()
        && current.space_transition_result != next.space_transition_result
    {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    let access_is_filled_during_joiner_preparation = current.target_access_state.is_none()
        && next.target_access_state.is_some()
        && matches!(
            current.role_state,
            AdmissionAttemptRoleStateV1::Joiner(uc_core::membership::JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Initiated,
            })
        )
        && matches!(
            next.role_state,
            AdmissionAttemptRoleStateV1::Joiner(uc_core::membership::JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Prepared,
            })
        );
    let access_is_valid = current.target_access_state == next.target_access_state
        || access_is_filled_during_joiner_preparation
        || (current_transition.is_some()
            && next_transition.is_none()
            && next.terminal_result == Some(AdmissionTerminalResultV1::Rejected)
            && next.target_access_state.is_none());
    if !access_is_valid {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    if current.identity_binding.is_some() && current.identity_binding != next.identity_binding {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    Ok(())
}

fn validate_terminal_delivery_update(
    current: &AdmissionAttemptV1,
    next: &AdmissionAttemptV1,
) -> Result<(), AdmissionAttemptRepositoryError> {
    if next.cleanup_pending != current.cleanup_pending
        || !next.inbox_dedup.starts_with(&current.inbox_dedup)
        || next.outboxes.len() < current.outboxes.len()
    {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    for (offset, record) in next.inbox_dedup[current.inbox_dedup.len()..]
        .iter()
        .enumerate()
    {
        if next.inbox_dedup[..current.inbox_dedup.len() + offset].contains(record) {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
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
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    let complete_id = current
        .outboxes
        .iter()
        .find(|message| message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::Complete)
        .map(|message| message.message_id);
    let appended = &next.outboxes[current.outboxes.len()..];
    if (!appended.is_empty() && complete_id.is_none())
        || appended.iter().any(|message| {
            !matches!(
                message.purpose,
                uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                    | uc_core::membership::AdmissionOutboxPurposeV1::HistoryOrReceiptBatch
            ) || message.predecessor_message_id != complete_id
                || message.superseded
        })
    {
        return Err(AdmissionAttemptRepositoryError::Corrupt);
    }
    Ok(())
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> AdmissionAttemptRepositoryPort
    for DieselAdmissionAttemptStore<E>
{
    async fn create(
        &self,
        attempt: &AdmissionAttemptV1,
        consumed_invitation_digest: Option<[u8; 32]>,
        initial_membership_history_v2: Option<&[u8]>,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        if initial_membership_history_v2.is_some_and(|history| history.is_empty()) {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
        }
        let attempt = attempt.clone();
        let initial_membership_history_v2 = initial_membership_history_v2.map(ToOwned::to_owned);
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.attempts.contains_key(&attempt.attempt_id)
                        || state.terminals.contains_key(&attempt.attempt_id)
                    {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::AlreadyExists
                        ));
                    }
                    if attempt.join_id.is_some()
                        && state
                            .terminals
                            .values()
                            .any(|terminal| terminal.join_id == attempt.join_id)
                    {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::AlreadyExists
                        ));
                    }
                    for (persisted_id, persisted) in &state.attempts {
                        let persisted = self
                            .open_attempt(*persisted_id, persisted)
                            .map_err(|error| anyhow::anyhow!(error))?;
                        if attempt.join_id.is_some() && persisted.join_id == attempt.join_id {
                            return Err(anyhow::anyhow!(
                                AdmissionAttemptRepositoryError::AlreadyExists
                            ));
                        }
                        if !persisted.is_terminal()
                            || persisted.write_ahead_recovery.is_some()
                            || persisted.cleanup_pending
                        {
                            return Err(anyhow::anyhow!(
                                AdmissionAttemptRepositoryError::VersionConflict
                            ));
                        }
                    }
                    if attempt.is_joiner() {
                        if attempt.local_join_ordinal
                            != Some(state.metadata.next_local_join_ordinal)
                        {
                            return Err(anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt));
                        }
                        state.metadata.next_local_join_ordinal = state
                            .metadata
                            .next_local_join_ordinal
                            .checked_add(1)
                            .ok_or_else(|| {
                                anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt)
                            })?;
                    }
                    if let Some(digest) = consumed_invitation_digest {
                        if state
                            .metadata
                            .consumed_invitation_attempts
                            .contains_key(&digest)
                        {
                            return Err(anyhow::anyhow!(
                                AdmissionAttemptRepositoryError::AlreadyExists
                            ));
                        }
                        state
                            .metadata
                            .consumed_invitation_attempts
                            .insert(digest, attempt.attempt_id);
                    }
                    if let Some(initial_history) = &initial_membership_history_v2 {
                        match &state.membership_history_v2 {
                            Some(current) if current != initial_history => {
                                return Err(anyhow::anyhow!(
                                    AdmissionAttemptRepositoryError::VersionConflict
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
                        .create_wrapped_attempt_key(*attempt.attempt_id.as_bytes())
                        .map_err(|error| anyhow::anyhow!(map_key_error(error)))?;
                    let stored = self
                        .seal_attempt(&attempt, wrapped, consumed_invitation_digest)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    state.attempts.insert(attempt.attempt_id, stored);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn load(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<AdmissionAttemptV1>, AdmissionAttemptRepositoryError> {
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

    async fn compare_and_advance(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
        next: &AdmissionAttemptV1,
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        let next = next.clone();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let stored = state.attempts.get(&attempt_id).cloned().ok_or_else(|| {
                        anyhow::anyhow!(AdmissionAttemptRepositoryError::NotFound)
                    })?;
                    let current = self
                        .open_attempt(attempt_id, &stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if current.record_version != expected_record_version
                        || next.record_version != expected_record_version.saturating_add(1)
                    {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
                    }
                    if next.attempt_id != attempt_id
                        || !current.same_role_as(&next)
                        || next.stage_rank() < current.stage_rank()
                    {
                        return Err(anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt));
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
                            return Err(anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt));
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
                        .ok_or_else(|| anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn compare_and_advance_with_membership_history_v2(
        &self,
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
        next: &AdmissionAttemptV1,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        if membership_history_v2.is_empty() {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
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
                    let stored = state.attempts.get(&attempt_id).cloned().ok_or_else(|| {
                        anyhow::anyhow!(AdmissionAttemptRepositoryError::NotFound)
                    })?;
                    let current = self
                        .open_attempt(attempt_id, &stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.membership_history_v2.as_deref()
                        != expected_membership_history_v2.as_deref()
                    {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
                    }
                    if current.record_version != expected_record_version
                        || next.record_version != expected_record_version.saturating_add(1)
                        || next.attempt_id != attempt_id
                        || !current.same_role_as(&next)
                        || next.stage_rank() < current.stage_rank()
                        || current.is_terminal()
                    {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
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
                        .ok_or_else(|| anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn compare_and_replace_membership_history_v2(
        &self,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        if membership_history_v2.is_empty() {
            return Err(AdmissionAttemptRepositoryError::Corrupt);
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
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
                    }
                    state.membership_history_v2 = Some(membership_history_v2);
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }

    async fn load_membership_history_v2(
        &self,
    ) -> Result<Option<Vec<u8>>, AdmissionAttemptRepositoryError> {
        self.executor
            .run(|conn| {
                self.load_state_on(conn)
                    .map(|state| state.membership_history_v2)
                    .map_err(|error| anyhow::anyhow!(error))
            })
            .map_err(executor_error)
    }

    async fn scan_recoverable(
        &self,
    ) -> Result<Vec<AdmissionAttemptV1>, AdmissionAttemptRepositoryError> {
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
        attempt_id: AdmissionAttemptId,
        expected_record_version: u64,
    ) -> Result<TerminalAdmissionAttemptV1, AdmissionAttemptRepositoryError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    let stored = state.attempts.get(&attempt_id).cloned().ok_or_else(|| {
                        anyhow::anyhow!(AdmissionAttemptRepositoryError::NotFound)
                    })?;
                    let attempt = self
                        .open_attempt(attempt_id, &stored)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if attempt.record_version != expected_record_version {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
                    }
                    let terminal_result = attempt
                        .terminal_result
                        .ok_or_else(|| anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt))?;
                    let identity_binding = attempt.identity_binding.clone();
                    if terminal_result != AdmissionTerminalResultV1::Rejected
                        && identity_binding.is_none()
                    {
                        return Err(anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt));
                    }
                    if attempt.outboxes.iter().any(|message| !message.superseded)
                        || attempt.write_ahead_recovery.is_some()
                        || attempt.cleanup_pending
                    {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
                    }
                    let terminal = TerminalAdmissionAttemptV1 {
                        format_version: TERMINAL_ADMISSION_ATTEMPT_FORMAT_V1,
                        attempt_id,
                        join_id: attempt.join_id,
                        local_join_ordinal: attempt.local_join_ordinal,
                        invitation_digest: stored.consumed_invitation_digest,
                        identity_binding,
                        terminal_result,
                        rejection_reason: attempt.rejection_reason,
                        candidate_event_id: attempt.candidate_event_id,
                        cancel_outcome: attempt.cancel_outcome,
                        replay_result: attempt.completion.unwrap_or_default(),
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
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<TerminalAdmissionAttemptV1>, AdmissionAttemptRepositoryError> {
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
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
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
    ) -> Result<Option<CurrentLocalJoinProjectionV1>, AdmissionAttemptRepositoryError> {
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
                        Some(CurrentLocalJoinProjectionV1 {
                            device_trust_revision: state.metadata.device_trust_revision,
                            attempt_id: attempt.attempt_id,
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
                        let ordinal = terminal.local_join_ordinal?;
                        if ordinal < state.metadata.join_projection_floor_ordinal {
                            return None;
                        }
                        Some(CurrentLocalJoinProjectionV1 {
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
    ) -> Result<AdmissionProfileMetadataV1, AdmissionAttemptRepositoryError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self
                        .load_state_on(conn)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    if state.metadata.device_trust_revision != expected_device_trust_revision {
                        return Err(anyhow::anyhow!(
                            AdmissionAttemptRepositoryError::VersionConflict
                        ));
                    }
                    for (attempt_id, stored) in &state.attempts {
                        let attempt = self
                            .open_attempt(*attempt_id, stored)
                            .map_err(|error| anyhow::anyhow!(error))?;
                        if attempt.has_recovery_work() {
                            return Err(anyhow::anyhow!(
                                AdmissionAttemptRepositoryError::VersionConflict
                            ));
                        }
                    }
                    state.metadata.join_projection_floor_ordinal =
                        state.metadata.next_local_join_ordinal;
                    state.metadata.device_trust_revision = state
                        .metadata
                        .device_trust_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!(AdmissionAttemptRepositoryError::Corrupt))?;
                    self.save_state_on(conn, &state)
                        .map_err(|error| anyhow::anyhow!(error))?;
                    Ok(state.metadata)
                })
            })
            .map_err(executor_error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::sync::{Arc, Mutex};

    use diesel::RunQueryDsl;
    use tempfile::tempdir;
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionAttemptRepositoryPort, AdmissionAttemptRoleStateV1,
        AdmissionAttemptV1, AdmissionInboxRecordV1, AdmissionOutboxMessageV1,
        AdmissionOutboxPurposeV1, AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2,
        AdmissionTerminalResultV1, CrossSpaceTransitionPhaseV2, CrossSpaceTransitionResultV2,
        CrossSpaceTransitionV2, JoinerAdmissionStageV1, SponsorAdmissionStageV1,
        SponsorAdmissionStateV1, CROSS_SPACE_TRANSITION_FORMAT_V2,
    };
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::init_db_pool;
    use crate::security::AdmissionKeyManager;

    #[derive(Default)]
    struct MemorySecureStorage {
        values: Mutex<HashMap<String, Vec<u8>>>,
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

    fn cross_space_transition(
        attempt_id: AdmissionAttemptId,
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

    fn prepared_joiner(attempt_id: AdmissionAttemptId) -> AdmissionAttemptV1 {
        let mut attempt = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0xb5; 16],
            JoinerAdmissionStageV1::Prepared,
        );
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

    #[test]
    fn cross_space_transition_is_joiner_bound_and_forward_only() {
        let attempt_id = AdmissionAttemptId::from_bytes([0xb8; 32]);
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
                AdmissionAttemptId::from_bytes([0xb9; 32]),
                CrossSpaceTransitionPhaseV2::TargetStaged,
            ))
            .encode();
        assert!(super::validate_attempt(&wrong_attempt).is_err());

        let mut sponsor = current.clone();
        sponsor.join_id = None;
        sponsor.local_join_ordinal = None;
        sponsor.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Prepared,
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
        let attempt_id = AdmissionAttemptId::from_bytes([0xc0; 32]);
        let mut initiated = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0xc1; 16],
            JoinerAdmissionStageV1::Initiated,
        );
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
        let attempt_id = AdmissionAttemptId::from_bytes([0xba; 32]);
        let transition =
            cross_space_transition(attempt_id, CrossSpaceTransitionPhaseV2::CleanupPending);
        let mut active = prepared_joiner(attempt_id);
        active.role_state =
            AdmissionAttemptRoleStateV1::Joiner(uc_core::membership::JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Completed,
            });
        active.activation_receipt = Some(b"activation-receipt".to_vec());
        active.completion = Some(b"completion".to_vec());
        active.terminal_result = Some(AdmissionTerminalResultV1::Active);
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
        let attempt_id = AdmissionAttemptId::from_bytes([0x22; 32]);
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );

        let mut initiated = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0x33; 16],
            JoinerAdmissionStageV1::Initiated,
        );
        initiated.local_join_ordinal = Some(0);
        store.create(&initiated, None, None).await.unwrap();

        let mut prepared = initiated.clone();
        prepared.record_version = 1;
        assert!(prepared.set_joiner_stage(JoinerAdmissionStageV1::Prepared));
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
        prepared.outboxes.push(AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::Prepared,
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

        let reopened = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(secure_storage, generation),
        );
        assert_eq!(reopened.load(attempt_id).await.unwrap(), Some(prepared));
    }

    #[tokio::test]
    async fn attempt_and_membership_history_roll_back_together_on_write_failure() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-history-atomic.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let generation = [0x23; 16];
        let attempt_id = AdmissionAttemptId::from_bytes([0x24; 32]);
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );
        let mut initiated = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0x25; 16],
            JoinerAdmissionStageV1::Initiated,
        );
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
        next.outboxes.push(AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::JoinRequest,
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

        let reopened = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(secure_storage, generation),
        );
        assert_eq!(reopened.load(attempt_id).await.unwrap(), Some(initiated));
        assert_eq!(
            reopened.load_membership_history_v2().await.unwrap(),
            Some(b"base-membership-history".to_vec())
        );
    }

    #[tokio::test]
    async fn a_second_non_terminal_attempt_cannot_take_the_profile_slot() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("admission-slot.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x51; 16]),
        );
        let mut first = AdmissionAttemptV1::new_joiner(
            AdmissionAttemptId::from_bytes([0x52; 32]),
            [0x53; 16],
            JoinerAdmissionStageV1::Initiated,
        );
        first.local_join_ordinal = Some(0);
        store.create(&first, None, None).await.unwrap();

        let mut second = AdmissionAttemptV1::new_joiner(
            AdmissionAttemptId::from_bytes([0x54; 32]),
            [0x55; 16],
            JoinerAdmissionStageV1::Initiated,
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
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x61; 16]),
        );
        let attempt_id = AdmissionAttemptId::from_bytes([0x62; 32]);
        let mut initiated = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0x63; 16],
            JoinerAdmissionStageV1::Initiated,
        );
        initiated.local_join_ordinal = Some(0);
        store.create(&initiated, None, None).await.unwrap();

        let mut incomplete = initiated;
        incomplete.record_version = 1;
        assert!(incomplete.set_joiner_stage(JoinerAdmissionStageV1::Prepared));
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
        let attempt_id = AdmissionAttemptId::from_bytes([0x72; 32]);
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            AdmissionKeyManager::new(secure_storage.clone(), generation),
        );
        let mut initiated = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0x73; 16],
            JoinerAdmissionStageV1::Initiated,
        );
        initiated.local_join_ordinal = Some(0);
        store.create(&initiated, None, None).await.unwrap();

        let mut rejected = initiated;
        rejected.record_version = 1;
        assert!(rejected.set_joiner_stage(JoinerAdmissionStageV1::Rejected));
        rejected.terminal_result = Some(uc_core::membership::AdmissionTerminalResultV1::Rejected);
        rejected.rejection_reason =
            Some(uc_core::membership::AdmissionRejectionReasonV1::Cancelled);
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

        let reopened = super::DieselAdmissionAttemptStore::new(
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
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0xa1; 16]),
        );
        let attempt_id = AdmissionAttemptId::from_bytes([0xa2; 32]);
        let mut attempt = AdmissionAttemptV1::new_joiner(
            attempt_id,
            [0xa3; 16],
            JoinerAdmissionStageV1::Initiated,
        );
        attempt.local_join_ordinal = Some(0);
        store.create(&attempt, None, None).await.unwrap();

        let first_inbox = AdmissionInboxRecordV1 {
            message_id: [0xa4; 32],
            payload_digest: [0xa5; 32],
            acknowledgment_payload: b"first-ack".to_vec(),
        };
        let second_inbox = AdmissionInboxRecordV1 {
            message_id: [0xa6; 32],
            payload_digest: [0xa7; 32],
            acknowledgment_payload: b"second-ack".to_vec(),
        };
        let mut terminal = attempt;
        terminal.record_version = 1;
        assert!(terminal.set_joiner_stage(JoinerAdmissionStageV1::Rejected));
        terminal.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        terminal.rejection_reason =
            Some(uc_core::membership::AdmissionRejectionReasonV1::Cancelled);
        terminal.identity_binding = Some(b"terminal-binding".to_vec());
        terminal.inbox_dedup.push(first_inbox.clone());
        terminal.outboxes.extend([
            AdmissionOutboxMessageV1 {
                purpose: AdmissionOutboxPurposeV1::JoinRequest,
                recipient: b"sponsor".to_vec(),
                message_id: [0xa8; 32],
                predecessor_message_id: None,
                payload: b"join-request".to_vec(),
                superseded: true,
            },
            AdmissionOutboxMessageV1 {
                purpose: AdmissionOutboxPurposeV1::Rejected,
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
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x74; 16]),
        );
        assert!(store.project_current_local_join().await.unwrap().is_none());

        let first_id = AdmissionAttemptId::from_bytes([0x75; 32]);
        let mut first =
            AdmissionAttemptV1::new_joiner(first_id, [0x76; 16], JoinerAdmissionStageV1::Initiated);
        first.local_join_ordinal = Some(0);
        store.create(&first, None, None).await.unwrap();
        let pending = store.project_current_local_join().await.unwrap().unwrap();
        assert_eq!(pending.attempt_id, first_id);
        assert_eq!(pending.local_join_ordinal, 0);
        assert_eq!(pending.terminal_result, None);

        let mut rejected = first;
        rejected.record_version = 1;
        assert!(rejected.set_joiner_stage(JoinerAdmissionStageV1::Rejected));
        rejected.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        rejected.rejection_reason =
            Some(uc_core::membership::AdmissionRejectionReasonV1::Cancelled);
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
            Some(AdmissionTerminalResultV1::Rejected)
        );

        let second_id = AdmissionAttemptId::from_bytes([0x77; 32]);
        let mut second = AdmissionAttemptV1::new_joiner(
            second_id,
            [0x78; 16],
            JoinerAdmissionStageV1::Initiated,
        );
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
        assert!(second_rejected.set_joiner_stage(JoinerAdmissionStageV1::Rejected));
        second_rejected.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        second_rejected.rejection_reason =
            Some(uc_core::membership::AdmissionRejectionReasonV1::IdentityConflict);
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
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x81; 16]),
        );
        let join_id = [0x82; 16];
        let first_id = AdmissionAttemptId::from_bytes([0x83; 32]);
        let mut first =
            AdmissionAttemptV1::new_joiner(first_id, join_id, JoinerAdmissionStageV1::Initiated);
        first.local_join_ordinal = Some(0);
        store.create(&first, None, None).await.unwrap();
        let mut rejected = first;
        rejected.record_version = 1;
        assert!(rejected.set_joiner_stage(JoinerAdmissionStageV1::Rejected));
        rejected.terminal_result = Some(uc_core::membership::AdmissionTerminalResultV1::Rejected);
        rejected.rejection_reason =
            Some(uc_core::membership::AdmissionRejectionReasonV1::Cancelled);
        rejected.identity_binding = Some(b"joiner-and-sponsor-binding".to_vec());
        store
            .compare_and_advance(first_id, 0, &rejected)
            .await
            .unwrap();
        store.compact_terminal(first_id, 1).await.unwrap();

        let mut replay = AdmissionAttemptV1::new_joiner(
            AdmissionAttemptId::from_bytes([0x84; 32]),
            join_id,
            JoinerAdmissionStageV1::Initiated,
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
        let store = super::DieselAdmissionAttemptStore::new(
            DieselSqliteExecutor::new(pool),
            AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [0x91; 16]),
        );
        let mut attempt = AdmissionAttemptV1::new_joiner(
            AdmissionAttemptId::from_bytes([0x92; 32]),
            [0x93; 16],
            JoinerAdmissionStageV1::Initiated,
        );
        attempt.local_join_ordinal = Some(0);
        attempt.resume_private_key = Some(b"private-resume-key-marker".to_vec());
        attempt.invitation_claim = Some(b"private-invitation-marker".to_vec());
        attempt.target_access_state = Some(b"private-target-access-marker".to_vec());
        attempt.outboxes.push(AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::JoinRequest,
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
}
