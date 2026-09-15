use uc_core::membership::{
    AdmissionAbandonedV2, AdmissionMessageId, AdmissionReplayDecision, AdmissionRole,
    SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1,
};

use super::super::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessageError,
    SpaceAdmissionMessageReply, SponsorAdmissionMutation, SponsorAdmissionState,
};
use crate::space::admission::protocol::SponsorAdmissionService;

impl SponsorAdmissionService {
    pub(in super::super) async fn handle_abandonment(
        &self,
        message: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        let loaded = self.state.load(&message).await?;
        let (peer_binding, abandonment, canonical_digest, _, _) = message.into_parts();
        let evidence = abandonment.evidence(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the Abandonment canonical digest is invalid"
            ))
        })?;
        let (state, commit_token) = loaded.into_parts();
        let SponsorAdmissionState::Existing(aggregate) = state else {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(
                anyhow::anyhow!("Abandonment cannot start a fresh Sponsor admission"),
            ));
        };
        let expected_peer_binding = aggregate.sponsor_peer_binding().ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the Sponsor admission has no authenticated peer binding"
            ))
        })?;
        if peer_binding != expected_peer_binding {
            return Err(HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                anyhow::anyhow!("the Abandonment channel peer binding differs from saved state"),
            ));
        }
        if aggregate.current_exact_reply().is_some_and(|reply| {
            reply.kind() == uc_core::membership::SpaceAdmissionMessageKind::Abandoned
        }) {
            return match aggregate.replay_or_reject(&evidence)? {
                AdmissionReplayDecision::ExactReply(_) => {
                    SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
                        HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(
                            anyhow::anyhow!("the saved Abandonment reply is unavailable"),
                        )
                    })
                }
                AdmissionReplayDecision::Duplicate | AdmissionReplayDecision::New => {
                    Err(HandleAuthenticatedSpaceAdmissionMessageError::conflict(
                        anyhow::anyhow!("the terminal Abandonment differs from saved state"),
                    ))
                }
            };
        }
        let abandoned = AdmissionAbandonedV2::new(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the Abandonment digest is invalid"
            ))
        })?;
        let message_id = AdmissionMessageId::from_bytes(canonical_digest).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!(
                "the Abandonment reply message id is invalid"
            ))
        })?;
        let reply = SpaceAdmissionEnvelopeV1::reply_to(
            &abandonment,
            AdmissionRole::Sponsor,
            4,
            message_id,
            SpaceAdmissionBodyV1::Abandoned(abandoned),
        )
        .map_err(HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
        let transition = aggregate.accept_abandonment(abandonment, canonical_digest, reply)?;
        let committed = self
            .state
            .commit(commit_token, SponsorAdmissionMutation::new(transition))
            .await?;
        let (aggregate, _) = committed.into_parts();
        SpaceAdmissionMessageReply::new(aggregate).ok_or_else(|| {
            HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!(
                "the committed Sponsor Abandoned reply is unavailable"
            ))
        })
    }
}
