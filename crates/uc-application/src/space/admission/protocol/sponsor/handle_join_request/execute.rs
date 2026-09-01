use uc_core::membership::{AdmissionReplayDecision, SponsorAdmission};

use super::{AuthenticatedSpaceAdmissionMessage, SpaceAdmissionMessageReply};
use crate::space::admission::protocol::{
    CommittedSponsorAdmission, HandleAuthenticatedSpaceAdmissionMessageError,
    SponsorAdmissionMutation, SponsorAdmissionService, SponsorAdmissionState,
};

impl SponsorAdmissionService {
    pub(in crate::space::admission::protocol::sponsor) async fn handle_join_request(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let load_started = std::time::Instant::now();
        let loaded_result = self.state.load(&message).await;
        super::super::super::record_performance_phase(
            "sponsor_state_load",
            load_started,
            loaded_result.is_ok(),
        );
        let loaded = loaded_result?;
        let (peer_binding, envelope, canonical_digest, continuation) = message.into_parts();
        let evidence = envelope.evidence(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the JoinRequest canonical digest is invalid"
            ))
        })?;
        let (state, commit_token) = loaded.into_parts();

        let committed = match state {
            SponsorAdmissionState::Existing(aggregate) => {
                match aggregate.replay_or_reject(&evidence)? {
                    AdmissionReplayDecision::ExactReply(_) => {
                        return SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
                            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(
                                anyhow::anyhow!("the saved JoinRequest reply is unavailable"),
                            )
                        });
                    }
                    AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {
                        CommittedSponsorAdmission::new(aggregate, commit_token)
                    }
                }
            }
            SponsorAdmissionState::Fresh {
                invitation_claim,
                base_snapshot,
            } => {
                let continuation = continuation.ok_or_else(|| {
                    HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                        "a fresh JoinRequest requires a continuation credential"
                    ))
                })?;
                let admission_id = envelope.header().admission_id();
                let transition = SponsorAdmission::accept_join_request(
                    admission_id,
                    invitation_claim,
                    envelope,
                    evidence,
                    base_snapshot,
                    peer_binding,
                    continuation,
                )?;
                let commit_started = std::time::Instant::now();
                let commit_result = self
                    .state
                    .commit(commit_token, SponsorAdmissionMutation::new(transition))
                    .await;
                super::super::super::record_performance_phase(
                    "sponsor_state_commit",
                    commit_started,
                    commit_result.is_ok(),
                );
                commit_result?
            }
        };

        let (aggregate, commit_token) = committed.into_parts();
        let preparation = aggregate.sponsor_candidate_preparation().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the accepted Sponsor state has no Candidate preparation"
            ))
        })?;
        let prepare_started = std::time::Instant::now();
        let prepared_result = self
            .prepare_candidate
            .prepare(aggregate.admission_id(), preparation)
            .await;
        super::super::super::record_performance_phase(
            "sponsor_prepare_candidate",
            prepare_started,
            prepared_result.is_ok(),
        );
        let prepared = prepared_result?;
        let (candidate_reply, staged_security) = prepared.into_parts();
        let transition = aggregate.fix_candidate(candidate_reply, staged_security)?;
        let commit_started = std::time::Instant::now();
        let committed_result = self
            .state
            .commit(commit_token, SponsorAdmissionMutation::new(transition))
            .await;
        super::super::super::record_performance_phase(
            "sponsor_state_commit",
            commit_started,
            committed_result.is_ok(),
        );
        let committed = committed_result?;
        let (aggregate, _) = committed.into_parts();
        SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the committed Sponsor Candidate reply is unavailable"
            ))
        })
    }
}
