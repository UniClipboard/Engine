use async_trait::async_trait;
use uc_core::membership::{AdmissionReplayDecision, AdmissionReplayError, SpaceAdmissionAggregate};

use super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    HandleAuthenticatedSpaceAdmissionMessagePort, SpaceAdmissionMessageReply,
    SponsorAdmissionMutation, SponsorJoinRequestState, SponsorJoinRequestStateError,
};
use crate::space::admission::protocol::SpaceAdmissionProtocol;

#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for SpaceAdmissionProtocol {
    async fn handle(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let _guard = self.execution_lock.lock().await;
        let loaded = self
            .sponsor_join_request_state
            .load(&message)
            .await
            .map_err(map_state_error)?;
        let (peer_binding, envelope, canonical_digest, continuation) = message.into_parts();
        let evidence = envelope
            .evidence(canonical_digest)
            .ok_or(HandleAuthenticatedSpaceAdmissionMessageError::Invalid)?;
        let (state, commit_token) = loaded.into_parts();

        let committed = match state {
            SponsorJoinRequestState::Existing(aggregate) => {
                match aggregate
                    .replay_or_reject(&evidence)
                    .map_err(map_replay_error)?
                {
                    AdmissionReplayDecision::ExactReply(_) => {
                        return SpaceAdmissionMessageReply::new(aggregate).ok_or(
                            HandleAuthenticatedSpaceAdmissionMessageError::RecoveryRequired,
                        );
                    }
                    AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {
                        super::CommittedSponsorAdmission::new(aggregate, commit_token)
                    }
                }
            }
            SponsorJoinRequestState::Fresh {
                invitation_claim,
                base_snapshot,
            } => {
                let continuation =
                    continuation.ok_or(HandleAuthenticatedSpaceAdmissionMessageError::Invalid)?;
                let admission_id = envelope.header().admission_id();
                let transition = SpaceAdmissionAggregate::accept_join_request(
                    admission_id,
                    invitation_claim,
                    envelope,
                    evidence,
                    base_snapshot,
                    peer_binding,
                    continuation,
                )
                .map_err(|_| HandleAuthenticatedSpaceAdmissionMessageError::Invalid)?;
                self.sponsor_join_request_state
                    .commit(commit_token, SponsorAdmissionMutation::new(transition))
                    .await
                    .map_err(map_state_error)?
            }
        };

        let (aggregate, commit_token) = committed.into_parts();
        let preparation = aggregate
            .sponsor_candidate_preparation()
            .ok_or(HandleAuthenticatedSpaceAdmissionMessageError::RecoveryRequired)?;
        let prepared = self
            .prepare_sponsor_candidate
            .prepare(aggregate.admission_id(), preparation)
            .await
            .map_err(|error| match error {
                super::PrepareSponsorCandidateError::Invalid => {
                    HandleAuthenticatedSpaceAdmissionMessageError::Invalid
                }
                super::PrepareSponsorCandidateError::Unavailable => {
                    HandleAuthenticatedSpaceAdmissionMessageError::Unavailable
                }
            })?;
        let (candidate_reply, staged_security) = prepared.into_parts();
        let transition = aggregate
            .fix_candidate(candidate_reply, staged_security)
            .map_err(|_| HandleAuthenticatedSpaceAdmissionMessageError::Invalid)?;
        let committed = self
            .sponsor_join_request_state
            .commit(commit_token, SponsorAdmissionMutation::new(transition))
            .await
            .map_err(map_state_error)?;
        let (aggregate, _) = committed.into_parts();
        SpaceAdmissionMessageReply::new(aggregate)
            .ok_or(HandleAuthenticatedSpaceAdmissionMessageError::RecoveryRequired)
    }
}

fn map_state_error(
    error: SponsorJoinRequestStateError,
) -> HandleAuthenticatedSpaceAdmissionMessageError {
    match error {
        SponsorJoinRequestStateError::Locked => {
            HandleAuthenticatedSpaceAdmissionMessageError::Locked
        }
        SponsorJoinRequestStateError::StateChanged => {
            HandleAuthenticatedSpaceAdmissionMessageError::StateChanged
        }
        SponsorJoinRequestStateError::RecoveryRequired => {
            HandleAuthenticatedSpaceAdmissionMessageError::RecoveryRequired
        }
        SponsorJoinRequestStateError::Unavailable => {
            HandleAuthenticatedSpaceAdmissionMessageError::Unavailable
        }
    }
}

fn map_replay_error(error: AdmissionReplayError) -> HandleAuthenticatedSpaceAdmissionMessageError {
    match error {
        AdmissionReplayError::Conflict => HandleAuthenticatedSpaceAdmissionMessageError::Conflict,
        AdmissionReplayError::OutOfOrder => {
            HandleAuthenticatedSpaceAdmissionMessageError::OutOfOrder
        }
    }
}
