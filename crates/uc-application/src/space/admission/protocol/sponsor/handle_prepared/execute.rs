use uc_core::membership::AdmissionReplayDecision;

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    SpaceAdmissionMessageReply, SponsorAdmissionMutation, SponsorAdmissionState,
};
use crate::space::admission::protocol::SponsorAdmissionService;

impl SponsorAdmissionService {
    pub(in crate::space::admission::protocol::sponsor) async fn handle_prepared(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let loaded = self.state.load(&message).await?;
        let (peer_binding, prepared, canonical_digest, _) = message.into_parts();
        let evidence = prepared.evidence(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the Prepared canonical digest is invalid"
            ))
        })?;
        let (state, commit_token) = loaded.into_parts();
        let SponsorAdmissionState::Existing(aggregate) = state else {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("Prepared cannot start a fresh Sponsor admission"),
            ));
        };
        let expected_peer_binding = aggregate.sponsor_peer_binding().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor admission has no authenticated peer binding"
            ))
        })?;
        if peer_binding != expected_peer_binding {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                anyhow::anyhow!("the Prepared channel peer binding differs from saved state"),
            ));
        }
        match aggregate.replay_or_reject(&evidence)? {
            AdmissionReplayDecision::ExactReply(_) => {
                return SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
                    HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(
                        anyhow::anyhow!("the saved Prepared reply is unavailable"),
                    )
                });
            }
            AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {}
        }
        let preparation = aggregate.sponsor_commit_preparation().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor Candidate state has no Commit preparation"
            ))
        })?;
        let material = self
            .prepare_commit
            .prepare(aggregate.admission_id(), preparation, &prepared)
            .await?;
        let (committed_history, sealed_security, commit_reply) = material.into_parts();
        let transition = aggregate.commit_prepared(
            prepared,
            canonical_digest,
            committed_history,
            sealed_security,
            commit_reply,
        )?;
        let committed = self
            .state
            .commit(commit_token, SponsorAdmissionMutation::new(transition))
            .await?;
        let (aggregate, _) = committed.into_parts();
        SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the committed Sponsor Commit reply is unavailable"
            ))
        })
    }
}
