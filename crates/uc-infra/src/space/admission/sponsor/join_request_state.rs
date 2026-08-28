use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission, LoadedSponsorJoinRequest,
    SponsorAdmissionMutation, SponsorJoinRequestCommitToken, SponsorJoinRequestState,
    SponsorJoinRequestStateError, SponsorJoinRequestStatePort,
};
use uc_core::membership::{
    AdmissionInvitationClaim, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1,
};

use crate::db::ports::DbExecutor;

use super::super::repository::codec::{into_anyhow, map_executor_error};
use super::super::repository::token::{sponsor_existing_token, sponsor_fresh_token};
use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

const INVITATION_CLAIM_FORMAT_V1: u16 = 1;

#[derive(Serialize, Deserialize)]
struct PersistedInvitationClaimV1 {
    format_version: u16,
    admission_id: [u8; 32],
    invitation_id: [u8; 32],
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> SponsorJoinRequestStatePort for SqliteSpaceAdmissionState<E> {
    #[tracing::instrument(name = "space_admission.sponsor_state.load", skip_all, err)]
    async fn load(
        &self,
        message: &AuthenticatedSpaceAdmissionMessage,
    ) -> Result<LoadedSponsorJoinRequest, SponsorJoinRequestStateError> {
        let admission_id = *message.envelope().header().admission_id().as_bytes();
        let invitation_id =
            join_request_invitation_id(message.envelope()).map_err(map_sponsor_error)?;

        let existing = self
            .executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                state
                    .records
                    .get(&admission_id)
                    .map(|stored| {
                        let aggregate = self
                            .open_record(admission_id, stored)
                            .map_err(into_anyhow)?;
                        let token = SponsorJoinRequestCommitToken::from_bytes(
                            sponsor_existing_token(state.profile_generation, &aggregate),
                        )
                        .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        Ok(LoadedSponsorJoinRequest::new(
                            SponsorJoinRequestState::Existing(aggregate),
                            token,
                        ))
                    })
                    .transpose()
            })
            .map_err(map_executor_error)
            .map_err(map_sponsor_error)?;
        if let Some(existing) = existing {
            return Ok(existing);
        }

        let base_snapshot = self
            .load_sponsor_base_snapshot()
            .await
            .map_err(map_sponsor_error)?;
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                if let Some(stored) = state.records.get(&admission_id) {
                    let aggregate = self
                        .open_record(admission_id, stored)
                        .map_err(into_anyhow)?;
                    let token = SponsorJoinRequestCommitToken::from_bytes(sponsor_existing_token(
                        state.profile_generation,
                        &aggregate,
                    ))
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                    return Ok(LoadedSponsorJoinRequest::new(
                        SponsorJoinRequestState::Existing(aggregate),
                        token,
                    ));
                }
                if state.claimed_invitations.contains_key(&invitation_id) {
                    return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                }
                let claim =
                    encode_invitation_claim(admission_id, invitation_id).map_err(into_anyhow)?;
                let token = SponsorJoinRequestCommitToken::from_bytes(sponsor_fresh_token(
                    &state,
                    admission_id,
                    invitation_id,
                    base_snapshot.as_bytes(),
                ))
                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(LoadedSponsorJoinRequest::new(
                    SponsorJoinRequestState::Fresh {
                        invitation_claim: claim,
                        base_snapshot,
                    },
                    token,
                ))
            })
            .map_err(map_executor_error)
            .map_err(map_sponsor_error)
    }

    #[tracing::instrument(name = "space_admission.sponsor_state.commit", skip_all, err)]
    async fn commit(
        &self,
        token: SponsorJoinRequestCommitToken,
        mutation: SponsorAdmissionMutation,
    ) -> Result<CommittedSponsorAdmission, SponsorJoinRequestStateError> {
        let replacement = mutation.into_transition().into_replacement();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self.load_state_on(conn).map_err(into_anyhow)?;
                    let admission_id = *replacement.admission_id().as_bytes();
                    if let Some(stored) = state.records.get(&admission_id).cloned() {
                        let current = self
                            .open_record(admission_id, &stored)
                            .map_err(into_anyhow)?;
                        let expected = sponsor_existing_token(state.profile_generation, &current);
                        let expected_version = current
                            .record_version()
                            .checked_add(1)
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        if token.as_bytes() != &expected
                            || replacement.record_version() != expected_version
                        {
                            return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                        }
                        let sealed = self
                            .seal_record(&replacement, stored.wrapped_data_key)
                            .map_err(into_anyhow)?;
                        state.records.insert(admission_id, sealed);
                    } else {
                        let preparation = replacement
                            .sponsor_candidate_preparation()
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        let invitation_id = join_request_invitation_id(preparation.join_request())
                            .map_err(into_anyhow)?;
                        if replacement.record_version() != 0
                            || state.claimed_invitations.contains_key(&invitation_id)
                        {
                            return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                        }
                        let expected = sponsor_fresh_token(
                            &state,
                            admission_id,
                            invitation_id,
                            preparation.base_snapshot().as_bytes(),
                        );
                        if token.as_bytes() != &expected {
                            return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                        }
                        let sealed = self.seal_new_record(&replacement).map_err(into_anyhow)?;
                        state.records.insert(admission_id, sealed);
                        state
                            .claimed_invitations
                            .insert(invitation_id, admission_id);
                    }
                    self.save_state_on(conn, &state).map_err(into_anyhow)?;
                    let next_token = SponsorJoinRequestCommitToken::from_bytes(
                        sponsor_existing_token(state.profile_generation, &replacement),
                    )
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                    Ok(CommittedSponsorAdmission::new(replacement, next_token))
                })
            })
            .map_err(map_executor_error)
            .map_err(map_sponsor_error)
    }
}

fn join_request_invitation_id(
    envelope: &SpaceAdmissionEnvelopeV1,
) -> Result<[u8; 32], SpaceAdmissionStateStoreError> {
    let SpaceAdmissionBodyV1::JoinRequest(request) = envelope.body() else {
        return Err(SpaceAdmissionStateStoreError::Corrupt);
    };
    Ok(*request.invitation_id().as_bytes())
}

fn encode_invitation_claim(
    admission_id: [u8; 32],
    invitation_id: [u8; 32],
) -> Result<AdmissionInvitationClaim, SpaceAdmissionStateStoreError> {
    let encoded = postcard::to_stdvec(&PersistedInvitationClaimV1 {
        format_version: INVITATION_CLAIM_FORMAT_V1,
        admission_id,
        invitation_id,
    })
    .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
    AdmissionInvitationClaim::from_bytes(encoded)
        .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)
}

fn map_sponsor_error(error: SpaceAdmissionStateStoreError) -> SponsorJoinRequestStateError {
    match error {
        SpaceAdmissionStateStoreError::Locked => SponsorJoinRequestStateError::Locked,
        SpaceAdmissionStateStoreError::Conflict => SponsorJoinRequestStateError::StateChanged,
        SpaceAdmissionStateStoreError::Corrupt => SponsorJoinRequestStateError::RecoveryRequired,
        SpaceAdmissionStateStoreError::Unavailable => SponsorJoinRequestStateError::Unavailable,
    }
}
