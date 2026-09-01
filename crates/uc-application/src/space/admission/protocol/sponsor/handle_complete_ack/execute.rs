use uc_core::membership::AdmissionReplayDecision;

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    SpaceAdmissionMessageReply, SponsorAdmissionMutation, SponsorAdmissionState,
};
use crate::space::admission::protocol::SponsorAdmissionService;

impl SponsorAdmissionService {
    pub(in crate::space::admission::protocol::sponsor) async fn handle_complete_ack(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let loaded = self.state.load(&message).await?;
        let (peer_binding, complete_ack, canonical_digest, _) = message.into_parts();
        let evidence = complete_ack.evidence(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the CompleteAck canonical digest is invalid"
            ))
        })?;
        let (state, commit_token) = loaded.into_parts();
        let SponsorAdmissionState::Existing(aggregate) = state else {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("CompleteAck cannot start a fresh Sponsor admission"),
            ));
        };
        let expected_peer_binding = aggregate.sponsor_peer_binding().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor admission has no authenticated peer binding"
            ))
        })?;
        if peer_binding != expected_peer_binding {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                anyhow::anyhow!("the CompleteAck channel peer binding differs from saved state"),
            ));
        }
        match aggregate.replay_or_reject(&evidence)? {
            AdmissionReplayDecision::ExactReply(_) => {
                return SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
                    HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(
                        anyhow::anyhow!("the saved CompleteAck reply is unavailable"),
                    )
                });
            }
            AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {}
        }
        let preparation = aggregate.sponsor_settlement_preparation().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor Applied state has no settlement preparation"
            ))
        })?;
        let settled = self
            .prepare_settled
            .prepare(aggregate.admission_id(), preparation, &complete_ack)
            .await?;
        let transition =
            aggregate.settle_complete_ack(complete_ack, canonical_digest, settled.into_reply())?;
        let committed = self
            .state
            .commit(commit_token, SponsorAdmissionMutation::new(transition))
            .await?;
        let (aggregate, _) = committed.into_parts();
        SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the committed Sponsor Settled reply is unavailable"
            ))
        })
    }
}
